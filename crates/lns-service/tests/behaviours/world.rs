use cucumber::World;
use lns_ipc::Response;
use std::sync::Arc;
use std::time::Instant;

use crate::approval_rig::ApprovalRig;
use crate::credential_rig::CredentialRig;
use crate::forward_rig::ForwardFake;
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

    /// Custom-provider scenarios: the loaded policy file and the boot-time wire credentials seeded into the workload at launch.
    pub custom_policy: Option<lns_policy::Policy>,
    pub running_seed: Option<Vec<lns_service::approval_flow::protocol::Credential>>,
    pub launched_seed: Option<Vec<lns_service::approval_flow::protocol::Credential>>,

    pub image_env: Option<Vec<String>>,
    pub composed_env: Option<lns_service::workload_env::WorkloadEnv>,
    pub user_env: Vec<String>,

    pub forward_fake: Option<Arc<ForwardFake>>,
    pub forward_specs: Vec<PortPublish>,
    pub forward_guard: Option<ForwardGuard>,
    pub forward_error: Option<String>,

    pub volume: Option<VolumeRig>,
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
}
