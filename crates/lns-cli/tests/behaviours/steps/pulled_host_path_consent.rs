use std::sync::Mutex;

use cucumber::{given, then, when};
use lns_cli::run::host_path_consent::{DocumentOrigin, decide_host_paths};
use lns_cli::run::summary::FilesetSummary;
use lns_policy::decision_store::DecisionStore;
use lns_policy::host_path_decisions::{HostPathDecision, HostPathDecisionFile, decision_key};

use crate::world::{BehaviourWorld, ScriptedTerminal};

struct FakeStore {
    state: Mutex<HostPathDecisionFile>,
    saves: Mutex<usize>,
    load_fails: bool,
}

impl DecisionStore<HostPathDecision> for FakeStore {
    fn load(&self) -> std::io::Result<HostPathDecisionFile> {
        if self.load_fails {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "decisions file is not json",
            ));
        }
        Ok(self.state.lock().unwrap().clone())
    }
    fn save(&self, state: &HostPathDecisionFile) -> std::io::Result<()> {
        *self.state.lock().unwrap() = state.clone();
        *self.saves.lock().unwrap() += 1;
        Ok(())
    }
}

fn fileset(host_path: &str, optional: bool) -> FilesetSummary {
    let suffix = if optional { " (optional)" } else { "" };
    FilesetSummary {
        source: format!("host file {host_path}{suffix}"),
        guest_path: format!("/home/agent/{}", host_path.trim_start_matches("~/")),
        owner: "workload".into(),
        host_path: Some(host_path.to_string()),
        optional,
        declared_by: None,
    }
}

#[given("no host path decision is recorded")]
fn no_decision_recorded(world: &mut BehaviourWorld) {
    world.host_paths = Default::default();
}

#[given(regex = r#"^a pulled sandbox "([^"]+)" reads host file "([^"]+)"$"#)]
fn pulled_reads_host_file(world: &mut BehaviourWorld, reference: String, host_path: String) {
    world.host_paths.reference = Some(reference);
    world.host_paths.filesets = vec![fileset(&host_path, false)];
}

#[given(regex = r#"^a pulled sandbox "([^"]+)" reads optional host file "([^"]+)"$"#)]
fn pulled_reads_optional_host_file(
    world: &mut BehaviourWorld,
    reference: String,
    host_path: String,
) {
    world.host_paths.reference = Some(reference);
    world.host_paths.filesets = vec![fileset(&host_path, true)];
}

/// A packed fileset travels inside the artifact the developer already approved, so it raises no question about this machine's files.
#[given(regex = r#"^a pulled sandbox "([^"]+)" declares no host file$"#)]
fn pulled_declares_no_host_file(world: &mut BehaviourWorld, reference: String) {
    world.host_paths.reference = Some(reference);
    world.host_paths.filesets = vec![FilesetSummary {
        source: "./skills".into(),
        guest_path: "/home/agent/.agent/skills".into(),
        owner: "root".into(),
        host_path: None,
        optional: false,
        declared_by: None,
    }];
}

#[given(
    regex = r#"^a pulled sandbox "([^"]+)" reads optional host file "([^"]+)" and required host file "([^"]+)"$"#
)]
fn pulled_reads_two_host_files(
    world: &mut BehaviourWorld,
    reference: String,
    optional_path: String,
    required_path: String,
) {
    world.host_paths.reference = Some(reference);
    world.host_paths.filesets = vec![
        fileset(&optional_path, true),
        fileset(&required_path, false),
    ];
}

#[given(regex = r#"^the user will answer "([^"]+)" then "([^"]+)" to the host file prompts$"#)]
fn will_answer_twice(world: &mut BehaviourWorld, first: String, second: String) {
    world.host_paths.answer = Some(format!("{first}\n{second}\n"));
}

#[given(regex = r#"^a local sandbox whose mixin "([^"]+)" reads host file "([^"]+)"$"#)]
fn local_mixin_reads_host_file(world: &mut BehaviourWorld, mixin: String, host_path: String) {
    world.host_paths.reference = None;
    let mut entry = fileset(&host_path, false);
    entry.declared_by = Some(mixin);
    world.host_paths.filesets = vec![entry];
}

#[given(regex = r#"^a local sandbox reads host file "([^"]+)"$"#)]
fn local_reads_host_file(world: &mut BehaviourWorld, host_path: String) {
    world.host_paths.reference = None;
    world.host_paths.filesets = vec![fileset(&host_path, false)];
}

#[given(regex = r#"^the user will answer "([^"]+)" to the host file prompt$"#)]
fn will_answer(world: &mut BehaviourWorld, answer: String) {
    world.host_paths.answer = Some(format!("{answer}\n"));
}

#[given(regex = r#"^the user (allowed|denied) "([^"]+)" for "([^"]+)"$"#)]
fn already_decided(
    world: &mut BehaviourWorld,
    verb: String,
    host_path: String,
    repository: String,
) {
    let decision = if verb == "allowed" {
        HostPathDecision::Allow
    } else {
        HostPathDecision::Deny
    };
    world
        .host_paths
        .recorded
        .insert(decision_key(&repository, &host_path), decision);
}

