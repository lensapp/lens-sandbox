/// Declared-tools scenarios: a definition with `spec.tools`, launched through the
/// same planning refusals the orchestrator applies pre-boot.
#[derive(Debug, Default)]
pub struct ToolsRig {
    pub definition: Option<String>,
    pub error: Option<String>,
}
