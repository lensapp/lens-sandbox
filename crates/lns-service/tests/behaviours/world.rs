use cucumber::World;
use lns_ipc::Response;
use std::sync::Arc;
use std::time::Instant;

use crate::approval_rig::ApprovalRig;
use crate::artifact_rig::ArtifactRig;
use crate::assembly_rig::AssemblyRig;
use crate::bind_rig::BindRig;
use crate::credential_rig::CredentialRig;
use crate::declared_rig::DeclaredRig;
use crate::forward_rig::ForwardFake;
use crate::image_rig::ImageRig;
use crate::policy_rig::PolicyRig;
use crate::volume_rig::VolumeRig;
use lns_ipc::PortPublish;
use lns_service::forward::ForwardGuard;

#[derive(Debug, Default, World)]
pub struct BehaviourWorld {
    pub started_at: Option<Instant>,
    pub response: Option<Response>,
    pub approval: Option<ApprovalRig>,

    /// Lazily-built credential-flow rig — see `BehaviourWorld::credential`.
    pub credential: Option<CredentialRig>,

    /// The oauth connector id under test, set when an oauth sign-in scenario builds its rig.
    pub oauth_id: Option<String>,

    /// Set when a sign-in scenario needs the accept step to spawn the sign-in so a later step can cancel it mid-flight.
    pub spawn_connect: bool,

    /// The in-flight sign-in spawned by an accept step, awaited by the cancel step.
    pub connect_task: Option<tokio::task::JoinHandle<()>>,

    pub image_env: Option<Vec<String>>,
    pub image_user: Option<String>,
    pub spec_user: Option<String>,
    pub resolved_run_as: Option<lns_service::vm::RunAs>,
    pub image_workdir: Option<String>,
    pub composed_env: Option<lns_service::workload_env::WorkloadEnv>,
    pub resolved_workdir: Option<Option<String>>,
    pub user_env: Vec<String>,
    /// `spec.env` entries, merged ahead of `-e` exactly as `sandbox_launch` merges them.
    pub definition_env: Vec<String>,
    /// Env vars a connected connector manages for the run; `-e` overrides of these are refused.
    pub managed_vars: Vec<String>,

    pub forward_fake: Option<Arc<ForwardFake>>,
    pub forward_specs: Vec<PortPublish>,
    pub forward_guard: Option<ForwardGuard>,
    pub forward_error: Option<String>,

    pub volume: Option<VolumeRig>,

    pub bind: Option<BindRig>,

    pub image: Option<ImageRig>,

    pub artifact: Option<ArtifactRig>,

    pub assembly: Option<AssemblyRig>,

    pub policy: Option<PolicyRig>,

    pub declared: Option<DeclaredRig>,
    pub tools: Option<crate::tools_rig::ToolsRig>,

    /// The sandbox definition JSON a fileset-planning scenario stages.
    pub fileset_definition: Option<Vec<u8>>,
    /// The staged file name inside the scenario's path fileset directory.
    pub fileset_snapshot_file: Option<String>,
    pub fileset_plan: Option<lns_service::artifact::assembly::ResolvedSandbox>,
    pub fileset_problems: Option<Vec<String>>,
    pub fileset_specs: Option<Vec<String>>,
    pub fileset_contents: std::collections::HashMap<String, String>,
    /// The chown-manifest body the planned specs ship for lns-init, when any fileset is workload-owned.
    pub fileset_manifest: Option<String>,
    /// How many packed layers the scenario's own artifact carries, when it is a published one.
    pub fileset_artifact_layers: Option<usize>,
    /// Host files a hostPath scenario stages, by resolved absolute path, with the mode a probe reports.
    pub host_files: std::collections::HashMap<std::path::PathBuf, u32>,
    /// The home this scenario's machine reports; `None` means the machine has none.
    pub host_home: Option<std::path::PathBuf>,
    /// Host-file guest writes the plan produced, as (host source, guest path).
    pub host_file_writes: Vec<(String, String)>,

