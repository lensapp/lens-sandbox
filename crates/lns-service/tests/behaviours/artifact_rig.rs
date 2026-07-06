use lns_service::artifact::RunPath;

#[derive(Debug, Default)]
pub struct ArtifactRig {
    pub artifact_type: Option<String>,
    pub path: Option<RunPath>,
    pub error: Option<String>,
}
