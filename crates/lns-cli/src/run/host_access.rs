mod real;

use std::io;

use anyhow::Context;
use lns_policy::host_access::{HostAccess, Locate, SocketSpec};
pub use real::RealHostFacts;

pub struct HostCommandOutput {
    pub status: i32,
    pub stdout: String,
}

/// The host facts a capability is resolved from: what a locate command prints, and what an env var holds. Injected so Layer 2 can script a host without running one.
pub trait HostFacts: Send + Sync {
    fn run(&self, program: &str, args: &[String]) -> io::Result<HostCommandOutput>;
    fn env(&self, name: &str) -> Option<String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSetting {
    pub key: String,
    pub value: String,
}

/// What the host's own git config asks of a run. `Off` means the user does not sign, so the capability stays absent rather than mandatory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningNeed {
    Off,
    Openpgp,
    Ssh,
}

/// `git config --list -z` output: NUL-separated entries, key and value split by the first newline. A value may itself contain newlines, which is why `-z` is used.
pub fn parse_git_config(stdout: &str) -> Vec<GitSetting> {
    stdout
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(|entry| match entry.split_once('\n') {
            Some((key, value)) => GitSetting {
                key: key.to_string(),
                value: value.to_string(),
            },
            None => GitSetting {
                key: entry.to_string(),
                value: String::new(),
            },
        })
        .collect()
}

/// Git's own boolean spelling; an empty value means the key was set with no value, which git reads as true.
fn git_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "yes" | "on" | "1" | ""
    )
}

/// Later entries win, which is how a repository's own config overrides the global one — `git config --list` emits system, then global, then local.
fn last_value<'a>(settings: &'a [GitSetting], key: &str) -> Option<&'a str> {
    settings
        .iter()
        .rev()
        .find(|s| s.key == key)
        .map(|s| s.value.as_str())
}

pub fn signing_need(settings: &[GitSetting]) -> SigningNeed {
    if !last_value(settings, "commit.gpgsign").is_some_and(git_bool) {
        return SigningNeed::Off;
    }
    match last_value(settings, "gpg.format") {
        Some(format) if format.trim().eq_ignore_ascii_case("ssh") => SigningNeed::Ssh,
        _ => SigningNeed::Openpgp,
    }
}

/// Matched against the whole lowercased key with no dot boundary required, because tools spell compound leaves like `oauthClientSecret` and `smtpPass`. Over-matching only drops a setting, which is the safe direction.
const SECRET_KEY_SUFFIXES: &[&str] = &[
    "extraheader",
    "helper",
    "token",
    "password",
    "pass",
    "apikey",
    "secret",
    "privatekey",
];

/// A URL carrying userinfo (`scheme://user:secret@host`) is a credential wherever it appears, so the check runs over the key too — an `insteadOf` rewrite spells its secret in the key.
fn carries_url_userinfo(text: &str) -> bool {
    let authorities = match text.split_once("://") {
        // A schemeless `user:pass@host:port` is how a proxy setting spells its credential.
        None => vec![text],
        Some(_) => text.split("://").skip(1).collect(),
    };
    authorities.into_iter().any(|rest| {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        authority.rsplit_once('@').is_some_and(|(userinfo, _)| {
            userinfo.contains(':') && !userinfo.contains(char::is_whitespace)
        })
    })
}

/// `git config --list` emits include directives alongside the values it already flattened, and their paths are host paths that mean nothing in the guest.
fn is_include_directive(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key == "include.path" || (key.starts_with("includeif.") && key.ends_with(".path"))
}

pub fn looks_like_secret(setting: &GitSetting) -> bool {
    let key = setting.key.to_ascii_lowercase();
    SECRET_KEY_SUFFIXES
        .iter()
        .any(|suffix| key.ends_with(suffix))
        || carries_url_userinfo(&setting.key)
        || carries_url_userinfo(&setting.value)
}

