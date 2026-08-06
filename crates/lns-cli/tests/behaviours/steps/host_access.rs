use crate::world::{BehaviourWorld, HostAccessOutcome};
use cucumber::{given, then, when};
use lns_cli::run::host_access::{
    Console, HostAccessPorts, HostAccessRequest, HostCommandOutput, HostFacts, record_grants,
    resolve,
};
use lns_cli::run::summary::format_host_access;
use lns_policy::Policy;
use lns_policy::host_access_decisions::{
    HostAccessDecisionFile, HostAccessDecisionStore, HostAccessVerdict,
};
use lns_policy::host_bind_decisions::{
    HostBindDecisionFile, HostBindDecisionStore, SecretDisposition,
};
use std::sync::Mutex;

struct ScriptedHost {
    git_config: Option<String>,
    openpgp_socket: Option<String>,
    ssh_socket: Option<String>,
}

impl HostFacts for ScriptedHost {
    fn run(&self, program: &str, args: &[String]) -> std::io::Result<HostCommandOutput> {
        let answer = match (program, args.first().map(String::as_str)) {
            ("git", Some("config")) => self.git_config.clone(),
            ("gpgconf", _) => self.openpgp_socket.clone(),
            _ => None,
        };
        Ok(match answer {
            Some(stdout) => HostCommandOutput { status: 0, stdout },
            None => HostCommandOutput {
                status: 1,
                stdout: String::new(),
            },
        })
    }

    fn env(&self, name: &str) -> Option<String> {
        match name {
            "SSH_AUTH_SOCK" => self.ssh_socket.clone(),
            _ => None,
        }
    }
}

struct FakeSecrets(Mutex<HostBindDecisionFile>);

impl HostBindDecisionStore for FakeSecrets {
    fn load(&self) -> std::io::Result<HostBindDecisionFile> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, state: &HostBindDecisionFile) -> std::io::Result<()> {
        *self.0.lock().unwrap() = state.clone();
        Ok(())
    }
}

struct FakeVerdicts(Mutex<HostAccessDecisionFile>);

impl HostAccessDecisionStore for FakeVerdicts {
    fn load(&self) -> std::io::Result<HostAccessDecisionFile> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, state: &HostAccessDecisionFile) -> std::io::Result<()> {
        *self.0.lock().unwrap() = state.clone();
        Ok(())
    }
}

fn cwd(world: &mut BehaviourWorld) -> std::path::PathBuf {
    if world.cwd.is_none() {
        world.cwd = Some(tempfile::TempDir::new().expect("tempdir"));
    }
    world.cwd.as_ref().unwrap().path().to_path_buf()
}

fn policy_path(world: &mut BehaviourWorld) -> std::path::PathBuf {
    cwd(world).join("lns-policy.yaml")
}

fn git_config_stdout(world: &BehaviourWorld) -> Option<String> {
    if world.host_access.no_git {
        return None;
    }
    Some(
        world
            .host_access
            .git_settings
            .iter()
            .map(|(key, value)| format!("{key}\n{value}\0"))
            .collect(),
    )
}

fn drive(world: &mut BehaviourWorld, interactive: bool) {
    let host = ScriptedHost {
        git_config: git_config_stdout(world),
        openpgp_socket: world.host_access.openpgp_socket.clone(),
        ssh_socket: world.host_access.ssh_socket.clone(),
    };
    let secrets = FakeSecrets(Mutex::new(
        world
            .host_access
            .secret_decisions
            .iter()
            .map(|(key, value)| (format!("gitconfig:{key}"), *value))
            .collect(),
    ));
    let verdicts = FakeVerdicts(Mutex::new(
        world
            .host_access
            .declines
            .iter()
            .map(|id| (id.clone(), HostAccessVerdict::Declined))
            .collect(),
    ));
    // The card is answered before any secret prompt, so the scripted answers queue in that order.
    let mut answers = String::new();
    if let Some(answer) = &world.host_access.card_answer {
        answers.push_str(answer);
        answers.push('\n');
    }
    for _ in 0..8 {
        if let Some(answer) = &world.host_access.secret_answer {
            answers.push_str(answer);
            answers.push('\n');
        }
    }
    let mut input = std::io::Cursor::new(answers.into_bytes());
    let mut captured: Vec<u8> = Vec::new();
    let request = HostAccessRequest {
        declared: world.host_access.declared.clone(),
        granted: world.host_access.granted.clone(),
    };
    let result = {
        let ports = HostAccessPorts {
            facts: &host,
            secrets: &secrets,
            verdicts: &verdicts,
        };
        let mut console = Console {
            input: &mut input,
            output: &mut captured,
        };
        resolve(&request, &ports, interactive, &mut console)
    };
    let path = policy_path(world);
    let (result, summary) = match result {
        Ok(resolution) => {
            record_grants(&path, &resolution.newly_granted).expect("recording grants");
            let summary = format_host_access(&resolution.outcomes);
            (Ok(resolution.outcomes), summary)
        }
        Err(e) => (Err(format!("{e:#}")), String::new()),
    };
    let policy_host_access = Policy::load_or_default(&path)
        .map(|p| p.host_access)
        .unwrap_or_default();
    world.host_access.outcome = Some(HostAccessOutcome {
        result,
        prompt: String::from_utf8_lossy(&captured).into_owned(),
        persisted_secrets: secrets.0.into_inner().unwrap(),
        persisted_declines: verdicts.0.into_inner().unwrap().into_keys().collect(),
        summary,
        policy_host_access,
    });
}

