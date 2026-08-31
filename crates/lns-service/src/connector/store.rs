//! The connector records of `docs/sandbox-spec.md` §7.1.
//!
//! `DecisionStore::save` replaces a whole map, so every mutation is
//! load-mutate-save under `write`; an in-process lock suffices only while every
//! connector write arrives over the service's IPC, leaving lns-service the sole
//! writer of these files.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use lns_artifact::connector::ConnectorDefinition;
use lns_policy::decision_store::{DecisionFile, DecisionStore};
use serde::{Deserialize, Serialize};

use super::conflicts::refuse_a_conflict;

/// The separator in a composite key: no run id, OCI reference, or connection label may contain one, so no composed key is ambiguous.
const SEP: char = '\0';

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub name: String,
    pub digest: String,
    pub document: Vec<u8>,
}

pub trait InstalledSet: Send + Sync {
    fn list(&self) -> io::Result<Vec<Installed>>;
    fn put(
        &self,
        name: &str,
        digest: &str,
        document: &[u8],
        filesets: &[Vec<u8>],
    ) -> io::Result<()>;
    /// One packed fileset by its index in the document's `path` entries.
    fn fileset_layer(&self, name: &str, index: usize) -> io::Result<Vec<u8>>;
    fn remove(&self, name: &str) -> io::Result<bool>;
}

/// The permissions an authentication reported. A set cannot represent an order, so the canonicality failure §3.2.4 warns of is unrepresentable rather than merely forbidden.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Authority(pub BTreeSet<String>);

impl Authority {
    /// A `kind: token` exchange reports no authority, which is [`Default`]; anything else names what it reported.
    pub fn of<I: IntoIterator<Item = S>, S: Into<String>>(scopes: I) -> Self {
        Self(scopes.into_iter().map(Into::into).collect())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub method: String,
    #[serde(default)]
    pub authority: Authority,
    #[serde(default)]
    pub values: BTreeMap<String, String>,
}

impl std::fmt::Debug for Connection {
    /// Hand-written so a `log::debug!` of a connection cannot print the token it holds.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("method", &self.method)
            .field("authority", &self.authority)
            .field("values", &format_args!("<{} redacted>", self.values.len()))
            .finish()
    }
}

/// A run's one answer about one connector (§8.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RunDecision {
    Granted {
        /// The bytes consented to. A grant does not carry to a different digest, so any update offers again (§7.1).
        digest: String,
        method: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connection: Option<String>,
        #[serde(default)]
        authority: Authority,
    },
    /// Keyed without a digest, deliberately: an update does not re-offer what a run turned down (§7.1).
    Declined,
}

impl RunDecision {
    /// Whether this answers for the bytes installed now. A grant bound to other bytes does not, so any update offers again; a decline answers for every version (§7.1).
    pub fn decides(&self, installed_digest: &str) -> bool {
        match self {
            Self::Granted { digest, .. } => digest == installed_digest,
            Self::Declined => true,
        }
    }
}

/// What one connector may keep, over every fileset of every method it declares. §3.2.3 caps a method, and nothing caps how many methods a document declares.
pub const MAX_CONNECTOR_FILESET_BYTES: u64 =
    4 * lns_artifact::connector::MAX_METHOD_FILESET_BYTES as u64;

/// The document read against the directories its `path` entries pack, so §3.2.3's byte count and §3.2.5's content check hold at install exactly as they held at push.
fn read_with_its_filesets(document: &[u8], filesets: &[Vec<u8>]) -> Result<ConnectorDefinition> {
    let shape = lns_artifact::connector::parse(document)?;
    let packing = lns_artifact::connector::path_filesets(&shape.spec);
    if packing.len() != filesets.len() {
        bail!(
            "this connector declares {} packed fileset(s) but {} came with it",
            packing.len(),
            filesets.len()
        );
    }
    let mut by_path: BTreeMap<String, super::layer::PackedFiles> = BTreeMap::new();
    let mut spent = 0u64;
    for ((method, path), layer) in packing.iter().zip(filesets) {
        // Charged against what a layer expands to, never against what it travelled as: a megabyte of zeros compresses to a kilobyte, and it is the expansion this machine holds.
        let ceiling = (lns_artifact::connector::MAX_METHOD_FILESET_BYTES as u64)
            .min(MAX_CONNECTOR_FILESET_BYTES - spent);
        let files = super::layer::expand(layer, ceiling)
            .map_err(|e| match e.downcast_ref::<super::layer::BudgetExceeded>() {
                // The narrower of the two ceilings is the one that bit, and only this caller knows which that was.
                Some(_) if ceiling < lns_artifact::connector::MAX_METHOD_FILESET_BYTES as u64 => {
                    anyhow::anyhow!(
                        "this connector's filesets total more than the {MAX_CONNECTOR_FILESET_BYTES}-byte limit one connector may bring"
                    )
                }
                _ => e,
            })
            .with_context(|| format!("method {method} fileset {path}"))?;
        spent += files
            .values()
            .map(|file| file.bytes.len() as u64)
            .sum::<u64>();
        // One `path` names one directory, so two layers under it are two answers to one question, and the check would see only the last.
        if let Some(first) = by_path.insert((*path).to_string(), files.clone())
            && first != files
        {
            bail!("this connector packs {path} more than once, with different bytes each time");
        }
    }
    let read: BTreeMap<String, BTreeMap<String, Vec<u8>>> = by_path
        .into_iter()
        .map(|(path, files)| {
            (
                path,
                files
                    .into_iter()
                    .map(|(name, file)| (name, file.bytes))
                    .collect(),
            )
        })
        .collect();
    lns_artifact::connector::parse_with_path_files(document, &read)
}