/// Renders settings back into a git config file. Every setting lands in its own one-line section header, so a subsection name containing a dot round-trips instead of being re-split into the wrong key.
pub fn render_git_config(settings: &[GitSetting]) -> String {
    let mut out = String::new();
    for setting in settings {
        let (section, key) = match setting.key.rsplit_once('.') {
            Some(split) => split,
            None => continue,
        };
        match section.split_once('.') {
            Some((top, sub)) => {
                let quoted = sub.replace('\\', "\\\\").replace('"', "\\\"");
                out.push_str(&format!("[{top} \"{quoted}\"]\n"));
            }
            None => out.push_str(&format!("[{top}]\n", top = section)),
        }
        out.push_str(&format!("\t{key} = {}\n", escape_value(&setting.value)));
    }
    out
}

fn escape_value(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

pub fn locate_socket(facts: &dyn HostFacts, spec: &SocketSpec) -> Option<String> {
    let raw = match &spec.locate {
        Locate::Command(argv) => {
            let (program, args) = argv.split_first()?;
            let output = facts.run(program, args).ok()?;
            if output.status != 0 {
                return None;
            }
            output.stdout
        }
        Locate::Env(name) => facts.env(name)?,
    };
    let path = raw.trim();
    if path.is_empty() {
        return None;
    }
    Some(path.to_string())
}

pub fn socket_spec_for(entry: &HostAccess, need: SigningNeed) -> Option<&SocketSpec> {
    match need {
        SigningNeed::Off => None,
        SigningNeed::Openpgp => Some(&entry.openpgp_socket),
        SigningNeed::Ssh => Some(&entry.ssh_socket),
    }
}

/// The per-machine keep/drop verdicts for projected config keys share the host-bind decision file; the key is namespaced so a config key can never be mistaken for a bind path.
fn decision_key(config_key: &str) -> String {
    format!("gitconfig:{config_key}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmedHostAccess {
    pub id: String,
    pub socket_source: String,
    pub socket_target: String,
    pub git_config: String,
    pub git_config_target: String,
    pub gnupg_home: String,
    pub dropped_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostAccessOutcome {
    /// The host does not sign, so the capability is not forwarded and nothing is projected.
    Absent {
        id: String,
    },
    Armed(ArmedHostAccess),
}

pub struct HostAccessRequest {
    pub declared: Vec<String>,
    pub granted: Vec<String>,
}

pub struct HostAccessResolution {
    pub outcomes: Vec<HostAccessOutcome>,
    /// Ids the operator granted on this run, for the caller to record in the directory's policy.
    pub newly_granted: Vec<String>,
}

/// The operator's terminal, bundled so a prompt-bearing call keeps a readable signature.
pub struct Console<'a> {
    pub input: &'a mut dyn std::io::BufRead,
    pub output: &'a mut dyn std::io::Write,
}

pub struct HostAccessPorts<'a> {
    pub facts: &'a dyn HostFacts,
    pub secrets: &'a dyn lns_policy::host_bind_decisions::HostBindDecisionStore,
    pub verdicts: &'a dyn lns_policy::host_access_decisions::HostAccessDecisionStore,
}

fn read_git_config(facts: &dyn HostFacts) -> Vec<GitSetting> {
    match facts.run("git", &["config".into(), "--list".into(), "-z".into()]) {
        Ok(output) if output.status == 0 => parse_git_config(&output.stdout),
        _ => Vec::new(),
    }
}

fn prompt_grant(
    entry: &HostAccess,
    socket_source: &str,
    socket_target: &str,
    console: &mut Console<'_>,
) -> anyhow::Result<bool> {
    writeln!(
        console.output,
        "Host access: {} wants the host agent at {socket_source}, reachable in the sandbox at {socket_target}.",
        entry.name
    )?;
    writeln!(
        console.output,
        "While the run is live the workload can ask your agent to sign anything. It cannot read the key."
    )?;
    write!(console.output, "Grant it? [g]rant / [D]ecline (default): ")?;
    console.output.flush()?;
    let mut answer = String::new();
    console.input.read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "g" | "grant"
    ))
}

