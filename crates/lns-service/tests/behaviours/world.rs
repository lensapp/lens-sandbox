use cucumber::World;
use lns_ipc::Response;
use std::sync::Arc;
use std::time::Instant;

use crate::approval_rig::ApprovalRig;
use crate::artifact_rig::ArtifactRig;
use crate::assembly_rig::AssemblyRig;
use crate::bind_rig::BindRig;
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

    pub image_env: Option<Vec<String>>,
    pub image_user: Option<String>,
    pub spec_user: Option<String>,
    pub resolved_run_as: Option<lns_service::vm::RunAs>,
    pub image_workdir: Option<String>,
    pub composed_env: Option<lns_service::workload_env::WorkloadEnv>,
    pub resolved_workdir: Option<Option<String>>,
    pub user_env: Vec<String>,
    /// Each variable a granted connector fills for the run, by the connector that fills it.
    pub filled_by_a_grant: std::collections::BTreeMap<String, String>,
    /// What the run dropped because a grant fills it, from every source.
    pub refused_env: Vec<lns_service::workload_env::Refused>,
    /// `spec.env` entries, merged ahead of `-e` exactly as `sandbox_launch` merges them.
    pub definition_env: Vec<String>,

    pub forward_fake: Option<Arc<ForwardFake>>,
    pub forward_specs: Vec<PortPublish>,
    pub forward_guard: Option<ForwardGuard>,
    pub forward_error: Option<String>,

    pub volume: Option<VolumeRig>,

    pub bind: Option<BindRig>,

    pub connector: Option<crate::connector_rig::ConnectorRig>,

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
    /// The named volumes the scenario's run mounts, which a fileset landing under one is staged past.
    pub fileset_volumes: Vec<lns_ipc::VolumeMount>,
    /// The pre-start scripts a scenario declares, before they become a document.
    pub script_declaration: Option<crate::steps::guest_scripts::ScriptDeclaration>,
    /// The runtime-layer specs a planned run stages for its pre-start scripts.
    pub script_specs: Option<Vec<lns_service::runtime_layer::RuntimeFileSpec>>,
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

    /// What a stopped-run start scenario's preflight refuses with; `None` refuses nothing.
    pub startrun_refusal: Option<String>,
    /// Frames the StartRun exchange wrote to its client.
    pub startrun_frames: Vec<Response>,
    /// Whether the StartRun exchange reached the boot step.
    pub startrun_served: bool,
    /// The handle a StartRun scenario targets.
    pub startrun_target: Option<String>,
    /// Registry ids a StartRun scenario registered, deregistered after the exchange.
    pub startrun_cleanup: Vec<String>,
    /// The target run's status snapshotted after the exchange, before cleanup.
    pub startrun_status_after: Option<lns_ipc::RunStatus>,
    /// The volume holder's short run id, for the held-volume refusal.
    pub startrun_volume_holder: Option<String>,
    /// The record the scenario registered its stopped run with.
    pub startrun_record: Option<lns_service::run_record::RunRecord>,
    /// The record the scripted boot received.
    pub startrun_booted:
        std::sync::Arc<std::sync::Mutex<Option<lns_service::run_record::RunRecord>>>,
    /// Run dirs the removal scenarios' fake remover reclaimed.
    pub rm_reclaimed: std::sync::Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>,
    /// The stopped run ids a prune scenario staged.
    pub prune_stopped: Vec<String>,
    /// The orphan run dir id a prune scenario staged.
    pub prune_orphan: Option<String>,
    /// Held from a StartRun scenario's first Given to the end of its When: these scenarios share fixed names and the global stopped-run listing, so they must not interleave.
    pub startrun_serial: Option<tokio::sync::OwnedMutexGuard<()>>,
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
    pub primary_detach_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    pub first_target: Option<lns_ipc::SessionTarget>,
    pub second_target: Option<lns_ipc::SessionTarget>,
    pub first_event: Option<lns_service::vm::session_client::SessionInput>,
    pub response: Option<Response>,
    /// Whether the disconnected exec's guest task was observed cancelled by the production stream driver.
    pub first_task_terminated: Option<bool>,
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