fn outcome(world: &BehaviourWorld) -> Result<&HostAccessOutcome, String> {
    world
        .host_access
        .outcome
        .as_ref()
        .ok_or_else(|| "no host-access resolution ran".to_string())
}

fn armed(world: &BehaviourWorld) -> Result<&lns_cli::run::host_access::ArmedHostAccess, String> {
    let outcomes = outcome(world)?
        .result
        .as_ref()
        .map_err(|e| format!("resolution failed: {e}"))?;
    outcomes
        .iter()
        .find_map(|o| match o {
            lns_cli::run::host_access::HostAccessOutcome::Armed(a) => Some(a),
            _ => None,
        })
        .ok_or_else(|| format!("no host access was armed: {outcomes:?}"))
}

fn set_git(world: &mut BehaviourWorld, key: &str, value: &str) {
    world
        .host_access
        .git_settings
        .push((key.to_string(), value.to_string()));
}

#[given(regex = r#"^the sandbox definition declares host access "([^"]+)"$"#)]
fn definition_declares(world: &mut BehaviourWorld, id: String) {
    world.host_access.declared.push(id);
}

#[given(regex = r"^the directory has no sandbox definition$")]
fn no_definition(world: &mut BehaviourWorld) {
    world.host_access.declared.clear();
}

#[given(regex = r#"^the directory's policy already records host access "([^"]+)"$"#)]
fn policy_grants(world: &mut BehaviourWorld, id: String) {
    world.host_access.granted.push(id);
}

#[given(regex = r"^the host git config leaves commit\.gpgsign off$")]
fn gpgsign_off(world: &mut BehaviourWorld) {
    set_git(world, "commit.gpgsign", "false");
}

#[given(regex = r"^the host git config enables commit\.gpgsign$")]
fn gpgsign_on(world: &mut BehaviourWorld) {
    set_git(world, "commit.gpgsign", "true");
}

#[given(regex = r"^the host git config disables commit\.gpgsign globally$")]
fn gpgsign_off_globally(world: &mut BehaviourWorld) {
    set_git(world, "commit.gpgsign", "false");
}

#[given(regex = r"^the repository at the working directory enables commit\.gpgsign$")]
fn gpgsign_on_locally(world: &mut BehaviourWorld) {
    set_git(world, "commit.gpgsign", "true");
}

#[given(regex = r#"^the host git config sets user\.email to "([^"]+)"$"#)]
fn set_email(world: &mut BehaviourWorld, value: String) {
    set_git(world, "user.email", &value);
}

#[given(regex = r#"^the host git config sets gpg\.format to "([^"]+)"$"#)]
fn set_format(world: &mut BehaviourWorld, value: String) {
    set_git(world, "gpg.format", &value);
}

#[given(regex = r#"^the host git config sets "([^"]+)"$"#)]
fn set_bare_key(world: &mut BehaviourWorld, key: String) {
    set_git(world, &key, "some-host-secret");
}

#[given(regex = r#"^the host git config sets an undecided "([^"]+)"$"#)]
fn set_undecided_key(world: &mut BehaviourWorld, key: String) {
    set_git(world, &key, "some-host-secret");
}

