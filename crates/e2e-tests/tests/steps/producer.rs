use crate::E2eWorld;
use crate::registry::LocalRegistry;
use crate::specutil::run_cli_in_dir;
use cucumber::{given, then, when};
use oci_client::client::{ClientConfig, ClientProtocol};
use oci_client::{Reference, secrets::RegistryAuth};

fn extract_digest(text: &str) -> Option<String> {
    let start = text.find("sha256:")?;
    let rest = &text[start..];
    let hex_len = rest["sha256:".len()..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        .count();
    (hex_len == 64).then(|| rest[.."sha256:".len() + 64].to_string())
}

fn cache_env(world: &mut E2eWorld) -> Vec<(String, String)> {
    let home = world
        .home
        .get_or_insert_with(|| tempfile::TempDir::new().expect("home tempdir"));
    vec![
        (
            "HOME".to_string(),
            home.path().to_string_lossy().into_owned(),
        ),
        (
            "XDG_CACHE_HOME".to_string(),
            home.path().join(".cache").to_string_lossy().into_owned(),
        ),
    ]
}

#[given("a local registry")]
fn a_local_registry(world: &mut E2eWorld) {
    world.registry.get_or_insert_with(LocalRegistry::start);
}

async fn seed_base_image(host: &str) -> String {
    use sha2::{Digest, Sha256};
    let reference: Reference = format!("{host}/e2e-base:1")
        .parse()
        .expect("base ref parses");
    let client = oci_client::Client::new(ClientConfig {
        protocol: ClientProtocol::Http,
        ..Default::default()
    });
    let layer = b"e2e base layer tar".to_vec();
    let layer_digest = format!("sha256:{:x}", Sha256::digest(&layer));
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    };
    let config = format!(
        r#"{{"architecture":"{arch}","os":"linux","rootfs":{{"type":"layers","diff_ids":["{layer_digest}"]}}}}"#
    )
    .into_bytes();
    let config_digest = format!("sha256:{:x}", Sha256::digest(&config));
    let manifest = format!(
        r#"{{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{{"mediaType":"application/vnd.oci.image.config.v1+json","digest":"{config_digest}","size":{}}},"layers":[{{"mediaType":"application/vnd.oci.image.layer.v1.tar","digest":"{layer_digest}","size":{}}}]}}"#,
        config.len(),
        layer.len()
    )
    .into_bytes();
    let manifest_digest = format!("sha256:{:x}", Sha256::digest(&manifest));
    client
        .push_blob(&reference, layer, &layer_digest)
        .await
        .expect("push base layer");
    client
        .push_blob(&reference, config, &config_digest)
        .await
        .expect("push base config");
    client
        .push_manifest_raw(
            &reference,
            manifest,
            http::HeaderValue::from_static("application/vnd.oci.image.manifest.v1+json"),
        )
        .await
        .expect("push base manifest");
    format!("{host}/e2e-base@{manifest_digest}")
}

#[given("the user pushes a sandbox built from ./lns.yaml in one step")]
#[when("the user pushes a sandbox built from ./lns.yaml in one step")]
async fn push_sandbox_from_lns_yaml(world: &mut E2eWorld) {
    let host = world.registry.as_ref().expect("a registry").host();
    let reference = format!("{host}/e2e-cache-sandbox:1");
    let base = seed_base_image(&host).await;
    let definition = format!(
        "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: e2e-cache-sandbox\nspec:\n  image: {base}\n  workdir: /workspace\n  volumes:\n    - type: bind\n      source: .\n      target: /workspace\n    - type: volume\n      source: e2e-cache\n      target: /home/sandbox/.cache\n      readOnly: true\n"
    );
    let env = cache_env(world);
    let project = world.home.as_ref().expect("home set by cache_env").path();
    std::fs::write(project.join("lns.yaml"), definition).expect("write lns.yaml fixture");

    let pushed = run_cli_in_dir(project, ["push".to_string(), reference.clone()], env);
    assert_eq!(
        pushed.exit_code, 0,
        "lns push must build ./lns.yaml and upload it in one step:\n{}\n{}",
        pushed.stdout, pushed.stderr
    );
    assert!(
        pushed.stdout.contains("built and pushed"),
        "push must report the built-and-pushed reference:\n{}",
        pushed.stdout
    );
    world.pushed_digest = extract_digest(&format!("{}\n{}", pushed.stdout, pushed.stderr));
    world.pushed_ref = Some(reference);
}

#[when("the user pushes a sandbox declaring a path fileset in one step")]
async fn push_sandbox_with_fileset(world: &mut E2eWorld) {
    let host = world.registry.as_ref().expect("a registry").host();
    let reference = format!("{host}/e2e-fileset-sandbox:1");
    let base = seed_base_image(&host).await;
    let definition = format!(
        "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: e2e-fileset-sandbox\nspec:\n  image: {base}\n  filesets:\n    - path: ./skills\n      mountPath: /opt/agent-skills\n"
    );
    let env = cache_env(world);
    let project = world.home.as_ref().expect("home set by cache_env").path();
    std::fs::write(project.join("lns.yaml"), definition).expect("write lns.yaml fixture");
    std::fs::create_dir_all(project.join("skills")).expect("create fileset dir");
    std::fs::write(project.join("skills/prompts.md"), "fileset payload\n")
        .expect("write fileset file");

    let pushed = run_cli_in_dir(project, ["push".to_string(), reference.clone()], env);
    assert_eq!(
        pushed.exit_code, 0,
        "lns push must pack the fileset and upload the pinned sandbox:\n{}\n{}",
        pushed.stdout, pushed.stderr
    );
    assert!(
        pushed.stdout.contains("pushed fileset"),
        "push must report the packed fileset ref:\n{}",
        pushed.stdout
    );
    world.pushed_ref = Some(reference);
}