fn connection_key(name: &str, label: &str) -> String {
    format!("{name}{SEP}{label}")
}

fn run_key(run: &str, name: &str) -> String {
    format!("{run}{SEP}{name}")
}

fn splits(key: &str) -> Option<(&str, &str)> {
    key.split_once(SEP)
}

/// The runs a re-authentication of `name`'s `connection` invalidates the grant of: every grant naming that connection whose consented authority differs from what came back. Difference is the test, not widening (§3.2.4).
///
/// Runs, not keys: these are reported to the user, and a key carries the separator.
pub fn grants_invalidated_by(
    grants: &DecisionFile<RunDecision>,
    name: &str,
    connection: &str,
    reported: &Authority,
) -> Vec<String> {
    let mut invalidated: Vec<String> = grants
        .iter()
        .filter(|(key, decision)| invalidates(key, decision, name, connection, reported))
        .filter_map(|(key, _)| splits(key).map(|(run, _)| run.to_string()))
        .collect();
    invalidated.sort();
    invalidated
}

/// One row's own answer, so what is reported and what is dropped cannot drift apart — a run may hold grants of several connectors, and only this one's is invalidated.
fn invalidates(
    key: &str,
    decision: &RunDecision,
    name: &str,
    connection: &str,
    reported: &Authority,
) -> bool {
    splits(key).is_some_and(|(_, keyed)| keyed == name)
        && names_connection_with_other_authority(decision, connection, reported)
}

fn names_connection_with_other_authority(
    decision: &RunDecision,
    connection: &str,
    reported: &Authority,
) -> bool {
    match decision {
        RunDecision::Granted {
            connection: Some(named),
            authority,
            ..
        } => named == connection && authority != reported,
        _ => false,
    }
}

pub struct ConnectorStore<'a> {
    installed: &'a dyn InstalledSet,
    values: &'a dyn DecisionStore<Connection>,
    grants: &'a dyn DecisionStore<RunDecision>,
    write: Mutex<()>,
}

impl<'a> ConnectorStore<'a> {
    pub fn new(
        installed: &'a dyn InstalledSet,
        values: &'a dyn DecisionStore<Connection>,
        grants: &'a dyn DecisionStore<RunDecision>,
    ) -> Self {
        Self {
            installed,
            values,
            grants,
            write: Mutex::new(()),
        }
    }

    pub fn installed(&self) -> io::Result<Vec<Installed>> {
        self.installed.list()
    }

    /// Every installed document, parsed, beside the names of any that could not be.
    pub fn installed_definitions(&self) -> io::Result<(Vec<ConnectorDefinition>, Vec<String>)> {
        let mut parsed = Vec::new();
        let mut unreadable = Vec::new();
        for entry in self.installed()? {
            match lns_artifact::connector::parse(&entry.document) {
                Ok(definition) => parsed.push(definition),
                Err(_) => unreadable.push(entry.name),
            }
        }
        Ok((parsed, unreadable))
    }

    /// Parse, refuse what the installed set already answers for, then store. Installing grants nothing (§7.1).
    pub fn install(
        &self,
        digest: &str,
        document: &[u8],
        filesets: &[Vec<u8>],
    ) -> Result<ConnectorDefinition> {
        let candidate = read_with_its_filesets(document, filesets)?;
        let _guard = self.lock();
        let (installed, unreadable) = self.installed_definitions()?;
        // An unreadable document hides its own `serves` and variables, so a conflict could not be decided and would surface as an ambiguous offer at some later launch instead.
        if !unreadable.is_empty() {
            bail!(
                "cannot read the installed connector(s) {}, so an overlap with {} cannot be decided; uninstall them or upgrade lns",
                unreadable.join(", "),
                candidate.name
            );
        }
        if let Err(conflict) = refuse_a_conflict(&candidate, &installed) {
            bail!("{conflict}");
        }
        self.installed
            .put(&candidate.name, digest, document, filesets)
            .map_err(anyhow::Error::from)?;
        Ok(candidate)
    }

    /// One packed fileset of an installed connector, by its index in the document's `path` entries.
    pub fn fileset_layer(&self, name: &str, index: usize) -> io::Result<Vec<u8>> {
        self.installed.fileset_layer(name, index)
    }

    /// Removes every connection the connector held, then the connector, and leaves what runs granted untouched (§7.1).
    ///
    /// Connections go first so a failed write cannot leave real values behind under a name nothing installed.
    pub fn uninstall(&self, name: &str) -> io::Result<bool> {
        let _guard = self.lock();
        self.retain_connections(|key| splits(key).is_some_and(|(keyed, _)| keyed != name))?;
        self.installed.remove(name)
    }

    pub fn connections_of(&self, name: &str) -> io::Result<BTreeMap<String, Connection>> {
        Ok(self
            .values
            .load()?
            .into_iter()
            .filter_map(|(key, connection)| match splits(&key) {
                Some((keyed, label)) if keyed == name => Some((label.to_string(), connection)),
                _ => None,
            })
            .collect())
    }

    /// Drops every grant whose consented authority this authentication no longer matches, then stores what it returned. The returned keys are the grants to ask about again (§3.2.4).
    ///
    /// The grants go first so a failed write cannot leave a grant standing beside a connection whose authority the project never consented to — a grant silently widened.
    pub fn record_authentication(
        &self,
        name: &str,
        label: &str,
        connection: Connection,
    ) -> io::Result<Vec<String>> {
        let _guard = self.lock();
        let mut grants = self.grants.load()?;
        let invalidated = grants_invalidated_by(&grants, name, label, &connection.authority);
        if !invalidated.is_empty() {
            grants.retain(|key, decision| {
                !invalidates(key, decision, name, label, &connection.authority)
            });
            self.grants.save(&grants)?;
        }

        let mut values = self.values.load()?;
        values.insert(connection_key(name, label), connection);
        self.values.save(&values)?;
        Ok(invalidated)
    }

