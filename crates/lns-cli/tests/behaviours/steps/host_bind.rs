use crate::world::{BehaviourWorld, HostBindOutcome, ResolvedRunView};
use cucumber::{given, then, when};
use lns_cli::cli::{RunArgs, split_mounts};
use lns_cli::command::parse_args;
use lns_cli::run::host_bind::{DirScan, ResolvedBind, resolve_binds};
use lns_cli::run::summary::{PolicySource, format_bind_dispositions, format_summary};
use lns_policy::Policy;
use lns_policy::host_bind_decisions::{
    HostBindDecisionFile, HostBindDecisionStore, SecretDisposition,
};
use std::path::Path;
use std::sync::Mutex;

#[when(regex = r"^the mounts are resolved for `lns run ([^`]+)`$")]
fn resolve_mounts(world: &mut BehaviourWorld, flags: String) {
    let mut argv = vec!["lns".to_string(), "run".to_string()];
    argv.extend(flags.split_whitespace().map(str::to_string));
    let args: RunArgs = parse_args(&argv).expect("argv must parse against the CLI grammar");
    let (volumes, binds) = split_mounts(&args.mounts);
    world.resolved_run = Some(ResolvedRunView {
        volumes: volumes
            .iter()
            .map(|v| {
                let ro = if v.read_only { ":ro" } else { "" };
                format!("{}:{}{ro}", v.name, v.target)
            })
            .collect(),
        binds: binds
            .iter()
            .map(|b| {
                let ro = if b.read_only { ":ro" } else { "" };
                format!("{} -> {}{ro}", b.host_source, b.target)
            })
            .collect(),
        ..Default::default()
    });
}

#[then(regex = r#"^the resolved volumes are exactly "([^"]*)"$"#)]
fn resolved_volumes_are(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    let view = world
        .resolved_run
        .as_ref()
        .ok_or_else(|| "no resolved run captured".to_string())?;
    let rendered = view.volumes.join(", ");
    if rendered == expected {
        Ok(())
    } else {
        Err(format!("expected volumes {expected:?}, got {rendered:?}"))
    }
}

#[then(regex = r#"^the resolved host binds are exactly "([^"]*)"$"#)]
fn resolved_binds_are(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    let view = world
        .resolved_run
        .as_ref()
        .ok_or_else(|| "no resolved run captured".to_string())?;
    let rendered = view.binds.join(", ");
    if rendered == expected {
        Ok(())
    } else {
        Err(format!(
            "expected host binds {expected:?}, got {rendered:?}"
        ))
    }
}

struct FakeDir {
    entries: Vec<String>,
    lensignore: Option<String>,
    missing: bool,
    not_a_dir: bool,
}
impl DirScan for FakeDir {
    fn exists(&self, _path: &Path) -> bool {
        !self.missing
    }
    fn is_dir(&self, _path: &Path) -> bool {
        !self.missing && !self.not_a_dir
    }
    fn entries(&self, _dir: &Path) -> Vec<String> {
        self.entries.clone()
    }
    fn read_to_string(&self, path: &Path) -> Option<String> {
        if path.ends_with(".lensignore") {
            self.lensignore.clone()
        } else {
            None
        }
    }
}

struct FakeStore {
    state: Mutex<HostBindDecisionFile>,
}
impl HostBindDecisionStore for FakeStore {
    fn load(&self) -> std::io::Result<HostBindDecisionFile> {
        Ok(self.state.lock().unwrap().clone())
    }
    fn save(&self, state: &HostBindDecisionFile) -> std::io::Result<()> {
        *self.state.lock().unwrap() = state.clone();
        Ok(())
    }
}

fn parsed_bind_specs(flags: &str) -> Vec<lns_ipc::BindSpec> {
    let mut argv = vec!["lns".to_string(), "run".to_string()];
    argv.extend(flags.split_whitespace().map(str::to_string));
    let args: RunArgs = parse_args(&argv).expect("argv must parse");
    split_mounts(&args.mounts).1
}

fn dir_from(world: &BehaviourWorld) -> FakeDir {
    FakeDir {
        entries: world.host_bind.entries.clone(),
        lensignore: world.host_bind.lensignore.clone(),
        missing: world.host_bind.missing,
        not_a_dir: world.host_bind.not_a_dir,
    }
}