#[given("this machine's recorded answers cannot be read")]
fn recorded_answers_unreadable(world: &mut BehaviourWorld) {
    world.host_paths.load_fails = true;
}

#[given("host file input is non-interactive")]
fn non_interactive(world: &mut BehaviourWorld) {
    world.host_paths.interactive = false;
}

#[given("the user accepts every effect in advance")]
fn assume_yes(world: &mut BehaviourWorld) {
    world.host_paths.assume_yes = true;
}

fn decide(world: &mut BehaviourWorld, origin: DocumentOrigin) {
    let store = FakeStore {
        state: Mutex::new(world.host_paths.recorded.clone()),
        saves: Mutex::new(0),
        load_fails: world.host_paths.load_fails,
    };
    let answers = world.host_paths.answer.clone().unwrap_or_default();
    let answers: Vec<&str> = answers.lines().collect();
    let mut terminal = if world.host_paths.interactive {
        ScriptedTerminal::answering(&answers)
    } else {
        ScriptedTerminal::absent()
    };
    let mut output = Vec::<u8>::new();
    let outcome = decide_host_paths(
        &origin,
        &world.host_paths.filesets,
        &store,
        world.host_paths.assume_yes,
        &mut terminal,
        &mut output,
    );
    world.host_paths.denied = outcome
        .as_ref()
        .map(|grant| grant.denied.clone())
        .unwrap_or_default();
    world.host_paths.refusal = outcome.err().map(|e| format!("{e:#}"));
    world.host_paths.prompt = String::from_utf8(output).expect("non-utf8 prompt");
    world.host_paths.saves = *store.saves.lock().unwrap();
    world.host_paths.persisted = store.state.lock().unwrap().clone();
}

#[when("the pulled sandbox host files are decided")]
fn pulled_decided(world: &mut BehaviourWorld) {
    let reference = world
        .host_paths
        .reference
        .clone()
        .expect("a pulled scenario must stage a reference");
    decide(world, DocumentOrigin::Pulled { reference });
}

#[when("the local sandbox host files are decided")]
fn local_decided(world: &mut BehaviourWorld) {
    decide(world, DocumentOrigin::OwnDirectory);
}

#[then(regex = r#"^the prompt names "([^"]+)"$"#)]
fn prompt_names(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    if world.host_paths.prompt.contains(&expected) {
        Ok(())
    } else {
        Err(format!(
            "prompt did not name {expected:?}: {:?}",
            world.host_paths.prompt
        ))
    }
}

#[then(regex = r#"^the recorded answer for "([^"]+)" is "(allow|deny)"$"#)]
fn recorded_answer_is(
    world: &mut BehaviourWorld,
    host_path: String,
    expected: String,
) -> Result<(), String> {
    let reference = world
        .host_paths
        .filesets
        .iter()
        .find(|f| f.host_path.as_deref() == Some(host_path.as_str()))
        .and_then(|f| f.declared_by.as_deref())
        .or(world.host_paths.reference.as_deref())
        .ok_or("no deciding artifact staged")?;
    let wanted = if expected == "allow" {
        HostPathDecision::Allow
    } else {
        HostPathDecision::Deny
    };
    match world
        .host_paths
        .persisted
        .get(&decision_key(reference, &host_path))
    {
        Some(found) if *found == wanted => Ok(()),
        Some(found) => Err(format!("recorded {found:?}, wanted {wanted:?}")),
        None => Err(format!(
            "nothing recorded for {host_path:?}: {:?}",
            world.host_paths.persisted
        )),
    }
}

#[then("the developer is not asked")]
fn not_asked(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.host_paths.prompt.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "developer was asked: {:?}",
            world.host_paths.prompt
        ))
    }
}

#[then("the run reads the host file")]
fn run_reads(world: &mut BehaviourWorld) -> Result<(), String> {
    if let Some(refusal) = &world.host_paths.refusal {
        return Err(format!("run was refused: {refusal}"));
    }
    if world.host_paths.denied.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "host file was denied: {:?}",
            world.host_paths.denied
        ))
    }
}

#[then("the run does not read the host file")]
fn run_does_not_read(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.host_paths.denied.is_empty() {
        Err("host file was not denied".to_string())
    } else {
        Ok(())
    }
}

#[then("the run continues")]
fn run_continues(world: &mut BehaviourWorld) -> Result<(), String> {
    match &world.host_paths.refusal {
        Some(refusal) => Err(format!("run was refused: {refusal}")),
        None => Ok(()),
    }
}

#[then(regex = r#"^the run is refused naming "([^"]+)"$"#)]
fn run_refused(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    match &world.host_paths.refusal {
        Some(refusal) if refusal.contains(&expected) => Ok(()),
        Some(refusal) => Err(format!("refusal did not name {expected:?}: {refusal}")),
        None => Err("run was not refused".to_string()),
    }
}

#[then("nothing is recorded")]
fn nothing_recorded(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.host_paths.saves == 0 {
        Ok(())
    } else {
        Err(format!(
            "{} decision writes reached the store",
            world.host_paths.saves
        ))
    }
}
