use cucumber::{given, then, when};
use lns_service::artifact::assembly::assemble;
use lns_service::artifact::resolved_from_sandbox;
use lns_service::artifact::scripts::{absorb, staged_manifest};
use lns_service::runtime_layer::RuntimeFileSpec;

use crate::world::BehaviourWorld;

/// The declaration the scenario is building, before it becomes a document.
#[derive(Debug, Default)]
pub struct ScriptDeclaration {
    pub own: Vec<String>,
    pub mixin: Vec<String>,
}

fn entry(body: &str, user: Option<&str>, description: Option<&str>) -> String {
    let quoted = |value: &str| serde_json::to_string(value).expect("a str encodes");
    let mut fields = vec![
        r#""when":"pre-start""#.to_string(),
        format!(r#""run":{}"#, quoted(body)),
    ];
    if let Some(user) = user {
        fields.push(format!(r#""user":{}"#, quoted(user)));
    }
    if let Some(description) = description {
        fields.push(format!(r#""description":{}"#, quoted(description)));
    }
    format!("{{{}}}", fields.join(","))
}

/// A Gherkin step writes `\n` as two characters, but a script body is where a real newline matters.
fn body(raw: &str) -> String {
    raw.replace("\\n", "\n")
}

fn declaration(world: &mut BehaviourWorld) -> &mut ScriptDeclaration {
    world
        .script_declaration
        .get_or_insert_with(Default::default)
}

#[given(
    regex = r#"^a local definition declaring the pre-start scripts? "([^"]*)"(?: and "([^"]*)")?$"#
)]
fn declaring_scripts(world: &mut BehaviourWorld, first: String, second: String) {
    let declared = declaration(world);
    declared.own.push(entry(&body(&first), None, None));
    if !second.is_empty() {
        declared.own.push(entry(&body(&second), None, None));
    }
}

#[given(regex = r#"^a local definition declaring a pre-start script "([^"]*)" as "([^"]*)"$"#)]
fn declaring_script_as_user(world: &mut BehaviourWorld, script: String, user: String) {
    declaration(world)
        .own
        .push(entry(&body(&script), Some(&user), None));
}

#[given(
    regex = r#"^a local definition declaring a pre-start script "([^"]*)" described as "([^"]*)"$"#
)]
fn declaring_described_script(world: &mut BehaviourWorld, script: String, description: String) {
    declaration(world)
        .own
        .push(entry(&body(&script), None, Some(&description)));
}

#[given("a local definition declaring no pre-start scripts")]
fn declaring_no_scripts(world: &mut BehaviourWorld) {
    declaration(world);
}

#[given(regex = r#"^it layers on a mixin declaring the pre-start script "([^"]*)"$"#)]
fn mixin_declaring_script(world: &mut BehaviourWorld, script: String) {
    declaration(world)
        .mixin
        .push(entry(&body(&script), None, None));
}

#[when("the run is planned")]
fn plan_the_definition(world: &mut BehaviourWorld) {
    let declared = world
        .script_declaration
        .take()
        .expect("the scenario must declare a definition");
    let sandbox: lns_artifact::sandbox::SandboxSpec = serde_json::from_str(&format!(
        r#"{{"image":"registry.example.test/runtime:1","scripts":[{}]}}"#,
        declared.own.join(",")
    ))
    .expect("valid sandbox fixture");
    let mixin: lns_artifact::sandbox::SandboxSpec =
        serde_json::from_str(&format!(r#"{{"scripts":[{}]}}"#, declared.mixin.join(",")))
            .expect("valid mixin fixture");
    let sources = [
        (lns_artifact::merge::ROOT_LABEL, &sandbox),
        ("ghcr.io/acme/postgres-tools", &mixin),
    ]
    .map(|(label, spec)| lns_artifact::merge::Source { label, spec });
    let merged = lns_artifact::merge::merge(&sources).expect("these sources resolve");
    let def = lns_artifact::sandbox::Definition {
        name: "some-sandbox".to_string(),
        spec: merged.spec,
    };
    let resolved = resolved_from_sandbox(&def, &Default::default());
    let mut materialized = Default::default();
    absorb(&assemble(&resolved).scripts, &mut materialized).expect("staging succeeds");
    world.script_specs = Some(materialized.into_specs());
}

fn specs(world: &BehaviourWorld) -> &[RuntimeFileSpec] {
    world
        .script_specs
        .as_deref()
        .expect("the definition must be planned first")
}

fn manifest(world: &BehaviourWorld) -> lns_session::ScriptManifest {
    staged_manifest(specs(world)).expect("a run declaring scripts stages a manifest")
}

fn staged_body(world: &BehaviourWorld, path: &str) -> String {
    specs(world)
        .iter()
        .find(|spec| spec.guest_path == path)
        .and_then(|spec| spec.source.as_bytes())
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_else(|| panic!("no script staged at {path}"))
}

fn step_for(world: &BehaviourWorld, script: &str) -> lns_session::ScriptManifestStep {
    let wanted = body(script);
    manifest(world)
        .steps
        .into_iter()
        .find(|step| staged_body(world, &step.script) == wanted)
        .unwrap_or_else(|| panic!("no staged script whose body is {wanted:?}"))
}

#[then(regex = r"^the run stages (\d+) scripts?$")]
fn stages_n_scripts(world: &mut BehaviourWorld, count: usize) {
    assert_eq!(
        manifest(world).steps.len(),
        count,
        "the manifest is what tells the guest how many scripts to run, so it has to name every one the merge kept"
    );
}

#[then(regex = r#"^the staged scripts run in the order "([^"]*)", "([^"]*)"$"#)]
fn scripts_run_in_order(world: &mut BehaviourWorld, first: String, second: String) {
    let ordered: Vec<String> = manifest(world)
        .steps
        .iter()
        .map(|step| staged_body(world, &step.script))
        .collect();
    assert_eq!(
        ordered,
        [body(&first), body(&second)],
        "the manifest's order is the merge's order, and a mixin's script appending after the sandbox's own is the whole ordering decision this staging carries"
    );
}

#[then(regex = r#"^the staged script for "([^"]*)" names the user "([^"]*)"$"#)]
fn script_names_user(world: &mut BehaviourWorld, script: String, user: String) {
    assert_eq!(
        step_for(world, &script).user.as_deref(),
        Some(user.as_str()),
        "the user travels beside the body, because only the guest can resolve it against its own passwd"
    );
}

#[then(regex = r#"^the staged script for "([^"]*)" names no user$"#)]
fn script_names_no_user(world: &mut BehaviourWorld, script: String) {
    assert!(
        step_for(world, &script).user.is_none(),
        "an absent user must stay absent through staging, or the guest cannot tell 'defer to the run's identity' from a named one"
    );
}

#[then(regex = r#"^the staged script for "([^"]*)" is labelled "([^"]*)"$"#)]
fn script_is_labelled(world: &mut BehaviourWorld, script: String, label: String) {
    assert_eq!(
        step_for(world, &script).label,
        label,
        "a failure names the script by this label, and a script has no name of its own to use instead"
    );
}

#[then(regex = r#"^every staged script sits beneath "([^"]*)" and is not writable$"#)]
fn scripts_are_staged_readonly(world: &mut BehaviourWorld, dir: String) {
    for spec in specs(world) {
        assert!(
            spec.guest_path.starts_with(&format!("{dir}/")),
            "the guest reads scripts from one place, so a spec outside {dir} is one it will never run: {}",
            spec.guest_path
        );
        assert_eq!(
            spec.mode & 0o222,
            0,
            "a workload that could rewrite a staged script could change what the next run of the same sandbox executes: {}",
            spec.guest_path
        );
    }
}

#[then("the run stages no script manifest")]
fn stages_no_manifest(world: &mut BehaviourWorld) {
    assert!(
        specs(world).is_empty(),
        "a run declaring nothing must stage nothing, so an absent manifest stays the guest's signal that there is no work"
    );
}