fn run_resolve(world: &mut BehaviourWorld, flags: &str, interactive: bool) {
    let specs = parsed_bind_specs(flags);
    let dir = dir_from(world);
    let store = FakeStore {
        state: Mutex::new(world.host_bind.decisions.clone()),
    };
    let mut input = std::io::Cursor::new(world.host_bind.answer.clone().unwrap_or_default());
    let mut out = Vec::new();
    let result = resolve_binds(&specs, &dir, &store, interactive, &mut input, &mut out)
        .map_err(|e| e.to_string());
    let summary = match &result {
        Ok(resolved) => {
            let mut argv = vec!["lns".to_string(), "run".to_string()];
            argv.extend(flags.split_whitespace().map(str::to_string));
            let args: RunArgs = parse_args(&argv).expect("argv must parse");
            let mut text = format_summary(
                &args,
                lns_cli::run::summary::resolved_size(Default::default(), &args),
                &Policy::default(),
                Path::new("./lns-policy.yaml"),
                &PolicySource::FoundInCwd,
            );
            text.push_str(&format_bind_dispositions(resolved));
            text
        }
        Err(_) => String::new(),
    };
    world.host_bind.outcome = Some(HostBindOutcome {
        result,
        prompt: String::from_utf8(out).unwrap(),
        persisted: store.state.into_inner().unwrap(),
        summary,
    });
}

fn outcome(world: &BehaviourWorld) -> Result<&HostBindOutcome, String> {
    world
        .host_bind
        .outcome
        .as_ref()
        .ok_or_else(|| "no host-bind outcome captured".to_string())
}

fn resolved_binds(world: &BehaviourWorld) -> Result<&[ResolvedBind], String> {
    outcome(world)?
        .result
        .as_deref()
        .map_err(|e| format!("resolution failed: {e}"))
}

#[given(regex = r#"^the host path "([^"]+)" does not exist$"#)]
fn host_path_missing(world: &mut BehaviourWorld, _path: String) {
    world.host_bind.missing = true;
}

#[given(regex = r#"^the host path "([^"]+)" is a file, not a directory$"#)]
fn host_path_is_a_file(world: &mut BehaviourWorld, _path: String) {
    world.host_bind.not_a_dir = true;
}

#[given(regex = r#"^the host directory "([^"]+)" contains no secret-shaped files$"#)]
fn dir_is_clean(world: &mut BehaviourWorld, _dir: String) {
    world.host_bind.entries = vec!["src".into(), "Cargo.toml".into()];
}

#[given(regex = r#"^the host directory "([^"]+)" contains "([^"]+)"$"#)]
fn dir_contains(world: &mut BehaviourWorld, _dir: String, name: String) {
    world.host_bind.entries.push(name);
}

#[given(regex = r#"^the host directory "([^"]+)" contains "([^"]+)" and "([^"]+)"$"#)]
fn dir_contains_two(world: &mut BehaviourWorld, _dir: String, a: String, b: String) {
    world.host_bind.entries.push(a);
    world.host_bind.entries.push(b);
}

#[given(regex = r#"^the host directory "([^"]+)" contains an undecided "([^"]+)"$"#)]
fn dir_contains_undecided(world: &mut BehaviourWorld, _dir: String, name: String) {
    world.host_bind.entries.push(name);
}

#[given(regex = r#"^no prior decision is recorded for "([^"]+)"$"#)]
fn no_prior_decision(_world: &mut BehaviourWorld, _path: String) {}

#[given(regex = r#"^a per-machine KEEP decision is recorded for "([^"]+)"$"#)]
fn keep_decision_recorded(world: &mut BehaviourWorld, path: String) {
    world
        .host_bind
        .decisions
        .insert(path, SecretDisposition::Keep);
}

#[given(regex = r#"^the host directory "([^"]+)" has a "\.lensignore" listing "([^"]+)"$"#)]
fn lensignore_lists(world: &mut BehaviourWorld, _dir: String, name: String) {
    world.host_bind.lensignore = Some(format!("{name}\n"));
}

#[given(regex = r#"^the operator will answer the secret prompt with "([^"]+)"$"#)]
fn operator_answers(world: &mut BehaviourWorld, answer: String) {
    world.host_bind.answer = Some(format!("{answer}\n"));
}

#[when(regex = r"^the user runs `lns run ([^`]+)` interactively$")]
fn run_interactively(world: &mut BehaviourWorld, flags: String) {
    run_resolve(world, &flags, true);
}

#[when(regex = r"^the user runs `lns run ([^`]+)` with no terminal$")]
fn run_non_interactively(world: &mut BehaviourWorld, flags: String) {
    run_resolve(world, &flags, false);
}

#[then(regex = r#"^the operator is prompted to KEEP or DROP "([^"]+)"$"#)]
fn prompted_for(world: &mut BehaviourWorld, path: String) -> Result<(), String> {
    let prompt = &outcome(world)?.prompt;
    if prompt.contains(&path) {
        Ok(())
    } else {
        Err(format!("expected a prompt for {path:?}, got {prompt:?}"))
    }
}

