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
    /// True when the integration under test signs in via the pkce browser redirect, so the fake sign-in renders the browser-opened prompt instead of a device code.
    pub signin_is_pkce: bool,
    pub resolved_run: Option<ResolvedRunView>,
    pub volume: VolumeCliRig,
    pub image: ImageCliRig,
    pub merged_env: Option<Result<Vec<String>, String>>,
    pub sandbox: SandboxCliRig,
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
