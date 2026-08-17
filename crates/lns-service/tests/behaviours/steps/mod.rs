pub mod approval_flow;
pub mod artifact_dispatch;
pub mod credential_at_boot;
pub mod credential_flow;
pub mod declared_connectors;
pub mod declared_tools;
pub mod env_injection;
pub mod forward;
pub mod host_binds;
pub mod image_management;
pub mod ipc;
pub mod mixin_directory;
pub mod mixin_flag;
pub mod mixin_resolution;
pub mod oauth_connector;
pub mod pkce_connector;
pub mod policy_guardrail;
pub mod run_as_env;
pub mod run_lifecycle;
pub mod run_naming;
pub mod run_user;
pub mod sandbox_filesets;
pub mod volume_management;
pub mod volumes;
pub mod workdir;

/// The origins a document's own `path` filesets have when nothing merges into it, which is what the service's resolution hands a plan.
pub fn own_fileset_origins(definition: &[u8]) -> Vec<lns_ipc::FilesetOrigin> {
    let Ok(def) = lns_artifact::sandbox::parse(definition) else {
        return Vec::new();
    };
    lns_service::artifact::mixin::fileset_origins_on_the_wire(
        &lns_artifact::merge::own_fileset_origins(&def.spec),
    )
}
