use crate::E2eWorld;
use crate::registry::LocalRegistry;
use crate::specutil::run_cli_with_timeout;
use cucumber::{given, when};
use oci_client::client::ClientConfig;
use oci_client::{Reference, secrets::RegistryAuth};
use std::time::Duration;

/// The arch-specific (platform) manifest digest of `alpine:3.20`, pulled straight
/// from the index so the sandbox baseImage is pinned to the exact manifest
/// `lns run` fetches (pinning the multi-arch index digest fails verification,
/// since lns resolves it to the platform manifest whose bytes hash differently).
async fn base_image_platform_digest() -> String {
    use oci_client::manifest::OciManifest;
    use oci_spec::image::{Arch, Os};
    let want = if std::env::consts::ARCH == "aarch64" {
        Arch::ARM64
    } else {
        Arch::Amd64
    };
    let client = oci_client::Client::new(ClientConfig::default());
    let reference: Reference = "docker.io/library/alpine:3.20".parse().expect("alpine ref");
    let (manifest, tag_digest) = client
        .pull_manifest(&reference, &RegistryAuth::Anonymous)
        .await
        .expect("pull alpine manifest");
    match manifest {
        OciManifest::Image(_) => tag_digest,
        OciManifest::ImageIndex(index) => index
            .manifests
            .iter()
            .find(|e| {
                !e.media_type.contains("attestation")
                    && e.platform
                        .as_ref()
                        .is_some_and(|p| matches!(p.os, Os::Linux) && p.architecture == want)
            })
            .map(|e| e.digest.clone())
            .expect("alpine index has a linux entry for this arch"),
    }
}

const BUILD_TIMEOUT: Duration = Duration::from_secs(120);
const RUN_TIMEOUT: Duration = Duration::from_secs(180);

fn socket_env(world: &E2eWorld) -> Vec<(&'static str, std::ffi::OsString)> {
    world
        .service_socket
        .as_ref()
        .map(|socket| vec![("LNS_SOCKET_PATH", socket.clone().into())])
        .unwrap_or_default()
}

fn run(world: &E2eWorld, args: &[String], timeout: Duration) -> crate::specutil::CliResult {
    run_cli_with_timeout(args.to_vec(), socket_env(world), timeout)
}

