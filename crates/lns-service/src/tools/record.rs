use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const RECORD_SCHEMA_VERSION: u32 = 1;

/// The machine-level memory of what each requested spec resolved to, so `node@22` never drifts between runs on this machine even when upstream releases a newer 22.x; a `@latest` entry is instead the fallback for a run that cannot reach the index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedRecord {
    pub schema_version: u32,
    #[serde(default)]
    pub engine_version: String,
    #[serde(default)]
    pub tools: BTreeMap<String, ResolvedEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedEntry {
    pub resolved: lns_artifact::tools::SafeVersion,
    pub backend: String,
    pub source_host: String,
    pub resolved_at_unix: u64,
}

impl ResolvedRecord {
    pub fn recorded(&self, spec: &str) -> Option<&ResolvedEntry> {
        self.tools.get(spec)
    }

    /// `@latest` re-resolves every run, so its entry is a last-known-good fallback for an unreachable index rather than a pin.
    pub fn replace(&mut self, spec: &str, entry: ResolvedEntry) {
        self.tools.insert(spec.to_string(), entry);
    }

    /// A bounded request's resolution is never overwritten — the first resolution on a machine wins.
    pub fn merge_new(&mut self, spec: &str, entry: ResolvedEntry) -> bool {
        if self.tools.contains_key(spec) {
            return false;
        }
        self.tools.insert(spec.to_string(), entry);
        true
    }
}

pub trait ToolRecordStore {
    fn load(&self) -> Result<Option<ResolvedRecord>>;
    fn save(&self, record: &ResolvedRecord) -> Result<()>;
}

pub struct RealRecordStore {
    path: PathBuf,
}

impl RealRecordStore {
    pub fn new(tools_root: &Path) -> Self {
        Self {
            path: tools_root.join("resolved.json"),
        }
    }
}

impl ToolRecordStore for RealRecordStore {
    fn load(&self) -> Result<Option<ResolvedRecord>> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(e).with_context(|| format!("reading {}", self.path.display()));
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(record) => Ok(Some(record)),
            Err(e) => {
                let path = self.path.display();
                crate::log::warn!("ignoring unparseable tool record at {path}: {e}");
                Ok(None)
            }
        }
    }

    fn save(&self, record: &ResolvedRecord) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(record).context("serializing the tool record")?;
        std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("installing {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(resolved: &str) -> ResolvedEntry {
        ResolvedEntry {
            resolved: resolved.parse().expect("a usable version"),
            backend: "core:some-tool".into(),
            source_host: "upstream.example.test".into(),
            resolved_at_unix: 1_700_000_000,
        }
    }

    #[test]
    fn a_saved_record_round_trips() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = RealRecordStore::new(dir.path());
        let mut record = ResolvedRecord {
            schema_version: RECORD_SCHEMA_VERSION,
            engine_version: "2026.7.14".into(),
            ..Default::default()
        };
        record.merge_new("some-tool@1", entry("1.2.3"));
        store.save(&record).unwrap();
        assert_eq!(store.load().unwrap(), Some(record));
    }

    #[test]
    fn a_missing_record_loads_as_none() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(RealRecordStore::new(dir.path()).load().unwrap(), None);
    }

    #[test]
    fn a_corrupt_record_is_ignored_with_a_warning_so_the_first_run_re_resolves() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("resolved.json"), b"not json").unwrap();
        let subscriber = tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::WARN)
            .with_writer(std::io::sink)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(RealRecordStore::new(dir.path()).load().unwrap(), None);
        });
    }

    #[test]
    fn a_record_naming_an_unusable_version_is_ignored_rather_than_trusted() {
        // A resolved version becomes a host path component and a word in the provisioner's shell driver, so a tampered file must not be readable at all.
        let dir = tempfile::TempDir::new().unwrap();
        for tampered in [
            "../../..",
            "22.0' && touch /tmp/pwned; '",
            "22.0;id",
            "$(id)",
        ] {
            let record = serde_json::json!({
                "schema_version": RECORD_SCHEMA_VERSION,
                "engine_version": "2026.7.14",
                "tools": {
                    "some-tool@1": {
                        "resolved": tampered,
                        "backend": "core:some-tool",
                        "source_host": "upstream.example.test",
                        "resolved_at_unix": 1_700_000_000,
                    }
                }
            });
            std::fs::write(
                dir.path().join("resolved.json"),
                serde_json::to_vec(&record).unwrap(),
            )
            .unwrap();
            let subscriber = tracing_subscriber::FmtSubscriber::builder()
                .with_max_level(tracing::Level::WARN)
                .with_writer(std::io::sink)
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                assert_eq!(
                    RealRecordStore::new(dir.path()).load().unwrap(),
                    None,
                    "record naming {tampered:?} must read as absent so the tool re-resolves"
                );
            });
        }
    }

    #[test]
    fn an_unreadable_record_surfaces_the_error() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("resolved.json")).unwrap();
        assert!(RealRecordStore::new(dir.path()).load().is_err());
    }

    #[test]
    fn merge_new_never_overwrites_an_existing_resolution() {
        let mut record = ResolvedRecord::default();
        assert!(record.merge_new("some-tool@1", entry("1.2.3")));
        assert!(!record.merge_new("some-tool@1", entry("1.9.9")));
        assert_eq!(
            record.recorded("some-tool@1").unwrap().resolved.as_str(),
            "1.2.3"
        );
        assert_eq!(record.recorded("other-tool@2"), None);
    }
}