    /// Drops one connection, or every connection of a connector when `label` is absent. The connector stays installed and grants naming it stay (§3.3 `disconnect`).
    pub fn drop_connections(&self, name: &str, label: Option<&str>) -> io::Result<usize> {
        let _guard = self.lock();
        let before = self.values.load()?.len();
        self.retain_connections(|key| match splits(key) {
            Some((keyed, keyed_label)) => {
                keyed != name || label.is_some_and(|wanted| keyed_label != wanted)
            }
            None => true,
        })?;
        Ok(before - self.values.load()?.len())
    }

    pub fn decision(&self, run: &str, name: &str) -> io::Result<Option<RunDecision>> {
        Ok(self.grants.load()?.remove(&run_key(run, name)))
    }

    /// The grant that applies to the digest installed right now. A grant bound to other bytes is not one, so any update offers again (§7.1).
    pub fn grant_for(
        &self,
        run: &str,
        name: &str,
        installed_digest: &str,
    ) -> io::Result<Option<RunDecision>> {
        Ok(self.decision(run, name)?.filter(|decision| {
            matches!(decision, RunDecision::Granted { .. }) && decision.decides(installed_digest)
        }))
    }

    /// Records a run's answer, returning whatever it displaces. A run holds one answer per connector, so granting again replaces the prior one (§3.2.4).
    pub fn decide(
        &self,
        run: &str,
        name: &str,
        decision: RunDecision,
    ) -> io::Result<Option<RunDecision>> {
        let _guard = self.lock();
        let mut grants = self.grants.load()?;
        let displaced = grants.insert(run_key(run, name), decision);
        self.grants.save(&grants)?;
        Ok(displaced)
    }

    /// Clears what a run decided, granted or declined, so its next start asks again (§8.4).
    pub fn forget(&self, run: &str, name: &str) -> io::Result<bool> {
        let _guard = self.lock();
        let mut grants = self.grants.load()?;
        let removed = grants.remove(&run_key(run, name)).is_some();
        if removed {
            self.grants.save(&grants)?;
        }
        Ok(removed)
    }

    fn retain_connections(&self, keep: impl Fn(&str) -> bool) -> io::Result<()> {
        let mut values = self.values.load()?;
        let before = values.len();
        values.retain(|key, _| keep(key));
        if values.len() != before {
            self.values.save(&values)?;
        }
        Ok(())
    }

    /// A poisoned lock means a prior writer panicked mid-mutation; the map is still whole on disk, so the next writer proceeds rather than refusing every connector operation.
    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.write
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    struct FakeMap<T> {
        state: StdMutex<DecisionFile<T>>,
        fail_save: StdMutex<bool>,
    }

    /// Hand-written because a derived `Default` would demand `T: Default`, which neither stored value has or needs.
    impl<T> Default for FakeMap<T> {
        fn default() -> Self {
            Self {
                state: StdMutex::new(DecisionFile::new()),
                fail_save: StdMutex::new(false),
            }
        }
    }

    impl<T: Clone + Send + Sync> DecisionStore<T> for FakeMap<T> {
        fn load(&self) -> io::Result<DecisionFile<T>> {
            Ok(self.state.lock().unwrap().clone())
        }

        fn save(&self, state: &DecisionFile<T>) -> io::Result<()> {
            if *self.fail_save.lock().unwrap() {
                return Err(io::Error::other("disk full"));
            }
            *self.state.lock().unwrap() = state.clone();
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeSet {
        entries: StdMutex<Vec<Installed>>,
        fail_put: StdMutex<bool>,
        layers: StdMutex<std::collections::BTreeMap<String, Vec<Vec<u8>>>>,
    }

    impl InstalledSet for FakeSet {
        fn list(&self) -> io::Result<Vec<Installed>> {
            Ok(self.entries.lock().unwrap().clone())
        }

        fn put(
            &self,
            name: &str,
            digest: &str,
            document: &[u8],
            filesets: &[Vec<u8>],
        ) -> io::Result<()> {
            if *self.fail_put.lock().unwrap() {
                return Err(io::Error::other("disk full"));
            }
            let mut entries = self.entries.lock().unwrap();
            entries.retain(|e| e.name != name);
            entries.push(Installed {
                name: name.to_string(),
                digest: digest.to_string(),
                document: document.to_vec(),
            });
            self.layers
                .lock()
                .unwrap()
                .insert(name.to_string(), filesets.to_vec());
            Ok(())
        }

        fn fileset_layer(&self, name: &str, index: usize) -> io::Result<Vec<u8>> {
            self.layers
                .lock()
                .unwrap()
                .get(name)
                .and_then(|layers| layers.get(index))
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such fileset layer"))
        }

        fn remove(&self, name: &str) -> io::Result<bool> {
            let mut entries = self.entries.lock().unwrap();
            let before = entries.len();
            entries.retain(|e| e.name != name);
            Ok(entries.len() != before)
        }
    }

    struct Rig {
        set: FakeSet,
        values: FakeMap<Connection>,
        grants: FakeMap<RunDecision>,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                set: FakeSet::default(),
                values: FakeMap::default(),
                grants: FakeMap::default(),
            }
        }

