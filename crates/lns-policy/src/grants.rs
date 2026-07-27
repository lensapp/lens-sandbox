//! Per-workload connector grants live in `~/.lns-workload-grants.json`, per-machine and never committed — a grant lets one workload spend a machine-global credential, which is a per-machine risk acceptance, not a shareable rule.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

/// What a sandbox run is, for the purpose of remembering a connector grant against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkloadIdentity {
    Definition { dir: String },
    Reference { repo: String, digest: String },
}

impl WorkloadIdentity {
    pub fn key(&self) -> String {
        match self {
            WorkloadIdentity::Definition { dir } => format!("def:{dir}"),
            WorkloadIdentity::Reference { repo, digest } => format!("ref:{repo}@{digest}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GrantVerdict {
    Allow,
    Deny,
}

/// A remembered per-workload decision, carrying the connector's env-var + injection-domain snapshot so a later catalog redefinition invalidates the grant instead of silently inheriting it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantRecord {
    pub project: String,
    pub workload: String,
    pub connector: String,
    pub verdict: GrantVerdict,
    pub env_var: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injection_domains: Vec<String>,
}

impl GrantRecord {
    fn new(
        project: impl Into<String>,
        workload: &WorkloadIdentity,
        connector: impl Into<String>,
        verdict: GrantVerdict,
        env_var: impl Into<String>,
        injection_domains: Vec<String>,
    ) -> Self {
        Self {
            project: project.into(),
            workload: workload.key(),
            connector: connector.into(),
            verdict,
            env_var: env_var.into(),
            injection_domains,
        }
    }

    pub fn allow(
        project: impl Into<String>,
        workload: &WorkloadIdentity,
        connector: impl Into<String>,
        env_var: impl Into<String>,
        injection_domains: Vec<String>,
    ) -> Self {
        Self::new(
            project,
            workload,
            connector,
            GrantVerdict::Allow,
            env_var,
            injection_domains,
        )
    }

    pub fn deny(
        project: impl Into<String>,
        workload: &WorkloadIdentity,
        connector: impl Into<String>,
        env_var: impl Into<String>,
        injection_domains: Vec<String>,
    ) -> Self {
        Self::new(
            project,
            workload,
            connector,
            GrantVerdict::Deny,
            env_var,
            injection_domains,
        )
    }

    /// True when the (env var, injection domains) this grant was recorded against still matches what the connector discloses now, so a redefinition invalidates the grant instead of inheriting it; `domains` must already be sorted and de-duplicated, as a provider's disclosure snapshot is.
    pub fn matches_disclosure(&self, env_var: &str, domains: &[String]) -> bool {
        let mut recorded = self.injection_domains.clone();
        recorded.sort();
        recorded.dedup();
        self.env_var == env_var && recorded == domains
    }

    /// True when this grant was recorded without a disclosure to pin it to — a decision about an id the run resolved no provider for, which no redefinition can invalidate.
    pub fn has_no_disclosure(&self) -> bool {
        self.env_var.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkloadGrantFile {
    pub grants: Vec<GrantRecord>,
}

impl WorkloadGrantFile {
    fn triple_matches(g: &GrantRecord, project: &str, workload_key: &str, connector: &str) -> bool {
        g.project == project && g.workload == workload_key && g.connector == connector
    }

    fn position(
        &self,
        project: &str,
        workload: &WorkloadIdentity,
        connector: &str,
    ) -> Option<usize> {
        let key = workload.key();
        self.grants
            .iter()
            .position(|g| Self::triple_matches(g, project, &key, connector))
    }

    pub fn lookup(
        &self,
        project: &str,
        workload: &WorkloadIdentity,
        connector: &str,
    ) -> Option<&GrantRecord> {
        self.position(project, workload, connector)
            .map(|i| &self.grants[i])
    }

    pub fn upsert(&mut self, record: GrantRecord) {
        match self.grants.iter().position(|g| {
            Self::triple_matches(g, &record.project, &record.workload, &record.connector)
        }) {
            Some(i) => self.grants[i] = record,
            None => self.grants.push(record),
        }
    }

    pub fn remove(&mut self, project: &str, workload: &WorkloadIdentity, connector: &str) -> bool {
        let key = workload.key();
        let before = self.grants.len();
        self.grants
            .retain(|g| !Self::triple_matches(g, project, &key, connector));
        before != self.grants.len()
    }

    pub fn clear_project_connector(&mut self, project: &str, connector: &str) -> usize {
        let before = self.grants.len();
        self.grants
            .retain(|g| !(g.project == project && g.connector == connector));
        before - self.grants.len()
    }

    pub fn for_project<'a>(&'a self, project: &'a str) -> impl Iterator<Item = &'a GrantRecord> {
        self.grants.iter().filter(move |g| g.project == project)
    }
}

/// A read-modify-write of the sidecar failed to serialize against other writers, to load it, or — after the mutation was applied in memory — to persist it.
#[derive(Debug)]
pub enum GrantUpdateError {
    Lock(io::Error),
    Load(io::Error),
    Save(io::Error),
}

impl std::fmt::Display for GrantUpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrantUpdateError::Lock(e) => write!(f, "locking the grant sidecar: {e}"),
            GrantUpdateError::Load(e) => write!(f, "reading the grant sidecar: {e}"),
            GrantUpdateError::Save(e) => write!(f, "writing the grant sidecar: {e}"),
        }
    }
}

impl std::error::Error for GrantUpdateError {}

/// Runs the load-mutate-save under the process-wide lock; a store whose backing medium is shared with other processes wraps this in a cross-process lock of its own.
fn serialized_update<S: GrantStore + ?Sized>(
    store: &S,
    mutate: &mut dyn FnMut(&mut WorkloadGrantFile) -> bool,
) -> Result<WorkloadGrantFile, GrantUpdateError> {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let mut file = store.load().map_err(GrantUpdateError::Load)?;
    if mutate(&mut file)
        && let Err(e) = store.save(&file)
    {
        return Err(GrantUpdateError::Save(e));
    }
    Ok(file)
}

pub trait GrantStore: Send + Sync {
    fn load(&self) -> io::Result<WorkloadGrantFile>;
    fn save(&self, state: &WorkloadGrantFile) -> io::Result<()>;

