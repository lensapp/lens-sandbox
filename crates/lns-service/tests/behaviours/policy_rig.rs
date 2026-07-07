use lns_policy::Policy;

#[derive(Debug, Default)]
pub struct PolicyRig {
    pub bundle_ships_policy: bool,
    pub bundle_policy: Policy,
    pub summary: Option<String>,
}
