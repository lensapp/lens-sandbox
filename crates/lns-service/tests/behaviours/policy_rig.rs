use lns_policy::Policy;

#[derive(Debug, Default)]
pub struct PolicyRig {
    pub sandbox_ships_policy: bool,
    pub sandbox_policy: Policy,
    pub summary: Option<String>,
}
