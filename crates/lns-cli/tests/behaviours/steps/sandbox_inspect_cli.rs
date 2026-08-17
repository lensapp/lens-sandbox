use crate::world::BehaviourWorld;
use cucumber::given;
use lns_ipc::{
    ArtifactInspection, ImageView, Response, SandboxMount, SandboxMountKind, SandboxPort,
    SandboxView,
};

fn full_digest() -> String {
    format!("sha256:{}", "a".repeat(64))
}

fn not_running(reference: &str) -> Response {
    Response::Error {
        message: format!("no active run with id {reference}"),
    }
}

fn cached_artifact(world: &mut BehaviourWorld, reference: &str, inspection: ArtifactInspection) {
    world.sandbox.response = Some(not_running(reference));
    world.sandbox.inspect_image_response = Some(Response::ImageInspected { inspection });
}

#[given(regex = r#"^the service inspects "([^"]+)" as a plain image$"#)]
fn inspects_plain_image(world: &mut BehaviourWorld, reference: String) {
    let inspection = ArtifactInspection::Image(ImageView {
        reference: reference.clone(),
        digest: full_digest(),
    });
    cached_artifact(world, &reference, inspection);
}

#[given(regex = r#"^the service inspects "([^"]+)" as a sandbox with launch settings$"#)]
fn inspects_sandbox_settings(world: &mut BehaviourWorld, reference: String) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        mixins: Vec::new(),
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        reference: reference.clone(),
        digest: full_digest(),
        image: "registry.example.test/runtime:1".into(),
        workdir: Some("/workspace".into()),
        user: None,
        mounts: vec![
            SandboxMount {
                kind: SandboxMountKind::Bind,
                source: ".".into(),
                target: "/workspace".into(),
                read_only: false,
                exclude: Vec::new(),
                optional: false,
            },
            SandboxMount {
                kind: SandboxMountKind::Volume,
                source: "some-cache".into(),
                target: "/home/node/.cache".into(),
                read_only: true,
                exclude: Vec::new(),
                optional: false,
            },
        ],
        ports: Vec::new(),
        filesets: Vec::new(),
        connectors: Vec::new(),
        env: Vec::new(),
        credentials: Vec::new(),
        tools: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(
    regex = r#"^the service inspects "([^"]+)" as a sandbox declaring ports 3003 and 8080:9090$"#
)]
fn inspects_sandbox_ports(world: &mut BehaviourWorld, reference: String) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        mixins: Vec::new(),
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        reference: reference.clone(),
        digest: full_digest(),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
        user: None,
        mounts: Vec::new(),
        ports: vec![
            SandboxPort {
                host: None,
                container: 3003,
            },
            SandboxPort {
                host: Some(8080),
                container: 9090,
            },
        ],
        filesets: Vec::new(),
        connectors: Vec::new(),
        env: Vec::new(),
        credentials: Vec::new(),
        tools: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(regex = r#"^the service inspects "([^"]+)" as a mixin declaring the tool "([^"]+)"$"#)]
fn inspects_a_published_mixin(world: &mut BehaviourWorld, reference: String, tool: String) {
    let inspection = ArtifactInspection::Mixin(Box::new(lns_ipc::MixinView {
        reference: reference.clone(),
        digest: full_digest(),
        mixins: vec![format!("ghcr.io/acme/base@sha256:{}", "a".repeat(64))],
        mounts: Vec::new(),
        ports: vec![lns_ipc::SandboxPort {
            host: None,
            container: 9090,
        }],
        filesets: Vec::new(),
        env: vec!["MODE=research".into()],
        credentials: vec![lns_spec::Credential {
            env_var: "SOME_TOKEN".into(),
            placeholder: "some_LNSPLACEHOLDER0000".into(),
            injections: vec![lns_spec::InjectionDef {
                kind: lns_spec::InjectionKind::BearerHeader,
                domain: "api.some-provider.example".into(),
                header: None,
            }],
        }],
        tools: vec![tool],
        policy_flags: Vec::new(),
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(
    regex = r#"^the service inspects "([^"]+)" as a sandbox the user's mixin "([^"]+)" resolved into "([^"]+)"$"#
)]
fn inspects_sandbox_with_a_pinned_flag_mixin(
    world: &mut BehaviourWorld,
    reference: String,
    _tag: String,
    pinned: String,
) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        mixins: vec![pinned.clone()],
        pinned_mixins: vec![pinned],
        contributions: Vec::new(),
        reference: reference.clone(),
        digest: full_digest(),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
        user: None,
        mounts: Vec::new(),
        ports: Vec::new(),
        filesets: Vec::new(),
        connectors: Vec::new(),
        env: Vec::new(),
        credentials: Vec::new(),
        tools: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(
    regex = r#"^the service inspects "([^"]+)" as a sandbox resolved from the mixin "([^"]+)"$"#
)]
fn inspects_sandbox_resolved_from_a_mixin(
    world: &mut BehaviourWorld,
    reference: String,
    mixin: String,
) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        mixins: vec![mixin],
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        reference: reference.clone(),
        digest: full_digest(),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
        user: None,
        mounts: Vec::new(),
        ports: Vec::new(),
        filesets: Vec::new(),
        connectors: Vec::new(),
        env: Vec::new(),
        credentials: Vec::new(),
        tools: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(
    regex = r#"^the service inspects "([^"]+)" as a sandbox declaring a fileset at "([^"]+)"$"#
)]
fn inspects_sandbox_filesets(world: &mut BehaviourWorld, reference: String, mount: String) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        mixins: Vec::new(),
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        reference: reference.clone(),
        digest: full_digest(),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
        user: None,
        mounts: Vec::new(),
        ports: Vec::new(),
        filesets: vec![lns_ipc::SandboxFileset {
            path: Some("./skills".into()),
            inline: false,
            host_path: None,
            optional: false,
            mount_path: mount,
            owner: lns_ipc::SandboxFilesetOwner::Workload,
        }],
        connectors: Vec::new(),
        env: Vec::new(),
        credentials: Vec::new(),
        tools: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(regex = r#"^the service inspects "([^"]+)" as a sandbox declaring user "([^"]+)"$"#)]
fn inspects_sandbox_user(world: &mut BehaviourWorld, reference: String, user: String) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        mixins: Vec::new(),
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        reference: reference.clone(),
        digest: full_digest(),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
        user: Some(user),
        mounts: Vec::new(),
        ports: Vec::new(),
        filesets: Vec::new(),
        connectors: Vec::new(),
        env: Vec::new(),
        credentials: Vec::new(),
        tools: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(
    regex = r#"^the service inspects "([^"]+)" as a sandbox declaring the "([^"]+)" credential for "([^"]+)"$"#
)]
fn inspects_sandbox_credential(
    world: &mut BehaviourWorld,
    reference: String,
    env_var: String,
    domain: String,
) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        mixins: Vec::new(),
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        reference: reference.clone(),
        digest: full_digest(),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
        user: None,
        mounts: Vec::new(),
        ports: Vec::new(),
        filesets: Vec::new(),
        connectors: Vec::new(),
        env: Vec::new(),
        credentials: vec![lns_spec::Credential {
            placeholder: format!("lns-placeholder-{env_var}"),
            env_var,
            injections: vec![lns_spec::InjectionDef {
                kind: lns_spec::InjectionKind::BearerHeader,
                domain,
                header: None,
            }],
        }],
        tools: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(
    regex = r#"^the service inspects "([^"]+)" as a sandbox whose policy allows every destination$"#
)]
fn inspects_sandbox_permissive_policy(world: &mut BehaviourWorld, reference: String) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        mixins: Vec::new(),
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        reference: reference.clone(),
        digest: full_digest(),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
        user: None,
        mounts: Vec::new(),
        ports: Vec::new(),
        filesets: Vec::new(),
        connectors: Vec::new(),
        env: Vec::new(),
        credentials: Vec::new(),
        tools: Vec::new(),
        policy_flags: vec![
            "wildcard allow — a catch-all or whole-suffix host pattern is permitted".into(),
        ],
        cpus: None,
        mem_mib: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(regex = r#"^the service inspects "([^"]+)" as a sandbox setting env "([^"]+)"$"#)]
fn inspects_sandbox_env(world: &mut BehaviourWorld, reference: String, entry: String) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        mixins: Vec::new(),
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        reference: reference.clone(),
        digest: full_digest(),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
        user: None,
        mounts: Vec::new(),
        ports: Vec::new(),
        filesets: Vec::new(),
        connectors: Vec::new(),
        env: vec![entry],
        credentials: Vec::new(),
        tools: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(regex = r#"^the service reports "inspect" needs a login for host "([^"]+)"$"#)]
fn inspect_needs_login(world: &mut BehaviourWorld, host: String) {
    world.sandbox.response = Some(not_running("some-registry.example/some-sandbox:research"));
    world.sandbox.inspect_image_response = Some(Response::Error {
        message: format!("inspecting the sandbox needs a login for {host}: run `lns login {host}`"),
    });
}
