use lns_artifact::spec::Resources;
use lns_service::artifact::resources::{ResourceOverrides, VmSize};

#[derive(Debug, Default)]
pub struct ResourceRig {
    pub bundle: Option<Resources>,
    pub overrides: ResourceOverrides,
    pub size: Option<VmSize>,
}