fn prompt_secret(
    config_key: &str,
    console: &mut Console<'_>,
) -> anyhow::Result<lns_policy::host_bind_decisions::SecretDisposition> {
    use lns_policy::host_bind_decisions::SecretDisposition;
    write!(
        console.output,
        "Host config: {config_key} looks like a secret. Project it into the workload? [k]eep / [D]rop (default): "
    )?;
    console.output.flush()?;
    let mut answer = String::new();
    console.input.read_line(&mut answer)?;
    Ok(match answer.trim().to_ascii_lowercase().as_str() {
        "k" | "keep" => SecretDisposition::Keep,
        _ => SecretDisposition::Drop,
    })
}

struct ProjectedConfig {
    rendered: String,
    dropped: Vec<String>,
}

fn project_config(
    settings: &[GitSetting],
    ports: &HostAccessPorts<'_>,
    interactive: bool,
    console: &mut Console<'_>,
) -> anyhow::Result<ProjectedConfig> {
    use lns_policy::host_bind_decisions::SecretDisposition;
    let mut recorded = ports.secrets.load()?;
    let mut changed = false;
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for setting in settings {
        if is_include_directive(&setting.key) {
            continue;
        }
        if !looks_like_secret(setting) {
            kept.push(setting.clone());
            continue;
        }
        let key = decision_key(&setting.key);
        let disposition = match recorded.get(&key).copied() {
            Some(known) => known,
            None if interactive => {
                let chosen = prompt_secret(&setting.key, console)?;
                recorded.insert(key, chosen);
                changed = true;
                chosen
            }
            None => {
                writeln!(
                    console.output,
                    "lns: no terminal to ask — dropping secret-shaped host config {} (run interactively to keep it)",
                    setting.key
                )?;
                SecretDisposition::Drop
            }
        };
        match disposition {
            SecretDisposition::Keep => kept.push(setting.clone()),
            SecretDisposition::Drop => dropped.push(setting.key.clone()),
        }
    }
    if changed {
        ports.secrets.save(&recorded)?;
    }
    Ok(ProjectedConfig {
        rendered: render_git_config(&kept),
        dropped,
    })
}

/// Decides whether the operator has already answered for this id, asks when they have not, and records a decline so the next run refuses without asking again.
fn settle_grant(
    entry: &HostAccess,
    already_granted: bool,
    socket_source: &str,
    socket_target: &str,
    ports: &HostAccessPorts<'_>,
    interactive: bool,
    console: &mut Console<'_>,
) -> anyhow::Result<bool> {
    use lns_policy::host_access_decisions::HostAccessVerdict;
    if already_granted {
        return Ok(false);
    }
    if !interactive {
        anyhow::bail!(
            "no terminal to confirm host access {:?}; grant it with `lns host-access grant {}` and run again",
            entry.id,
            entry.id
        );
    }
    if !prompt_grant(entry, socket_source, socket_target, console)? {
        let mut verdicts = ports.verdicts.load()?;
        verdicts.insert(entry.id.clone(), HostAccessVerdict::Declined);
        ports.verdicts.save(&verdicts)?;
        anyhow::bail!("host access declined: {:?}", entry.id);
    }
    Ok(true)
}

fn resolve_one(
    id: &str,
    request: &HostAccessRequest,
    ports: &HostAccessPorts<'_>,
    interactive: bool,
    console: &mut Console<'_>,
) -> anyhow::Result<(HostAccessOutcome, bool)> {
    let entry = lns_policy::host_access::find(id).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown host access {id:?}; this machine's catalog does not know it — run `lns host-access list` to see what is available, then remove the id from spec.hostAccess, this directory's lns-policy.yaml, or the --host-access flag"
        )
    })?;
    let settings = read_git_config(ports.facts);
    let need = signing_need(&settings);
    let Some(spec) = socket_spec_for(entry, need) else {
        return Ok((HostAccessOutcome::Absent { id: id.to_string() }, false));
    };
    let already_granted = request.granted.iter().any(|g| g == id);
    // Checked even when the policy grants it: a policy file travels with a clone, so the weaker signal must not outrank the operator's machine-wide no. `lns host-access grant` is the only way to clear it.
    if ports.verdicts.load()?.contains_key(&entry.id) {
        anyhow::bail!(
            "host access declined: {id:?} carries a standing decline on this machine; clear it with `lns host-access grant {id}`"
        );
    }
    let socket_source = locate_socket(ports.facts, spec).ok_or_else(|| {
        anyhow::anyhow!(
            "no signing agent found on this host, but commit.gpgsign is enabled in your git config, so this sandbox would not be able to sign; start your agent, or turn commit.gpgsign off"
        )
    })?;
    let newly_granted = settle_grant(
        entry,
        already_granted,
        &socket_source,
        &spec.target,
        ports,
        interactive,
        console,
    )?;
    let projected = project_config(&settings, ports, interactive, console)?;
    Ok((
        HostAccessOutcome::Armed(ArmedHostAccess {
            id: id.to_string(),
            socket_source,
            socket_target: spec.target.clone(),
            git_config: projected.rendered,
            git_config_target: entry.git_config.clone(),
            gnupg_home: entry.gnupg_home.clone(),
            dropped_keys: projected.dropped,
        }),
        newly_granted,
    ))
}

