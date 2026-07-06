use lns_service::artifact::RunPath;
use lns_service::artifact::assembly::{AssembledWorkload, ResolvedBundle};

#[derive(Debug, Default)]
pub struct ArtifactRig {
    pub artifact_type: Option<String>,
    pub path: Option<RunPath>,
    pub error: Option<String>,
    pub bundle: ResolvedBundle,
    pub assembled: Option<AssembledWorkload>,
}