        fn store(&self) -> ConnectorStore<'_> {
            ConnectorStore::new(&self.set, &self.values, &self.grants)
        }
    }

    fn packing(name: &str, path: &str) -> Vec<u8> {
        serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": name,
            "spec": {
                "serves": [format!("api.{name}.example")],
                "methods": [{
                    "name": "open",
                    "filesets": [{ "path": path, "guestPath": "~/.some-provider" }],
                }],
            },
        })
        .to_string()
        .into_bytes()
    }

    fn layer_of(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let entries: Vec<lns_artifact::build::FileEntry> = files
            .iter()
            .map(|(path, data)| lns_artifact::build::FileEntry {
                path: (*path).to_string(),
                data: data.clone(),
                mode: 0o600,
            })
            .collect();
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"seed","spec":{"filesets":[{"path":"./seed","guestPath":"/seed"}]}}"#;
        lns_artifact::build::build_artifact(doc, &[entries], None)
            .expect("a packable directory")
            .fileset_layers()
            .next()
            .expect("one layer")
            .data
            .clone()
    }

    #[test]
    fn installing_keeps_the_packed_fileset_that_came_with_the_document() {
        let rig = Rig::new();
        let layer = layer_of(&[("config.json", b"{}".to_vec())]);

        rig.store()
            .install(
                "sha256:abc",
                &packing("some-provider", "./seed"),
                std::slice::from_ref(&layer),
            )
            .expect("a connector packing one directory");

        assert_eq!(
            rig.store().fileset_layer("some-provider", 0).unwrap(),
            layer,
            "the bytes outlive the install, because a grant sends them on every policy change"
        );
    }

    #[test]
    fn installing_refuses_a_packed_fileset_past_what_its_method_may_write() {
        // The count runs where the bytes are: a document alone cannot show what a directory holds, so install reads the layer it was given.
        let rig = Rig::new();
        let oversized = layer_of(&[(
            "config.json",
            vec![b'a'; lns_artifact::connector::MAX_METHOD_FILESET_BYTES + 1],
        )]);

        let err = rig
            .store()
            .install(
                "sha256:abc",
                &packing("some-provider", "./seed"),
                &[oversized],
            )
            .unwrap_err();

        assert!(format!("{err:#}").contains("byte limit"), "got: {err:#}");
        assert!(
            rig.store().installed().unwrap().is_empty(),
            "a refused install leaves nothing installed"
        );
    }

    #[test]
    fn a_full_budget_does_not_rewrite_every_other_refusal_as_its_own() {
        // The remaining budget being narrow does not make it the reason a layer was refused, and a user told the wrong reason trims the wrong thing.
        let rig = Rig::new();
        let methods: Vec<serde_json::Value> = (0..5)
            .map(|i| {
                serde_json::json!({
                    "name": format!("m{i}"),
                    "filesets": [{ "path": format!("./seed{i}"), "guestPath": format!("~/.p{i}") }],
                })
            })
            .collect();
        let doc = serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": "some-provider",
            "spec": { "serves": ["api.some-provider.example"], "methods": methods },
        })
        .to_string()
        .into_bytes();
        let fat = layer_of(&[(
            "config.json",
            vec![0u8; lns_artifact::connector::MAX_METHOD_FILESET_BYTES],
        )]);
        let mut layers = vec![fat; 4];
        layers.push(super::super::layer::fixtures::raw_layer("../escape", b"x"));

        let err = rig
            .store()
            .install("sha256:abc", &doc, &layers)
            .unwrap_err();

        assert!(
            format!("{err:#}").contains("escapes the path"),
            "got: {err:#}"
        );
        assert!(
            format!("{err:#}").contains("./seed4"),
            "the refusal must still say which fileset it was; got: {err:#}"
        );
    }

    #[test]
    fn the_budget_counts_what_a_layer_expands_to_not_what_it_travelled_as() {
        // A megabyte of zeros compresses to about a kilobyte, so a budget over compressed bytes admits thousands of layers and expands gigabytes. Install is the seam both arms pass through, so this covers a local install too.
        let rig = Rig::new();
        let methods: Vec<serde_json::Value> = (0..100)
            .map(|i| {
                serde_json::json!({
                    "name": format!("m{i}"),
                    "filesets": [{ "path": format!("./seed{i}"), "guestPath": format!("~/.p{i}") }],
                })
            })
            .collect();
        let doc = serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": "some-provider",
            "spec": { "serves": ["api.some-provider.example"], "methods": methods },
        })
        .to_string()
        .into_bytes();
        let fat = layer_of(&[(
            "config.json",
            vec![0u8; lns_artifact::connector::MAX_METHOD_FILESET_BYTES],
        )]);
        let layers = vec![fat; 100];

        let err = rig
            .store()
            .install("sha256:abc", &doc, &layers)
            .unwrap_err();

        assert!(
            format!("{err:#}").contains("one connector may bring"),
            "got: {err:#}"
        );
    }

    #[test]
    fn installing_refuses_two_layers_under_one_path_that_disagree() {
        // Keyed by path alone, the second layer would replace the first and the first would be stored unchecked — and a registry supplies each one independently.
        let rig = Rig::new();
        let doc = serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": "some-provider",
            "spec": {
                "serves": ["api.some-provider.example"],
                "methods": [
                    { "name": "a", "filesets": [{ "path": "./seed", "guestPath": "~/.a" }] },
                    { "name": "b", "filesets": [{ "path": "./seed", "guestPath": "~/.b" }] },
                ],
            },
        })
        .to_string()
        .into_bytes();
        let smuggled = layer_of(&[("credentials.json", br#"{"token":"sk-live-real"}"#.to_vec())]);
        let benign = layer_of(&[("config.json", b"{}".to_vec())]);

        let err = rig
            .store()
            .install("sha256:abc", &doc, &[smuggled, benign])
            .unwrap_err();

        assert!(
            format!("{err:#}").contains("more than once, with different bytes"),
            "got: {err:#}"
        );
    }

    #[test]
    fn installing_refuses_a_document_whose_filesets_did_not_come_with_it() {
        // A count that silently saw no bytes would pass every oversized directory.
        let rig = Rig::new();
        let err = rig
            .store()
            .install("sha256:abc", &packing("some-provider", "./seed"), &[])
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("declares 1 packed fileset(s) but 0 came with it"),
            "got: {err:#}"
        );
    }

    #[test]
    fn installing_refuses_a_packed_secret_that_carries_no_placeholder() {
        // §3.2.5 is checkable only where the bytes are, and install is the second place they are in hand.
        let rig = Rig::new();
        let layer = layer_of(&[("credentials.json", br#"{"token":"sk-live-real"}"#.to_vec())]);

        let err = rig
            .store()
            .install("sha256:abc", &packing("some-provider", "./seed"), &[layer])
            .unwrap_err();

        assert!(
            format!("{err:#}").contains("carries no placeholder"),
            "got: {err:#}"
        );
    }

    fn document(name: &str, host: &str, var: &str) -> Vec<u8> {
        serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": name,
            "spec": {
                "serves": [host],
                "methods": [{
                    "name": "token",
                    "auth": { "kind": "token" },
                    "credentials": [{
                        "envVar": var,
                        "placeholder": "some_LNSPLACEHOLDER0000000000",
                    }],
                }],
            },
        })
        .to_string()
        .into_bytes()
    }

    fn connection(authority: Authority) -> Connection {
        Connection {
            method: "token".to_string(),
            authority,
            values: [("SOME_TOKEN".to_string(), "real-secret".to_string())].into(),
        }
    }

    fn granted(digest: &str, connection: Option<&str>, authority: Authority) -> RunDecision {
        RunDecision::Granted {
            digest: digest.to_string(),
            method: "token".to_string(),
            connection: connection.map(str::to_string),
            authority,
        }
    }

    #[test]
    fn installing_stores_the_document_and_grants_nothing() {
        let rig = Rig::new();
        let store = rig.store();
        let installed = store
            .install(
                "sha256:abc",
                &document("some-provider", "api.some-provider.example", "SOME_TOKEN"),
                &[],
            )
            .unwrap();
        assert_eq!(installed.name, "some-provider");
        assert_eq!(store.installed().unwrap().len(), 1);
        assert!(store.connections_of("some-provider").unwrap().is_empty());
        assert_eq!(store.decision("/work", "some-provider").unwrap(), None);
    }

    #[test]
    fn a_document_that_does_not_parse_is_refused_before_anything_is_stored() {
        let rig = Rig::new();
        assert!(
            rig.store()
                .install("sha256:abc", b"not a document", &[])
                .is_err()
        );
        assert!(rig.store().installed().unwrap().is_empty());
    }

    #[test]
    fn a_second_connector_covering_an_installed_destination_is_refused() {
        let rig = Rig::new();
        let store = rig.store();
        store
            .install(
                "sha256:abc",
                &document("some-provider", "*.some-provider.example", "SOME_TOKEN"),
                &[],
            )
            .unwrap();
        let err = store
            .install(
                "sha256:def",
                &document("other-provider", "api.some-provider.example", "OTHER_TOKEN"),
                &[],
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("some-provider"), "{err}");
        assert_eq!(store.installed().unwrap().len(), 1, "nothing was stored");
    }

    #[test]
    fn reinstalling_a_connector_at_a_new_digest_replaces_it() {
        let rig = Rig::new();
        let store = rig.store();
        let doc = document("some-provider", "api.some-provider.example", "SOME_TOKEN");
        store.install("sha256:old", &doc, &[]).unwrap();
        store.install("sha256:new", &doc, &[]).unwrap();
        let installed = store.installed().unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].digest, "sha256:new");
    }

    #[test]
    fn an_install_whose_write_fails_surfaces_the_error() {
        let rig = Rig::new();
        *rig.set.fail_put.lock().unwrap() = true;
        assert!(
            rig.store()
                .install(
                    "sha256:abc",
                    &document("some-provider", "api.some-provider.example", "SOME_TOKEN"),
                    &[],
                )
                .is_err()
        );
    }

    #[test]
    fn an_unreadable_stored_document_refuses_the_next_install_and_names_it() {
        // Its `serves` and its variables are invisible while it cannot be parsed, so an overlapping connector would install cleanly and the ambiguous offer would surface at some later launch instead.
        let rig = Rig::new();
        rig.set
            .put("mystery", "sha256:xyz", b"not a document", &[])
            .unwrap();
        let err = rig
            .store()
            .install(
                "sha256:abc",
                &document("some-provider", "api.some-provider.example", "SOME_TOKEN"),
                &[],
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("mystery"), "{err}");
    }

    #[test]
    fn an_unreadable_stored_document_can_still_be_uninstalled() {
        // Refusing the install must not brick the machine: removing the offender needs no parse.
        let rig = Rig::new();
        rig.set
            .put("mystery", "sha256:xyz", b"not a document", &[])
            .unwrap();
        assert!(rig.store().uninstall("mystery").unwrap());
        assert!(
            rig.store()
                .install(
                    "sha256:abc",
                    &document("some-provider", "api.some-provider.example", "SOME_TOKEN"),
                    &[],
                )
                .is_ok()
        );
    }

    #[test]
    fn uninstalling_drops_every_connection_and_keeps_what_projects_granted() {
        // The pair most easily got backwards: connections are machine state, a grant is a project's decision.
        let rig = Rig::new();
        let store = rig.store();
        store
            .install(
                "sha256:abc",
                &document("some-provider", "api.some-provider.example", "SOME_TOKEN"),
                &[],
            )
            .unwrap();
        store
            .record_authentication("some-provider", "work", connection(Authority::default()))
            .unwrap();
        store
            .decide(
                "/work",
                "some-provider",
                granted("sha256:abc", Some("work"), Authority::default()),
            )
            .unwrap();

        assert!(store.uninstall("some-provider").unwrap());

        assert!(store.installed().unwrap().is_empty());
        assert!(store.connections_of("some-provider").unwrap().is_empty());
        assert_eq!(
            store.decision("/work", "some-provider").unwrap(),
            Some(granted("sha256:abc", Some("work"), Authority::default())),
            "uninstalling stops the offer; it does not retract a grant"
        );
    }

    #[test]
    fn uninstalling_leaves_another_connectors_connections_alone() {
        let rig = Rig::new();
        let store = rig.store();
        store
            .record_authentication("some-provider", "work", connection(Authority::default()))
            .unwrap();
        store
            .record_authentication("other-provider", "work", connection(Authority::default()))
            .unwrap();
        store.uninstall("some-provider").unwrap();
        assert_eq!(store.connections_of("other-provider").unwrap().len(), 1);
    }

    #[test]
    fn uninstalling_something_never_installed_reports_it() {
        assert!(!Rig::new().store().uninstall("absent").unwrap());
    }

    #[test]
    fn a_machine_may_hold_several_connections_of_one_connector() {
        let rig = Rig::new();
        let store = rig.store();
        store
            .record_authentication("some-provider", "work", connection(Authority::default()))
            .unwrap();
        store
            .record_authentication(
                "some-provider",
                "personal",
                connection(Authority::default()),
            )
            .unwrap();
        let held = store.connections_of("some-provider").unwrap();
        assert_eq!(
            held.keys().collect::<Vec<_>>(),
            ["personal", "work"],
            "both accounts stay signed in"
        );
    }

    #[test]
    fn dropping_one_connection_leaves_the_others() {
        let rig = Rig::new();
        let store = rig.store();
        for label in ["work", "personal"] {
            store
                .record_authentication("some-provider", label, connection(Authority::default()))
                .unwrap();
        }
        assert_eq!(
            store
                .drop_connections("some-provider", Some("work"))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .connections_of("some-provider")
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["personal"]
        );
    }

    #[test]
    fn dropping_with_no_label_drops_every_connection_of_that_connector() {
        let rig = Rig::new();
        let store = rig.store();
        for label in ["work", "personal"] {
            store
                .record_authentication("some-provider", label, connection(Authority::default()))
                .unwrap();
        }
        store
            .record_authentication("other-provider", "work", connection(Authority::default()))
            .unwrap();
        assert_eq!(store.drop_connections("some-provider", None).unwrap(), 2);
        assert_eq!(store.connections_of("other-provider").unwrap().len(), 1);
    }

    #[test]
    fn dropping_a_connection_a_connector_never_held_drops_nothing() {
        assert_eq!(
            Rig::new()
                .store()
                .drop_connections("some-provider", Some("work"))
                .unwrap(),
            0
        );
    }

    #[test]
    fn granting_again_replaces_the_prior_answer_and_reports_what_it_displaced() {
        let rig = Rig::new();
        let store = rig.store();
        let first = granted("sha256:abc", Some("work"), Authority::default());
        assert_eq!(
            store
                .decide("/work", "some-provider", first.clone())
                .unwrap(),
            None
        );
        let displaced = store
            .decide(
                "/work",
                "some-provider",
                granted("sha256:abc", Some("personal"), Authority::default()),
            )
            .unwrap();
        assert_eq!(
            displaced,
            Some(first),
            "a project holds one answer per connector"
        );
    }

    #[test]
    fn a_decline_replaces_a_grant_and_a_grant_replaces_a_decline() {
        let rig = Rig::new();
        let store = rig.store();
        store
            .decide("/work", "some-provider", RunDecision::Declined)
            .unwrap();
        assert_eq!(
            store.decision("/work", "some-provider").unwrap(),
            Some(RunDecision::Declined)
        );
        store
            .decide(
                "/work",
                "some-provider",
                granted("sha256:abc", None, Authority::default()),
            )
            .unwrap();
        assert!(matches!(
            store.decision("/work", "some-provider").unwrap(),
            Some(RunDecision::Granted { .. })
        ));
    }

    #[test]
    fn one_project_decision_does_not_reach_another_directory() {
        let rig = Rig::new();
        let store = rig.store();
        store
            .decide(
                "/work",
                "some-provider",
                granted("sha256:abc", None, Authority::default()),
            )
            .unwrap();
        assert_eq!(store.decision("/other", "some-provider").unwrap(), None);
    }

    #[test]
    fn a_grant_bound_to_other_bytes_does_not_apply_so_any_update_offers_again() {
        let rig = Rig::new();
        let store = rig.store();
        store
            .decide(
                "/work",
                "some-provider",
                granted("sha256:old", Some("work"), Authority::default()),
            )
            .unwrap();
        assert_eq!(
            store
                .grant_for("/work", "some-provider", "sha256:new")
                .unwrap(),
            None,
            "a republished connector has no grant here"
        );
        assert!(
            store
                .grant_for("/work", "some-provider", "sha256:old")
                .unwrap()
                .is_some(),
            "reinstalling the same digest resumes that grant"
        );
    }

    #[test]
    fn a_decline_is_never_read_as_a_grant_for_any_digest() {
        let rig = Rig::new();
        let store = rig.store();
        store
            .decide("/work", "some-provider", RunDecision::Declined)
            .unwrap();
        assert_eq!(
            store
                .grant_for("/work", "some-provider", "sha256:abc")
                .unwrap(),
            None
        );
    }

    #[test]
    fn forgetting_clears_a_grant_and_a_decline_alike() {
        let rig = Rig::new();
        let store = rig.store();
        for decision in [
            granted("sha256:abc", None, Authority::default()),
            RunDecision::Declined,
        ] {
            store.decide("/work", "some-provider", decision).unwrap();
            assert!(store.forget("/work", "some-provider").unwrap());
            assert_eq!(store.decision("/work", "some-provider").unwrap(), None);
        }
    }

    #[test]
    fn forgetting_a_project_that_decided_nothing_reports_it() {
        assert!(!Rig::new().store().forget("/work", "some-provider").unwrap());
    }

    #[test]
    fn a_re_authentication_reporting_the_same_authority_keeps_every_grant() {
        // Signing in again must not cost the user their consent, or updates become something to avoid.
        let rig = Rig::new();
        let store = rig.store();
        let authority = Authority::of(["repo:read"]);
        store
            .decide(
                "/work",
                "some-provider",
                granted("sha256:abc", Some("work"), authority.clone()),
            )
            .unwrap();
        let invalidated = store
            .record_authentication("some-provider", "work", connection(authority))
            .unwrap();
        assert!(invalidated.is_empty());
        assert!(store.decision("/work", "some-provider").unwrap().is_some());
    }

    #[test]
    fn a_re_authentication_reporting_different_authority_invalidates_the_grants_naming_it() {
        let rig = Rig::new();
        let store = rig.store();
        store
            .decide(
                "/work",
                "some-provider",
                granted("sha256:abc", Some("work"), Authority::of(["repo:read"])),
            )
            .unwrap();
        let invalidated = store
            .record_authentication(
                "some-provider",
                "work",
                connection(Authority::of(["repo:read", "repo:write"])),
            )
            .unwrap();
        assert_eq!(invalidated.len(), 1);
        assert_eq!(
            store.decision("/work", "some-provider").unwrap(),
            None,
            "the project is asked again rather than silently widened"
        );
    }

    #[test]
    fn narrowed_authority_invalidates_too_because_difference_is_the_test() {
        let rig = Rig::new();
        let store = rig.store();
        store
            .decide(
                "/work",
                "some-provider",
                granted(
                    "sha256:abc",
                    Some("work"),
                    Authority::of(["repo:read", "repo:write"]),
                ),
            )
            .unwrap();
        assert_eq!(
            store
                .record_authentication(
                    "some-provider",
                    "work",
                    connection(Authority::of(["repo:read"]))
                )
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn invalidation_reaches_only_the_grants_naming_that_connection() {
        let rig = Rig::new();
        let store = rig.store();
        store
            .decide(
                "/work",
                "some-provider",
                granted("sha256:abc", Some("work"), Authority::of(["repo:read"])),
            )
            .unwrap();
        store
            .decide(
                "/other",
                "some-provider",
                granted("sha256:abc", Some("personal"), Authority::of(["repo:read"])),
            )
            .unwrap();
        store
            .decide(
                "/work",
                "other-provider",
                granted("sha256:abc", Some("work"), Authority::of(["repo:read"])),
            )
            .unwrap();

        let invalidated = store
            .record_authentication(
                "some-provider",
                "work",
                connection(Authority::of(["admin"])),
            )
            .unwrap();

        assert_eq!(invalidated.len(), 1);
        assert!(store.decision("/other", "some-provider").unwrap().is_some());
        assert!(store.decision("/work", "other-provider").unwrap().is_some());
    }

    #[test]
    fn a_grant_naming_no_connection_is_never_invalidated_by_an_authentication() {
        // A method with no `auth` has no connection, so nothing an authentication reports can concern it.
        let rig = Rig::new();
        let store = rig.store();
        store
            .decide(
                "/work",
                "some-provider",
                granted("sha256:abc", None, Authority::default()),
            )
            .unwrap();
        assert!(
            store
                .record_authentication(
                    "some-provider",
                    "work",
                    connection(Authority::of(["admin"]))
                )
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_declined_project_is_not_invalidated_by_an_authentication() {
        let rig = Rig::new();
        let store = rig.store();
        store
            .decide("/work", "some-provider", RunDecision::Declined)
            .unwrap();
        assert!(
            store
                .record_authentication(
                    "some-provider",
                    "work",
                    connection(Authority::of(["admin"]))
                )
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.decision("/work", "some-provider").unwrap(),
            Some(RunDecision::Declined)
        );
    }

    #[test]
    fn a_values_write_that_fails_surfaces_rather_than_reporting_a_stored_connection() {
        let rig = Rig::new();
        *rig.values.fail_save.lock().unwrap() = true;
        assert!(
            rig.store()
                .record_authentication("some-provider", "work", connection(Authority::default()))
                .is_err()
        );
    }

    #[test]
    fn a_grants_write_that_fails_during_invalidation_never_leaves_a_grant_backed_by_wider_authority()
     {
        // A grant that survives beside a re-authenticated connection is a grant silently widened: the run would apply the method backed by authority the project never consented to.
        let rig = Rig::new();
        rig.grants.state.lock().unwrap().insert(
            run_key("/work", "some-provider"),
            granted("sha256:abc", Some("work"), Authority::of(["repo:read"])),
        );
        *rig.grants.fail_save.lock().unwrap() = true;

        assert!(
            rig.store()
                .record_authentication(
                    "some-provider",
                    "work",
                    connection(Authority::of(["admin"]))
                )
                .is_err()
        );

        let store = rig.store();
        let grant_applies = store
            .grant_for("/work", "some-provider", "sha256:abc")
            .unwrap()
            .is_some();
        let connection_widened = store
            .connections_of("some-provider")
            .unwrap()
            .get("work")
            .is_some_and(|held| held.authority == Authority::of(["admin"]));
        assert!(
            !(grant_applies && connection_widened),
            "the grant still applies while the connection reports different authority"
        );
    }

    #[test]
    fn a_grants_write_that_fails_surfaces_from_decide_and_forget() {
        let rig = Rig::new();
        *rig.grants.fail_save.lock().unwrap() = true;
        assert!(
            rig.store()
                .decide(
                    "/work",
                    "some-provider",
                    granted("sha256:abc", None, Authority::default())
                )
                .is_err()
        );
        rig.grants
            .state
            .lock()
            .unwrap()
            .insert(run_key("/work", "some-provider"), RunDecision::Declined);
        assert!(rig.store().forget("/work", "some-provider").is_err());
    }

    #[test]
    fn a_values_write_that_fails_during_uninstall_never_leaves_values_without_their_connector() {
        // Values outliving the connector both keep a real token on disk after `uninstall` and let a reinstall resume the connection, which §7.1 says a reinstall must not do.
        let rig = Rig::new();
        rig.set
            .put(
                "some-provider",
                "sha256:abc",
                &document("some-provider", "api.some-provider.example", "SOME_TOKEN"),
                &[],
            )
            .unwrap();
        rig.values.state.lock().unwrap().insert(
            connection_key("some-provider", "work"),
            connection(Authority::default()),
        );
        *rig.values.fail_save.lock().unwrap() = true;

        assert!(rig.store().uninstall("some-provider").is_err());

        let store = rig.store();
        let still_installed = store
            .installed()
            .unwrap()
            .iter()
            .any(|entry| entry.name == "some-provider");
        assert!(
            still_installed || store.connections_of("some-provider").unwrap().is_empty(),
            "the connector is gone and its values are not"
        );
    }

    #[test]
    fn a_connection_debug_does_not_print_the_value_it_holds() {
        // A derived Debug would put a real token into every log line that mentions a connection.
        let rendered = format!("{:?}", connection(Authority::of(["repo:read"])));
        assert!(!rendered.contains("real-secret"), "{rendered}");
        assert!(rendered.contains("redacted"), "{rendered}");
        assert!(rendered.contains("repo:read"), "{rendered}");
    }

    #[test]
    fn an_authority_is_stored_sorted_so_two_orders_of_one_set_compare_equal() {
        // The canonicality §3.2.4 requires of a mechanism, made unrepresentable rather than asserted.
        let one = Authority::of(["b", "a"]);
        let other = Authority::of(["a", "b"]);
        assert_eq!(one, other);
        assert_eq!(serde_json::to_string(&one).unwrap(), r#"["a","b"]"#);
        assert_eq!(one.0.iter().collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn a_token_exchange_reports_no_authority() {
        assert!(Authority::default().0.is_empty());
        assert!(!Authority::of(["repo:read"]).0.is_empty());
    }

    #[test]
    fn a_key_holding_no_separator_is_ignored_rather_than_panicking() {
        // Nothing this store writes lacks one, so this pins that a hand-edited file cannot crash the service.
        let rig = Rig::new();
        rig.values
            .state
            .lock()
            .unwrap()
            .insert("handwritten".to_string(), connection(Authority::default()));
        rig.grants
            .state
            .lock()
            .unwrap()
            .insert("handwritten".to_string(), RunDecision::Declined);
        let store = rig.store();
        assert!(store.connections_of("handwritten").unwrap().is_empty());
        assert_eq!(store.drop_connections("handwritten", None).unwrap(), 0);
        assert!(
            grants_invalidated_by(
                &rig.grants.load().unwrap(),
                "handwritten",
                "work",
                &Authority::default()
            )
            .is_empty()
        );
    }

    #[test]
    fn an_invalidated_grant_names_the_run_that_holds_it_and_not_the_store_key() {
        // These reach the user through `lns connector connect`, and a store key carries the NUL separator.
        let mut grants = DecisionFile::new();
        grants.insert(
            run_key("1a2b3c4d", "some-provider"),
            granted("sha256:abc", Some("work"), Authority::of(["repo:read"])),
        );
        assert_eq!(
            grants_invalidated_by(&grants, "some-provider", "work", &Authority::of(["admin"])),
            vec!["1a2b3c4d".to_string()]
        );
    }

    #[test]
    fn the_invalidated_keys_come_back_in_a_stable_order() {
        // They are reported to the user, so two runs must not shuffle them.
        let mut grants = DecisionFile::new();
        for dir in ["/z-project", "/a-project", "/m-project"] {
            grants.insert(
                run_key(dir, "some-provider"),
                granted("sha256:abc", Some("work"), Authority::of(["repo:read"])),
            );
        }
        let invalidated =
            grants_invalidated_by(&grants, "some-provider", "work", &Authority::of(["admin"]));
        let mut sorted = invalidated.clone();
        sorted.sort();
        assert_eq!(invalidated, sorted);
        assert_eq!(invalidated.len(), 3);
    }
}
