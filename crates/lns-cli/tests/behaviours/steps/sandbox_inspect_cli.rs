use crate::world::BehaviourWorld;
use cucumber::{given, then};
use lns_ipc::{
    ArtifactInspection, ImageView, Response, SandboxMount, SandboxMountKind, SandboxPort,
    SandboxView,
};

fn full_digest() -> String {
    format!("sha256:{}", "a".repeat(64))
}

fn not_running(reference: &str) -> Response {
    Response::RunUnknown {
        run: reference.to_string(),
    }
}

fn cached_artifact(world: &mut BehaviourWorld, reference: &str, inspection: ArtifactInspection) {
    world.sandbox.response = Some(not_running(reference));
    world.sandbox.inspect_image_response = Some(Response::ImageInspected { inspection });
    world.sandbox.cached_references = vec![reference.to_string()];
}

#[given(regex = r#"^the service inspects "([^"]+)" as a plain image$"#)]
fn inspects_plain_image(world: &mut BehaviourWorld, reference: String) {
    let inspection = ArtifactInspection::Image(ImageView {
        reference: reference.clone(),
        digest: full_digest(),
    });
    cached_artifact(world, &reference, inspection);
}

#[given(
    regex = r#"^the service inspects "([^"]+)" as a sandbox built for arm64 and amd64 from "([^"]+)"$"#
)]
fn inspects_sandbox_built_for_two_architectures(
    world: &mut BehaviourWorld,
    reference: String,
    from: String,
) {
    inspects_sandbox_built_from(world, reference, from);
    let Some(Response::ImageInspected {
        inspection: ArtifactInspection::Sandbox(view),
    }) = world.sandbox.inspect_image_response.as_mut()
    else {
        unreachable!("the step above staged a sandbox inspection")
    };
    view.image_architectures = vec![
        lns_ipc::BuiltArchitecture {
            architecture: "arm64".into(),
            digest: "sha256:aaaa".into(),
        },
        lns_ipc::BuiltArchitecture {
            architecture: "amd64".into(),
            digest: "sha256:bbbb".into(),
        },
    ];
}

