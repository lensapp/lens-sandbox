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
pub use lns_artifact::tools::{LATEST, SafeVersion, ToolRef};
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
    pub resolved: SafeVersion,
    pub arch: Arch,
    pub libc: Libc,
}

/// One tool tree the provisioner staged: identity, provenance, the tarred tree, and its bin dirs relative to the tree root.
#[derive(Debug, Clone)]
pub struct StagedTool {
    pub name: String,
    pub resolved: SafeVersion,
    /// The other tools that installed in the same provisioner guest, as declared (`name@version`); each of them ran its own install code as root beside this tree.
    pub co_installed: Vec<String>,
    pub backend: String,
    pub source_host: Option<String>,
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
    pub source_host: Option<String>,
}

#[derive(Debug)]
pub struct EnsuredTools {
    pub specs: Vec<crate::runtime_layer::RuntimeFileSpec>,
    pub bin_paths: Vec<String>,
    /// Only newly-fetched tools — empty on a warm run, so audit events match actual acquisition.
    pub provisioned: Vec<ProvisionOutcome>,
}

/// One request to satisfy a declared tool set: what was asked for, what it has to run on, and where each acquisition is disclosed as it commits.
pub struct EnsureRequest<'a> {
    pub requests: &'a [ToolRef],
    pub target: &'a ProvisionTarget,
    pub engine_version: &'a str,
    pub now_unix_secs: u64,
    pub disclose: &'a (dyn Fn(&ProvisionOutcome) + Send + Sync),
}

/// What a declared tool set needs before it can be injected: the trees already cached, the requests that still have to be installed, and the record as the plan left it.
struct ToolPlan {
    record: record::ResolvedRecord,
    hits: HashMap<String, cache::ToolManifest>,
    misses: Vec<ToolRef>,
    record_changed: bool,
    pins: ResolvedPins,
}

impl ToolPlan {
    fn is_fully_warm(&self) -> bool {
        self.misses.is_empty() && !self.record_changed
    }
}

#[derive(Clone)]
pub(crate) struct ResolvedPins(Vec<ResolvedPin>);

#[derive(Clone)]
struct ResolvedPin {
    request: String,
    version: Option<SafeVersion>,
}

pub(crate) struct PreparedTools {
    pub ready: Option<EnsuredTools>,
    pub pins: ResolvedPins,
}

/// A blackholing proxy or captive portal answers neither way, so the index query is bounded: the documented fallback to the last version resolved here only holds if a stalled query becomes an error instead of a hung launch.
const INDEX_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

async fn newest_within_budget<P: ToolProvisioner>(
    provisioner: &P,
    name: &str,
) -> anyhow::Result<SafeVersion> {
    match tokio::time::timeout(INDEX_BUDGET, provisioner.newest_version(name)).await {
        Ok(answer) => answer?.parse(),
        Err(_) => Err(anyhow::anyhow!(
            "the version index did not answer within {INDEX_BUDGET:?}"
        )),
    }
}

async fn effective_pin<P: ToolProvisioner>(
    provisioner: &P,
    request: &ToolRef,
    recorded: Option<SafeVersion>,
) -> (Option<SafeVersion>, bool) {
    match request.version.as_str() {
        LATEST => match newest_within_budget(provisioner, &request.name).await {
            Ok(newest) => {
                let changed = recorded.as_ref() != Some(&newest);
                (Some(newest), changed)
            }
            Err(e) => {
                crate::log::warn!(
                    "could not re-check the newest {} ({e:#}); using the last version resolved here",
                    request.name
                );
                (recorded, false)
            }
        },
        // Any bounded request is tried as its own cache key — a fuzzy one simply misses — so no version shape ever needs a record to find its cached tree.
        version => (recorded.or_else(|| version.parse().ok()), false),
    }
}