#[given(regex = r#"^the host git config includes a file setting user\.email to "([^"]+)"$"#)]
fn set_email_via_include(world: &mut BehaviourWorld, value: String) {
    // Real `git config --list` emits the include directive alongside the flattened value, so the fake must too.
    set_git(world, "include.path", "/Users/dev/.gitconfig-work");
    set_git(world, "user.email", &value);
}

#[given(regex = r"^the host has no git config and no agent$")]
fn no_git_at_all(world: &mut BehaviourWorld) {
    world.host_access.no_git = true;
    world.host_access.openpgp_socket = None;
    world.host_access.ssh_socket = None;
}

#[given(regex = r#"^the host agent socket is located at "([^"]+)"$"#)]
fn openpgp_socket_at(world: &mut BehaviourWorld, path: String) {
    world.host_access.openpgp_socket = Some(path);
}

#[given(regex = r#"^the host ssh agent socket is located at "([^"]+)"$"#)]
fn ssh_socket_at(world: &mut BehaviourWorld, path: String) {
    world.host_access.ssh_socket = Some(path);
}

#[given(regex = r"^no agent socket can be located on the host$")]
fn no_socket(world: &mut BehaviourWorld) {
    world.host_access.openpgp_socket = None;
    world.host_access.ssh_socket = None;
}

#[given(regex = r#"^the operator will answer the host-access card with "([^"]+)"$"#)]
fn card_answer(world: &mut BehaviourWorld, answer: String) {
    world.host_access.card_answer = Some(answer);
}

#[given(regex = r#"^the operator will answer the config secret prompt with "([^"]+)"$"#)]
fn secret_answer(world: &mut BehaviourWorld, answer: String) {
    world.host_access.secret_answer = Some(answer);
}

#[given(regex = r#"^a per-machine decline is recorded for host access "([^"]+)"$"#)]
fn decline_recorded(world: &mut BehaviourWorld, id: String) {
    world.host_access.declines.push(id);
}

#[when(regex = r"^the host access is resolved for `lns run ([^`]+)` interactively$")]
fn resolve_interactively(world: &mut BehaviourWorld, _flags: String) {
    drive(world, true);
}

#[when(regex = r"^the host access is resolved for `lns run ([^`]+)` with no terminal$")]
fn resolve_non_interactively(world: &mut BehaviourWorld, _flags: String) {
    drive(world, false);
}

#[then(regex = r"^no host-access card is shown$")]
fn no_card(world: &mut BehaviourWorld) -> Result<(), String> {
    let prompt = &outcome(world)?.prompt;
    if prompt.contains("Grant it?") {
        return Err(format!("a card was shown: {prompt:?}"));
    }
    Ok(())
}

#[then(regex = r"^no agent socket is forwarded$")]
fn no_socket_forwarded(world: &mut BehaviourWorld) -> Result<(), String> {
    match armed(world) {
        Ok(a) => Err(format!("a socket was forwarded: {}", a.socket_source)),
        Err(_) => Ok(()),
    }
}

#[then(regex = r"^no git config is projected$")]
fn no_config_projected(world: &mut BehaviourWorld) -> Result<(), String> {
    match armed(world) {
        Ok(a) => Err(format!("a config was projected: {:?}", a.git_config)),
        Err(_) => Ok(()),
    }
}

#[then(regex = r#"^the host-access summary shows "([^"]+)"$"#)]
fn summary_shows(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let summary = &outcome(world)?.summary;
    if summary.contains(&needle) {
        Ok(())
    } else {
        Err(format!("expected {needle:?} in summary, got {summary:?}"))
    }
}

#[then(regex = r#"^the host-access summary shows a line "([^"]+)"$"#)]
fn summary_shows_line(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    summary_shows(world, needle)
}

#[then(regex = r#"^the projected git config carries "([^"]+)"$"#)]
fn config_carries(world: &mut BehaviourWorld, pair: String) -> Result<(), String> {
    let (key, value) = pair
        .split_once('=')
        .ok_or_else(|| format!("expected key=value, got {pair:?}"))?;
    let leaf = key.rsplit('.').next().unwrap_or(key);
    let rendered = &armed(world)?.git_config;
    let expected = format!("{leaf} = \"{value}\"");
    if rendered.contains(&expected) {
        Ok(())
    } else {
        Err(format!("expected {expected:?} in:\n{rendered}"))
    }
}

