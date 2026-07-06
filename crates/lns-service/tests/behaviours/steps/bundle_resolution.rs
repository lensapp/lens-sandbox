use crate::resolve_rig::{Canned, FakeFetcher};
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::artifact::resolve::{BundleSpec, DeclaredComponent, resolve};

const HOST_ARCH: &str = "test-arch";

fn present(kind: &str) -> Canned {
    Canned::Present {
        kind: kind.to_string(),
        arch: None,
        refs: Vec::new(),
    }
}

#[given("a bundle whose sandbox, agent, and fileset are all present and supported")]
async fn well_formed(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    rig.components = vec![
        ("sandbox".into(), "reg/base:1".into()),
        ("agent".into(), "reg/agent:1".into()),
        ("skills".into(), "reg/skills:1".into()),
    ];
    rig.canned.insert(
        "reg/base:1".into(),
        Canned::Present {
            kind: "Sandbox".into(),
            arch: Some(HOST_ARCH.into()),
            refs: Vec::new(),
        },
    );
    rig.canned.insert("reg/agent:1".into(), present("Agent"));
    rig.canned.insert("reg/skills:1".into(), present("FileSet"));
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
    rig.canned.insert(
        "reg/base:1".into(),
        Canned::Present {
            kind: "Sandbox".into(),
            arch: Some("other-arch".into()),
            refs: Vec::new(),
        },
    );
}

#[given("a bundle whose component graph contains a reference cycle")]
async fn reference_cycle(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    rig.components = vec![("a".into(), "reg/a:1".into())];
    rig.canned.insert(
        "reg/a:1".into(),
        Canned::Present {
            kind: "Sandbox".into(),
            arch: None,
            refs: vec!["reg/b:1".into()],
        },
    );
    rig.canned.insert(
        "reg/b:1".into(),
        Canned::Present {
            kind: "FileSet".into(),
            arch: None,
            refs: vec!["reg/a:1".into()],
        },
    );
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
    let result = {
        let fetcher = FakeFetcher(&rig.canned);
        resolve(&spec, &fetcher, HOST_ARCH)
    };
    match result {
        Ok(()) => rig.ok = true,
        Err(e) => rig.error = Some(format!("{e:#}")),
    }
}

#[then("resolution succeeds")]
async fn succeeds(world: &mut BehaviourWorld) {
    let rig = world.resolve();
    assert!(rig.ok, "expected success, got error: {:?}", rig.error);
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

#[then(regex = r#"^the refusal names the missing component "([^"]+)"$"#)]
async fn names_missing(world: &mut BehaviourWorld, name: String) {
    assert_refusal_contains(world, &name);
}

#[then(regex = r#"^the refusal names the unsupported kind "([^"]+)"$"#)]
async fn names_unsupported_kind(world: &mut BehaviourWorld, kind: String) {
    assert_refusal_contains(world, &kind);
}

#[then("the refusal reports the architecture mismatch")]
async fn reports_arch_mismatch(world: &mut BehaviourWorld) {
    assert_refusal_contains(world, "architecture");
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