/// The read-only half: consult the record and the cache for each request, re-asking the index for `@latest`. Nothing is written and no guest boots, so this is safe to run without holding the provision lock.
async fn plan_tools<R, C, P>(
    records: &R,
    cache: &C,
    provisioner: &P,
    ask: &EnsureRequest<'_>,
    resolved: Option<&ResolvedPins>,
) -> Result<ToolPlan, ProvisionError>
where
    R: ToolRecordStore,
    C: ToolCache,
    P: ToolProvisioner,
{
    let loaded = records
        .load()
        .map_err(|e| ProvisionError::Engine(format!("loading the tool record: {e:#}")))?;
    let mut record = match loaded {
        Some(record) if record.schema_version == record::RECORD_SCHEMA_VERSION => record,
        Some(other) => {
            crate::log::warn!(
                "the tool record was written by schema v{} and this build speaks v{}; re-resolving every declared tool",
                other.schema_version,
                record::RECORD_SCHEMA_VERSION
            );
            record::ResolvedRecord::default()
        }
        None => record::ResolvedRecord::default(),
    };
    let mut record_changed = record.schema_version != record::RECORD_SCHEMA_VERSION
        || record.engine_version != ask.engine_version;
    record.schema_version = record::RECORD_SCHEMA_VERSION;
    record.engine_version = ask.engine_version.to_string();

    let recorded: Vec<Option<SafeVersion>> = ask
        .requests
        .iter()
        .map(|request| {
            record
                .recorded(&request.to_string())
                .map(|entry| entry.resolved.clone())
        })
        .collect();
    let pins = match resolved {
        Some(resolved) => {
            if resolved.0.len() != ask.requests.len()
                || ask
                    .requests
                    .iter()
                    .zip(&resolved.0)
                    .any(|(request, pinned)| request.to_string() != pinned.request)
            {
                return Err(ProvisionError::Engine(
                    "resolved tool pins do not match the declared tool set".into(),
                ));
            }
            for ((request, recorded), pinned) in ask.requests.iter().zip(&recorded).zip(&resolved.0)
            {
                record_changed |= request.version == LATEST && recorded != &pinned.version;
            }
            resolved
                .0
                .iter()
                .map(|pinned| pinned.version.clone())
                .collect()
        }
        None => {
            let pins_and_changes = futures_util::future::join_all(
                ask.requests
                    .iter()
                    .zip(recorded)
                    .map(|(request, recorded)| effective_pin(provisioner, request, recorded)),
            )
            .await;
            let mut pins = Vec::with_capacity(pins_and_changes.len());
            for (pinned, pin_changed) in pins_and_changes {
                record_changed |= pin_changed;
                pins.push(pinned);
            }
            pins
        }
    };

    // What this sandbox declares is what it already trusts, so it is the yardstick for adopting a tree a neighbour shared a guest with.
    let mut declared: Vec<String> = ask.requests.iter().map(ToolRef::to_string).collect();
    declared.extend(
        ask.requests
            .iter()
            .zip(&pins)
            .filter_map(|(request, pinned)| {
                pinned
                    .as_ref()
                    .map(|resolved| format!("{}@{resolved}", request.name))
            }),
    );
    let mut hits: HashMap<String, cache::ToolManifest> = HashMap::new();
    let mut misses: Vec<ToolRef> = Vec::new();
    for (request, pinned) in ask.requests.iter().zip(&pins) {
        let manifest = match pinned {
            Some(resolved) => cache
                .lookup(
                    &ToolCacheKey {
                        name: request.name.clone(),
                        resolved: resolved.clone(),
                        arch: ask.target.arch,
                        libc: ask.target.libc,
                    },
                    &declared,
                )
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
                            resolved_at_unix: ask.now_unix_secs,
                        },
                    );
                }
                hits.insert(request.name.clone(), manifest);
            }
            None => misses.push(ToolRef {
                name: request.name.clone(),
                version: pinned
                    .clone()
                    .map(SafeVersion::into)
                    .unwrap_or_else(|| request.version.clone()),
            }),
        }
    }
    Ok(ToolPlan {
        record,
        hits,
        misses,
        record_changed,
        pins: ResolvedPins(
            ask.requests
                .iter()
                .zip(pins)
                .map(|(request, version)| ResolvedPin {
                    request: request.to_string(),
                    version,
                })
                .collect(),
        ),
    })
}

fn compose(
    requests: &[ToolRef],
    hits: &HashMap<String, cache::ToolManifest>,
    provisioned: Vec<ProvisionOutcome>,
) -> Result<EnsuredTools, ProvisionError> {
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

/// The read-only pass: `ready` is `Some` when every tool is cached and the record needs no update, so the run can be injected without touching the provisioner or queueing behind another run's install; the pins carry the `@latest` answers forward so the install pass does not re-ask the index.
pub(crate) async fn prepare_tools<R, C, P>(
    records: &R,
    cache: &C,
    provisioner: &P,
    ask: &EnsureRequest<'_>,
) -> Result<PreparedTools, ProvisionError>
where
    R: ToolRecordStore,
    C: ToolCache,
    P: ToolProvisioner,
{
    let plan = plan_tools(records, cache, provisioner, ask, None).await?;
    let ready = if plan.is_fully_warm() {
        Some(compose(ask.requests, &plan.hits, Vec::new())?)
    } else {
        None
    };
    Ok(PreparedTools {
        ready,
        pins: plan.pins,
    })
}

/// Extracting and hashing a tool tree holds the thread for the length of the tar, so it moves off the async worker; a current-thread runtime (tests) has nowhere to move it and runs it inline.
fn ingest_off_runtime<C: ToolCache>(
    cache: &C,
    key: &ToolCacheKey,
    staged: &StagedTool,
) -> anyhow::Result<cache::ToolManifest> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| cache.ingest(key, staged))
        }
        _ => cache.ingest(key, staged),
    }
}

/// Provision-or-reuse for a declared tool set: a bounded request pins to the version it first resolved to, `@latest` re-asks the index every run, cache hits contribute specs without booting anything, and every miss goes to the provisioner in one call.
pub async fn ensure_tools<R, C, P>(
    records: &R,
    cache: &C,
    provisioner: &P,
    ask: &EnsureRequest<'_>,
) -> Result<EnsuredTools, ProvisionError>
where
    R: ToolRecordStore,
    C: ToolCache,
    P: ToolProvisioner,
{
    let plan = plan_tools(records, cache, provisioner, ask, None).await?;
    ensure_tools_from_plan(records, cache, provisioner, ask, plan).await
}