    /// Serializes a load-mutate-save against concurrent writers so they can't lose each other's grants; `mutate` returns whether it changed anything (only then is a save attempted) and must not call back into `update`.
    fn update(
        &self,
        mutate: &mut dyn FnMut(&mut WorkloadGrantFile) -> bool,
    ) -> Result<WorkloadGrantFile, GrantUpdateError> {
        serialized_update(self, mutate)
    }
}

/// The grant-store project key for a policy file: its canonical path, so the same directory keys identically no matter how a run addressed it, falling back to the raw path when canonicalization can't resolve it.
pub fn project_key(policy_path: &Path) -> String {
    policy_path
        .canonicalize()
        .unwrap_or_else(|_| policy_path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Falls back to `./.lns-workload-grants.json` when `HOME` is unset rather than panicking.
pub fn default_workload_grants_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LNS_WORKLOAD_GRANTS_PATH") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".lns-workload-grants.json"))
        .unwrap_or_else(|| PathBuf::from(".lns-workload-grants.json"))
}

pub struct JsonFileGrantStore {
    pub path: PathBuf,
}

impl JsonFileGrantStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Suffixed rather than `with_extension`, which would replace the sidecar's own extension on an arbitrary `LNS_WORKLOAD_GRANTS_PATH`.
    fn lock_path(&self) -> PathBuf {
        let mut p = self.path.clone().into_os_string();
        p.push(".lock");
        PathBuf::from(p)
    }
}

impl GrantStore for JsonFileGrantStore {
    /// The `lns` CLI and `lns-service` both read-modify-write this file, so the in-process lock alone would let a `lns connector revoke` be silently undone by a run persisting a grant it read beforehand.
    fn update(
        &self,
        mutate: &mut dyn FnMut(&mut WorkloadGrantFile) -> bool,
    ) -> Result<WorkloadGrantFile, GrantUpdateError> {
        let _flock = crate::secure_file::lock_sidecar_exclusive(&self.lock_path())
            .map_err(GrantUpdateError::Lock)?;
        serialized_update(self, mutate)
    }

