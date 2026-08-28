use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::author::{Fs, load_definition_json_at};
use super::fileset::local_mixin_document;

/// Where each mixin reference comes from, since §3.3.1 roots a declared entry at the document that named it and a flag where the user typed it.
pub struct Wanted<'a> {
    pub declared: &'a [String],
    pub document_dir: &'a Path,
    pub flags: &'a [String],
    pub invocation_dir: &'a Path,
}

/// The mixin graph an offline render merges, keyed by the folded absolute path of the document each reference resolved to — §3.3.1's identity, so a directory and the `lns.yaml` inside it are one source.
pub struct LocalMixins {
    pub declared_keys: Vec<String>,
    pub flag_keys: Vec<String>,
    /// Published references met along the way, which an offline render lists rather than merges.
    pub unresolved: Vec<String>,
    pub graph: BTreeMap<String, lns_artifact::sandbox::SandboxSpec>,
}

/// Read every mixin reachable by path from this machine. Nothing here reaches the network, which is why a published reference is listed instead of fetched — and why a flag naming one is refused outright, the user having asked for it by hand.
pub fn resolve<F: Fs + ?Sized>(fs: &F, wanted: &Wanted) -> Result<LocalMixins> {
    let mut walk = Walk {
        fs,
        graph: BTreeMap::new(),
        unresolved: Vec::new(),
    };
    let declared_keys = walk.reach_all(wanted.declared, wanted.document_dir)?;
    let flag_keys = walk.reach_demanded(wanted.flags, wanted.invocation_dir)?;
    Ok(LocalMixins {
        declared_keys,
        flag_keys,
        unresolved: walk.unresolved,
        graph: walk.graph,
    })
}

struct Walk<'a, F: Fs + ?Sized> {
    fs: &'a F,
    graph: BTreeMap<String, lns_artifact::sandbox::SandboxSpec>,
    unresolved: Vec<String>,
}

impl<F: Fs + ?Sized> Walk<'_, F> {
    fn reach_all(&mut self, references: &[String], dir: &Path) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        for reference in references {
            match self.reach(reference, dir)? {
                Some(key) => keys.push(key),
                None => self.note_unresolved(reference),
            }
        }
        Ok(keys)
    }

    fn reach_demanded(&mut self, references: &[String], dir: &Path) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        for reference in references {
            let Some(key) = self.reach(reference, dir)? else {
                bail!(
                    "--mixin {reference} names a published mixin, and an offline render resolves nothing: name it by a local path to merge it here, or inspect a published artifact instead"
                )
            };
            keys.push(key);
        }
        Ok(keys)
    }

    /// `None` when the reference is published, since an offline walk reaches only what this machine already holds.
    fn reach(&mut self, reference: &str, dir: &Path) -> Result<Option<String>> {
        if !lns_artifact::sandbox::names_a_local_path(reference) {
            return Ok(None);
        }
        let document = local_mixin_document(self.fs, dir, reference);
        let key = document
            .to_str()
            .with_context(|| format!("mixin path {} is not utf-8", document.display()))?
            .to_string();
        if self.graph.contains_key(&key) {
            return Ok(Some(key));
        }
        let json = load_definition_json_at(self.fs, &document)?;
        let mixin = lns_artifact::sandbox::parse_mixin(&json)
            .with_context(|| format!("{} is not a mixin", document.display()))?;
        let mut spec = mixin.spec;
        // Reserved before the descent so a mixin reachable from itself stops here, and the merge is the one place that names the cycle.
        self.graph.insert(key.clone(), spec.clone());
        let own_dir = document.parent().unwrap_or(dir).to_path_buf();
        spec.mixins = self.reach_all(&spec.mixins.clone(), &own_dir)?;
        self.graph.insert(key.clone(), spec);
        Ok(Some(key))
    }

    fn note_unresolved(&mut self, reference: &str) {
        if !self.unresolved.iter().any(|seen| seen == reference) {
            self.unresolved.push(reference.to_string());
        }
    }
}