#[given(regex = r#"^the service inspects "([^"]+)" as a sandbox built from "([^"]+)"$"#)]
fn inspects_sandbox_built_from(world: &mut BehaviourWorld, reference: String, from: String) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        image_architectures: Vec::new(),
        image_source: Some(lns_ipc::BuildSourceView {
            containerfile: from,
            text: "FROM docker.io/library/node:24-bookworm\nRUN npm install -g @anthropic-ai/claude-code\n".into(),
            context: vec![
                lns_ipc::BuildContextFile {
                    path: "Containerfile".into(),
                    bytes: 87,
                },
                lns_ipc::BuildContextFile {
                    path: "app/main.js".into(),
                    bytes: 15,
                },
            ],
        }),
        mixins: Vec::new(),
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        reference: reference.clone(),
        digest: full_digest(),
        image: format!("ghcr.io/team/hermes@{}", full_digest()),
        workdir: None,
        user: None,
        mounts: vec![lns_ipc::SandboxMount {
            kind: lns_ipc::SandboxMountKind::Bind,
            source: ".".into(),
            target: "/workspace".into(),
            read_only: false,
            exclude: Vec::new(),
            optional: false,
            size_bytes: None,
        }],
        ports: Vec::new(),
        filesets: Vec::new(),
        credentials: Vec::new(),
        env: Vec::new(),
        tools: Vec::new(),
        scripts: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
        disk_bytes: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(regex = r#"^the service inspects "([^"]+)" as a sandbox with launch settings$"#)]
fn inspects_sandbox_settings(world: &mut BehaviourWorld, reference: String) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        image_architectures: Vec::new(),
        image_source: None,
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
                size_bytes: None,
            },
            SandboxMount {
                kind: SandboxMountKind::Volume,
                source: "some-cache".into(),
                target: "/home/node/.cache".into(),
                read_only: true,
                exclude: Vec::new(),
                optional: false,
                size_bytes: None,
            },
        ],
        ports: Vec::new(),
        filesets: Vec::new(),
        credentials: Vec::new(),
        env: Vec::new(),
        tools: Vec::new(),
        scripts: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
        disk_bytes: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(
    regex = r#"^the service inspects "([^"]+)" as a sandbox declaring ports 3003 and 8080:9090$"#
)]
fn inspects_sandbox_ports(world: &mut BehaviourWorld, reference: String) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        image_architectures: Vec::new(),
        image_source: None,
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
        credentials: Vec::new(),
        env: Vec::new(),
        tools: Vec::new(),
        scripts: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
        disk_bytes: None,
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
        credentials: vec![lns_spec::Credential {
            env_var: Some("SOME_TOKEN".into()),
            placeholder: "some_LNSPLACEHOLDER0000000000".into(),
            field: None,
            injections: vec![lns_spec::InjectionDef {
                kind: lns_spec::InjectionKind::BearerHeader,
                domain: "api.some-provider.example".into(),
                header: None,
            }],
        }],
        env: vec!["MODE=research".into()],
        tools: vec![tool],
        scripts: Vec::new(),
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
        image_architectures: Vec::new(),
        image_source: None,
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
        credentials: Vec::new(),
        env: Vec::new(),
        tools: Vec::new(),
        scripts: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
        disk_bytes: None,
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
        image_architectures: Vec::new(),
        image_source: None,
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
        credentials: Vec::new(),
        env: Vec::new(),
        tools: Vec::new(),
        scripts: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
        disk_bytes: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(
    regex = r#"^the service inspects "([^"]+)" as a sandbox declaring a fileset at "([^"]+)"$"#
)]
fn inspects_sandbox_filesets(world: &mut BehaviourWorld, reference: String, mount: String) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        image_architectures: Vec::new(),
        image_source: None,
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
            guest_path: mount,
            owner: lns_ipc::SandboxFilesetOwner::Workload,
        }],
        credentials: Vec::new(),
        env: Vec::new(),
        tools: Vec::new(),
        scripts: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
        disk_bytes: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(regex = r#"^the service inspects "([^"]+)" as a sandbox declaring user "([^"]+)"$"#)]
fn inspects_sandbox_user(world: &mut BehaviourWorld, reference: String, user: String) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        image_architectures: Vec::new(),
        image_source: None,
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
        credentials: Vec::new(),
        env: Vec::new(),
        tools: Vec::new(),
        scripts: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
        disk_bytes: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(
    regex = r#"^the service inspects "([^"]+)" as a sandbox whose policy allows every destination$"#
)]
fn inspects_sandbox_permissive_policy(world: &mut BehaviourWorld, reference: String) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        image_architectures: Vec::new(),
        image_source: None,
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
        credentials: Vec::new(),
        env: Vec::new(),
        tools: Vec::new(),
        scripts: Vec::new(),
        policy_flags: vec![
            "wildcard allow — a catch-all or whole-suffix host pattern is permitted".into(),
        ],
        cpus: None,
        mem_mib: None,
        disk_bytes: None,
    }));
    cached_artifact(world, &reference, inspection);
}

#[given(regex = r#"^the service inspects "([^"]+)" as a sandbox setting env "([^"]+)"$"#)]
fn inspects_sandbox_env(world: &mut BehaviourWorld, reference: String, entry: String) {
    let inspection = ArtifactInspection::Sandbox(Box::new(SandboxView {
        image_architectures: Vec::new(),
        image_source: None,
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
        credentials: Vec::new(),
        env: vec![entry],
        tools: Vec::new(),
        scripts: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
        disk_bytes: None,
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

/// §7.3 renders the instructions as the last thing an approver reads, locally and off a pulled artifact alike.
#[then(regex = r#"^the output prints "([^"]+)" after "([^"]+)"$"#)]
fn the_output_prints_after(world: &mut BehaviourWorld, last: String, first: String) {
    let output = &world.result.as_ref().expect("no CLI run captured").output;
    let at = |needle: &str| {
        output
            .find(needle)
            .unwrap_or_else(|| panic!("the output does not carry {needle:?}:\n{output}"))
    };
    assert!(
        at(&last) > at(&first),
        "{last:?} must come after {first:?}:\n{output}"
    );
}