pub fn resolve(
    request: &HostAccessRequest,
    ports: &HostAccessPorts<'_>,
    interactive: bool,
    console: &mut Console<'_>,
) -> anyhow::Result<HostAccessResolution> {
    let mut outcomes = Vec::new();
    let mut newly_granted = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for id in request.declared.iter().chain(request.granted.iter()) {
        if !seen.insert(id.as_str()) {
            continue;
        }
        let (outcome, granted) = resolve_one(id, request, ports, interactive, console)?;
        if granted {
            newly_granted.push(id.clone());
        }
        outcomes.push(outcome);
    }
    Ok(HostAccessResolution {
        outcomes,
        newly_granted,
    })
}

/// Records the ids the operator granted on this run into the directory's shareable policy, so the next run arms without a card.
pub fn record_grants(policy_path: &std::path::Path, ids: &[String]) -> anyhow::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut policy = lns_policy::Policy::load_or_default(policy_path)
        .with_context(|| format!("reading {}", policy_path.display()))?;
    for id in ids {
        policy.grant_host_access(id.clone());
    }
    policy
        .save_atomic(policy_path)
        .with_context(|| format!("writing {}", policy_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(key: &str, value: &str) -> GitSetting {
        GitSetting {
            key: key.into(),
            value: value.into(),
        }
    }

    #[test]
    fn parse_splits_nul_separated_entries_at_the_first_newline() {
        let parsed = parse_git_config("user.name\nAda\0commit.gpgsign\ntrue\0");
        assert_eq!(
            parsed,
            vec![
                setting("user.name", "Ada"),
                setting("commit.gpgsign", "true")
            ]
        );
    }

    #[test]
    fn parse_keeps_a_value_that_itself_contains_a_newline() {
        let parsed = parse_git_config("alias.lg\n!f() {\n  echo hi\n}; f\0");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].key, "alias.lg");
        assert_eq!(parsed[0].value, "!f() {\n  echo hi\n}; f");
    }

    #[test]
    fn parse_treats_a_valueless_entry_as_an_empty_value() {
        let parsed = parse_git_config("commit.gpgsign\0");
        assert_eq!(parsed, vec![setting("commit.gpgsign", "")]);
    }

    #[test]
    fn a_host_that_does_not_sign_needs_nothing() {
        assert_eq!(signing_need(&[]), SigningNeed::Off);
        assert_eq!(
            signing_need(&[setting("commit.gpgsign", "false")]),
            SigningNeed::Off
        );
    }

    #[test]
    fn every_git_boolean_spelling_of_true_enables_signing() {
        for value in ["true", "TRUE", "yes", "on", "1", ""] {
            assert_eq!(
                signing_need(&[setting("commit.gpgsign", value)]),
                SigningNeed::Openpgp,
                "{value:?} is git-true"
            );
        }
    }

    #[test]
    fn a_repository_setting_overrides_the_global_one_because_it_comes_last() {
        let settings = [
            setting("commit.gpgsign", "false"),
            setting("commit.gpgsign", "true"),
        ];
        assert_eq!(signing_need(&settings), SigningNeed::Openpgp);
        let reversed = [
            setting("commit.gpgsign", "true"),
            setting("commit.gpgsign", "false"),
        ];
        assert_eq!(signing_need(&reversed), SigningNeed::Off);
    }

    #[test]
    fn an_ssh_format_host_needs_the_ssh_agent() {
        let settings = [
            setting("commit.gpgsign", "true"),
            setting("gpg.format", "ssh"),
        ];
        assert_eq!(signing_need(&settings), SigningNeed::Ssh);
    }

    #[test]
    fn an_openpgp_format_is_the_default_and_an_unknown_format_falls_back_to_it() {
        for format in ["openpgp", "x509", "SSHX"] {
            let settings = [
                setting("commit.gpgsign", "true"),
                setting("gpg.format", format),
            ];
            assert_eq!(signing_need(&settings), SigningNeed::Openpgp, "{format:?}");
        }
    }

    #[test]
    fn credential_bearing_keys_are_classified_as_secret() {
        for key in [
            "http.https://git.example.test/.extraheader",
            "credential.helper",
            "credential.https://git.example.test.helper",
            "some.token",
            "sendemail.smtppass",
            "some.password",
            "service.apikey",
            "imap.pass",
            "credential.https://git.example.test.oauthClientSecret",
        ] {
            assert!(
                looks_like_secret(&setting(key, "x")),
                "{key:?} must be treated as a secret"
            );
        }
    }

    #[test]
    fn an_ordinary_setting_is_not_classified_as_secret() {
        for key in [
            "user.name",
            "user.email",
            "user.signingkey",
            "commit.gpgsign",
            "gpg.format",
            "alias.lg",
            "init.defaultbranch",
            "pull.rebase",
        ] {
            assert!(
                !looks_like_secret(&setting(key, "x")),
                "{key:?} must be projected"
            );
        }
    }

    #[test]
    fn a_url_with_embedded_credentials_is_secret_whether_it_sits_in_the_key_or_the_value() {
        assert!(looks_like_secret(&setting(
            "url.https://user:tok@git.example.test/.insteadof",
            "https://git.example.test/"
        )));
        assert!(looks_like_secret(&setting(
            "some.remote",
            "https://user:tok@git.example.test/repo"
        )));
    }

    #[test]
    fn a_schemeless_value_carrying_userinfo_is_secret() {
        assert!(looks_like_secret(&setting(
            "http.proxy",
            "user:pass@proxy.example.test:8080"
        )));
    }

    #[test]
    fn an_address_that_merely_contains_an_at_sign_is_not_secret() {
        assert!(!looks_like_secret(&setting(
            "user.email",
            "me@example.test"
        )));
    }

    #[test]
    fn an_include_directive_is_dropped_rather_than_projected() {
        assert!(is_include_directive("include.path"));
        assert!(is_include_directive(
            "includeIf.gitdir:~/work/.path"
                .to_ascii_lowercase()
                .as_str()
        ));
        assert!(!is_include_directive("user.path"));
    }

    #[test]
    fn a_url_without_userinfo_is_not_secret() {
        assert!(!looks_like_secret(&setting(
            "url.https://git.example.test/.insteadof",
            "git@git.example.test:"
        )));
    }

    #[test]
    fn render_emits_a_parseable_config_with_subsections_quoted() {
        let rendered = render_git_config(&[
            setting("user.email", "me@example.test"),
            setting("http.https://git.example.test/.sslverify", "true"),
        ]);
        assert!(
            rendered.contains("[user]\n\temail = \"me@example.test\"\n"),
            "got:\n{rendered}"
        );
        assert!(
            rendered.contains("[http \"https://git.example.test/\"]\n\tsslverify = \"true\"\n"),
            "a subsection keeps its dots instead of being re-split into the wrong key:\n{rendered}"
        );
    }

    #[test]
    fn render_escapes_a_quote_inside_a_subsection_name() {
        let rendered = render_git_config(&[setting(r#"url.https://x/"a.insteadof"#, "https://x/")]);
        assert!(
            rendered.contains(r#"[url "https://x/\"a"]"#),
            "an unescaped quote closes the subsection early and git refuses the whole file:\n{rendered}"
        );
    }

    #[test]
    fn render_escapes_a_value_that_would_otherwise_break_the_file() {
        let rendered = render_git_config(&[setting("alias.lg", "!f() {\n\techo \"hi\"\n}; f")]);
        assert!(
            !rendered.contains("\n\techo"),
            "a newline must not end the line early:\n{rendered}"
        );
        assert!(rendered.contains("\\n"), "got:\n{rendered}");
        assert!(rendered.contains("\\\"hi\\\""), "got:\n{rendered}");
    }

    #[test]
    fn render_skips_a_key_with_no_section_because_git_cannot_express_it() {
        assert_eq!(render_git_config(&[setting("bare", "x")]), "");
    }

    #[derive(Default)]
    struct FakeFacts {
        stdout: String,
        status: i32,
        fail: bool,
        env: Option<String>,
        calls: std::sync::Mutex<Vec<(String, Vec<String>)>>,
    }

    impl HostFacts for FakeFacts {
        fn run(&self, program: &str, args: &[String]) -> io::Result<HostCommandOutput> {
            self.calls
                .lock()
                .unwrap()
                .push((program.to_string(), args.to_vec()));
            if self.fail {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no such program"));
            }
            Ok(HostCommandOutput {
                status: self.status,
                stdout: self.stdout.clone(),
            })
        }

        fn env(&self, _name: &str) -> Option<String> {
            self.env.clone()
        }
    }

    fn command_spec() -> SocketSpec {
        SocketSpec {
            locate: Locate::Command(vec!["some-locate".into(), "--dir".into()]),
            target: "~/.gnupg/S.gpg-agent".into(),
        }
    }

    #[test]
    fn locate_runs_the_command_and_trims_the_path_it_prints() {
        let facts = FakeFacts {
            stdout: "/run/user/501/agent.sock\n".into(),
            ..Default::default()
        };
        assert_eq!(
            locate_socket(&facts, &command_spec()).as_deref(),
            Some("/run/user/501/agent.sock")
        );
        assert_eq!(
            facts.calls.lock().unwrap().as_slice(),
            [("some-locate".to_string(), vec!["--dir".to_string()])]
        );
    }

    #[test]
    fn locate_finds_nothing_when_the_command_is_missing_fails_or_prints_nothing() {
        let missing = FakeFacts {
            fail: true,
            ..Default::default()
        };
        assert!(locate_socket(&missing, &command_spec()).is_none());
        let failed = FakeFacts {
            status: 2,
            stdout: "/ignored".into(),
            ..Default::default()
        };
        assert!(locate_socket(&failed, &command_spec()).is_none());
        let silent = FakeFacts {
            stdout: "  \n".into(),
            ..Default::default()
        };
        assert!(locate_socket(&silent, &command_spec()).is_none());
    }

    #[test]
    fn locate_reads_an_env_var_when_the_spec_names_one() {
        let spec = SocketSpec {
            locate: Locate::Env("SOME_AUTH_SOCK".into()),
            target: "~/.ssh/lns-agent.sock".into(),
        };
        let set = FakeFacts {
            env: Some("/run/user/501/ssh-agent.sock".into()),
            ..Default::default()
        };
        assert_eq!(
            locate_socket(&set, &spec).as_deref(),
            Some("/run/user/501/ssh-agent.sock")
        );
        assert!(locate_socket(&FakeFacts::default(), &spec).is_none());
    }

    #[test]
    fn locate_finds_nothing_when_the_spec_names_an_empty_command() {
        let spec = SocketSpec {
            locate: Locate::Command(Vec::new()),
            target: "~/x".into(),
        };
        assert!(locate_socket(&FakeFacts::default(), &spec).is_none());
    }

    #[test]
    fn the_signing_format_decides_which_socket_is_forwarded() {
        let entry = lns_policy::host_access::find("git-signing").expect("bundled");
        assert!(socket_spec_for(entry, SigningNeed::Off).is_none());
        assert_eq!(
            socket_spec_for(entry, SigningNeed::Openpgp),
            Some(&entry.openpgp_socket)
        );
        assert_eq!(
            socket_spec_for(entry, SigningNeed::Ssh),
            Some(&entry.ssh_socket)
        );
    }
}