#[then(regex = r#"^the projected git config omits "([^"]+)"$"#)]
fn config_omits(world: &mut BehaviourWorld, key: String) -> Result<(), String> {
    let leaf = key.rsplit('.').next().unwrap_or(&key);
    let rendered = &armed(world)?.git_config;
    if rendered.contains(leaf) {
        return Err(format!("expected {leaf:?} to be dropped from:\n{rendered}"));
    }
    Ok(())
}

#[then(regex = r"^the projected git config names no host include path$")]
fn config_has_no_include(world: &mut BehaviourWorld) -> Result<(), String> {
    let rendered = &armed(world)?.git_config;
    if rendered.contains("include") {
        return Err(format!("an include survived into the guest:\n{rendered}"));
    }
    Ok(())
}

#[then(regex = r#"^the forwarded agent socket is "([^"]+)"$"#)]
fn forwarded_socket_is(world: &mut BehaviourWorld, path: String) -> Result<(), String> {
    let armed = armed(world)?;
    if armed.socket_source == path {
        Ok(())
    } else {
        Err(format!(
            "expected {path:?} forwarded, got {:?}",
            armed.socket_source
        ))
    }
}

#[then(regex = r#"^the host-access resolution fails with "([^"]+)"$"#)]
fn resolution_fails_with(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    match &outcome(world)?.result {
        Err(e) if e.contains(&needle) => Ok(()),
        other => Err(format!(
            "expected failure containing {needle:?}, got {other:?}"
        )),
    }
}

#[then(regex = r"^the host-access resolution succeeds$")]
fn resolution_succeeds(world: &mut BehaviourWorld) -> Result<(), String> {
    match &outcome(world)?.result {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("expected success, got failure: {e}")),
    }
}

#[then(regex = r#"^the failure names "([^"]+)" as the setting that required it$"#)]
#[then(regex = r#"^the failure names "([^"]+)" as the fix$"#)]
fn failure_names(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    resolution_fails_with(world, needle)
}

#[then(regex = r"^the sandbox is not launched$")]
fn sandbox_not_launched(world: &mut BehaviourWorld) -> Result<(), String> {
    match &outcome(world)?.result {
        Err(_) => Ok(()),
        Ok(outcomes) => Err(format!(
            "resolution succeeded, so the run would proceed: {outcomes:?}"
        )),
    }
}

#[then(regex = r#"^the directory's policy records host access "([^"]+)"$"#)]
fn policy_records(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let recorded = &outcome(world)?.policy_host_access;
    if recorded.contains(&id) {
        Ok(())
    } else {
        Err(format!("expected {id:?} granted, got {recorded:?}"))
    }
}

#[given(regex = r"^the directory's policy records no host access$")]
#[then(regex = r"^the directory's policy records no host access$")]
fn policy_records_nothing(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.host_access.outcome.is_none() {
        world.host_access.granted.clear();
        return Ok(());
    }
    let recorded = &outcome(world)?.policy_host_access;
    if recorded.is_empty() {
        Ok(())
    } else {
        Err(format!("expected no grant, got {recorded:?}"))
    }
}

#[then(regex = r#"^a per-machine (KEEP|DROP) decision is recorded for the config key "([^"]+)"$"#)]
fn secret_decision_recorded(
    world: &mut BehaviourWorld,
    kind: String,
    key: String,
) -> Result<(), String> {
    let want = if kind == "KEEP" {
        SecretDisposition::Keep
    } else {
        SecretDisposition::Drop
    };
    let persisted = &outcome(world)?.persisted_secrets;
    match persisted.get(&format!("gitconfig:{key}")) {
        Some(got) if *got == want => Ok(()),
        other => Err(format!("expected a recorded {kind}, got {other:?}")),
    }
}

#[then(regex = r#"^the dropped config key "([^"]+)" is reported on stderr$"#)]
fn drop_reported(world: &mut BehaviourWorld, key: String) -> Result<(), String> {
    let prompt = &outcome(world)?.prompt;
    if prompt.contains(&key) && prompt.contains("no terminal to ask") {
        Ok(())
    } else {
        Err(format!("expected {key:?} reported, got {prompt:?}"))
    }
}

#[then(regex = r"^no config secret prompt is shown$")]
fn no_secret_prompt(world: &mut BehaviourWorld) -> Result<(), String> {
    let prompt = &outcome(world)?.prompt;
    if prompt.contains("looks like a secret") {
        return Err(format!("a secret prompt was shown: {prompt:?}"));
    }
    Ok(())
}