#[then(regex = r#"^the operator is prompted only for "([^"]+)"$"#)]
fn prompted_only_for(world: &mut BehaviourWorld, path: String) -> Result<(), String> {
    let prompt = &outcome(world)?.prompt;
    let count = prompt.matches("looks like a secret").count();
    if prompt.contains(&path) && count == 1 {
        Ok(())
    } else {
        Err(format!(
            "expected exactly one prompt for {path:?}, got {prompt:?}"
        ))
    }
}

#[then(regex = r"^no KEEP or DROP prompt is shown$")]
fn no_prompt(world: &mut BehaviourWorld) -> Result<(), String> {
    let prompt = &outcome(world)?.prompt;
    if prompt.contains("looks like a secret") {
        Err(format!("expected no prompt, got {prompt:?}"))
    } else {
        Ok(())
    }
}

#[then(regex = r"^the run starts$")]
fn run_starts(world: &mut BehaviourWorld) -> Result<(), String> {
    resolved_binds(world).map(|_| ())
}

#[then(regex = r#"^"([^"]+)" is exposed to the guest under "([^"]+)"$"#)]
fn exposed(world: &mut BehaviourWorld, name: String, _target: String) -> Result<(), String> {
    let b = resolved_binds(world)?;
    if b.iter().any(|m| m.dropped.contains(&name)) {
        Err(format!("{name:?} was dropped, expected it kept"))
    } else {
        Ok(())
    }
}

#[then(regex = r#"^"([^"]+)" is dropped from the bind$"#)]
fn dropped(world: &mut BehaviourWorld, name: String) -> Result<(), String> {
    let b = resolved_binds(world)?;
    if b.iter().any(|m| m.dropped.contains(&name)) {
        Ok(())
    } else {
        Err(format!("expected {name:?} dropped, binds: {b:?}"))
    }
}

#[then(regex = r#"^a per-machine (KEEP|DROP) decision is recorded for "([^"]+)"$"#)]
fn decision_recorded(world: &mut BehaviourWorld, kind: String, path: String) -> Result<(), String> {
    let want = if kind == "KEEP" {
        SecretDisposition::Keep
    } else {
        SecretDisposition::Drop
    };
    match outcome(world)?.persisted.get(&path).copied() {
        Some(d) if d == want => Ok(()),
        other => Err(format!("expected {want:?} for {path:?}, got {other:?}")),
    }
}

#[then(regex = r"^a later run with the same bind shows no prompt$")]
fn later_run_no_prompt(world: &mut BehaviourWorld) -> Result<(), String> {
    let persisted = outcome(world)?.persisted.clone();
    let specs: Vec<lns_ipc::BindSpec> = resolved_binds(world)?
        .iter()
        .map(|m| lns_ipc::BindSpec {
            host_source: m.host_source.clone(),
            target: m.target.clone(),
            read_only: m.read_only,
            exclude: Vec::new(),
            optional: false,
        })
        .collect();
    let dir = dir_from(world);
    let store = FakeStore {
        state: Mutex::new(persisted),
    };
    let mut input = std::io::Cursor::new(String::new());
    let mut out = Vec::new();
    resolve_binds(&specs, &dir, &store, true, &mut input, &mut out).map_err(|e| e.to_string())?;
    let prompt = String::from_utf8(out).unwrap();
    if prompt.contains("looks like a secret") {
        Err(format!("a later run re-prompted: {prompt:?}"))
    } else {
        Ok(())
    }
}

#[then(regex = r#"^the command fails with "([^"]+)"$"#)]
fn command_fails_with(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    match &outcome(world)?.result {
        Err(e) if e.contains(&needle) => Ok(()),
        other => Err(format!(
            "expected failure containing {needle:?}, got {other:?}"
        )),
    }
}

#[then(regex = r#"^the dropped path "([^"]+)" is reported on stderr$"#)]
fn dropped_reported(world: &mut BehaviourWorld, path: String) -> Result<(), String> {
    let prompt = &outcome(world)?.prompt;
    if prompt.contains(&path) {
        Ok(())
    } else {
        Err(format!("expected {path:?} reported, got {prompt:?}"))
    }
}

fn summary_contains(world: &BehaviourWorld, needle: &str) -> Result<(), String> {
    let summary = &outcome(world)?.summary;
    if summary.contains(needle) {
        Ok(())
    } else {
        Err(format!(
            "expected summary to contain {needle:?}, got:\n{summary}"
        ))
    }
}

#[then(regex = r#"^the summary shows a bind line "([^"]+)"$"#)]
fn summary_shows_bind_line(world: &mut BehaviourWorld, line: String) -> Result<(), String> {
    summary_contains(world, &line)
}

#[then(regex = r#"^the summary shows "([^"]+)"$"#)]
fn summary_shows(world: &mut BehaviourWorld, line: String) -> Result<(), String> {
    summary_contains(world, &line)
}

