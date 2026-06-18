use crate::runner::CliRun;
use cucumber::World;
use lns_cli::update_check::StatusReader;
use lns_ipc::UpdateStatus;
use tempfile::TempDir;
use tokio::io::DuplexStream;

#[derive(Debug, Default, World)]
pub struct BehaviourWorld {
    pub result: Option<CliRun>,
    pub argv: Vec<String>,
    pub cwd: Option<TempDir>,
    pub summary_output: String,
    pub phase_output: Vec<u8>,
    pub detached_stdout: Vec<u8>,
    pub pipe: Option<PhasePipe>,
    pub detached: bool,
    pub canned_sequence: CannedSequence,
    pub early_exit_code: Option<i32>,
    pub attached_stdout: Vec<u8>,
    pub attached_status: Vec<u8>,
    pub attached_stdout_is_terminal: Option<bool>,
    pub uc: UpdateCheckRig,
    pub signin_outcome: Option<lns_cli::integration::SignInOutcome>,
    pub resolved_run: Option<ResolvedRunView>,
    pub volume: VolumeCliRig,
    pub image: ImageCliRig,
    pub merged_env: Option<Result<Vec<String>, String>>,
    pub sandbox: SandboxCliRig,
    pub registry: FakeRegistryClient,
    pub push_digest: Option<String>,
    pub pull_digest: Option<String>,
    pub available_creds: Vec<String>,
    pub resolved_image: Option<String>,
    pub resolved_cmd: Vec<String>,
    pub resolved_policy: Option<std::path::PathBuf>,
    pub resolve_guard: Option<tempfile::NamedTempFile>,
    pub resolve_writer: String,
    pub resolve_error: Option<String>,
}

/// An in-memory stand-in for the service-backed OCI registry: push stores the
/// blob (and its artifactType) under its reference and returns a content-derived
/// digest; pull returns what was stored, so a push→pull round-trip is exercised
/// end to end.
type ArtifactStore =
    std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, (String, Vec<u8>, String)>>>;

#[derive(Debug, Default, Clone)]
pub struct FakeRegistryClient {
    inner: ArtifactStore,
}

impl FakeRegistryClient {
    fn digest(blob: &[u8]) -> String {
        let sum: u64 = blob.iter().map(|b| *b as u64).sum();
        format!("sha256:{sum:016x}")
    }
}

impl lns_cli::registry::RegistryClient for FakeRegistryClient {
    fn push_image<'a>(
        &'a self,
        source_reference: &'a str,
        _target_reference: &'a str,
    ) -> lns_cli::integration::LocalBoxFuture<'a, anyhow::Result<String>> {
        Box::pin(async move { Ok(Self::digest(source_reference.as_bytes())) })
    }

    fn push_artifact<'a>(
        &'a self,
        reference: &'a str,
        artifact_type: &'a str,
        _config_media_type: &'a str,
        config_blob: &'a [u8],
    ) -> lns_cli::integration::LocalBoxFuture<'a, anyhow::Result<String>> {
        Box::pin(async move {
            let digest = Self::digest(config_blob);
            self.inner.lock().unwrap().insert(
                reference.to_string(),
                (
                    artifact_type.to_string(),
                    config_blob.to_vec(),
                    digest.clone(),
                ),
            );
            Ok(digest)
        })
    }

    fn pull<'a>(
        &'a self,
        reference: &'a str,
    ) -> lns_cli::integration::LocalBoxFuture<'a, anyhow::Result<lns_cli::registry::Pulled>> {
        Box::pin(async move {
            let stored = self.inner.lock().unwrap().get(reference).cloned();
            match stored {
                Some((artifact_type, config_blob, digest)) => {
                    Ok(lns_cli::registry::Pulled::Artifact {
                        artifact_type,
                        config_blob,
                        digest,
                    })
                }
                None => anyhow::bail!("no artifact at {reference}"),
            }
        })
    }
}

#[derive(Debug, Default)]
pub struct ResolvedRunView {
    pub summary: String,
    pub env: Vec<String>,
    pub volumes: Vec<String>,
    pub publish: Vec<String>,
}

/// Scripted state for the fake volume service plus the user's prompt answer.
#[derive(Debug, Default)]
pub struct VolumeCliRig {
    pub volumes: Vec<lns_ipc::VolumeInfo>,
    pub prune_plan: Option<(Vec<String>, u64)>,
    pub prune_failed: Vec<lns_ipc::VolumePruneFailure>,
    pub refuse_message: Option<String>,
    pub unreachable: bool,
    pub requests: std::sync::Arc<std::sync::Mutex<Vec<lns_ipc::Request>>>,
    pub prompt_answer: Option<String>,
}

/// Scripted state for the fake image service plus the user's prompt answer.
#[derive(Debug, Default)]
pub struct ImageCliRig {
    pub images: Vec<lns_ipc::ImageInfo>,
    pub pull_result: Option<lns_ipc::ImageInfo>,
    pub remove_result: Option<(String, u64)>,
    pub prune_plan: Option<(Vec<String>, u64)>,
    pub refuse_message: Option<String>,
    pub unreachable: bool,
    pub requests: std::sync::Arc<std::sync::Mutex<Vec<lns_ipc::Request>>>,
    pub prompt_answer: Option<String>,
}

#[derive(Debug, Default)]
pub struct SandboxCliRig {
    pub response: Option<lns_ipc::Response>,
    pub frames: Vec<Vec<u8>>,
    pub unreachable: bool,
    pub policy: Option<serde_json::Value>,
    pub requests: std::sync::Arc<std::sync::Mutex<Vec<lns_ipc::Request>>>,
    pub workload_stdout: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct UpdateCheckRig {
    pub reader: FakeReader,
    pub out: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct FakeReader {
    pub status: Option<UpdateStatus>,
    pub install_id: Option<String>,
}
impl StatusReader for FakeReader {
    fn read_status(&self) -> Option<UpdateStatus> {
        self.status.clone()
    }
    fn read_install_id(&self) -> Option<String> {
        self.install_id.clone()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CannedSequence {
    #[default]
    None,
    ColdCache,
    WarmCache,
}

#[derive(Debug)]
pub struct PhasePipe {
    pub client: DuplexStream,
    pub server: DuplexStream,
}

impl PhasePipe {
    pub fn new() -> Self {
        let (client, server) = tokio::io::duplex(8192);
        Self { client, server }
    }
}
