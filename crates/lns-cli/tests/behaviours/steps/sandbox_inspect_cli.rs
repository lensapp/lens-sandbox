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
    let inspection = ArtifactInspection::Sandbox(SandboxView {
        reference: reference.clone(),
        digest: full_digest(),
        image: "registry.example.test/runtime:1".into(),
        workdir: Some("/workspace".into()),
        mounts: vec![
            SandboxMount {
                kind: SandboxMountKind::Bind,
                source: ".".into(),
                target: "/workspace".into(),
                read_only: false,
            },
            SandboxMount {
                kind: SandboxMountKind::Volume,
                source: "some-cache".into(),
                target: "/home/node/.cache".into(),
                read_only: true,
            },
        ],
        ports: Vec::new(),
        filesets: Vec::new(),
        integrations: Vec::new(),
        policy_flags: Vec::new(),
    });
    cached_artifact(world, &reference, inspection);
}

#[given(
    regex = r#"^the service inspects "([^"]+)" as a sandbox declaring ports 3003 and 8080:9090$"#
)]
fn inspects_sandbox_ports(world: &mut BehaviourWorld, reference: String) {
    let inspection = ArtifactInspection::Sandbox(SandboxView {
        reference: reference.clone(),
        digest: full_digest(),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
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
        integrations: Vec::new(),
        policy_flags: Vec::new(),
    });
    cached_artifact(world, &reference, inspection);
}

#[given(
    regex = r#"^the service inspects "([^"]+)" as a sandbox declaring a fileset at "([^"]+)"$"#
)]
fn inspects_sandbox_filesets(world: &mut BehaviourWorld, reference: String, mount: String) {
    let inspection = ArtifactInspection::Sandbox(SandboxView {
        reference: reference.clone(),
        digest: full_digest(),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
        mounts: Vec::new(),
        ports: Vec::new(),
        filesets: vec![lns_ipc::SandboxFileset {
            path: None,
            reference: Some(format!(
                "registry.example.test/team/skills@sha256:{}",
                "a".repeat(64)
            )),
            mount_path: mount,
        }],
        integrations: Vec::new(),
        policy_flags: Vec::new(),
    });
    cached_artifact(world, &reference, inspection);
}

#[given(regex = r#"^the service inspects "([^"]+)" as a sandbox whose policy defaults to allow$"#)]
fn inspects_sandbox_permissive_policy(world: &mut BehaviourWorld, reference: String) {
    let inspection = ArtifactInspection::Sandbox(SandboxView {
        reference: reference.clone(),
        digest: full_digest(),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
        mounts: Vec::new(),
        ports: Vec::new(),
        filesets: Vec::new(),
        integrations: Vec::new(),
        policy_flags: vec![
            "permissive defaultVerdict: allow — the sandbox is open by default".into(),
        ],
    });
    cached_artifact(world, &reference, inspection);
}

#[given(regex = r#"^the service reports "inspect" needs a login for host "([^"]+)"$"#)]
fn inspect_needs_login(world: &mut BehaviourWorld, host: String) {
    world.sandbox.response = Some(not_running("some-registry.example/some-sandbox:research"));
    world.sandbox.inspect_image_response = Some(Response::Error {
        message: format!("inspecting the sandbox needs a login for {host}: run `lns login {host}`"),
    });
}
