use std::collections::HashMap;

use crate::resolve_rig::Canned;
use lns_service::artifact::RunPath;
use lns_service::artifact::assembly::{AssembledWorkload, ResolvedBundle};
use lns_service::artifact::signature::{SignatureStatus, Verdict};

#[derive(Debug, Default)]
pub struct ArtifactRig {
    pub artifact_type: Option<String>,
    pub config_media_type: Option<String>,
    pub path: Option<RunPath>,
    pub error: Option<String>,
    pub bundle: ResolvedBundle,
    pub canned: HashMap<String, Canned>,
    pub bundle_fileset_refs: Vec<String>,
    pub with_refs: Vec<String>,
    pub override_error: Option<String>,
    pub assembled: Option<AssembledWorkload>,
    pub trusted_keys_configured: bool,
    pub signature_status: Option<SignatureStatus>,
    pub verdict: Option<Verdict>,
}
