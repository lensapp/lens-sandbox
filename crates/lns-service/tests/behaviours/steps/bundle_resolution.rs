use crate::resolve_rig::{Canned, FakeFetcher};
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::artifact::resolve::{BundleSpec, DeclaredComponent, FetchedComponent, resolve};

const HOST_ARCH: &str = "test-arch";

fn component(kind: &str) -> FetchedComponent {
    FetchedComponent {
        kind: kind.to_string(),
        ..Default::default()
    }
}

fn present(kind: &str) -> Canned {
    Canned::Present(component(kind))
}

fn sandbox_component() -> FetchedComponent {
    FetchedComponent {
        kind: "Sandbox".into(),
        arch: Some(HOST_ARCH.into()),
        base_image: Some("registry.example.test/base:1".into()),
        ..Default::default()
    }
}

fn agent_component() -> FetchedComponent {
    let mut env = std::collections::BTreeMap::new();
    env.insert("MODE".into(), "research".into());
    FetchedComponent {
        kind: "Agent".into(),
        command: Some("agent --serve".into()),
        env,
        ..Default::default()
    }
}

#[given("a bundle whose sandbox, agent, and fileset are all present and supported")]
async fn well_formed(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    rig.components = vec![
        ("sandbox".into(), "reg/base:1".into()),
        ("agent".into(), "reg/agent:1".into()),
        ("skills".into(), "reg/skills:1".into()),
        ("policy".into(), "reg/policy:1".into()),
    ];
    let mut fileset = component("FileSet");
    fileset.name = "skills".into();
    fileset.mount_path = Some("/root/.some-agent/skills".into());
    rig.canned
        .insert("reg/base:1".into(), Canned::Present(sandbox_component()));
    rig.canned
        .insert("reg/agent:1".into(), Canned::Present(agent_component()));
    rig.canned
        .insert("reg/skills:1".into(), Canned::Present(fileset));
    rig.canned.insert("reg/policy:1".into(), present("Policy"));
}

#[given("a bundle whose sandbox and agent both reference the same fileset")]
async fn shared_fileset(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    rig.components = vec![
        ("sandbox".into(), "reg/base:1".into()),
        ("agent".into(), "reg/agent:1".into()),
    ];
    let mut sandbox = sandbox_component();
    sandbox.references = vec!["reg/shared:1".into()];
    let mut agent = agent_component();
    agent.references = vec!["reg/shared:1".into()];
    rig.canned
        .insert("reg/base:1".into(), Canned::Present(sandbox));
    rig.canned
        .insert("reg/agent:1".into(), Canned::Present(agent));
    rig.canned.insert("reg/shared:1".into(), present("FileSet"));
}

#[given("a bundle with no sandbox")]
async fn no_sandbox(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    rig.components = vec![("agent".into(), "reg/agent:1".into())];
    rig.canned
        .insert("reg/agent:1".into(), Canned::Present(agent_component()));
}

#[given("a bundle with two sandboxes")]
async fn two_sandboxes(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    rig.components = vec![
        ("sandbox-a".into(), "reg/base-a:1".into()),
        ("sandbox-b".into(), "reg/base-b:1".into()),
        ("agent".into(), "reg/agent:1".into()),
    ];
    rig.canned
        .insert("reg/base-a:1".into(), Canned::Present(sandbox_component()));
    rig.canned
        .insert("reg/base-b:1".into(), Canned::Present(sandbox_component()));
    rig.canned
        .insert("reg/agent:1".into(), Canned::Present(agent_component()));
}

#[given("a bundle with no agent")]
async fn no_agent(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    rig.components = vec![("sandbox".into(), "reg/base:1".into())];
    rig.canned
        .insert("reg/base:1".into(), Canned::Present(sandbox_component()));
}

#[given("a bundle with two agents")]
async fn two_agents(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    rig.components = vec![
        ("sandbox".into(), "reg/base:1".into()),
        ("agent-a".into(), "reg/agent-a:1".into()),
        ("agent-b".into(), "reg/agent-b:1".into()),
    ];
    rig.canned
        .insert("reg/base:1".into(), Canned::Present(sandbox_component()));
    rig.canned
        .insert("reg/agent-a:1".into(), Canned::Present(agent_component()));
    rig.canned
        .insert("reg/agent-b:1".into(), Canned::Present(agent_component()));
}

#[given(
    regex = r#"^a bundle referencing a fileset "([^"]+)" that is not present in the registry$"#
)]
async fn missing_fileset(world: &mut BehaviourWorld, name: String) {
    world.resolve().components = vec![(name, "reg/absent:1".into())];
}

#[given(regex = r#"^a bundle referencing a component of kind "([^"]+)"$"#)]
async fn unsupported_kind(world: &mut BehaviourWorld, kind: String) {
    let rig = world.resolve();
    rig.components = vec![("comp".into(), "reg/comp:1".into())];
    rig.canned.insert("reg/comp:1".into(), present(&kind));
}

#[given("a bundle whose sandbox base image is built for a foreign architecture")]
async fn foreign_arch(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    rig.components = vec![("sandbox".into(), "reg/base:1".into())];
    let mut sandbox = sandbox_component();
    sandbox.arch = Some("other-arch".into());
    rig.canned
        .insert("reg/base:1".into(), Canned::Present(sandbox));
}

#[given("a bundle whose component graph contains a reference cycle")]
async fn reference_cycle(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    rig.components = vec![("a".into(), "reg/a:1".into())];
    let mut sandbox = component("Sandbox");
    sandbox.references = vec!["reg/b:1".into()];
    let mut fileset = component("FileSet");
    fileset.references = vec!["reg/a:1".into()];
    rig.canned
        .insert("reg/a:1".into(), Canned::Present(sandbox));
    rig.canned
        .insert("reg/b:1".into(), Canned::Present(fileset));
}

