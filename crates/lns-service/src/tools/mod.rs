pub mod cache;
pub mod libc;
pub mod mise;
pub(crate) mod provisioner;
pub mod real;
pub mod record;
pub mod registry;

use std::collections::HashMap;
use std::future::Future;

pub use cache::ToolCache;
pub use lns_artifact::tools::{LATEST, ToolRef};
pub use record::ToolRecordStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Libc {
    Gnu,
    Musl,
}

impl std::fmt::Display for Libc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Libc::Gnu => "gnu",
            Libc::Musl => "musl",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    Aarch64,
    X86_64,
}

impl std::fmt::Display for Arch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Arch::Aarch64 => "aarch64",
            Arch::X86_64 => "x86_64",
        })
    }
}

pub fn host_arch() -> Arch {
    #[cfg(target_arch = "x86_64")]
    {
        Arch::X86_64
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        Arch::Aarch64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvisionTarget {
    pub arch: Arch,
    pub libc: Libc,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolCacheKey {
    pub name: String,
    pub resolved: String,
    pub arch: Arch,
    pub libc: Libc,
}

/// One tool tree the provisioner staged: identity, provenance, the tarred tree, and its bin dirs relative to the tree root.
#[derive(Debug, Clone)]
pub struct StagedTool {
    pub name: String,
    pub resolved: String,
    pub backend: String,
    pub source_host: String,
    pub tar: StagedTar,
    pub bin_paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum StagedTar {
    File(std::path::PathBuf),
    Bytes(Vec<u8>),
}

/// How we talk to the tool engine: `provision` is the expensive half — boot the disposable guest and install every requested tool — while `newest_version` is a host-side index query with no guest at all.
pub trait ToolProvisioner {
    fn provision(
        &self,
        requests: &[ToolRef],
        target: &ProvisionTarget,
    ) -> impl Future<Output = Result<Vec<StagedTool>, ProvisionError>> + Send;

    fn newest_version(&self, name: &str) -> impl Future<Output = anyhow::Result<String>> + Send;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionOutcome {
    pub tool: String,
    pub requested: String,
    pub resolved: String,
    pub backend: String,
    pub source_host: String,
}

#[derive(Debug)]
pub struct EnsuredTools {
    pub specs: Vec<crate::runtime_layer::RuntimeFileSpec>,
    pub bin_paths: Vec<String>,
    /// Only newly-fetched tools — empty on a warm run, so audit events match actual acquisition.
    pub provisioned: Vec<ProvisionOutcome>,
}

/// Provision-or-reuse for a declared tool set: a bounded request pins to the version it first resolved to, `@latest` re-asks the index every run, cache hits contribute specs without booting anything, and every miss goes to the provisioner in one call.
pub async fn ensure_tools<R, C, P>(
    records: &R,
    cache: &C,
    provisioner: &P,
    requests: &[ToolRef],
    target: &ProvisionTarget,
    engine_version: &str,
    now_unix_secs: u64,
) -> Result<EnsuredTools, ProvisionError>
where
    R: ToolRecordStore,
    C: ToolCache,
    P: ToolProvisioner,
{
    let mut record = records
        .load()
        .map_err(|e| ProvisionError::Engine(format!("loading the tool record: {e:#}")))?
        .unwrap_or_default();
    record.schema_version = record::RECORD_SCHEMA_VERSION;
    record.engine_version = engine_version.to_string();

    let mut hits: HashMap<String, cache::ToolManifest> = HashMap::new();
    let mut misses: Vec<ToolRef> = Vec::new();
    let mut record_refreshed = false;
    for request in requests {
        let recorded = record
            .recorded(&request.to_string())
            .map(|entry| entry.resolved.clone());
        let pinned = match request.version.as_str() {
            LATEST => match provisioner.newest_version(&request.name).await {
                Ok(newest) => {
                    record_refreshed |= recorded.as_deref() != Some(newest.as_str());
                    Some(newest)
                }
                Err(e) => {
                    crate::log::warn!(
                        "could not re-check the newest {} ({e:#}); using the last version resolved here",
                        request.name
                    );
                    recorded
                }
            },
            _ => recorded,
        };
        let manifest = match &pinned {
            Some(resolved) => cache
                .lookup(&ToolCacheKey {
                    name: request.name.clone(),
                    resolved: resolved.clone(),
                    arch: target.arch,
                    libc: target.libc,
                })
                .map_err(|e| ProvisionError::Engine(format!("tool cache lookup: {e:#}")))?,
            None => None,
        };
        match manifest {
            Some(manifest) => {
                if request.version == LATEST {
                    record.replace(
                        &request.to_string(),
                        record::ResolvedEntry {
                            resolved: manifest.resolved.clone(),
                            backend: manifest.backend.clone(),
                            source_host: manifest.source_host.clone(),
                            resolved_at_unix: now_unix_secs,
                        },
                    );
                }
                hits.insert(request.name.clone(), manifest);
            }
            None => misses.push(ToolRef {
                name: request.name.clone(),
                version: pinned.unwrap_or_else(|| request.version.clone()),
            }),
        }
    }

    let mut provisioned = Vec::new();
    if !misses.is_empty() {
        let staged = provisioner.provision(&misses, target).await?;
        for request in requests {
            let Some(tool) = staged.iter().find(|tool| tool.name == request.name) else {
                if hits.contains_key(&request.name) {
                    continue;
                }
                return Err(ProvisionError::Engine(format!(
                    "the provisioner returned no tree for {request}"
                )));
            };
            let manifest = cache
                .ingest(
                    &ToolCacheKey {
                        name: tool.name.clone(),
                        resolved: tool.resolved.clone(),
                        arch: target.arch,
                        libc: target.libc,
                    },
                    tool,
                )
                .map_err(|e| ProvisionError::Engine(format!("ingesting {}: {e:#}", tool.name)))?;
            let entry = record::ResolvedEntry {
                resolved: tool.resolved.clone(),
                backend: tool.backend.clone(),
                source_host: tool.source_host.clone(),
                resolved_at_unix: now_unix_secs,
            };
            let spec = request.to_string();
            if request.version == LATEST {
                record.replace(&spec, entry);
            } else {
                record.merge_new(&spec, entry);
            }
            provisioned.push(ProvisionOutcome {
                tool: tool.name.clone(),
                requested: request.to_string(),
                resolved: tool.resolved.clone(),
                backend: tool.backend.clone(),
                source_host: tool.source_host.clone(),
            });
            hits.insert(tool.name.clone(), manifest);
        }
        record_refreshed = true;
    }
    if record_refreshed {
        records
            .save(&record)
            .map_err(|e| ProvisionError::Engine(format!("saving the tool record: {e:#}")))?;
    }

    let mut specs = Vec::new();
    let mut bin_paths = Vec::new();
    for request in requests {
        let manifest = hits
            .get(&request.name)
            .ok_or_else(|| ProvisionError::Engine(format!("no manifest for {request}")))?;
        specs.extend(
            manifest
                .runtime_specs()
                .map_err(|e| ProvisionError::Engine(format!("composing {request}: {e:#}")))?,
        );
        bin_paths.extend(manifest.guest_bin_paths());
    }
    Ok(EnsuredTools {
        specs,
        bin_paths,
        provisioned,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    #[error(
        "spec.tools declares \"{entry}\": \"{name}\" is not a tool lns can provision; check the name against mise's registry"
    )]
    Unknown { entry: String, name: String },
    #[error(
        "spec.tools declares \"{entry}\": \"{name}\" needs the {backend} backend, which lns does not provision; bring it via spec.image instead"
    )]
    UnsupportedBackend {
        entry: String,
        name: String,
        backend: String,
    },
    #[error(
        "spec.tools declares {tool} but image {image} is musl-based and {reason}. Use a glibc base image (e.g. debian:12-slim) or remove {name} from spec.tools"
    )]
    LibcUnsupported {
        tool: String,
        name: String,
        image: String,
        reason: String,
    },
    #[error(
        "provisioning {tool} failed: {cause}. Nothing was cached; the next run retries from a clean state"
    )]
    FetchFailed { tool: String, cause: String },
    #[error("tool provisioning infrastructure failed: {0}")]
    Engine(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn recorded(records: &MemRecords, request: &str) -> String {
        records
            .record
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|saved| saved.recorded(request).map(|entry| entry.resolved.clone()))
            .unwrap_or_default()
    }

    #[derive(Default)]
    struct MemRecords {
        record: Mutex<Option<record::ResolvedRecord>>,
        saves: Mutex<u32>,
    }

    impl ToolRecordStore for MemRecords {
        fn load(&self) -> anyhow::Result<Option<record::ResolvedRecord>> {
            Ok(self.record.lock().unwrap().clone())
        }
        fn save(&self, record: &record::ResolvedRecord) -> anyhow::Result<()> {
            *self.record.lock().unwrap() = Some(record.clone());
            *self.saves.lock().unwrap() += 1;
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemCache {
        map: Mutex<HashMap<ToolCacheKey, cache::ToolManifest>>,
    }

    impl ToolCache for MemCache {
        fn lookup(&self, key: &ToolCacheKey) -> anyhow::Result<Option<cache::ToolManifest>> {
            Ok(self.map.lock().unwrap().get(key).cloned())
        }
        fn ingest(
            &self,
            key: &ToolCacheKey,
            staged: &StagedTool,
        ) -> anyhow::Result<cache::ToolManifest> {
            let manifest = cache::ToolManifest {
                schema_version: cache::MANIFEST_SCHEMA_VERSION,
                tool: staged.name.clone(),
                resolved: staged.resolved.clone(),
                backend: staged.backend.clone(),
                source_host: staged.source_host.clone(),
                engine_version: "2026.7.14".into(),
                entries: Vec::new(),
                bin_paths: staged.bin_paths.clone(),
            };
            self.map
                .lock()
                .unwrap()
                .insert(key.clone(), manifest.clone());
            Ok(manifest)
        }
        fn evict(&self, key: &ToolCacheKey) -> anyhow::Result<()> {
            self.map.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[derive(Default)]
    struct Scripted {
        resolves_to: Mutex<HashMap<String, String>>,
        fail: Mutex<Option<(String, String)>>,
        calls: Mutex<Vec<Vec<ToolRef>>>,
        index: Mutex<Option<String>>,
        index_calls: Mutex<u32>,
        drops_everything: Mutex<bool>,
    }

    impl Scripted {
        fn resolving(entries: &[(&str, &str)]) -> Self {
            Self {
                resolves_to: Mutex::new(
                    entries
                        .iter()
                        .map(|(name, resolved)| (name.to_string(), resolved.to_string()))
                        .collect(),
                ),
                ..Default::default()
            }
        }
    }

    impl Scripted {
        fn index_says(self, newest: &str) -> Self {
            *self.index.lock().unwrap() = Some(newest.to_string());
            self
        }

        fn dropping_every_request(self) -> Self {
            *self.drops_everything.lock().unwrap() = true;
            self
        }
    }

    impl ToolProvisioner for Scripted {
        fn newest_version(
            &self,
            _name: &str,
        ) -> impl Future<Output = anyhow::Result<String>> + Send {
            *self.index_calls.lock().unwrap() += 1;
            let scripted = self.index.lock().unwrap().clone();
            async move { scripted.ok_or_else(|| anyhow::anyhow!("the version index is unreachable")) }
        }

        fn provision(
            &self,
            requests: &[ToolRef],
            _target: &ProvisionTarget,
        ) -> impl Future<Output = Result<Vec<StagedTool>, ProvisionError>> + Send {
            self.calls.lock().unwrap().push(requests.to_vec());
            let fail = self.fail.lock().unwrap().clone();
            let staged: Vec<StagedTool> = if *self.drops_everything.lock().unwrap() {
                Vec::new()
            } else {
                let map = self.resolves_to.lock().unwrap();
                requests
                    .iter()
                    .map(|request| StagedTool {
                        name: request.name.clone(),
                        resolved: map
                            .get(&request.name)
                            .cloned()
                            .unwrap_or_else(|| request.version.clone()),
                        backend: format!("core:{}", request.name),
                        source_host: "upstream.example.test".into(),
                        tar: StagedTar::Bytes(Vec::new()),
                        bin_paths: vec!["bin".into()],
                    })
                    .collect()
            };
            async move {
                match fail {
                    Some((tool, cause)) => Err(ProvisionError::FetchFailed { tool, cause }),
                    None => Ok(staged),
                }
            }
        }
    }

    fn target() -> ProvisionTarget {
        ProvisionTarget {
            arch: Arch::Aarch64,
            libc: Libc::Gnu,
        }
    }

    fn refs(specs: &[&str]) -> Vec<ToolRef> {
        specs
            .iter()
            .map(|spec| lns_artifact::tools::parse(spec).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn a_cold_set_provisions_once_and_records_the_resolutions() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let provisioner = Scripted::resolving(&[("some-tool", "1.2.3"), ("other-tool", "2.0.1")]);
        let ensured = ensure_tools(
            &records,
            &cache,
            &provisioner,
            &refs(&["some-tool@1", "other-tool@2"]),
            &target(),
            "2026.7.14",
            1_700_000_000,
        )
        .await
        .unwrap();
        assert_eq!(provisioner.calls.lock().unwrap().len(), 1);
        assert_eq!(ensured.provisioned.len(), 2);
        assert_eq!(
            recorded(&records, "some-tool@1"),
            "1.2.3",
            "the saved record pins the first resolution"
        );
        assert_eq!(
            ensured.bin_paths,
            vec![
                "/.lens/tools/some-tool/1.2.3/bin".to_string(),
                "/.lens/tools/other-tool/2.0.1/bin".to_string()
            ]
        );
        assert_eq!(*records.saves.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn a_latest_request_re_asks_the_index_every_run_and_installs_a_newer_release() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let first = Scripted::resolving(&[("some-tool", "1.2.3")]).index_says("1.2.3");
        ensure_tools(
            &records,
            &cache,
            &first,
            &refs(&["some-tool@latest"]),
            &target(),
            "2026.7.14",
            1_700_000_000,
        )
        .await
        .unwrap();
        assert_eq!(recorded(&records, "some-tool@latest"), "1.2.3");

        let moved_on = Scripted::resolving(&[("some-tool", "1.9.9")]).index_says("1.9.9");
        let ensured = ensure_tools(
            &records,
            &cache,
            &moved_on,
            &refs(&["some-tool@latest"]),
            &target(),
            "2026.7.14",
            1_700_000_009,
        )
        .await
        .unwrap();
        assert_eq!(
            moved_on.calls.lock().unwrap()[0][0].version,
            "1.9.9",
            "the exact version the index named is what gets installed, not the keyword"
        );
        assert_eq!(ensured.provisioned[0].resolved, "1.9.9");
        assert_eq!(
            recorded(&records, "some-tool@latest"),
            "1.9.9",
            "the record follows the index instead of freezing the first answer"
        );
    }

    #[tokio::test]
    async fn a_latest_request_the_cache_already_holds_needs_no_provisioner() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let first = Scripted::resolving(&[("some-tool", "1.2.3")]).index_says("1.2.3");
        ensure_tools(
            &records,
            &cache,
            &first,
            &refs(&["some-tool@latest"]),
            &target(),
            "2026.7.14",
            1_700_000_000,
        )
        .await
        .unwrap();
        let unchanged = Scripted::resolving(&[("some-tool", "1.2.3")]).index_says("1.2.3");
        ensure_tools(
            &records,
            &cache,
            &unchanged,
            &refs(&["some-tool@latest"]),
            &target(),
            "2026.7.14",
            1_700_000_009,
        )
        .await
        .unwrap();
        assert_eq!(
            *unchanged.index_calls.lock().unwrap(),
            1,
            "the index is asked"
        );
        assert!(
            unchanged.calls.lock().unwrap().is_empty(),
            "an unchanged @latest is a cache hit — no guest boots"
        );
    }

    #[tokio::test]
    async fn an_unreachable_index_falls_back_to_the_last_version_resolved_here() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let online = Scripted::resolving(&[("some-tool", "1.2.3")]).index_says("1.2.3");
        ensure_tools(
            &records,
            &cache,
            &online,
            &refs(&["some-tool@latest"]),
            &target(),
            "2026.7.14",
            1_700_000_000,
        )
        .await
        .unwrap();

        let offline = Scripted::resolving(&[("some-tool", "9.9.9")]);
        let ensured = ensure_tools(
            &records,
            &cache,
            &offline,
            &refs(&["some-tool@latest"]),
            &target(),
            "2026.7.14",
            1_700_000_009,
        )
        .await
        .unwrap();
        assert!(
            offline.calls.lock().unwrap().is_empty(),
            "offline start reuses the cached tree instead of failing the launch"
        );
        assert_eq!(ensured.bin_paths, vec!["/.lens/tools/some-tool/1.2.3/bin"]);
    }

    #[tokio::test]
    async fn a_first_ever_latest_request_with_no_index_and_no_record_still_reaches_the_engine() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let offline = Scripted::resolving(&[("some-tool", "1.2.3")]);
        let ensured = ensure_tools(
            &records,
            &cache,
            &offline,
            &refs(&["some-tool@latest"]),
            &target(),
            "2026.7.14",
            1_700_000_000,
        )
        .await
        .unwrap();
        assert_eq!(
            offline.calls.lock().unwrap()[0][0].version,
            "latest",
            "with nothing recorded the keyword goes to the engine, which resolves it in-guest"
        );
        assert_eq!(ensured.provisioned[0].resolved, "1.2.3");
    }

    #[tokio::test]
    async fn a_warm_set_reuses_the_cache_without_the_provisioner_or_a_save() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let first = Scripted::resolving(&[("some-tool", "1.2.3")]);
        ensure_tools(
            &records,
            &cache,
            &first,
            &refs(&["some-tool@1"]),
            &target(),
            "2026.7.14",
            1_700_000_000,
        )
        .await
        .unwrap();
        let second = Scripted::default();
        let ensured = ensure_tools(
            &records,
            &cache,
            &second,
            &refs(&["some-tool@1"]),
            &target(),
            "2026.7.14",
            1_700_000_009,
        )
        .await
        .unwrap();
        assert!(second.calls.lock().unwrap().is_empty());
        assert!(ensured.provisioned.is_empty());
        assert_eq!(*records.saves.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn a_recorded_but_evicted_tool_reprovisions_at_the_recorded_exact_version() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let first = Scripted::resolving(&[("some-tool", "1.2.3")]);
        ensure_tools(
            &records,
            &cache,
            &first,
            &refs(&["some-tool@1"]),
            &target(),
            "2026.7.14",
            1_700_000_000,
        )
        .await
        .unwrap();
        cache
            .evict(&ToolCacheKey {
                name: "some-tool".into(),
                resolved: "1.2.3".into(),
                arch: Arch::Aarch64,
                libc: Libc::Gnu,
            })
            .unwrap();
        let upstream_moved_on = Scripted::resolving(&[("some-tool", "1.9.9")]);
        ensure_tools(
            &records,
            &cache,
            &upstream_moved_on,
            &refs(&["some-tool@1"]),
            &target(),
            "2026.7.14",
            1_700_000_009,
        )
        .await
        .unwrap();
        let asked = upstream_moved_on.calls.lock().unwrap();
        assert_eq!(
            asked[0][0].version, "1.2.3",
            "the recorded pin is requested exactly"
        );
        assert_eq!(
            recorded(&records, "some-tool@1"),
            "1.2.3",
            "the saved record pins the first resolution"
        );
    }

    #[tokio::test]
    async fn a_mixed_set_only_provisions_the_misses() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let first = Scripted::resolving(&[("some-tool", "1.2.3")]);
        ensure_tools(
            &records,
            &cache,
            &first,
            &refs(&["some-tool@1"]),
            &target(),
            "2026.7.14",
            1_700_000_000,
        )
        .await
        .unwrap();
        let second = Scripted::resolving(&[("other-tool", "2.0.1")]);
        let ensured = ensure_tools(
            &records,
            &cache,
            &second,
            &refs(&["some-tool@1", "other-tool@2"]),
            &target(),
            "2026.7.14",
            1_700_000_009,
        )
        .await
        .unwrap();
        let asked = second.calls.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].len(), 1, "only the miss goes to the provisioner");
        assert_eq!(asked[0][0].name, "other-tool");
        assert_eq!(ensured.provisioned.len(), 1);
        assert_eq!(ensured.bin_paths.len(), 2, "hits still compose");
    }

    #[tokio::test]
    async fn a_provisioner_failure_passes_through_and_caches_nothing() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let provisioner = Scripted {
            fail: Mutex::new(Some(("some-tool@1".into(), "connection timed out".into()))),
            ..Default::default()
        };
        let err = ensure_tools(
            &records,
            &cache,
            &provisioner,
            &refs(&["some-tool@1"]),
            &target(),
            "2026.7.14",
            1_700_000_000,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ProvisionError::FetchFailed { .. }));
        assert!(cache.map.lock().unwrap().is_empty());
        assert!(records.record.lock().unwrap().is_none(), "nothing recorded");
    }

    #[tokio::test]
    async fn a_provisioner_that_drops_a_request_is_an_engine_fault() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let dropper = Scripted::default().dropping_every_request();
        let err = ensure_tools(
            &records,
            &cache,
            &dropper,
            &refs(&["some-tool@1"]),
            &target(),
            "2026.7.14",
            1_700_000_000,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("no tree for some-tool@1"),
            "got: {err}"
        );
    }

    #[test]
    fn arch_and_libc_render_as_cache_path_segments() {
        assert_eq!(Arch::Aarch64.to_string(), "aarch64");
        assert_eq!(Arch::X86_64.to_string(), "x86_64");
        assert_eq!(Libc::Gnu.to_string(), "gnu");
        assert_eq!(Libc::Musl.to_string(), "musl");
    }

    #[test]
    fn host_arch_matches_the_compilation_target() {
        #[cfg(target_arch = "x86_64")]
        assert_eq!(host_arch(), Arch::X86_64);
        #[cfg(not(target_arch = "x86_64"))]
        assert_eq!(host_arch(), Arch::Aarch64);
    }

    #[test]
    fn a_fetch_failure_names_the_tool_the_cause_and_the_clean_retry() {
        let err = ProvisionError::FetchFailed {
            tool: "node@22".into(),
            cause: "connection timed out".into(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("node@22")
                && msg.contains("connection timed out")
                && msg.contains("Nothing was cached")
                && msg.contains("retries from a clean state"),
            "got: {msg}"
        );
    }

    #[test]
    fn an_engine_fault_is_distinguished_from_a_tool_fetch_failure() {
        let msg = ProvisionError::Engine("provisioner guest did not boot".into()).to_string();
        assert!(
            msg.contains("infrastructure failed") && msg.contains("did not boot"),
            "got: {msg}"
        );
    }
}