    fn load(&self) -> io::Result<WorkloadGrantFile> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(WorkloadGrantFile::default()),
            Err(e) => Err(e),
        }
    }

    fn save(&self, state: &WorkloadGrantFile) -> io::Result<()> {
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        crate::secure_file::write_json_secret_atomic(&self.path, json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn def(dir: &str) -> WorkloadIdentity {
        WorkloadIdentity::Definition { dir: dir.into() }
    }

    fn reference(repo: &str, digest: &str) -> WorkloadIdentity {
        WorkloadIdentity::Reference {
            repo: repo.into(),
            digest: digest.into(),
        }
    }

    #[test]
    fn definition_identity_keys_by_directory() {
        assert_eq!(def("/Users/me/app").key(), "def:/Users/me/app");
    }

    #[test]
    fn reference_identity_keys_by_repo_and_digest() {
        assert_eq!(
            reference("ghcr.io/acme/agent", "sha256:abc").key(),
            "ref:ghcr.io/acme/agent@sha256:abc"
        );
    }

    #[test]
    fn verdicts_serialize_to_kebab_case() {
        assert_eq!(
            serde_json::to_value(GrantVerdict::Allow).unwrap(),
            json!("allow")
        );
        assert_eq!(
            serde_json::to_value(GrantVerdict::Deny).unwrap(),
            json!("deny")
        );
    }

    #[test]
    fn unknown_verdict_deserializes_as_error() {
        let r: serde_json::Result<GrantVerdict> = serde_json::from_str(r#""maybe""#);
        assert!(r.is_err());
    }

    #[test]
    fn allow_and_deny_builders_set_the_workload_key_and_verdict() {
        let a = GrantRecord::allow(
            "/proj",
            &reference("ghcr.io/acme/agent", "sha256:abc"),
            "some-provider",
            "SOME_TOKEN",
            vec!["api.some-provider.example".into()],
        );
        assert_eq!(a.workload, "ref:ghcr.io/acme/agent@sha256:abc");
        assert_eq!(a.verdict, GrantVerdict::Allow);
        assert_eq!(a.env_var, "SOME_TOKEN");

        let d = GrantRecord::deny(
            "/proj",
            &def("/proj"),
            "some-provider",
            "SOME_TOKEN",
            vec![],
        );
        assert_eq!(d.workload, "def:/proj");
        assert_eq!(d.verdict, GrantVerdict::Deny);
    }

    #[test]
    fn matches_disclosure_accepts_the_recorded_snapshot_in_any_stored_order() {
        let g = GrantRecord::allow(
            "/proj",
            &def("/proj"),
            "some-provider",
            "SOME_TOKEN",
            vec!["b.example".into(), "a.example".into(), "a.example".into()],
        );
        assert!(
            g.matches_disclosure("SOME_TOKEN", &["a.example".into(), "b.example".into()]),
            "a hand-edited sidecar's unsorted duplicates must still match the provider's sorted snapshot"
        );
    }

    #[test]
    fn matches_disclosure_rejects_a_drifted_env_var_or_domain_set() {
        let g = GrantRecord::allow(
            "/proj",
            &def("/proj"),
            "some-provider",
            "SOME_TOKEN",
            vec!["api.some-provider.example".into()],
        );
        assert!(!g.matches_disclosure("OTHER_TOKEN", &["api.some-provider.example".into()]));
        assert!(!g.matches_disclosure("SOME_TOKEN", &["api.other.example".into()]));
        assert!(
            !g.matches_disclosure("SOME_TOKEN", &[]),
            "a connector that stopped injecting anywhere is not the one that was approved"
        );
    }

    #[test]
    fn has_no_disclosure_marks_only_a_record_with_no_env_var() {
        let pinned = GrantRecord::deny(
            "/proj",
            &def("/proj"),
            "some-provider",
            "SOME_TOKEN",
            vec![],
        );
        let unpinned = GrantRecord::deny("/proj", &def("/proj"), "some-provider", "", vec![]);
        assert!(!pinned.has_no_disclosure());
        assert!(unpinned.has_no_disclosure());
    }

    #[test]
    fn record_round_trips_and_omits_empty_domains() {
        let with_domains = GrantRecord::allow(
            "/proj",
            &def("/proj"),
            "some-provider",
            "SOME_TOKEN",
            vec!["api.some-provider.example".into()],
        );
        let v = serde_json::to_value(&with_domains).unwrap();
        assert_eq!(v["injection_domains"], json!(["api.some-provider.example"]));
        let parsed: GrantRecord = serde_json::from_value(v).unwrap();
        assert_eq!(parsed, with_domains);

        let no_domains = GrantRecord::allow(
            "/proj",
            &def("/proj"),
            "some-provider",
            "SOME_TOKEN",
            vec![],
        );
        let v = serde_json::to_value(&no_domains).unwrap();
        assert_eq!(
            v.get("injection_domains"),
            None,
            "empty domains are skipped"
        );
    }

    #[test]
    fn file_serializes_transparently_as_a_bare_array() {
        let mut file = WorkloadGrantFile::default();
        file.upsert(GrantRecord::allow(
            "/proj",
            &def("/proj"),
            "some-provider",
            "SOME_TOKEN",
            vec![],
        ));
        let v = serde_json::to_value(&file).unwrap();
        assert!(v.is_array(), "expected a bare array, got: {v}");
        assert_eq!(v.as_array().unwrap().len(), 1);
    }

    #[test]
    fn lookup_matches_only_the_exact_project_workload_connector_triple() {
        let mut file = WorkloadGrantFile::default();
        let workload = reference("ghcr.io/acme/agent", "sha256:abc");
        file.upsert(GrantRecord::allow(
            "/proj",
            &workload,
            "some-provider",
            "SOME_TOKEN",
            vec![],
        ));

        assert!(file.lookup("/proj", &workload, "some-provider").is_some());
        assert!(
            file.lookup("/other", &workload, "some-provider").is_none(),
            "a different project must not match"
        );
        assert!(
            file.lookup("/proj", &def("/proj"), "some-provider")
                .is_none(),
            "a different workload must not match"
        );
        assert!(
            file.lookup("/proj", &workload, "some-oauth").is_none(),
            "a different connector must not match"
        );
    }

    #[test]
    fn upsert_replaces_the_matching_triple_without_duplicating() {
        let mut file = WorkloadGrantFile::default();
        let workload = def("/proj");
        file.upsert(GrantRecord::allow(
            "/proj",
            &workload,
            "some-provider",
            "SOME_TOKEN",
            vec![],
        ));
        file.upsert(GrantRecord::deny(
            "/proj",
            &workload,
            "some-provider",
            "SOME_TOKEN",
            vec![],
        ));

        assert_eq!(file.grants.len(), 1, "same triple upserts in place");
        assert_eq!(
            file.lookup("/proj", &workload, "some-provider")
                .unwrap()
                .verdict,
            GrantVerdict::Deny
        );
    }

    #[test]
    fn upsert_keeps_distinct_triples_side_by_side() {
        let mut file = WorkloadGrantFile::default();
        file.upsert(GrantRecord::allow(
            "/proj",
            &def("/proj"),
            "some-provider",
            "T",
            vec![],
        ));
        file.upsert(GrantRecord::allow(
            "/proj",
            &reference("ghcr.io/acme/agent", "sha256:abc"),
            "some-provider",
            "T",
            vec![],
        ));
        assert_eq!(file.grants.len(), 2);
    }

    #[test]
    fn remove_reports_whether_a_triple_was_present() {
        let mut file = WorkloadGrantFile::default();
        let workload = def("/proj");
        file.upsert(GrantRecord::allow(
            "/proj",
            &workload,
            "some-provider",
            "T",
            vec![],
        ));

        assert!(file.remove("/proj", &workload, "some-provider"));
        assert!(!file.remove("/proj", &workload, "some-provider"));
        assert!(file.grants.is_empty());
    }

    #[test]
    fn remove_deletes_every_duplicate_of_a_triple_not_just_the_first() {
        let workload = def("/proj");
        let dup = GrantRecord::allow("/proj", &workload, "some-provider", "T", vec![]);
        let mut file = WorkloadGrantFile {
            grants: vec![dup.clone(), dup],
        };

        assert!(file.remove("/proj", &workload, "some-provider"));
        assert!(
            file.lookup("/proj", &workload, "some-provider").is_none(),
            "no shadow duplicate may survive a revoke"
        );
    }

    #[test]
    fn clear_project_connector_drops_all_workloads_for_that_pair_only() {
        let mut file = WorkloadGrantFile::default();
        file.upsert(GrantRecord::allow(
            "/proj",
            &def("/proj"),
            "some-provider",
            "T",
            vec![],
        ));
        file.upsert(GrantRecord::allow(
            "/proj",
            &reference("ghcr.io/acme/agent", "sha256:abc"),
            "some-provider",
            "T",
            vec![],
        ));
        file.upsert(GrantRecord::allow(
            "/proj",
            &def("/proj"),
            "some-oauth",
            "T",
            vec![],
        ));
        file.upsert(GrantRecord::allow(
            "/other",
            &def("/other"),
            "some-provider",
            "T",
            vec![],
        ));

        let removed = file.clear_project_connector("/proj", "some-provider");
        assert_eq!(removed, 2, "both /proj workloads for some-provider go");
        assert!(file.lookup("/proj", &def("/proj"), "some-oauth").is_some());
        assert!(
            file.lookup("/other", &def("/other"), "some-provider")
                .is_some()
        );
    }

    #[test]
    fn for_project_yields_only_that_projects_grants() {
        let mut file = WorkloadGrantFile::default();
        file.upsert(GrantRecord::allow(
            "/proj",
            &def("/proj"),
            "some-provider",
            "T",
            vec![],
        ));
        file.upsert(GrantRecord::allow(
            "/other",
            &def("/other"),
            "some-provider",
            "T",
            vec![],
        ));

        let projects: Vec<&str> = file
            .for_project("/proj")
            .map(|g| g.project.as_str())
            .collect();
        assert_eq!(projects, ["/proj"]);
    }

    #[test]
    fn load_returns_empty_state_when_file_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileGrantStore::new(dir.path().join("never-created.json"));
        assert!(store.load().unwrap().grants.is_empty());
    }

    #[test]
    fn load_returns_invalid_data_for_malformed_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("grants.json");
        fs::write(&path, "{ not json").unwrap();
        let store = JsonFileGrantStore::new(path);
        assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn load_propagates_non_not_found_io_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileGrantStore::new(dir.path().to_path_buf());
        assert_ne!(store.load().unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn save_then_load_round_trips_full_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileGrantStore::new(dir.path().join("grants.json"));
        let mut original = WorkloadGrantFile::default();
        original.upsert(GrantRecord::allow(
            "/proj",
            &reference("ghcr.io/acme/agent", "sha256:abc"),
            "some-provider",
            "SOME_TOKEN",
            vec!["api.some-provider.example".into()],
        ));
        original.upsert(GrantRecord::deny(
            "/proj",
            &def("/proj"),
            "some-oauth",
            "SOME_OAUTH_TOKEN",
            vec![],
        ));
        store.save(&original).unwrap();
        assert_eq!(store.load().unwrap(), original);
    }

    #[test]
    fn save_overwrites_existing_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileGrantStore::new(dir.path().join("grants.json"));
        let mut first = WorkloadGrantFile::default();
        first.upsert(GrantRecord::allow(
            "/proj",
            &def("/proj"),
            "some-provider",
            "T",
            vec![],
        ));
        store.save(&first).unwrap();

        let second = WorkloadGrantFile::default();
        store.save(&second).unwrap();
        assert!(store.load().unwrap().grants.is_empty());
    }

    #[test]
    fn project_key_canonicalizes_an_existing_policy_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        fs::write(&path, "network: {}\n").unwrap();
        assert_eq!(
            project_key(&path),
            fs::canonicalize(&path).unwrap().to_string_lossy()
        );
    }

    #[test]
    fn project_key_falls_back_to_the_raw_path_when_it_cannot_be_canonicalized() {
        let path = Path::new("/no/such/dir/lns-policy.yaml");
        assert_eq!(project_key(path), "/no/such/dir/lns-policy.yaml");
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_path_uses_override_when_set() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::set("LNS_WORKLOAD_GRANTS_PATH", "/tmp/custom-grants.json");
        let _g2 = EnvVarGuard::set("HOME", "/tmp/home-should-be-ignored");
        assert_eq!(
            default_workload_grants_path(),
            PathBuf::from("/tmp/custom-grants.json")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_path_falls_back_to_home_dotfile() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_WORKLOAD_GRANTS_PATH");
        let _g2 = EnvVarGuard::set("HOME", "/home/dev");
        assert_eq!(
            default_workload_grants_path(),
            PathBuf::from("/home/dev/.lns-workload-grants.json")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_path_falls_back_to_cwd_when_home_unset() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_WORKLOAD_GRANTS_PATH");
        let _g2 = EnvVarGuard::unset("HOME");
        assert_eq!(
            default_workload_grants_path(),
            PathBuf::from(".lns-workload-grants.json")
        );
    }

    struct FaultyStore {
        inner: Mutex<WorkloadGrantFile>,
        fail_load: bool,
        fail_save: bool,
    }

    impl GrantStore for FaultyStore {
        fn load(&self) -> io::Result<WorkloadGrantFile> {
            if self.fail_load {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "corrupt sidecar",
                ));
            }
            Ok(self.inner.lock().unwrap().clone())
        }
        fn save(&self, state: &WorkloadGrantFile) -> io::Result<()> {
            if self.fail_save {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
            }
            *self.inner.lock().unwrap() = state.clone();
            Ok(())
        }
    }

    #[test]
    fn update_applies_the_mutation_and_persists_when_it_reports_a_change() {
        let store = FaultyStore {
            inner: Mutex::new(WorkloadGrantFile::default()),
            fail_load: false,
            fail_save: false,
        };
        let returned = store
            .update(&mut |file| {
                file.upsert(GrantRecord::allow(
                    "/proj",
                    &def("/proj"),
                    "some-provider",
                    "SOME_TOKEN",
                    vec![],
                ));
                true
            })
            .unwrap();
        assert!(
            returned
                .lookup("/proj", &def("/proj"), "some-provider")
                .is_some(),
            "update returns the mutated file"
        );
        assert!(
            store
                .load()
                .unwrap()
                .lookup("/proj", &def("/proj"), "some-provider")
                .is_some(),
            "and persists it through the store"
        );
    }

    #[test]
    fn update_skips_the_save_when_the_mutation_reports_no_change() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("grants.json");
        let store = JsonFileGrantStore::new(path.clone());
        let returned = store.update(&mut |_| false).unwrap();
        assert!(returned.grants.is_empty());
        assert!(
            !path.exists(),
            "a no-change update must not create the sidecar file"
        );
    }

    #[test]
    fn update_surfaces_a_load_failure_as_the_load_variant() {
        let store = FaultyStore {
            inner: Mutex::new(WorkloadGrantFile::default()),
            fail_load: true,
            fail_save: false,
        };
        let err = store.update(&mut |_| true).unwrap_err();
        assert!(matches!(err, GrantUpdateError::Load(_)));
        assert_eq!(
            format!("{err}"),
            "reading the grant sidecar: corrupt sidecar"
        );
    }

    #[test]
    fn update_surfaces_a_save_failure_without_persisting_the_mutation() {
        let store = FaultyStore {
            inner: Mutex::new(WorkloadGrantFile::default()),
            fail_load: false,
            fail_save: true,
        };
        let err = store.update(&mut upsert_some_provider).unwrap_err();
        assert!(matches!(&err, GrantUpdateError::Save(_)));
        assert_eq!(format!("{err}"), "writing the grant sidecar: denied");
        assert!(
            store
                .load()
                .unwrap()
                .lookup("/proj", &def("/proj"), "some-provider")
                .is_none(),
            "a failed save leaves the stored file untouched, so the caller's error is the only signal the grant did not land"
        );
    }

    fn grant_of(connector: &str) -> GrantRecord {
        GrantRecord::allow("/proj", &def("/proj"), connector, "SOME_TOKEN", vec![])
    }

    fn upsert_some_provider(file: &mut WorkloadGrantFile) -> bool {
        file.upsert(grant_of("some-provider"));
        true
    }

    fn some_provider_granted(store: &JsonFileGrantStore) -> bool {
        store
            .load()
            .unwrap()
            .lookup("/proj", &def("/proj"), "some-provider")
            .is_some()
    }

    #[test]
    fn update_holds_the_file_lock_across_the_whole_load_mutate_save_window() {
        // The probe is an independent open-file-description, which flock excludes exactly as it would another process; it never touches the in-process mutex, so only the file lock can be what blocks it.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("grants.json");
        let lock_path = dir.path().join("grants.json.lock");
        let store = JsonFileGrantStore::new(path);

        store
            .update(&mut |file| {
                let probe = fs::File::open(&lock_path).expect("lockfile exists during the update");
                assert!(
                    matches!(probe.try_lock(), Err(fs::TryLockError::WouldBlock)),
                    "a second process must not read-modify-write the sidecar mid-update"
                );
                upsert_some_provider(file)
            })
            .unwrap();

        let after = fs::File::open(&lock_path).unwrap();
        assert!(
            after.try_lock().is_ok(),
            "the lock is released once the update completes"
        );
    }

    #[test]
    fn update_fails_closed_without_touching_the_sidecar_when_the_lock_cannot_be_acquired() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("grants.json");
        fs::create_dir(dir.path().join("grants.json.lock")).unwrap();
        let store = JsonFileGrantStore::new(path.clone());

        let err = store.update(&mut upsert_some_provider).unwrap_err();

        assert!(matches!(err, GrantUpdateError::Lock(_)));
        assert!(
            format!("{err}").starts_with("locking the grant sidecar: "),
            "got: {err}"
        );
        assert!(
            !path.exists(),
            "an unlockable sidecar must not be written unserialized"
        );
    }

    #[test]
    fn update_waits_for_a_foreign_lock_holder_rather_than_failing() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileGrantStore::new(dir.path().join("grants.json"));
        let held = crate::secure_file::lock_sidecar_exclusive(&dir.path().join("grants.json.lock"))
            .unwrap();
        // Set before the release, so a writer that only ever enters after the release always observes it — a `try_lock` that bailed on contention, or one that ignored the holder, would not.
        let released = AtomicBool::new(false);

        std::thread::scope(|scope| {
            let writer = scope.spawn(|| {
                store
                    .update(&mut |file| {
                        assert!(
                            released.load(Ordering::Acquire),
                            "a contended writer must wait for the holder, not read-modify-write alongside it"
                        );
                        upsert_some_provider(file)
                    })
                    .expect("contention is waited out, never surfaced as an error")
            });
            released.store(true, Ordering::Release);
            drop(held);
            writer.join().unwrap();
        });

        assert!(
            some_provider_granted(&store),
            "the grant the contended writer was carrying still lands"
        );
    }

    #[test]
    fn update_serializes_concurrent_writers_without_losing_grants() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileGrantStore::new(dir.path().join("grants.json"));
        std::thread::scope(|scope| {
            for i in 0..16 {
                let store = &store;
                scope.spawn(move || {
                    store
                        .update(&mut |file| {
                            file.upsert(GrantRecord::allow(
                                "/proj",
                                &def("/proj"),
                                format!("some-provider-{i}"),
                                "SOME_TOKEN",
                                vec![],
                            ));
                            true
                        })
                        .unwrap();
                });
            }
        });
        assert_eq!(
            store.load().unwrap().grants.len(),
            16,
            "every concurrent writer's grant must survive; none is clobbered by an interleaved read-modify-write"
        );
    }
}