/// Pull the first `sha256:<64hex>` out of some command output.
fn extract_digest(text: &str) -> Option<String> {
    let start = text.find("sha256:")?;
    let rest = &text[start..];
    let hex_len = rest["sha256:".len()..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        .count();
    (hex_len == 64).then(|| rest[.."sha256:".len() + 64].to_string())
}

fn fixture_path(world: &mut E2eWorld, name: &str, body: &str) -> String {
    let dir = world
        .fixture_dir
        .get_or_insert_with(|| tempfile::TempDir::new().expect("fixture tempdir"));
    let path = dir.path().join(name);
    std::fs::write(&path, body).expect("write fixture");
    path.to_string_lossy().into_owned()
}

fn build_push(world: &mut E2eWorld, manifest_path: &str, tag: &str) -> String {
    let args = vec![
        "build".to_string(),
        manifest_path.to_string(),
        "-t".to_string(),
        tag.to_string(),
        "--push".to_string(),
    ];
    let result = run(world, &args, BUILD_TIMEOUT);
    assert_eq!(
        result.exit_code, 0,
        "lns build --push {tag} failed:\nstdout={}\nstderr={}",
        result.stdout, result.stderr
    );
    extract_digest(&format!("{}\n{}", result.stdout, result.stderr))
        .unwrap_or_else(|| panic!("no digest in build output for {tag}:\n{}", result.stdout))
}

fn build_push_dir(world: &mut E2eWorld, dir: &str, mount: &str, tag: &str) -> String {
    let args = vec![
        "build".to_string(),
        dir.to_string(),
        "--mount".to_string(),
        mount.to_string(),
        "-t".to_string(),
        tag.to_string(),
        "--push".to_string(),
    ];
    let result = run(world, &args, BUILD_TIMEOUT);
    assert_eq!(
        result.exit_code, 0,
        "lns build --push {tag} (fileset) failed:\nstdout={}\nstderr={}",
        result.stdout, result.stderr
    );
    extract_digest(&format!("{}\n{}", result.stdout, result.stderr)).unwrap_or_else(|| {
        panic!(
            "no digest in fileset build output for {tag}:\n{}",
            result.stdout
        )
    })
}

fn fixture_dir_with_file(world: &mut E2eWorld, dir_name: &str, file: &str, body: &str) -> String {
    let dir = world
        .fixture_dir
        .get_or_insert_with(|| tempfile::TempDir::new().expect("fixture tempdir"));
    let sub = dir.path().join(dir_name);
    std::fs::create_dir_all(&sub).expect("create fixture dir");
    std::fs::write(sub.join(file), body).expect("write fixture file");
    sub.to_string_lossy().into_owned()
}

#[given("a local registry holding a bundle whose fileset the agent reads")]
async fn registry_with_fileset_bundle(world: &mut E2eWorld) {
    let host = world
        .registry
        .get_or_insert_with(LocalRegistry::start)
        .host();
    let base_digest = base_image_platform_digest().await;

    let sandbox = format!(
        "apiVersion: lens.dev/v1alpha1\nkind: Sandbox\nmetadata:\n  name: e2e-sandbox\nspec:\n  isolation: microvm\n  baseImage: alpine@{base_digest}\n"
    );
    let sandbox_path = fixture_path(world, "fs-sandbox.yaml", &sandbox);
    let sandbox_digest = build_push(world, &sandbox_path, &format!("{host}/e2e-fs-sandbox:1"));

    let skills_dir = fixture_dir_with_file(world, "skills", "greeting.txt", "hello-from-fileset");
    let fileset_digest = build_push_dir(
        world,
        &skills_dir,
        "/root/skills",
        &format!("{host}/e2e-skills:1"),
    );

    let agent = "apiVersion: lens.dev/v1alpha1\nkind: Agent\nmetadata:\n  name: e2e-fs-agent\nspec:\n  command: /bin/cat /root/skills/greeting.txt\n";
    let agent_path = fixture_path(world, "fs-agent.yaml", agent);
    let agent_digest = build_push(world, &agent_path, &format!("{host}/e2e-fs-agent:1"));

    let bundle = format!(
        "apiVersion: lens.dev/v1alpha1\nkind: AgentSystem\nmetadata:\n  name: e2e-fs-bundle\nspec:\n  components:\n    sandbox:\n      ref: {host}/e2e-fs-sandbox\n      digest: {sandbox_digest}\n    agents:\n      - ref: {host}/e2e-fs-agent\n        digest: {agent_digest}\n    filesets:\n      - ref: {host}/e2e-skills\n        digest: {fileset_digest}\n"
    );
    let bundle_path = fixture_path(world, "fs-bundle.yaml", &bundle);
    let bundle_ref = format!("{host}/e2e-fs-bundle:1");
    build_push(world, &bundle_path, &bundle_ref);
    world.bundle_ref = Some(bundle_ref);
}

#[given("a local registry holding a bundle whose components are referenced by tag")]
async fn registry_with_tag_bundle(world: &mut E2eWorld) {
    let host = world
        .registry
        .get_or_insert_with(LocalRegistry::start)
        .host();
    let base_digest = base_image_platform_digest().await;

    let sandbox = format!(
        "apiVersion: lens.dev/v1alpha1\nkind: Sandbox\nmetadata:\n  name: e2e-tag-sandbox\nspec:\n  isolation: microvm\n  baseImage: alpine@{base_digest}\n"
    );
    let sandbox_path = fixture_path(world, "tag-sandbox.yaml", &sandbox);
    build_push(world, &sandbox_path, &format!("{host}/e2e-tag-sandbox:1"));

    let agent = "apiVersion: lens.dev/v1alpha1\nkind: Agent\nmetadata:\n  name: e2e-tag-agent\nspec:\n  command: echo bundle-boot-ok\n";
    let agent_path = fixture_path(world, "tag-agent.yaml", agent);
    build_push(world, &agent_path, &format!("{host}/e2e-tag-agent:1"));

    let bundle = format!(
        "apiVersion: lens.dev/v1alpha1\nkind: AgentSystem\nmetadata:\n  name: e2e-tag-bundle\nspec:\n  components:\n    sandbox:\n      ref: {host}/e2e-tag-sandbox:1\n    agents:\n      - ref: {host}/e2e-tag-agent:1\n"
    );
    let bundle_path = fixture_path(world, "tag-bundle.yaml", &bundle);
    let bundle_ref = format!("{host}/e2e-tag-bundle:1");
    build_push(world, &bundle_path, &bundle_ref);
    world.bundle_ref = Some(bundle_ref);
}

#[given(regex = r#"^a local registry holding a bundle whose agent runs "([^"]+)"$"#)]
async fn registry_with_bundle(world: &mut E2eWorld, command: String) {
    let host = world
        .registry
        .get_or_insert_with(LocalRegistry::start)
        .host();
    let base_digest = base_image_platform_digest().await;

    let sandbox = format!(
        "apiVersion: lens.dev/v1alpha1\nkind: Sandbox\nmetadata:\n  name: e2e-sandbox\nspec:\n  isolation: microvm\n  baseImage: alpine@{base_digest}\n"
    );
    let sandbox_path = fixture_path(world, "sandbox.yaml", &sandbox);
    let sandbox_digest = build_push(world, &sandbox_path, &format!("{host}/e2e-sandbox:1"));

    let agent = format!(
        "apiVersion: lens.dev/v1alpha1\nkind: Agent\nmetadata:\n  name: e2e-agent\nspec:\n  command: {command}\n"
    );
    let agent_path = fixture_path(world, "agent.yaml", &agent);
    let agent_digest = build_push(world, &agent_path, &format!("{host}/e2e-agent:1"));

    let bundle = format!(
        "apiVersion: lens.dev/v1alpha1\nkind: AgentSystem\nmetadata:\n  name: e2e-bundle\nspec:\n  components:\n    sandbox:\n      ref: {host}/e2e-sandbox\n      digest: {sandbox_digest}\n    agents:\n      - ref: {host}/e2e-agent\n        digest: {agent_digest}\n"
    );
    let bundle_path = fixture_path(world, "bundle.yaml", &bundle);
    let bundle_ref = format!("{host}/e2e-bundle:1");
    build_push(world, &bundle_path, &bundle_ref);
    world.bundle_ref = Some(bundle_ref);
}

#[when("the user runs the bundle reference")]
fn run_bundle(world: &mut E2eWorld) {
    let bundle_ref = world
        .bundle_ref
        .clone()
        .expect("a bundle must be built before running it");
    let result = run(world, &["run".to_string(), bundle_ref], RUN_TIMEOUT);
    world.result = Some(result);
}
