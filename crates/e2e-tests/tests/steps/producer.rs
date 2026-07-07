use crate::E2eWorld;
use crate::registry::LocalRegistry;
use crate::specutil::run_cli_with_env;
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

#[when("the user builds a sandbox into the cache and then pushes it from the cache")]
fn build_into_cache_then_push(world: &mut E2eWorld) {
    let host = world.registry.as_ref().expect("a registry").host();
    let reference = format!("{host}/e2e-cache-sandbox:1");
    let manifest = format!(
        "apiVersion: lens.dev/v1alpha1\nkind: Sandbox\nmetadata:\n  name: e2e-cache-sandbox\nspec:\n  isolation: microvm\n  baseImage: reg/base@sha256:{}\n",
        "a".repeat(64)
    );
    let env = cache_env(world);
    let manifest_path = world
        .home
        .as_ref()
        .expect("home set by cache_env")
        .path()
        .join("cache-sandbox.yaml");
    std::fs::write(&manifest_path, manifest).expect("write manifest fixture");

    let built = run_cli_with_env(
        [
            "build".to_string(),
            manifest_path.to_string_lossy().into_owned(),
            "-t".to_string(),
            reference.clone(),
        ],
        env.clone(),
    );
    assert_eq!(
        built.exit_code, 0,
        "lns build (no push) must populate the cache:\n{}\n{}",
        built.stdout, built.stderr
    );
    assert!(
        !built.stdout.contains("pushed"),
        "a build without --push must not upload:\n{}",
        built.stdout
    );
    world.pushed_digest = extract_digest(&format!("{}\n{}", built.stdout, built.stderr));

    let pushed = run_cli_with_env(["push".to_string(), reference.clone()], env);
    assert_eq!(
        pushed.exit_code, 0,
        "lns push must upload the cached artifact:\n{}\n{}",
        pushed.stdout, pushed.stderr
    );
    world.pushed_ref = Some(reference);
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
