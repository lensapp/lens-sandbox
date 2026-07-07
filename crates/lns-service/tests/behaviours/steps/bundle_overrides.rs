use crate::resolve_rig::{Canned, FakeFetcher};
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::artifact::assembly::{Override, assemble};
use lns_service::artifact::plan_bundle;
use lns_service::artifact::resolve::FetchedComponent;

const SANDBOX_REF: &str = "reg/sandbox:1";
const AGENT_REF: &str = "reg/agent:1";

fn fileset_component(name: &str, path: &str) -> FetchedComponent {
    FetchedComponent {
        kind: "FileSet".into(),
        name: name.into(),
        mount_path: Some(path.into()),
        ..Default::default()
    }
}

fn present(world: &mut BehaviourWorld, reference: &str, component: FetchedComponent) {
    world
        .artifact()
        .canned
        .insert(reference.to_string(), Canned::Present(component));
}

#[given(regex = r#"^a bundle declaring a fileset "([^"]+)" mounting "([^"]+)"$"#)]
async fn declaring_fileset(world: &mut BehaviourWorld, name: String, path: String) {
    let reference = format!("reg/{name}:1");
    present(world, &reference, fileset_component(&name, &path));
    world.artifact().bundle_fileset_refs.push(reference);
}

async fn run_with_overrides(world: &mut BehaviourWorld) {
    present(
        world,
        SANDBOX_REF,
        FetchedComponent {
            kind: "Sandbox".into(),
            name: "some-sandbox".into(),
            base_image: Some("registry.example.test/base@sha256:abc".into()),
            ..Default::default()
        },
    );
    present(
        world,
        AGENT_REF,
        FetchedComponent {
            kind: "Agent".into(),
            name: "some-agent".into(),
            command: Some("agent --serve".into()),
            ..Default::default()
        },
    );
    let fileset_refs = world.artifact().bundle_fileset_refs.clone();
    let filesets = fileset_refs
        .iter()
        .map(|r| format!(r#"{{"ref":"{r}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let config = format!(
        r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"AgentSystem","metadata":{{"name":"some-bundle"}},"spec":{{"components":{{"sandbox":{{"ref":"{SANDBOX_REF}"}},"agents":[{{"ref":"{AGENT_REF}"}}],"filesets":[{filesets}]}}}}}}"#
    );
    let overrides: Vec<Override> = world
        .artifact()
        .with_refs
        .iter()
        .map(|r| Override {
            reference: r.clone(),
        })
        .collect();
    let canned = world.artifact().canned.clone();
    let fetcher = FakeFetcher::new(&canned);
    match plan_bundle(config.as_bytes(), &fetcher, "test-arch", &overrides).await {
        Ok(resolved) => world.artifact().assembled = Some(assemble(&resolved)),
        Err(e) => world.artifact().override_error = Some(format!("{e:#}")),
    }
}

fn override_fileset(world: &mut BehaviourWorld, name: &str, path: &str) {
    let reference = format!("reg/with-{name}:1");
    present(world, &reference, fileset_component(name, path));
    world.artifact().with_refs.push(reference);
}

#[when(regex = r#"^the bundle is run with --with a fileset "([^"]+)" mounting "([^"]+)"$"#)]
async fn run_with_one(world: &mut BehaviourWorld, name: String, path: String) {
    override_fileset(world, &name, &path);
    run_with_overrides(world).await;
}

#[when(
    regex = r#"^the bundle is run with --with a fileset "([^"]+)" mounting "([^"]+)" and --with a fileset "([^"]+)" mounting "([^"]+)"$"#
)]
async fn run_with_two(
    world: &mut BehaviourWorld,
    name_a: String,
    path_a: String,
    name_b: String,
    path_b: String,
) {
    override_fileset(world, &name_a, &path_a);
    override_fileset(world, &name_b, &path_b);
    run_with_overrides(world).await;
}

#[when(regex = r#"^the bundle is run with --with an artifact of kind "([^"]+)"$"#)]
async fn run_with_unsupported(world: &mut BehaviourWorld, kind: String) {
    let reference = "reg/unsupported:1".to_string();
    world.artifact().canned.insert(
        reference.clone(),
        Canned::UnsupportedKind {
            media_type: format!("application/vnd.lens.{}.v1+json", kind.to_lowercase()),
        },
    );
    world.artifact().with_refs.push(reference);
    run_with_overrides(world).await;
}

#[then("the run is refused because the override is not a mountable component")]
async fn refused_not_mountable(world: &mut BehaviourWorld) {
    let err = world
        .artifact()
        .override_error
        .clone()
        .expect("expected an override refusal");
    assert!(
        err.contains("unsupported component kind"),
        "expected a non-mountable-component refusal, got: {err}",
    );
}

#[then("nothing is assembled")]
async fn nothing_assembled(world: &mut BehaviourWorld) {
    assert!(
        world.artifact().assembled.is_none(),
        "a refused override must not assemble a workload",
    );
}
