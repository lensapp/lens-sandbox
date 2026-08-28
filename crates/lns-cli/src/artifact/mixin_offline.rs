use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::author::{Fs, load_definition_json_at};
use super::fileset::local_mixin_document;

/// The mixin graph an offline render merges, keyed by the document each reference resolves to so two spellings of one mixin are one source.
pub struct LocalMixins {
    pub keys: Vec<String>,
    pub graph: BTreeMap<String, lns_artifact::sandbox::SandboxSpec>,
}

/// Read every mixin the flags name from this machine, and each mixin those name in turn; nothing here reaches the network, which is why a published reference is refused rather than fetched.
pub fn resolve<F: Fs + ?Sized>(
    fs: &F,
    project_dir: &Path,
    references: &[String],
) -> Result<LocalMixins> {
    let mut graph = BTreeMap::new();
    let mut keys = Vec::with_capacity(references.len());
    for reference in references {
        keys.push(read_into(fs, project_dir, reference, &mut graph)?);
    }
    Ok(LocalMixins { keys, graph })
}

fn read_into<F: Fs + ?Sized>(
    fs: &F,
    dir: &Path,
    reference: &str,
    graph: &mut BTreeMap<String, lns_artifact::sandbox::SandboxSpec>,
) -> Result<String> {
    if !lns_artifact::sandbox::names_a_local_path(reference) {
        bail!(
            "--mixin {reference} names a published mixin, and an offline render resolves nothing: name it by a local path to merge it here, or inspect a published artifact instead"
        )
    }
    let document = local_mixin_document(fs, dir, reference);
    let key = document
        .to_str()
        .with_context(|| format!("mixin path {} is not utf-8", document.display()))?
        .to_string();
    if graph.contains_key(&key) {
        return Ok(key);
    }
    let json = load_definition_json_at(fs, &document)?;
    let mixin = lns_artifact::sandbox::parse_mixin(&json)
        .with_context(|| format!("{} is not a mixin", document.display()))?;
    let mut spec = mixin.spec;
    // Reserved before the descent so a mixin reachable from itself stops here, and the merge is the one place that names the cycle.
    graph.insert(key.clone(), spec.clone());
    let own_dir = document.parent().unwrap_or(dir).to_path_buf();
    let mut layered = Vec::with_capacity(spec.mixins.len());
    for child in &spec.mixins {
        layered.push(read_into(fs, &own_dir, child, graph)?);
    }
    spec.mixins = layered;
    graph.insert(key.clone(), spec);
    Ok(key)
}