    /// Run id registered by a lifecycle scenario (stop / inspect / logs).
    pub lifecycle_run: Option<String>,

    /// Run id registered by a naming scenario, addressable later by name or id.
    pub naming_run: Option<String>,
    /// Name a naming scenario last assigned or observed.
    pub naming_name: Option<String>,
    /// First auto-generated name captured when comparing auto-name uniqueness.
    pub naming_first_name: Option<String>,
    /// Refusal message from the last registration / rename in a naming scenario.
    pub naming_error: Option<String>,
    /// What a run-start scenario's host refuses with; `None` means it refuses nothing.
    pub start_refusal: Option<String>,
    /// Frames the run-start exchange wrote to its client.
    pub start_frames: Vec<Response>,
    /// Whether the run-start exchange reached the step that registers the run.
    pub start_served: bool,
    /// The name the run-start scenario asked for.
    pub start_name: Option<String>,
    /// Whether the run-start scenario's host never finishes preparing.
    pub start_never_finishes: bool,
    /// Whether the run-start exchange returned rather than pending forever.
    pub start_returned: bool,
    pub exec: ExecRoutingRig,
}

#[derive(Debug, Default)]
pub struct ExecRoutingRig {
    pub run_id: Option<String>,
    pub primary_rx:
        Option<tokio::sync::mpsc::Receiver<lns_service::vm::session_client::SessionInput>>,
    pub first_rx:
        Option<tokio::sync::mpsc::Receiver<lns_service::vm::session_client::SessionInput>>,
    pub second_rx:
        Option<tokio::sync::mpsc::Receiver<lns_service::vm::session_client::SessionInput>>,
    pub first_target: Option<lns_ipc::SessionTarget>,
    pub second_target: Option<lns_ipc::SessionTarget>,
    pub first_event: Option<lns_service::vm::session_client::SessionInput>,
    pub response: Option<Response>,
}

impl Drop for ExecRoutingRig {
    fn drop(&mut self) {
        if let Some(run_id) = &self.run_id {
            lns_service::run_registry::deregister(run_id);
        }
    }
}

impl BehaviourWorld {
    pub fn started_at(&self) -> Instant {
        self.started_at.unwrap_or_else(Instant::now)
    }

    pub fn approval(&mut self) -> &mut ApprovalRig {
        if self.approval.is_none() {
            self.approval = Some(ApprovalRig::new());
        }
        self.approval.as_mut().expect("approval rig must exist")
    }

    pub fn credential(&mut self) -> &mut CredentialRig {
        if self.credential.is_none() {
            self.credential = Some(CredentialRig::new());
        }
        self.credential.as_mut().expect("credential rig must exist")
    }

    pub fn forward_fake(&mut self) -> Arc<ForwardFake> {
        self.forward_fake
            .get_or_insert_with(|| Arc::new(ForwardFake::default()))
            .clone()
    }

    pub fn volume(&mut self) -> &mut VolumeRig {
        if self.volume.is_none() {
            self.volume = Some(VolumeRig::new());
        }
        self.volume.as_mut().expect("volume rig must exist")
    }

    pub fn bind(&mut self) -> &mut BindRig {
        if self.bind.is_none() {
            self.bind = Some(BindRig::new());
        }
        self.bind.as_mut().expect("bind rig must exist")
    }

    pub fn image(&mut self) -> &mut ImageRig {
        if self.image.is_none() {
            self.image = Some(ImageRig::new());
        }
        self.image.as_mut().expect("image rig must exist")
    }

    pub fn artifact(&mut self) -> &mut ArtifactRig {
        self.artifact.get_or_insert_with(ArtifactRig::default)
    }

    pub fn policy(&mut self) -> &mut PolicyRig {
        self.policy.get_or_insert_with(PolicyRig::default)
    }
}
