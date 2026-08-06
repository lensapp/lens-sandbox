use crate::runner::CliRun;

/// A fixed host, so a scenario's expectations do not depend on the machine running it — and so no step has to probe, which would spawn a process.
pub const TEST_HOST: lns_artifact::resources::HostCapacity =
    lns_artifact::resources::HostCapacity {
        cpus: 10,
        mem_mib: 16384,
    };
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
    pub signin_outcome: Option<lns_cli::connector::SignInOutcome>,
    /// True when the connector under test signs in via the pkce browser redirect, so the fake sign-in renders the browser-opened prompt instead of a device code.
    pub signin_is_pkce: bool,
    pub resolved_run: Option<ResolvedRunView>,
    pub volume: VolumeCliRig,
    pub merged_env: Option<Result<Vec<String>, String>>,
    pub sandbox: SandboxCliRig,
    pub sandbox_run: SandboxRunRig,
    /// Scripted `lns push` producer outcome: Ok(digest) or Err(message).
    pub push_outcome: Option<Result<String, String>>,
    /// FileSet artifact refs the push uploaded, in order.
    pub pushed_filesets: Vec<String>,
    /// The definition JSON a prepared local run would send to the service.
    pub wire_definition: Option<String>,
    /// The preflight view a pulled-run scenario stages.
    pub pulled_view: Option<lns_ipc::SandboxView>,
    /// The definition doc the push handed to build_and_push, when it got that far.
    pub pushed_doc: Option<Vec<u8>>,
    pub tool_index: std::collections::HashMap<String, String>,
    /// Exact pins the scripted index answers "not listed" for at push verification.
    pub unlisted_pins: std::collections::HashSet<String>,
    pub host_bind: HostBindRig,
    pub host_access: HostAccessRig,
    /// In-memory `./lns.yaml` (and friends) for the offline author verbs; keyed by path under the fake cwd `/work`.
    pub author_files: std::collections::HashMap<std::path::PathBuf, String>,
    /// Request sequence each shortcut-equivalence invocation sent, in invocation order.
    pub equivalence_requests: Vec<Vec<lns_ipc::Request>>,
}

use lns_policy::host_bind_decisions::SecretDisposition;

/// Scripted host facts plus recorded verdicts for driving host-access resolution.
#[derive(Debug, Default)]
pub struct HostAccessRig {
    /// `git config --list -z` settings as (key, value) pairs, in the order git would emit them.
    pub git_settings: Vec<(String, String)>,
    /// True when the host has no git at all, so the config command fails.
    pub no_git: bool,
    pub openpgp_socket: Option<String>,
    pub gnupg_home: Option<String>,
    pub keyring: Option<Vec<u8>>,
    pub trustdb: Option<Vec<u8>>,
    pub ssh_socket: Option<String>,
    pub declared: Vec<String>,
    pub granted: Vec<String>,
    pub card_answer: Option<String>,
    pub secret_answer: Option<String>,
    pub secret_decisions: std::collections::HashMap<String, SecretDisposition>,
    pub declines: Vec<String>,
    pub outcome: Option<HostAccessOutcome>,
    /// Exit code plus captured output of the last `lns host-access` invocation.
    pub cli: Option<(i32, String)>,
}

#[derive(Debug)]
pub struct HostAccessOutcome {
    pub result: Result<Vec<lns_cli::run::host_access::HostAccessOutcome>, String>,
    pub prompt: String,
    pub persisted_secrets: std::collections::HashMap<String, SecretDisposition>,
    pub persisted_declines: Vec<String>,
    pub summary: String,
    pub policy_host_access: Vec<String>,
}

/// Scripted host directory + recorded decisions for driving `resolve_binds`.
#[derive(Debug, Default)]
pub struct HostBindRig {
    pub entries: Vec<String>,
    pub lensignore: Option<String>,
    pub missing: bool,
    pub not_a_dir: bool,
    pub decisions: std::collections::HashMap<String, SecretDisposition>,
    pub answer: Option<String>,
    pub outcome: Option<HostBindOutcome>,
}

#[derive(Debug)]
pub struct HostBindOutcome {
    pub result: Result<Vec<lns_cli::run::host_bind::ResolvedBind>, String>,
    pub prompt: String,
    pub persisted: std::collections::HashMap<String, SecretDisposition>,
    pub summary: String,
}

#[derive(Debug, Default)]
pub struct ResolvedRunView {
    pub summary: String,
    pub workdir: Option<String>,
    pub volumes: Vec<String>,
    pub binds: Vec<String>,
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

/// Drives the in-process `lns run` target resolution: what image a run request would carry, and a service refusal to surface.
#[derive(Debug, Default)]
pub struct SandboxRunRig {
    pub request_image: Option<String>,
    pub verify_sandbox: Option<bool>,
    pub definition: Option<String>,
    pub project_dir: Option<std::path::PathBuf>,
    pub refusal: Option<String>,
}

#[derive(Debug, Default)]
pub struct SandboxCliRig {
    pub response: Option<lns_ipc::Response>,
    /// Response the fake returns for a `RunStats` request specifically, so `ps` can canned-serve both a run listing and its stats.
    pub stats_response: Option<lns_ipc::Response>,
    /// Response the fake returns for an `InspectImage` request, so `inspect` can fall back from a running run to the cached artifact.
    pub inspect_image_response: Option<lns_ipc::Response>,
    /// Response the fake returns for a `RemoveImage` request, so `rm` can resolve running-vs-cached then remove.
    pub remove_image_response: Option<lns_ipc::Response>,
    pub frames: Vec<Vec<u8>>,
    pub unreachable: bool,
    pub policy: Option<serde_json::Value>,
    pub requests: std::sync::Arc<std::sync::Mutex<Vec<lns_ipc::Request>>>,
    pub workload_stdout: Vec<u8>,
    pub prompt_answer: Option<String>,
    pub stdin_is_tty: bool,
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
