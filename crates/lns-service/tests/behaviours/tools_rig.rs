use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

use lns_service::tools::cache::{MANIFEST_SCHEMA_VERSION, ToolCache, ToolManifest};
use lns_service::tools::record::{ResolvedRecord, ToolRecordStore};
use lns_service::tools::{
    EnsuredTools, ProvisionError, ProvisionTarget, StagedTar, StagedTool, ToolProvisioner, ToolRef,
};

/// Declared-tools scenarios: a definition with `spec.tools`, launched through the
/// same planning refusals and ensure_tools composition the orchestrator applies
/// pre-boot, with the record, cache, and provisioner faked in memory.
#[derive(Debug, Default)]
pub struct ToolsRig {
    pub definition: Option<String>,
    pub error: Option<String>,
    pub ensured: Option<EnsuredTools>,
    pub records: MemRecordStore,
    pub cache: MemToolCache,
    pub provisioner: ScriptedProvisioner,
    pub audit: Option<(tempfile::TempDir, std::path::PathBuf)>,
}

impl ToolsRig {
    pub fn audit_file(&mut self) -> std::path::PathBuf {
        if self.audit.is_none() {
            let dir = tempfile::TempDir::new().expect("audit tempdir");
            let file = dir.path().join("audit.jsonl");
            self.audit = Some((dir, file));
        }
        self.audit.as_ref().expect("audit staged").1.clone()
    }
}

#[derive(Debug, Default)]
pub struct MemRecordStore {
    pub record: Mutex<Option<ResolvedRecord>>,
}

impl ToolRecordStore for MemRecordStore {
    fn load(&self) -> anyhow::Result<Option<ResolvedRecord>> {
        Ok(self.record.lock().unwrap().clone())
    }
    fn save(&self, record: &ResolvedRecord) -> anyhow::Result<()> {
        *self.record.lock().unwrap() = Some(record.clone());
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct MemToolCache {
    pub map: Mutex<HashMap<lns_service::tools::ToolCacheKey, ToolManifest>>,
}

impl ToolCache for MemToolCache {
    fn lookup(
        &self,
        key: &lns_service::tools::ToolCacheKey,
    ) -> anyhow::Result<Option<ToolManifest>> {
        Ok(self.map.lock().unwrap().get(key).cloned())
    }
    fn ingest(
        &self,
        key: &lns_service::tools::ToolCacheKey,
        staged: &StagedTool,
    ) -> anyhow::Result<ToolManifest> {
        let manifest = ToolManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
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
    fn evict(&self, key: &lns_service::tools::ToolCacheKey) -> anyhow::Result<()> {
        self.map.lock().unwrap().remove(key);
        Ok(())
    }
}

/// Resolves a fuzzy `node@22` to `{22}.{upstream_patch}` so a scenario can move upstream forward; an exact request echoes back unchanged; `fail_next` scripts one failed acquisition.
#[derive(Debug)]
pub struct ScriptedProvisioner {
    pub upstream_patch: Mutex<String>,
    pub fail_next: Mutex<Option<String>>,
    pub calls: Mutex<Vec<Vec<ToolRef>>>,
}

impl Default for ScriptedProvisioner {
    fn default() -> Self {
        Self {
            upstream_patch: Mutex::new("11.0".into()),
            fail_next: Mutex::new(None),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl ToolProvisioner for ScriptedProvisioner {
    fn newest_version(&self, _name: &str) -> impl Future<Output = anyhow::Result<String>> + Send {
        let newest = format!("22.{}", self.upstream_patch.lock().unwrap());
        async move { Ok(newest) }
    }

    fn provision(
        &self,
        requests: &[ToolRef],
        _target: &ProvisionTarget,
    ) -> impl Future<Output = Result<Vec<StagedTool>, ProvisionError>> + Send {
        self.calls.lock().unwrap().push(requests.to_vec());
        let fail = self.fail_next.lock().unwrap().take();
        let patch = self.upstream_patch.lock().unwrap().clone();
        let staged: Vec<StagedTool> = requests
            .iter()
            .map(|request| {
                let resolved = if request.version.matches('.').count() >= 2 {
                    request.version.clone()
                } else {
                    format!("{}.{patch}", request.version)
                };
                StagedTool {
                    name: request.name.clone(),
                    resolved,
                    backend: format!("core:{}", request.name),
                    source_host: "upstream.example.test".into(),
                    tar: StagedTar::Bytes(Vec::new()),
                    bin_paths: vec!["bin".into()],
                }
            })
            .collect();
        async move {
            match fail {
                Some(cause) => Err(ProvisionError::FetchFailed {
                    tool: "node@22".into(),
                    cause,
                }),
                None => Ok(staged),
            }
        }
    }
}
