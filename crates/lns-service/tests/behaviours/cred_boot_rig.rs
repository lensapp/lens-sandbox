use lns_artifact::spec::CredentialSlot;
use lns_service::artifact::credential_boot::{SlotOutcome, SlotPlan};

#[derive(Debug, Default)]
pub struct CredBootRig {
    pub slot: Option<CredentialSlot>,
    pub bound_value: Option<String>,
    pub placeholder: Option<String>,
    pub plan: Option<SlotPlan>,
    pub outcome: Option<SlotOutcome>,
}