#[given(regex = r#"^a definition declaring a bind from "([^"]+)" to "([^"]+)"$"#)]
fn definition_declares_bind(world: &mut BehaviourWorld, source: String, target: String) {
    world
        .declared_mounts
        .mounts
        .push(lns_cli::run::declarative::MountDefault {
            bind: true,
            source,
            target,
            read_only: false,
            exclude: Vec::new(),
            optional: false,
        });
}

#[given(regex = r#"^this machine's home directory is "([^"]+)"$"#)]
fn machine_home_is(world: &mut BehaviourWorld, home: String) {
    world.declared_mounts.home = Some(std::path::PathBuf::from(home));
}

#[given("this machine has no home directory")]
fn machine_has_no_home(world: &mut BehaviourWorld) {
    world.declared_mounts.home = None;
}

#[when("the declared mounts resolve")]
fn declared_mounts_resolve(world: &mut BehaviourWorld) {
    let defaults = lns_cli::run::declarative::Defaults {
        mounts: world.declared_mounts.mounts.clone(),
        ..Default::default()
    };
    world.declared_mounts.outcome = Some(
        lns_cli::run::declarative::resolve(
            &defaults,
            Path::new("/work/project"),
            world.declared_mounts.home.as_deref(),
            None,
            Vec::new(),
        )
        .map(|resolved| resolved.mounts)
        .map_err(|e| format!("{e:#}")),
    );
}

#[then(regex = r#"^the host bind source is "([^"]+)"$"#)]
fn host_bind_source_is(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    let mounts = world
        .declared_mounts
        .outcome
        .as_ref()
        .ok_or("no declared-mount resolution captured")?
        .as_ref()
        .map_err(|e| format!("resolution failed: {e}"))?;
    let sources: Vec<&str> = mounts
        .iter()
        .filter_map(|mount| match mount {
            lns_ipc::MountSpec::Bind(bind) => Some(bind.host_source.as_str()),
            lns_ipc::MountSpec::Named(_) => None,
        })
        .collect();
    if sources == [expected.as_str()] {
        Ok(())
    } else {
        Err(format!("expected [{expected:?}], got {sources:?}"))
    }
}

#[then(regex = r#"^the mount resolution fails naming "([^"]+)"$"#)]
fn mount_resolution_fails(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    match world
        .declared_mounts
        .outcome
        .as_ref()
        .ok_or("no declared-mount resolution captured")?
    {
        Err(message) if message.contains(&needle) => Ok(()),
        other => Err(format!(
            "expected a failure naming {needle:?}, got {other:?}"
        )),
    }
}

#[given(regex = r#"^an? (optional|required) declared bind from "([^"]+)" to "([^"]+)"$"#)]
fn declared_bind_spec(world: &mut BehaviourWorld, kind: String, source: String, target: String) {
    world.host_bind.declared_specs.push(lns_ipc::BindSpec {
        host_source: source,
        target,
        read_only: false,
        exclude: Vec::new(),
        optional: kind == "optional",
    });
}

#[when("the declared binds are resolved interactively")]
fn declared_binds_resolved(world: &mut BehaviourWorld) {
    let specs = world.host_bind.declared_specs.clone();
    let dir = dir_from(world);
    let store = FakeStore {
        state: Mutex::new(world.host_bind.decisions.clone()),
    };
    let mut input = std::io::Cursor::new(world.host_bind.answer.clone().unwrap_or_default());
    let mut out = Vec::new();
    let result =
        resolve_binds(&specs, &dir, &store, true, &mut input, &mut out).map_err(|e| e.to_string());
    world.resolved_run = Some(ResolvedRunView {
        binds: result
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|bind| format!("{} -> {}", bind.host_source, bind.target))
            .collect(),
        ..Default::default()
    });
    world.host_bind.outcome = Some(HostBindOutcome {
        result,
        prompt: String::from_utf8(out).unwrap(),
        persisted: store.state.into_inner().unwrap(),
        summary: String::new(),
    });
}

#[then("no host bind is resolved")]
fn no_host_bind_resolved(world: &mut BehaviourWorld) -> Result<(), String> {
    let binds = resolved_binds(world)?;
    if binds.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "a skipped bind must reach neither the secret scan, the wire, nor the audit record, got {binds:?}"
        ))
    }
}

#[then("the output says the bind was skipped because it is not present on this host")]
fn output_reports_skipped_bind(world: &mut BehaviourWorld) -> Result<(), String> {
    let prompt = &outcome(world)?.prompt;
    if prompt.contains("skipping optional bind") && prompt.contains("not present on this host") {
        Ok(())
    } else {
        Err(format!("expected a skip line, got {prompt:?}"))
    }
}
