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

#[when("the user pushes a sandbox built from ./lns.yaml in one step")]
fn push_sandbox_from_lns_yaml(world: &mut E2eWorld) {
    let host = world.registry.as_ref().expect("a registry").host();
    let reference = format!("{host}/e2e-cache-sandbox:1");
    let definition = format!(
        "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: e2e-cache-sandbox\nspec:\n  image: reg/base@sha256:{}\n",
        "a".repeat(64)
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