pub(crate) async fn ensure_tools_with_pins<R, C, P>(
    records: &R,
    cache: &C,
    provisioner: &P,
    ask: &EnsureRequest<'_>,
    pins: &ResolvedPins,
) -> Result<EnsuredTools, ProvisionError>
where
    R: ToolRecordStore,
    C: ToolCache,
    P: ToolProvisioner,
{
    let plan = plan_tools(records, cache, provisioner, ask, Some(pins)).await?;
    ensure_tools_from_plan(records, cache, provisioner, ask, plan).await
}

async fn ensure_tools_from_plan<R, C, P>(
    records: &R,
    cache: &C,
    provisioner: &P,
    ask: &EnsureRequest<'_>,
    plan: ToolPlan,
) -> Result<EnsuredTools, ProvisionError>
where
    R: ToolRecordStore,
    C: ToolCache,
    P: ToolProvisioner,
{
    let ToolPlan {
        mut record,
        mut hits,
        misses,
        mut record_changed,
        ..
    } = plan;

    let mut provisioned = Vec::new();
    if !misses.is_empty() {
        let staged = provisioner.provision(&misses, ask.target).await?;
        for request in ask.requests {
            let Some(tool) = staged.iter().find(|tool| tool.name == request.name) else {
                if hits.contains_key(&request.name) {
                    continue;
                }
                return Err(ProvisionError::Engine(format!(
                    "the provisioner returned no tree for {request}"
                )));
            };
            let manifest = ingest_off_runtime(
                cache,
                &ToolCacheKey {
                    name: tool.name.clone(),
                    resolved: tool.resolved.clone(),
                    arch: ask.target.arch,
                    libc: ask.target.libc,
                },
                tool,
            )
            .map_err(|e| ProvisionError::Engine(format!("ingesting {}: {e:#}", tool.name)))?;
            let entry = record::ResolvedEntry {
                resolved: tool.resolved.clone(),
                backend: tool.backend.clone(),
                source_host: tool.source_host.clone(),
                resolved_at_unix: ask.now_unix_secs,
            };
            let spec = request.to_string();
            if request.version == LATEST {
                record.replace(&spec, entry);
            } else {
                record.merge_new(&spec, entry);
            }
            let outcome = ProvisionOutcome {
                tool: tool.name.clone(),
                requested: request.to_string(),
                resolved: tool.resolved.to_string(),
                backend: tool.backend.clone(),
                source_host: tool.source_host.clone(),
            };
            hits.insert(tool.name.clone(), manifest);
            // Commit and disclose per tool: a failure ingesting a later one must not leave this tree cached, recorded, and unmentioned on any chain.
            records
                .save(&record)
                .map_err(|e| ProvisionError::Engine(format!("saving the tool record: {e:#}")))?;
            record_changed = false;
            (ask.disclose)(&outcome);
            provisioned.push(outcome);
        }
    }
    if record_changed {
        records
            .save(&record)
            .map_err(|e| ProvisionError::Engine(format!("saving the tool record: {e:#}")))?;
    }
    compose(ask.requests, &hits, provisioned)
}

#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    #[error(transparent)]
    Unprovisionable(#[from] lns_artifact::tools::registry::ToolRefusal),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PullProvisionDisposition {
    RetryOnRun,
    PermanentRefusal,
}