#[then(regex = r"^a later run with the same host config shows no config secret prompt$")]
fn later_run_is_silent(world: &mut BehaviourWorld) -> Result<(), String> {
    let persisted = outcome(world)?.persisted_secrets.clone();
    world.host_access.secret_decisions = persisted
        .into_iter()
        .map(|(key, value)| (key.trim_start_matches("gitconfig:").to_string(), value))
        .collect();
    world.host_access.secret_answer = None;
    drive(world, true);
    no_secret_prompt(world)
}

#[then(regex = r#"^a per-machine decline is now recorded for host access "([^"]+)"$"#)]
fn decline_now_recorded(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let persisted = &outcome(world)?.persisted_declines;
    if persisted.contains(&id) {
        Ok(())
    } else {
        Err(format!("expected a recorded decline, got {persisted:?}"))
    }
}

fn cli_run(world: &mut BehaviourWorld) -> Result<&(i32, String), String> {
    world
        .host_access
        .cli
        .as_ref()
        .ok_or_else(|| "no host-access command ran".to_string())
}

#[given(regex = r#"^this directory grants host access "([^"]+)"$"#)]
fn directory_grants(world: &mut BehaviourWorld, id: String) {
    let path = policy_path(world);
    let mut policy = Policy::load_or_default(&path).expect("load policy");
    policy.grant_host_access(id);
    policy.save_atomic(&path).expect("save policy");
}

#[given(regex = r#"^a standing decline is recorded for host access "([^"]+)"$"#)]
fn standing_decline(world: &mut BehaviourWorld, id: String) {
    world.host_access.declines.push(id);
}

#[when(regex = r#"^the user runs host-access command "([^"]+)"$"#)]
fn run_host_access_command(world: &mut BehaviourWorld, line: String) {
    let mut argv = vec!["lns".to_string(), "host-access".to_string()];
    argv.extend(line.split_whitespace().map(str::to_string));
    let args: lns_cli::host_access::HostAccessArgs =
        lns_cli::command::parse_args(&argv).expect("argv must parse against the CLI grammar");
    let verdicts = FakeVerdicts(Mutex::new(
        world
            .host_access
            .declines
            .iter()
            .map(|id| (id.clone(), HostAccessVerdict::Declined))
            .collect(),
    ));
    let path = policy_path(world);
    let mut out: Vec<u8> = Vec::new();
    let outcome = lns_cli::host_access::run(&args.command, &path, &verdicts, &mut out);
    let captured = match outcome {
        Ok(code) => (code, String::from_utf8_lossy(&out).into_owned()),
        Err(e) => (1, format!("{e:#}")),
    };
    world.result = Some(crate::runner::CliRun {
        exit_code: captured.0,
        output: captured.1.clone(),
    });
    world.host_access.cli = Some(captured);
    world.host_access.declines = verdicts.0.into_inner().unwrap().into_keys().collect();
}

#[then(regex = r#"^the host-access output contains "([^"]+)"$"#)]
fn cli_output_contains(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let (_, output) = cli_run(world)?;
    if output.contains(&needle) {
        Ok(())
    } else {
        Err(format!("expected {needle:?} in {output:?}"))
    }
}

#[then(regex = r#"^this directory's policy grants "([^"]+)"$"#)]
fn policy_grants_id(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let path = policy_path(world);
    let granted = Policy::load_or_default(&path)
        .map_err(|e| e.to_string())?
        .host_access;
    if granted.contains(&id) {
        Ok(())
    } else {
        Err(format!("expected {id:?} granted, got {granted:?}"))
    }
}

#[then(regex = r"^this directory's policy grants nothing$")]
fn policy_grants_nothing(world: &mut BehaviourWorld) -> Result<(), String> {
    let path = policy_path(world);
    let granted = Policy::load_or_default(&path)
        .map_err(|e| e.to_string())?
        .host_access;
    if granted.is_empty() {
        Ok(())
    } else {
        Err(format!("expected no grant, got {granted:?}"))
    }
}

#[then(regex = r#"^no standing decline remains for host access "([^"]+)"$"#)]
fn no_standing_decline(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    if world.host_access.declines.contains(&id) {
        return Err(format!(
            "the decline survived the grant: {:?}",
            world.host_access.declines
        ));
    }
    Ok(())
}
