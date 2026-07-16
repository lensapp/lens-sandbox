use lns_policy::Policy;
use lns_policy::integrations::Integration;

/// Drives a sandbox definition's declared integrations through the launch arming path: catalog + definition + overlay in, armed providers + running policy out.
#[derive(Debug, Default)]
pub struct DeclaredRig {
    pub catalog: Vec<Integration>,
    pub definition: Option<String>,
    pub overlay: Policy,
    /// Armed providers as (integration id, env var, placeholder).
    pub providers: Vec<(String, String, String)>,
    pub running_policy: Option<Policy>,
    pub error: Option<String>,
}