impl ProvisionError {
    pub(crate) fn pull_disposition(&self) -> PullProvisionDisposition {
        match self {
            ProvisionError::Unprovisionable(_) | ProvisionError::LibcUnsupported { .. } => {
                PullProvisionDisposition::PermanentRefusal
            }
            ProvisionError::FetchFailed { .. } | ProvisionError::Engine(_) => {
                PullProvisionDisposition::RetryOnRun
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Scenarios that are not about disclosure hand it nowhere.
    fn discard(_outcome: &ProvisionOutcome) {}

    fn version(literal: &str) -> SafeVersion {
        literal.parse().expect("a usable version")
    }

    fn recorded(records: &MemRecords, request: &str) -> String {
        records
            .record
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|saved| {
                saved
                    .recorded(request)
                    .map(|entry| entry.resolved.to_string())
            })
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
        ingests_before_failing: Mutex<Option<u32>>,
    }

    impl ToolCache for MemCache {
        fn lookup(
            &self,
            key: &ToolCacheKey,
            declared: &[String],
        ) -> anyhow::Result<Option<cache::ToolManifest>> {
            Ok(self
                .map
                .lock()
                .unwrap()
                .get(key)
                .filter(|manifest| {
                    manifest
                        .co_installed
                        .iter()
                        .all(|neighbour| declared.iter().any(|tool| tool == neighbour))
                })
                .cloned())
        }
        fn ingest(
            &self,
            key: &ToolCacheKey,
            staged: &StagedTool,
        ) -> anyhow::Result<cache::ToolManifest> {
            if let Some(remaining) = self.ingests_before_failing.lock().unwrap().as_mut() {
                if *remaining == 0 {
                    anyhow::bail!("ingesting {} failed", key.name);
                }
                *remaining -= 1;
            }
            let manifest = cache::ToolManifest {
                schema_version: cache::MANIFEST_SCHEMA_VERSION,
                tool: staged.name.clone(),
                resolved: staged.resolved.clone(),
                backend: staged.backend.clone(),
                source_host: staged.source_host.clone(),
                engine_version: "2026.7.14".into(),
                co_installed: staged.co_installed.clone(),
                entries: Vec::new(),
                bin_paths: staged.bin_paths.clone(),
            };
            self.map
                .lock()
                .unwrap()
                .insert(key.clone(), manifest.clone());
            Ok(manifest)
        }
    }

    /// An ingest that refuses to finish until some other task proves the runtime is still scheduling.
    struct StarvingCache {
        inner: MemCache,
        started: std::sync::Arc<std::sync::atomic::AtomicBool>,
        scheduled: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    }

    impl ToolCache for StarvingCache {
        fn lookup(
            &self,
            key: &ToolCacheKey,
            declared: &[String],
        ) -> anyhow::Result<Option<cache::ToolManifest>> {
            self.inner.lookup(key, declared)
        }
        fn ingest(
            &self,
            key: &ToolCacheKey,
            staged: &StagedTool,
        ) -> anyhow::Result<cache::ToolManifest> {
            self.started
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let scheduled = self.scheduled.lock().unwrap().take().expect("one ingest");
            let proof = scheduled.recv_timeout(std::time::Duration::from_millis(500));
            anyhow::ensure!(
                proof.is_ok(),
                "no other task ran while this ingest held its thread"
            );
            self.inner.ingest(key, staged)
        }
    }

    #[derive(Default)]
    struct Scripted {
        resolves_to: Mutex<HashMap<String, String>>,
        fail: Mutex<Option<(String, String)>>,
        calls: Mutex<Vec<Vec<ToolRef>>>,
        index: Mutex<Option<String>>,
        index_stalls: Mutex<bool>,
        index_barrier: Mutex<Option<std::sync::Arc<tokio::sync::Barrier>>>,
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

        fn index_never_answers(self) -> Self {
            *self.index_stalls.lock().unwrap() = true;
            self
        }

        fn index_waits_for_concurrent_calls(self, calls: usize) -> Self {
            *self.index_barrier.lock().unwrap() =
                Some(std::sync::Arc::new(tokio::sync::Barrier::new(calls)));
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
            let stalls = *self.index_stalls.lock().unwrap();
            let barrier = self.index_barrier.lock().unwrap().clone();
            async move {
                if let Some(barrier) = barrier {
                    barrier.wait().await;
                }
                if stalls {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                }
                scripted.ok_or_else(|| anyhow::anyhow!("the version index is unreachable"))
            }
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
                        co_installed: requests
                            .iter()
                            .filter(|other| other.name != request.name)
                            .map(ToolRef::to_string)
                            .collect(),
                        resolved: version(
                            map.get(&request.name)
                                .map(String::as_str)
                                .unwrap_or(&request.version),
                        ),
                        backend: format!("core:{}", request.name),
                        source_host: Some("upstream.example.test".into()),
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
            &EnsureRequest {
                requests: &refs(&["some-tool@1", "other-tool@2"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
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
        assert_eq!(
            *records.saves.lock().unwrap(),
            2,
            "the record is persisted per ingested tool, not once at the end"
        );
    }

    #[tokio::test]
    async fn a_tool_ingested_before_a_later_failure_stays_referenceable() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        *cache.ingests_before_failing.lock().unwrap() = Some(1);
        let provisioner = Scripted::resolving(&[("some-tool", "1.2.3")]);
        let disclosed = Mutex::new(Vec::new());
        let err = ensure_tools(
            &records,
            &cache,
            &provisioner,
            &EnsureRequest {
                requests: &refs(&["some-tool@1", "other-tool@2"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &|outcome| {
                    disclosed
                        .lock()
                        .unwrap()
                        .push(format!("{} -> {}", outcome.requested, outcome.resolved))
                },
            },
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("ingesting"), "got: {err:#}");
        assert_eq!(
            recorded(&records, "some-tool@1"),
            "1.2.3",
            "the tree ingested before the failure is still named by the record"
        );
        assert_eq!(
            disclosed.lock().unwrap().as_slice(),
            ["some-tool@1 -> 1.2.3"],
            "and it was disclosed when it committed, not after the whole set survived"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_long_ingest_does_not_starve_the_runtime() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let started = std::sync::Arc::new(AtomicBool::new(false));
        let witnessed = started.clone();
        let (prove_scheduling, scheduled) = std::sync::mpsc::channel();
        let work = tokio::spawn(async move {
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    if witnessed.load(Ordering::SeqCst) {
                        break;
                    }
                }
                let _ = prove_scheduling.send(());
            });
            let records = MemRecords::default();
            let cache = StarvingCache {
                inner: MemCache::default(),
                started,
                scheduled: Mutex::new(Some(scheduled)),
            };
            let provisioner = Scripted::resolving(&[("some-tool", "1.2.3")]);
            ensure_tools(
                &records,
                &cache,
                &provisioner,
                &EnsureRequest {
                    requests: &refs(&["some-tool@1.2.3"]),
                    target: &target(),
                    engine_version: "2026.7.14",
                    now_unix_secs: 1_700_000_000,
                    disclose: &discard,
                },
            )
            .await
            .map(|ensured| ensured.provisioned.len())
        });
        let provisioned = work
            .await
            .unwrap()
            .expect("the ingest held the runtime's only worker, starving every other task");
        assert_eq!(provisioned, 1);
    }

    #[tokio::test]
    async fn a_fully_cached_set_is_injectable_without_the_provisioner_or_the_install_queue() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let cold = Scripted::resolving(&[("some-tool", "1.2.3")]);
        ensure_tools(
            &records,
            &cache,
            &cold,
            &EnsureRequest {
                requests: &refs(&["some-tool@1"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();

        let warm = Scripted::default();
        let ensured = prepare_tools(
            &records,
            &cache,
            &warm,
            &EnsureRequest {
                requests: &refs(&["some-tool@1"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_009,
                disclose: &discard,
            },
        )
        .await
        .unwrap()
        .ready
        .expect("a cached set needs no install");
        assert_eq!(ensured.bin_paths, vec!["/.lens/tools/some-tool/1.2.3/bin"]);
        assert!(ensured.provisioned.is_empty(), "nothing was fetched");
        assert!(warm.calls.lock().unwrap().is_empty());
        assert_eq!(
            *records.saves.lock().unwrap(),
            1,
            "the warm pass writes nothing, so it needs no lock"
        );
    }

    #[tokio::test]
    async fn a_set_with_anything_missing_defers_to_the_serialized_install() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let cold = Scripted::resolving(&[("some-tool", "1.2.3")]);
        assert!(
            prepare_tools(
                &records,
                &cache,
                &cold,
                &EnsureRequest {
                    requests: &refs(&["some-tool@1"]),
                    target: &target(),
                    engine_version: "2026.7.14",
                    now_unix_secs: 1_700_000_000,
                    disclose: &discard,
                },
            )
            .await
            .unwrap()
            .ready
            .is_none(),
            "an uncached tool is not warm"
        );
        assert!(
            cold.calls.lock().unwrap().is_empty(),
            "the warm pass never provisions — it reports that it cannot"
        );
        assert_eq!(*records.saves.lock().unwrap(), 0, "and never writes");
    }

    #[tokio::test]
    async fn a_record_from_another_schema_is_discarded_rather_than_restamped() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        *records.record.lock().unwrap() = Some(record::ResolvedRecord {
            schema_version: record::RECORD_SCHEMA_VERSION + 1,
            engine_version: "2026.7.14".into(),
            tools: std::collections::BTreeMap::from([(
                "some-tool@1".to_string(),
                record::ResolvedEntry {
                    resolved: version("1.0.0-from-the-future"),
                    backend: "core:some-tool".into(),
                    source_host: Some("upstream.example.test".into()),
                    resolved_at_unix: 1_600_000_000,
                },
            )]),
        });
        let provisioner = Scripted::resolving(&[("some-tool", "1.2.3")]);
        let ensured = ensure_tools(
            &records,
            &cache,
            &provisioner,
            &EnsureRequest {
                requests: &refs(&["some-tool@1"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            provisioner.calls.lock().unwrap()[0][0].version,
            "1",
            "the unreadable pin is not handed to the engine as if it were ours"
        );
        assert_eq!(ensured.provisioned[0].resolved, "1.2.3");
        assert_eq!(recorded(&records, "some-tool@1"), "1.2.3");
        assert_eq!(
            records
                .record
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .schema_version,
            record::RECORD_SCHEMA_VERSION
        );
    }

    #[tokio::test]
    async fn a_cached_set_whose_engine_moved_on_is_not_warm() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let cold = Scripted::resolving(&[("some-tool", "1.2.3")]);
        ensure_tools(
            &records,
            &cache,
            &cold,
            &EnsureRequest {
                requests: &refs(&["some-tool@1"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        assert!(
            prepare_tools(
                &records,
                &cache,
                &Scripted::default(),
                &EnsureRequest {
                    requests: &refs(&["some-tool@1"]),
                    target: &target(),
                    engine_version: "2026.8.01",
                    now_unix_secs: 1_700_000_009,
                    disclose: &discard,
                },
            )
            .await
            .unwrap()
            .ready
            .is_none(),
            "the record still names the old engine, so the write has to happen under the lock"
        );
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
            &EnsureRequest {
                requests: &refs(&["some-tool@latest"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        assert_eq!(recorded(&records, "some-tool@latest"), "1.2.3");

        let moved_on = Scripted::resolving(&[("some-tool", "1.9.9")]).index_says("1.9.9");
        let ensured = ensure_tools(
            &records,
            &cache,
            &moved_on,
            &EnsureRequest {
                requests: &refs(&["some-tool@latest"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_009,
                disclose: &discard,
            },
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
    async fn independent_latest_index_queries_start_concurrently() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let provisioner = Scripted::default()
            .index_says("1.2.3")
            .index_waits_for_concurrent_calls(2);
        let requests = refs(&["some-tool@latest", "other-tool@latest"]);
        let plan = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            plan_tools(
                &records,
                &cache,
                &provisioner,
                &EnsureRequest {
                    requests: &requests,
                    target: &target(),
                    engine_version: "2026.7.14",
                    now_unix_secs: 1_700_000_000,
                    disclose: &discard,
                },
                None,
            ),
        )
        .await
        .expect("both independent index lookups should start before either must finish")
        .unwrap();

        assert_eq!(
            plan.misses,
            refs(&["some-tool@1.2.3", "other-tool@1.2.3"]),
            "concurrent results retain declaration order"
        );
    }

    #[tokio::test]
    async fn a_cold_latest_set_queries_each_index_once_across_the_warm_check_and_install() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let provisioner = Scripted::resolving(&[("some-tool", "1.2.3"), ("other-tool", "1.2.3")])
            .index_says("1.2.3");
        let requests = refs(&["some-tool@latest", "other-tool@latest"]);
        let ask = EnsureRequest {
            requests: &requests,
            target: &target(),
            engine_version: "2026.7.14",
            now_unix_secs: 1_700_000_000,
            disclose: &discard,
        };

        let prepared = prepare_tools(&records, &cache, &provisioner, &ask)
            .await
            .unwrap();
        assert!(prepared.ready.is_none());
        ensure_tools_with_pins(&records, &cache, &provisioner, &ask, &prepared.pins)
            .await
            .unwrap();

        assert_eq!(
            *provisioner.index_calls.lock().unwrap(),
            requests.len() as u32,
            "the install recheck must reuse the pins resolved before taking the lock"
        );
    }

    #[tokio::test]
    async fn the_install_recheck_adopts_trees_a_peer_cached_while_the_lock_was_waiting() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let requests = refs(&["some-tool@latest", "other-tool@latest"]);
        let ask = EnsureRequest {
            requests: &requests,
            target: &target(),
            engine_version: "2026.7.14",
            now_unix_secs: 1_700_000_000,
            disclose: &discard,
        };
        let waiting = Scripted::resolving(&[("some-tool", "1.2.3"), ("other-tool", "1.2.3")])
            .index_says("1.2.3");
        let prepared = prepare_tools(&records, &cache, &waiting, &ask)
            .await
            .unwrap();
        assert!(prepared.ready.is_none());

        let peer = Scripted::resolving(&[("some-tool", "1.2.3"), ("other-tool", "1.2.3")])
            .index_says("1.2.3");
        ensure_tools(&records, &cache, &peer, &ask).await.unwrap();
        let ensured = ensure_tools_with_pins(&records, &cache, &waiting, &ask, &prepared.pins)
            .await
            .unwrap();

        assert!(
            waiting.calls.lock().unwrap().is_empty(),
            "the locked recheck must adopt the trees the peer installed"
        );
        assert!(ensured.provisioned.is_empty());
        assert_eq!(
            *waiting.index_calls.lock().unwrap(),
            requests.len() as u32,
            "rechecking the peer-populated cache must not re-query the index"
        );
    }

    #[tokio::test]
    async fn a_resolved_snapshot_for_another_tool_set_is_rejected() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let requests = refs(&["some-tool@latest"]);
        let ask = EnsureRequest {
            requests: &requests,
            target: &target(),
            engine_version: "2026.7.14",
            now_unix_secs: 1_700_000_000,
            disclose: &discard,
        };
        let err = ensure_tools_with_pins(
            &records,
            &cache,
            &Scripted::default(),
            &ask,
            &ResolvedPins(vec![ResolvedPin {
                request: "other-tool@latest".into(),
                version: None,
            }]),
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("resolved tool pins do not match the declared tool set")
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
            &EnsureRequest {
                requests: &refs(&["some-tool@latest"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        let unchanged = Scripted::resolving(&[("some-tool", "1.2.3")]).index_says("1.2.3");
        ensure_tools(
            &records,
            &cache,
            &unchanged,
            &EnsureRequest {
                requests: &refs(&["some-tool@latest"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_009,
                disclose: &discard,
            },
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
    async fn unchanged_latest_neighbours_reuse_the_trees_they_shared() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let requests = refs(&["some-tool@latest", "other-tool@latest"]);
        let first = Scripted::resolving(&[("some-tool", "1.2.3"), ("other-tool", "1.2.3")])
            .index_says("1.2.3");
        ensure_tools(
            &records,
            &cache,
            &first,
            &EnsureRequest {
                requests: &requests,
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();

        let unchanged = Scripted::resolving(&[("some-tool", "1.2.3"), ("other-tool", "1.2.3")])
            .index_says("1.2.3");
        ensure_tools(
            &records,
            &cache,
            &unchanged,
            &EnsureRequest {
                requests: &requests,
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_009,
                disclose: &discard,
            },
        )
        .await
        .unwrap();

        assert!(
            unchanged.calls.lock().unwrap().is_empty(),
            "unchanged @latest neighbours should trust and reuse the exact trees they shared"
        );

        let offline = Scripted::default();
        ensure_tools(
            &records,
            &cache,
            &offline,
            &EnsureRequest {
                requests: &requests,
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_018,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        assert!(
            offline.calls.lock().unwrap().is_empty(),
            "recorded @latest neighbours should reuse their cached trees while offline"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_index_that_never_answers_falls_back_instead_of_hanging_the_launch() {
        let records = MemRecords::default();
        let cache = MemCache::default();
        let online = Scripted::resolving(&[("some-tool", "1.2.3")]).index_says("1.2.3");
        ensure_tools(
            &records,
            &cache,
            &online,
            &EnsureRequest {
                requests: &refs(&["some-tool@latest"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();

        let blackholed = Scripted::resolving(&[("some-tool", "9.9.9")]).index_never_answers();
        let ensured = ensure_tools(
            &records,
            &cache,
            &blackholed,
            &EnsureRequest {
                requests: &refs(&["some-tool@latest"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_009,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            ensured.bin_paths,
            vec!["/.lens/tools/some-tool/1.2.3/bin"],
            "the launch proceeds on the last version resolved here"
        );
        assert!(blackholed.calls.lock().unwrap().is_empty());
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
            &EnsureRequest {
                requests: &refs(&["some-tool@latest"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();

        let offline = Scripted::resolving(&[("some-tool", "9.9.9")]);
        let ensured = ensure_tools(
            &records,
            &cache,
            &offline,
            &EnsureRequest {
                requests: &refs(&["some-tool@latest"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_009,
                disclose: &discard,
            },
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
            &EnsureRequest {
                requests: &refs(&["some-tool@latest"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
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
            &EnsureRequest {
                requests: &refs(&["some-tool@1"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        let second = Scripted::default();
        let ensured = ensure_tools(
            &records,
            &cache,
            &second,
            &EnsureRequest {
                requests: &refs(&["some-tool@1"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_009,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        assert!(second.calls.lock().unwrap().is_empty());
        assert!(ensured.provisioned.is_empty());
        assert_eq!(*records.saves.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn an_exact_request_starts_from_the_cache_with_no_record_at_all() {
        // A pulled sandbox carries push-pinned versions; losing resolved.json must not send every one of them back to the network.
        let records = MemRecords::default();
        let cache = MemCache::default();
        let cold = Scripted::resolving(&[("some-tool", "1.2.3")]);
        ensure_tools(
            &records,
            &cache,
            &cold,
            &EnsureRequest {
                requests: &refs(&["some-tool@1.2.3"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();

        *records.record.lock().unwrap() = None;
        let offline = Scripted::default();
        let ensured = ensure_tools(
            &records,
            &cache,
            &offline,
            &EnsureRequest {
                requests: &refs(&["some-tool@1.2.3"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_009,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        assert!(
            offline.calls.lock().unwrap().is_empty(),
            "the request itself determines the cache key"
        );
        assert_eq!(ensured.bin_paths, vec!["/.lens/tools/some-tool/1.2.3/bin"]);
    }

    #[tokio::test]
    async fn a_vendor_exact_request_starts_from_the_cache_with_no_record_at_all() {
        // push pins vendor versions like temurin-21.0.5+11.0.LTS; losing resolved.json must not force a reinstall of a tree that is already cached.
        let records = MemRecords::default();
        let cache = MemCache::default();
        let cold = Scripted::default();
        ensure_tools(
            &records,
            &cache,
            &cold,
            &EnsureRequest {
                requests: &refs(&["some-tool@vendor-1.0.5+11.0.LTS"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();

        *records.record.lock().unwrap() = None;
        let offline = Scripted::default();
        let ensured = ensure_tools(
            &records,
            &cache,
            &offline,
            &EnsureRequest {
                requests: &refs(&["some-tool@vendor-1.0.5+11.0.LTS"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_009,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        assert!(
            offline.calls.lock().unwrap().is_empty(),
            "the request itself determines the cache key"
        );
        assert_eq!(
            ensured.bin_paths,
            vec!["/.lens/tools/some-tool/vendor-1.0.5+11.0.LTS/bin"]
        );
    }

    #[tokio::test]
    async fn the_recorded_pin_wins_over_the_literal_request_key() {
        // A tree keyed by the raw request string can exist on a machine; the record names what this machine resolved, so it must pick the tree.
        let records = MemRecords::default();
        let cache = MemCache::default();
        let cold = Scripted::resolving(&[("some-tool", "1.2.3")]);
        ensure_tools(
            &records,
            &cache,
            &cold,
            &EnsureRequest {
                requests: &refs(&["some-tool@1"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        cache
            .ingest(
                &ToolCacheKey {
                    name: "some-tool".into(),
                    resolved: version("1"),
                    arch: Arch::Aarch64,
                    libc: Libc::Gnu,
                },
                &StagedTool {
                    name: "some-tool".into(),
                    co_installed: Vec::new(),
                    resolved: version("1"),
                    backend: "core:some-tool".into(),
                    source_host: None,
                    tar: StagedTar::Bytes(Vec::new()),
                    bin_paths: vec!["bin".into()],
                },
            )
            .unwrap();
        let offline = Scripted::default();
        let ensured = ensure_tools(
            &records,
            &cache,
            &offline,
            &EnsureRequest {
                requests: &refs(&["some-tool@1"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_009,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        assert!(offline.calls.lock().unwrap().is_empty());
        assert_eq!(
            ensured.bin_paths,
            vec!["/.lens/tools/some-tool/1.2.3/bin"],
            "the record, not the literal key, names the tree"
        );
    }

    #[tokio::test]
    async fn a_pinned_set_with_no_record_recovers_to_warm_without_the_provisioner() {
        // Losing resolved.json costs exactly one deferred record write; no phase reinstalls, and the set is warm again afterwards.
        let records = MemRecords::default();
        let cache = MemCache::default();
        let cold = Scripted::default();
        let requests = refs(&["some-tool@vendor-1.0.5+11.0.LTS"]);
        let target = target();
        let ask = |now: u64| EnsureRequest {
            requests: &requests,
            target: &target,
            engine_version: "2026.7.14",
            now_unix_secs: now,
            disclose: &discard,
        };
        ensure_tools(&records, &cache, &cold, &ask(1_700_000_000))
            .await
            .unwrap();

        *records.record.lock().unwrap() = None;
        let offline = Scripted::default();
        let deferred = prepare_tools(&records, &cache, &offline, &ask(1_700_000_009))
            .await
            .unwrap();
        assert!(
            deferred.ready.is_none(),
            "re-establishing the record defers to the serialized install"
        );
        ensure_tools(&records, &cache, &offline, &ask(1_700_000_009))
            .await
            .unwrap();
        let warm = prepare_tools(&records, &cache, &offline, &ask(1_700_000_010))
            .await
            .unwrap();
        assert!(
            warm.ready.is_some(),
            "with the record back, the set injects without the install queue"
        );
        assert!(
            offline.calls.lock().unwrap().is_empty(),
            "nothing reinstalls"
        );
    }

    #[tokio::test]
    async fn an_unfamiliar_version_shape_is_still_its_own_cache_key() {
        // The runtime tries every non-latest request as a cache key; a fuzzy one simply misses, so an unrecognized exact format never reinstalls.
        let records = MemRecords::default();
        let cache = MemCache::default();
        let cold = Scripted::default();
        ensure_tools(
            &records,
            &cache,
            &cold,
            &EnsureRequest {
                requests: &refs(&["some-tool@odd-5"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();

        *records.record.lock().unwrap() = None;
        let offline = Scripted::default();
        let ensured = ensure_tools(
            &records,
            &cache,
            &offline,
            &EnsureRequest {
                requests: &refs(&["some-tool@odd-5"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_009,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        assert!(
            offline.calls.lock().unwrap().is_empty(),
            "the request itself determines the cache key"
        );
        assert_eq!(ensured.bin_paths, vec!["/.lens/tools/some-tool/odd-5/bin"]);
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
            &EnsureRequest {
                requests: &refs(&["some-tool@1"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        cache.map.lock().unwrap().clear();
        let upstream_moved_on = Scripted::resolving(&[("some-tool", "1.9.9")]);
        ensure_tools(
            &records,
            &cache,
            &upstream_moved_on,
            &EnsureRequest {
                requests: &refs(&["some-tool@1"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_009,
                disclose: &discard,
            },
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
            &EnsureRequest {
                requests: &refs(&["some-tool@1"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
        )
        .await
        .unwrap();
        let second = Scripted::resolving(&[("other-tool", "2.0.1")]);
        let ensured = ensure_tools(
            &records,
            &cache,
            &second,
            &EnsureRequest {
                requests: &refs(&["some-tool@1", "other-tool@2"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_009,
                disclose: &discard,
            },
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
            &EnsureRequest {
                requests: &refs(&["some-tool@1"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
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
            &EnsureRequest {
                requests: &refs(&["some-tool@1"]),
                target: &target(),
                engine_version: "2026.7.14",
                now_unix_secs: 1_700_000_000,
                disclose: &discard,
            },
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

    #[test]
    fn pull_provision_disposition_distinguishes_retries_from_refusals() {
        let retryable = [
            ProvisionError::FetchFailed {
                tool: "node@22".into(),
                cause: "connection timed out".into(),
            },
            ProvisionError::Engine("virtiofsd is unsupported".into()),
        ];
        assert!(
            retryable
                .iter()
                .all(|error| error.pull_disposition() == PullProvisionDisposition::RetryOnRun)
        );

        let permanent = [
            ProvisionError::Unprovisionable(lns_artifact::tools::registry::ToolRefusal::Unknown {
                entry: "some-tool@1".into(),
                name: "some-tool".into(),
            }),
            ProvisionError::LibcUnsupported {
                tool: "deno@2".into(),
                name: "deno".into(),
                image: "alpine:3.20".into(),
                reason: "Deno publishes no musl builds".into(),
            },
        ];
        assert!(permanent.iter().all(|error| {
            error.pull_disposition() == PullProvisionDisposition::PermanentRefusal
        }));
    }
}
