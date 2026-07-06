use lns_service::artifact::RunPath;
use lns_service::artifact::assembly::{AssembledWorkload, Override, ResolvedBundle};

#[derive(Debug, Default)]
pub struct ArtifactRig {
    pub artifact_type: Option<String>,
    pub config_media_type: Option<String>,
    pub path: Option<RunPath>,
    pub error: Option<String>,
    pub bundle: ResolvedBundle,
    pub overrides: Vec<Override>,
    pub override_error: Option<String>,
    pub assembled: Option<AssembledWorkload>,
}