#[when(
    regex = r#"^the user pushes a sandbox declaring a root-owned inline file with content \"([^\"]+)\" in one step$"#
)]
async fn push_sandbox_with_inline_fileset(world: &mut E2eWorld, content: String) {
    let host = world.registry.as_ref().expect("a registry").host();
    let reference = format!("{host}/e2e-inline-sandbox:1");
    let base = seed_base_image(&host).await;
    let definition = format!(
        "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: e2e-inline-sandbox\nspec:\n  image: {base}\n  filesets:\n    - inline:\n        mcp.json: {content}\n      mountPath: /etc/agent\n      owner: root\n"
    );
    let env = cache_env(world);
    let project = world.home.as_ref().expect("home set by cache_env").path();
    std::fs::write(project.join("lns.yaml"), definition).expect("write inline lns.yaml fixture");

    let pushed = run_cli_in_dir(project, ["push".to_string(), reference.clone()], env);
    assert_eq!(
        pushed.exit_code, 0,
        "lns push must upload the self-contained inline sandbox:\n{}\n{}",
        pushed.stdout, pushed.stderr
    );
    world.pushed_ref = Some(reference);
    world.result = Some(pushed);
}

#[then("no companion FileSet artifact is uploaded")]
fn no_companion_fileset_artifact(world: &mut E2eWorld) {
    let result = world.result.as_ref().expect("inline push result");
    assert!(
        !result.stdout.contains("pushed fileset"),
        "inline push must not publish a companion FileSet:\n{}",
        result.stdout
    );
    let repositories = world
        .registry
        .as_ref()
        .expect("a registry")
        .manifest_repositories();
    assert!(
        repositories.iter().all(|name| !name.contains("fileset")),
        "inline push unexpectedly published a FileSet repository: {repositories:?}"
    );
}

#[given(regex = r#"^a version index resolving "([^"]+)" to versions "([^"]+)"$"#)]
fn version_index_stub(world: &mut E2eWorld, tool: String, versions: String) {
    world.version_index = Some(crate::registry::VersionIndex::start(
        &tool,
        &versions.replace(',', "\n"),
    ));
}

#[when(regex = r#"^the user pushes a sandbox declaring tool "([^"]+)" in one step$"#)]
async fn push_sandbox_with_tool(world: &mut E2eWorld, tool: String) {
    let host = world.registry.as_ref().expect("a registry").host();
    let reference = format!("{host}/e2e-tools-pin-sandbox:1");
    let base = seed_base_image(&host).await;
    let definition = format!(
        "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: e2e-tools-pin\nspec:\n  image: {base}\n  tools:\n    - {tool}\n"
    );
    let mut env = cache_env(world);
    env.push((
        "LNS_TOOL_INDEX_URL".to_string(),
        world
            .version_index
            .as_ref()
            .expect("a version index")
            .url()
            .to_string(),
    ));
    let project = world.home.as_ref().expect("home set by cache_env").path();
    std::fs::write(project.join("lns.yaml"), definition).expect("write lns.yaml fixture");

    let pushed = run_cli_in_dir(project, ["push".to_string(), reference.clone()], env);
    assert_eq!(
        pushed.exit_code, 0,
        "lns push must resolve the tool and upload the pinned sandbox:\n{}\n{}",
        pushed.stdout, pushed.stderr
    );
    world.pushed_ref = Some(reference);
}

#[then(regex = r#"^the pushed config pins tool "([^"]+)"$"#)]
async fn pushed_config_pins_tool(world: &mut E2eWorld, pinned: String) {
    let reference = world.pushed_ref.clone().expect("a ref was pushed");
    let parsed: Reference = reference.parse().expect("valid ref");
    let client = oci_client::Client::new(ClientConfig {
        protocol: ClientProtocol::Http,
        ..Default::default()
    });
    let (_, _, config) = client
        .pull_manifest_and_config(&parsed, &RegistryAuth::Anonymous)
        .await
        .expect("registry serves the pushed config");
    let value: serde_json::Value = serde_json::from_str(&config).expect("pushed config is JSON");
    let tools = value["spec"]["tools"]
        .as_array()
        .expect("pushed config carries spec.tools");
    assert!(
        tools
            .iter()
            .any(|tool| tool.as_str() == Some(pinned.as_str())),
        "expected {pinned:?} among {tools:?}"
    );
}

#[then("the registry serves the pushed artifact at its ref")]
async fn registry_serves_pushed(world: &mut E2eWorld) {
    let reference = world.pushed_ref.clone().expect("a ref was pushed");
    let digest = world.pushed_digest.clone().expect("a build digest");
    let parsed: Reference = reference.parse().expect("valid ref");
    let client = oci_client::Client::new(ClientConfig {
        protocol: ClientProtocol::Http,
        ..Default::default()
    });
    let (_, served_digest) = client
        .pull_manifest(&parsed, &RegistryAuth::Anonymous)
        .await
        .expect("registry serves the pushed manifest");
    assert_eq!(
        served_digest, digest,
        "the registry must serve exactly the cached build's manifest digest"
    );
}