#[given(regex = r#"^a bundle declaring two components both named "([^"]+)"$"#)]
async fn duplicate_name(world: &mut BehaviourWorld, name: String) {
    world.resolve().components = vec![(name.clone(), "reg/x:1".into()), (name, "reg/y:1".into())];
}

#[given("a bundle referencing another bundle as a component")]
async fn nested_bundle(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    rig.components = vec![("nested".into(), "reg/inner:1".into())];
    rig.canned
        .insert("reg/inner:1".into(), present("AgentSystem"));
}

#[given(regex = r#"^a bundle whose fileset lives on "([^"]+)" with no stored credential$"#)]
async fn needs_login(world: &mut BehaviourWorld, host: String) {
    let rig = world.resolve();
    let reference = format!("{host}/skills:1");
    rig.components = vec![("skills".into(), reference.clone())];
    rig.canned.insert(reference, Canned::NeedsLogin { host });
}

#[when("the bundle is resolved")]
async fn do_resolve(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    let spec = BundleSpec {
        components: rig
            .components
            .iter()
            .map(|(name, reference)| DeclaredComponent {
                name: name.clone(),
                reference: reference.clone(),
            })
            .collect(),
    };
    let fetcher = FakeFetcher::new(&rig.canned);
    let result = resolve(&spec, &fetcher, HOST_ARCH).await;
    let calls = fetcher.calls.lock().unwrap().clone();
    drop(fetcher);
    rig.fetched = calls;
    match result {
        Ok(bundle) => {
            rig.ok = true;
            rig.resolved = Some(bundle);
        }
        Err(e) => rig.error = Some(format!("{e}")),
    }
}

#[then("resolution succeeds")]
async fn succeeds(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    assert!(rig.ok, "expected success, got error: {:?}", rig.error);
}

#[then("every declared component was fetched")]
async fn every_component_fetched(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    for (_, reference) in &rig.components {
        assert!(
            rig.fetched.contains(reference),
            "component {reference} was never fetched; fetched: {:?}",
            rig.fetched,
        );
    }
}

#[then("the shared fileset was fetched exactly once")]
async fn shared_fetched_once(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    let count = rig.fetched.iter().filter(|r| *r == "reg/shared:1").count();
    assert_eq!(
        count, 1,
        "shared fileset should be fetched exactly once; fetched: {:?}",
        rig.fetched,
    );
}

#[then(regex = r#"^the resolved bundle's base image is "([^"]+)"$"#)]
async fn resolved_base_image(world: &mut BehaviourWorld, image: String) {
    let rig = world.resolve();
    let bundle = rig.resolved.as_ref().expect("a resolved bundle");
    assert_eq!(bundle.base_image, image);
}

#[then(regex = r#"^the resolved bundle runs command "([^"]+)"$"#)]
async fn resolved_command(world: &mut BehaviourWorld, command: String) {
    let rig = world.resolve();
    let bundle = rig.resolved.as_ref().expect("a resolved bundle");
    assert_eq!(bundle.command.as_deref(), Some(command.as_str()));
}

#[then(regex = r#"^the resolved bundle includes fileset "([^"]+)"$"#)]
async fn resolved_includes_fileset(world: &mut BehaviourWorld, name: String) {
    let rig = world.resolve();
    let bundle = rig.resolved.as_ref().expect("a resolved bundle");
    assert!(
        bundle.filesets.iter().any(|f| f.name == name),
        "resolved bundle should include fileset {name}",
    );
}

#[then("resolution is refused")]
async fn refused(world: &mut BehaviourWorld) {
    assert!(
        world.resolve().error.is_some(),
        "expected a refusal, but resolution succeeded",
    );
}

fn assert_refusal_contains(world: &mut BehaviourWorld, needle: &str) {
    let err = world
        .resolve()
        .error
        .clone()
        .expect("expected a refusal error");
    assert!(
        err.contains(needle),
        "refusal should mention {needle}, got: {err}"
    );
}

#[then(regex = r#"^the refusal mentions "([^"]+)"$"#)]
async fn refusal_mentions(world: &mut BehaviourWorld, text: String) {
    assert_refusal_contains(world, &text);
}

#[then(regex = r#"^the refusal names the missing component "([^"]+)"$"#)]
async fn names_missing(world: &mut BehaviourWorld, name: String) {
    assert_refusal_contains(world, &name);
}

#[then(regex = r#"^the refusal names the unsupported kind "([^"]+)"$"#)]
async fn names_unsupported_kind(world: &mut BehaviourWorld, kind: String) {
    assert_refusal_contains(world, &kind);
}

#[then("the refusal reports both the image and host architectures")]
async fn reports_both_arches(world: &mut BehaviourWorld) {
    assert_refusal_contains(world, "other-arch");
    assert_refusal_contains(world, "test-arch");
}

#[then("the refusal reports the reference cycle")]
async fn reports_cycle(world: &mut BehaviourWorld) {
    assert_refusal_contains(world, "cycle");
}

#[then(regex = r#"^the refusal names the duplicated component "([^"]+)"$"#)]
async fn names_duplicate(world: &mut BehaviourWorld, name: String) {
    assert_refusal_contains(world, &name);
}

#[then("the refusal reports that nested bundles are not allowed")]
async fn reports_nested(world: &mut BehaviourWorld) {
    assert_refusal_contains(world, "nested bundle");
}

#[then(regex = r#"^the refusal names the registry host "([^"]+)" that needs a login$"#)]
async fn names_host(world: &mut BehaviourWorld, host: String) {
    assert_refusal_contains(world, &host);
    assert_refusal_contains(world, "login");
}
