use lns_policy::Policy;
use lns_service::artifact::policy::{EffectivePolicy, LayeredDecision};

#[derive(Debug, Default)]
pub struct PolicyRig {
    pub bundle_ships_policy: bool,
    pub cwd_present: bool,
    pub explicit_policy: Option<String>,
    pub bundle_policy: Policy,
    pub overlay_policy: Option<Policy>,
    pub replacement: Option<Policy>,
    pub effective: Option<EffectivePolicy>,
    pub summary: Option<String>,
    pub decision: Option<LayeredDecision>,
}
