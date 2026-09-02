use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// What the methods this run granted put in its environment (§3.2.4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectorEnv {
    /// Each variable a granted method fills with a placeholder, by the connector that fills it: no other source may set it.
    pub filled: BTreeMap<String, Filled>,
    /// What a granted method's own `env` block sets outright — non-secret config beside its credentials, carrying a plain value rather than a marker.
    pub granted_env: BTreeMap<String, String>,
}

impl ConnectorEnv {
    /// The connector filling a variable, unless the value there is already the marker it would write — an exec joins a run that already holds it, and calling that a displaced value would name a remedy for something nobody did.
    fn displaced_by(&self, key: &str, value: &str) -> Option<&str> {
        self.filled
            .get(key)
            .filter(|_| claimable(key))
            .filter(|filled| filled.placeholder != value)
            .map(|filled| filled.connector.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filled {
    pub connector: String,
    /// The marker the workload reads there; the boundary substitutes the real value on the wire.
    pub placeholder: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkloadEnv {
    pub env: Vec<String>,
    /// The variables whose value a grant displaced; the caller names each one on the run's own log.
    pub refused: Vec<Refused>,
}

/// Where the value a grant displaced came from, because each has its own remedy and naming the wrong one sends the user to a file that does not set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EnvSource {
    /// The command line — `-e`, or an env file the CLI folded into it.
    Flag,
    /// The sandbox definition's own `spec.env`.
    Definition,
    /// The run's own environment, as an exec joining it finds it — settled when the run started, and not this caller's to change.
    Run,
    /// The image's `ENV`.
    Image,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub key: String,
    pub source: EnvSource,
    /// The connector whose grant fills the variable, carried here because it is the only thing the remedy can name and nothing else knows it by the time the refusal is read.
    pub connector: String,
}

/// Names the grant that claimed the variable and the one remedy that fits where the value came from.
pub fn refusal_warning(refused: &Refused) -> String {
    let Refused { key, connector, .. } = refused;
    // `lns connector forget`, not `disconnect`: disconnect drops a connection and leaves the grant, so it would leave the variable filled and the message would repeat on the next start.
    let retract = format!("`lns connector forget {connector} --run <run>`");
    match refused.source {
        EnvSource::Flag => format!(
            "{key} holds {connector}'s placeholder for this run, not your value — run {retract}, or drop the -e"
        ),
        EnvSource::Definition => format!(
            "{key} holds {connector}'s placeholder for this run, not your value — run {retract}, or drop {key} from spec.env"
        ),
        EnvSource::Run => format!(
            "{key} holds {connector}'s placeholder for this run, not the value the run started with — run {retract} and start the run again"
        ),
        EnvSource::Image => format!(
            "{key} holds {connector}'s placeholder for this run, not the image's value — run {retract} to use the image's value"
        ),
    }
}

/// One refusal per variable, against the source the user can most directly act on: a flag they typed outranks their definition, which outranks the image they did not write. Every source is collected before this decides, so no two of them warn about one variable.
pub fn one_refusal_per_variable(refusals: impl IntoIterator<Item = Refused>) -> Vec<Refused> {
    let mut best: std::collections::BTreeMap<String, Refused> = Default::default();
    for refused in refusals {
        match best.get_mut(&refused.key) {
            Some(held) if held.source <= refused.source => {}
            Some(held) => *held = refused,
            None => {
                best.insert(refused.key.clone(), refused);
            }
        }
    }
    best.into_values().collect()
}

pub const REDACTED_ENV_VALUE: &str = "<redacted>";

/// The audit record needs the set of injected var names, never their values — a mistyped secret in `-e KEY=VALUE` must not become durable audit data, so every value is redacted.
pub fn injected_env(user_env: &[String]) -> Option<Map<String, Value>> {
    let mut env = Map::new();
    for kv in user_env {
        let Some((k, _v)) = kv.split_once('=') else {
            continue;
        };
        env.insert(k.to_string(), Value::String(REDACTED_ENV_VALUE.to_string()));
    }
    if env.is_empty() { None } else { Some(env) }
}

/// The run's own environment entries a grant does not fill, beside the ones it does. Applied before the environment travels anywhere, so what the supervisor and the audit chain see is what actually entered the sandbox.
///
/// `source_of` says where each key came from, because a run's environment is its definition's `spec.env` and the command line in one vector by the time it reaches here, and the two have different remedies.
pub fn without_what_a_grant_fills(
    run_env: &[String],
    connectors: &ConnectorEnv,
    source_of: impl Fn(&str) -> EnvSource,
) -> (Vec<String>, Vec<Refused>) {
    let mut kept = Vec::new();
    let mut refused = Vec::new();
    for kv in run_env {
        let (key, value) = kv.split_once('=').unwrap_or((kv, ""));
        let Some(connector) = connectors.displaced_by(key, value) else {
            kept.push(kv.clone());
            continue;
        };
        refused.push(Refused {
            key: key.to_string(),
            source: source_of(key),
            connector: connector.to_string(),
        });
    }
    (kept, refused)
}

/// Whether a key came from the command line, which is what tells a refusal to say "drop the -e" rather than "drop it from spec.env".
pub fn source_among(command_line: &[String]) -> impl Fn(&str) -> EnvSource + '_ {
    move |key| {
        let typed = command_line
            .iter()
            .any(|kv| kv.split_once('=').is_some_and(|(k, _)| k == key));
        if typed {
            EnvSource::Flag
        } else {
            EnvSource::Definition
        }
    }
}

/// A variable a granted method fills holds that method's placeholder and nothing else (§3.2.4).
fn compose_workload_env(
    image_env: Option<&[String]>,
    user_env: &[String],
    connectors: &ConnectorEnv,
) -> WorkloadEnv {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut refused: Vec<Refused> = Vec::new();
    let mut refuse = |key: &str, value: &str, source: EnvSource| {
        let Some(connector) = connectors.displaced_by(key, value) else {
            return false;
        };
        refused.push(Refused {
            key: key.to_string(),
            source,
            connector: connector.to_string(),
        });
        true
    };
    if let Some(image) = image_env {
        for kv in image {
            if let Some((k, v)) = kv.split_once('=').filter(|(k, _)| !is_internal(k))
                && !refuse(k, v, EnvSource::Image)
            {
                upsert(&mut entries, k, v);
            }
        }
    }
    for kv in user_env {
        let Some((k, v)) = kv.split_once('=') else {
            continue;
        };
        if is_internal(k) || refuse(k, v, EnvSource::Flag) {
            continue;
        }
        upsert(&mut entries, k, v);
    }
    for (key, filled) in connectors.filled.iter().filter(|(k, _)| claimable(k)) {
        upsert(&mut entries, key, &filled.placeholder);
    }
    WorkloadEnv {
        env: entries
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect(),
        refused,
    }
}

/// The PATH lns-init hands the guest; the workload inherits the same one so declared tools never silently drop the guest-tools dir the kernel PATH carries.
pub const GUEST_DEFAULT_PATH: &str =
    "/.lens/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// What a run's declared tools contribute to a workload's environment: the dirs that go on `PATH`, and the vars a tool needs to find its own payload (rustup's proxies read `RUSTUP_HOME`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolRuntime {
    pub bin_paths: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub const SUPERVISOR_PTY_OPT_IN_TERM: &str = "xterm-256color";

/// The vars the supervisor handshake owns; they describe the workload's own session, so replaying them into an exec would tell a piped command it has a colour terminal and hand it the workload's command line.
fn is_session_only(entry: &str) -> bool {
    let (key, value) = entry.split_once('=').unwrap_or((entry, ""));
    match key {
        "AGENT_COMMAND" | "WORKSPACE_PATH" => true,
        // Only the value we injected: a workload that declares its own TERM meant it.
        "TERM" => value == SUPERVISOR_PTY_OPT_IN_TERM,
        _ => key.starts_with("LENS_SANDBOX_"),
    }
}

/// The CA bundle paths a workload's runtimes are given, so a curl, git, python or node command in an exec trusts the proxy the way the workload does.
fn ca_bundle_env() -> [(&'static str, &'static str); 5] {
    [
        ("SSL_CERT_FILE", lns_session::SYSTEM_CA_BUNDLE_PATH),
        ("REQUESTS_CA_BUNDLE", lns_session::SYSTEM_CA_BUNDLE_PATH),
        ("NODE_EXTRA_CA_CERTS", lns_session::SYSTEM_CA_BUNDLE_PATH),
        ("CURL_CA_BUNDLE", lns_session::SYSTEM_CA_BUNDLE_PATH),
        ("GIT_SSL_CAINFO", lns_session::SYSTEM_CA_BUNDLE_PATH),
    ]
}

/// The proxy vars the supervisor hands its workload, so an HTTP client in an exec takes the route the agent's does rather than one the gate never sees.
fn proxy_env() -> [(&'static str, &'static str); 6] {
    [
        ("HTTPS_PROXY", lns_session::GUEST_PROXY_URL),
        ("https_proxy", lns_session::GUEST_PROXY_URL),
        ("HTTP_PROXY", lns_session::GUEST_PROXY_URL),
        ("http_proxy", lns_session::GUEST_PROXY_URL),
        ("NO_PROXY", lns_session::GUEST_NO_PROXY),
        ("no_proxy", lns_session::GUEST_NO_PROXY),
    ]
}

/// What an `lns exec` into a live run joins: the run's own resolved environment, then the caller's additions, what its granted methods set, its declared tools, and the proxy and CA bundle the workload is given.
pub fn exec_session_env(
    exec_environment: &crate::run_registry::ExecEnvironment,
    exec_env: &[String],
    connectors: &ConnectorEnv,
) -> WorkloadEnv {
    let joined: Vec<String> = exec_environment
        .session_env
        .iter()
        .filter(|entry| !is_session_only(entry))
        .filter(|entry| keeps_identity(entry, &exec_environment.declared_identity_keys))
        .cloned()
        .collect();
    let caller: Vec<String> = exec_env
        .iter()
        .filter(|entry| !is_session_only(entry))
        .cloned()
        .collect();
    let mut composed = compose_workload_env(Some(&joined), &caller, connectors);
    // The boot policy frame layers this over the guest's own environment, so an exec joining the same run must too.
    for (key, value) in connectors.granted_env.iter().filter(|(k, _)| claimable(k)) {
        overwrite(&mut composed.env, key, value);
    }
    // The composer's image slot here is the run's own environment, and telling this caller to edit an image that set nothing sends them nowhere.
    for refused in &mut composed.refused {
        if refused.source == EnvSource::Image {
            refused.source = EnvSource::Run;
        }
    }
    compose_guest_tool_env(&mut composed.env, &exec_environment.tools);
    for (key, value) in proxy_env().into_iter().chain(ca_bundle_env()) {
        overwrite(&mut composed.env, key, value);
    }
    composed
}

pub const IDENTITY_ENV_KEYS: [&str; 2] = ["HOME", "USER"];

/// Which identity vars the author set via `-e`/spec env — the same test that gates `LENS_SANDBOX_WORKLOAD_*` on the primary.
pub fn declared_identity_keys(user_env: &[String]) -> Vec<String> {
    IDENTITY_ENV_KEYS
        .iter()
        .filter(|key| declared(user_env, key).is_some())
        .map(|key| key.to_string())
        .collect()
}

fn keeps_identity(entry: &str, declared_keys: &[String]) -> bool {
    let key = entry.split_once('=').map(|(k, _)| k).unwrap_or(entry);
    !IDENTITY_ENV_KEYS.contains(&key) || declared_keys.iter().any(|d| d == key)
}

fn overwrite(env: &mut Vec<String>, key: &str, value: &str) {
    let entry = format!("{key}={value}");
    match env
        .iter_mut()
        .find(|kv| kv.split_once('=').is_some_and(|(k, _)| k == key))
    {
        Some(slot) => *slot = entry,
        None => env.push(entry),
    }
}

/// Everything the declared tools add to a guest's environment. `lns exec` into the same guest applies this same function, so what an exec session resolves matches the workload's.
pub fn compose_guest_tool_env(env: &mut Vec<String>, tools: &ToolRuntime) {
    compose_guest_path(env, &tools.bin_paths);
    for (name, value) in &tools.env {
        // A tool's own env only fills what nothing else set: a workload that declares its own CARGO_HOME means it.
        let already_set = env
            .iter()
            .any(|kv| kv.split_once('=').is_some_and(|(key, _)| key == name));
        if already_set {
            continue;
        }
        env.push(format!("{name}={value}"));
    }
}

/// The one PATH rule for a guest: declared tool dirs win over whatever the image (or `-e`) composed, and no blank segment survives either way. `lns exec` into the same guest applies this same rule, so the tool dirs it resolves match the workload's — two copies of this drift into exactly the "run finds node, exec does not" bug.
pub fn compose_guest_path(env: &mut Vec<String>, tool_bin_paths: &[String]) {
    let declared = env.iter().find_map(|kv| kv.strip_prefix("PATH="));
    // Nothing to prepend and nothing to sanitize: lns-init already exports the default, so composing one here would only duplicate it.
    if tool_bin_paths.is_empty() && declared.is_none() {
        return;
    }
    let existing = declared
        .filter(|path| !path.is_empty())
        .unwrap_or(GUEST_DEFAULT_PATH)
        .to_string();
    let value = format!("PATH={}", join_path(tool_bin_paths, &existing));
    match env.iter_mut().find(|kv| kv.starts_with("PATH=")) {
        Some(slot) => *slot = value,
        None => env.push(value),
    }
}

/// An empty PATH segment is the current directory to `execvp`, so a blank entry — from `PATH=` in the image config, or a stray `::` — must never survive into the workload's PATH.
fn join_path(tool_bin_paths: &[String], existing: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    tool_bin_paths
        .iter()
        .map(String::as_str)
        .chain(existing.split(':'))
        .filter(|segment| !segment.is_empty())
        .filter(|segment| seen.insert(*segment))
        .collect::<Vec<&str>>()
        .join(":")
}

pub fn run_workload_env(
    image_env: Option<&[String]>,
    user_env: &[String],
    agent_command: Option<&str>,
    workdir: Option<&str>,
    tools: &ToolRuntime,
    connectors: &ConnectorEnv,
) -> WorkloadEnv {
    let mut composed = compose_workload_env(image_env, user_env, connectors);
    // The broker's last-wins putenv would otherwise let the image PATH shadow the tool dirs.
    compose_guest_tool_env(&mut composed.env, tools);
    if let Some(agent_command) = agent_command {
        // Internal vars go last: the broker's last-wins putenv means a user `-e TERM=…` can't clobber the supervisor PTY opt-in, the command, or the agent cwd.
        composed.env.push(format!("AGENT_COMMAND={agent_command}"));
        if let Some(workdir) = workdir {
            composed.env.push(format!("WORKSPACE_PATH={workdir}"));
        }
        // nexus-agent-sandbox treats TERM unset or "linux" as "no PTY"; xterm-256color opts it into the PTY path needed for `lns run -it --policy`.
        composed
            .env
            .push(format!("TERM={SUPERVISOR_PTY_OPT_IN_TERM}"));
        // Only what the author declared: an image's own ENV HOME must not outrank the run-as identity, or an unprivileged workload inherits a home it cannot write.
        for key in ["HOME", "USER"] {
            if let Some(value) = declared(user_env, key) {
                composed
                    .env
                    .push(format!("LENS_SANDBOX_WORKLOAD_{key}={value}"));
            }
        }
    }
    composed
}

/// A connector may not take a variable the guest itself composes: the supervisor's own channel, the PATH that finds every binary the workload runs, or the identity a session lands in.
fn claimable(key: &str) -> bool {
    !is_internal(key) && key != "PATH" && !IDENTITY_ENV_KEYS.contains(&key)
}

/// The supervisor reads its own instructions out of this prefix, so no image or `-e` may write one.
fn is_internal(key: &str) -> bool {
    key.starts_with("LENS_SANDBOX_")
}

fn declared<'a>(user_env: &'a [String], key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    user_env
        .iter()
        .rev()
        .find_map(|entry| entry.strip_prefix(&prefix))
}

fn upsert(entries: &mut Vec<(String, String)>, key: &str, value: &str) {
    match entries.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value.to_string(),
        None => entries.push((key.to_string(), value.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(c: &WorkloadEnv) -> &[String] {
        &c.env
    }

    fn flag(key: &str) -> Refused {
        refused_from(key, EnvSource::Flag)
    }

    const MARKER: &str = "some_LNSPLACEHOLDER0000000000";

    fn fills(key: &str) -> ConnectorEnv {
        ConnectorEnv {
            filled: [(
                key.to_string(),
                Filled {
                    connector: "some-provider".to_string(),
                    placeholder: MARKER.to_string(),
                },
            )]
            .into(),
            ..Default::default()
        }
    }

    fn refused_from(key: &str, source: EnvSource) -> Refused {
        Refused {
            key: key.to_string(),
            source,
            connector: "some-provider".to_string(),
        }
    }

    fn tools(bin_paths: &[&str]) -> ToolRuntime {
        ToolRuntime {
            bin_paths: bin_paths.iter().map(|p| p.to_string()).collect(),
            env: Vec::new(),
        }
    }

    #[test]
    fn a_user_override_of_a_variable_a_grant_fills_is_dropped_and_named() {
        // The workload reads a placeholder for that variable and the boundary substitutes the real value; a -e would put the real secret inside the sandbox, which is the one thing the design does not do.
        let c = compose_workload_env(
            None,
            &["SOME_TOKEN=sk-live-real".into(), "SAFE=1".into()],
            &fills("SOME_TOKEN"),
        );
        assert_eq!(
            env(&c),
            ["SAFE=1", &format!("SOME_TOKEN={MARKER}")],
            "the real secret must not reach it, and the marker the boundary substitutes for must"
        );
        assert_eq!(c.refused, [flag("SOME_TOKEN")]);
        assert_eq!(
            refusal_warning(&flag("SOME_TOKEN")),
            "SOME_TOKEN holds some-provider's placeholder for this run, not your value — run `lns connector forget some-provider --run <run>`, or drop the -e"
        );
    }

    #[test]
    fn a_variable_no_grant_fills_is_carried_as_it_always_was() {
        let c = compose_workload_env(None, &["SAFE=1".into()], &fills("OTHER_TOKEN"));
        assert_eq!(env(&c), ["SAFE=1", &format!("OTHER_TOKEN={MARKER}")]);
        assert!(c.refused.is_empty());
    }

    #[test]
    fn an_image_env_var_a_grant_fills_is_dropped_too() {
        // The image is not the user, but a value it ships under that name shadows the placeholder exactly the same way.
        let c = compose_workload_env(
            Some(&["SOME_TOKEN=from-image".into()]),
            &[],
            &fills("SOME_TOKEN"),
        );
        assert_eq!(env(&c), [format!("SOME_TOKEN={MARKER}")]);
        assert_eq!(c.refused, [refused_from("SOME_TOKEN", EnvSource::Image)]);
        assert_eq!(
            refusal_warning(&c.refused[0]),
            "SOME_TOKEN holds some-provider's placeholder for this run, not the image's value — run `lns connector forget some-provider --run <run>` to use the image's value",
            "there is no -e here, so telling the user to drop one sends them nowhere"
        );
    }

    #[test]
    fn a_variable_more_than_one_source_set_is_named_once_against_the_one_to_act_on() {
        // Two warnings for one variable is two remedies, and the reader cannot tell which of them is theirs to take.
        let both = one_refusal_per_variable([
            refused_from("SOME_TOKEN", EnvSource::Image),
            flag("SOME_TOKEN"),
            refused_from("OTHER_TOKEN", EnvSource::Image),
        ]);
        assert_eq!(
            both,
            [
                refused_from("OTHER_TOKEN", EnvSource::Image),
                flag("SOME_TOKEN"),
            ],
            "the flag the user typed outranks the image they did not write"
        );
    }

    #[test]
    fn a_definition_outranks_the_image_and_yields_to_a_flag() {
        let definition = refused_from("SOME_TOKEN", EnvSource::Definition);
        let image = refused_from("SOME_TOKEN", EnvSource::Image);
        assert_eq!(
            one_refusal_per_variable([image.clone(), definition.clone()]),
            vec![definition.clone()]
        );
        assert_eq!(
            one_refusal_per_variable([definition, flag("SOME_TOKEN"), image]),
            vec![flag("SOME_TOKEN")]
        );
    }

    #[test]
    fn what_a_grant_fills_is_dropped_before_the_environment_travels_and_named_by_its_source() {
        // The audit chain records what entered the sandbox, so a variable dropped here must not reach it as an injection — and a run's environment is its definition and its flags in one vector, only one of which is a flag to drop.
        let typed: Vec<String> = vec!["SOME_TOKEN=sk-live-real".into()];
        let (kept, refused) = without_what_a_grant_fills(
            &[
                "SOME_TOKEN=sk-live-real".into(),
                "SAFE=1".into(),
                "FROM_SPEC=1".into(),
                "NOTANASSIGNMENT".into(),
            ],
            &ConnectorEnv {
                filled: ["SOME_TOKEN", "FROM_SPEC"]
                    .map(|key| {
                        (
                            key.to_string(),
                            Filled {
                                connector: "some-provider".to_string(),
                                placeholder: MARKER.to_string(),
                            },
                        )
                    })
                    .into(),
                ..Default::default()
            },
            source_among(&typed),
        );
        assert_eq!(kept, ["SAFE=1", "NOTANASSIGNMENT"]);
        assert_eq!(
            refused,
            [
                flag("SOME_TOKEN"),
                refused_from("FROM_SPEC", EnvSource::Definition),
            ]
        );
        assert_eq!(
            refusal_warning(&refused[1]),
            "FROM_SPEC holds some-provider's placeholder for this run, not your value — run `lns connector forget some-provider --run <run>`, or drop FROM_SPEC from spec.env",
            "no -e set this one, so telling the user to drop one sends them nowhere"
        );
        assert_eq!(
            injected_env(&kept).and_then(|e| e.get("SOME_TOKEN").cloned()),
            None
        );
    }

    #[test]
    fn a_granted_credential_puts_its_marker_in_the_variable_that_declares_it() {
        // §3.2.4: the variable holds the placeholder and the boundary substitutes the real value. Leaving it unset tells a tool that reads it that it is signed out, and it never makes the request the grant was given for.
        let c = compose_workload_env(None, &[], &fills("SOME_TOKEN"));
        assert_eq!(env(&c), [format!("SOME_TOKEN={MARKER}")]);
        assert!(
            c.refused.is_empty(),
            "nothing was displaced: {:?}",
            c.refused
        );
    }

    #[test]
    fn an_exec_joins_the_marker_the_run_already_holds_without_calling_it_a_displaced_value() {
        // The run composed this marker at its start, so an exec that warned about it would name a remedy for something nobody did.
        let run = crate::run_registry::ExecEnvironment {
            session_env: vec![format!("SOME_TOKEN={MARKER}")],
            ..Default::default()
        };

        let joining = exec_session_env(&run, &[], &fills("SOME_TOKEN"));

        assert!(joining.env.contains(&format!("SOME_TOKEN={MARKER}")));
        assert!(joining.refused.is_empty(), "got: {:?}", joining.refused);
    }

    #[test]
    fn no_connector_may_write_the_supervisors_own_control_channel() {
        // The supervisor reads its instructions out of this prefix, and an image and a `-e` are already refused it. A grant is consent to a credential, not to the channel the supervisor takes its orders from.
        let c = run_workload_env(
            None,
            &[],
            None,
            None,
            &Default::default(),
            &fills("LENS_SANDBOX_WORKLOAD_HOME"),
        );
        assert!(
            !c.env.iter().any(|kv| kv.starts_with("LENS_SANDBOX_")),
            "a connector reached the supervisor's control channel: {:?}",
            c.env
        );
    }

    #[test]
    fn no_connector_may_claim_the_variable_that_finds_the_guests_binaries() {
        // A placeholder passes as a path — 16 characters carrying "lns" — so a credential named PATH would otherwise leave the workload one directory to find every binary in.
        let c = run_workload_env(None, &[], None, None, &Default::default(), &fills("PATH"));
        assert!(
            !c.env.iter().any(|kv| kv.starts_with("PATH=")),
            "the guest default PATH must stand: {:?}",
            c.env
        );
    }

    #[test]
    fn a_connector_that_may_not_fill_path_may_not_empty_it_either() {
        // Refusing a value it cannot replace leaves the run without the directory the user asked for, and the warning names a remedy for a variable the connector may not fill.
        let c = compose_workload_env(None, &["PATH=/opt/mybin".into()], &fills("PATH"));
        assert_eq!(env(&c), ["PATH=/opt/mybin"]);
        assert!(c.refused.is_empty(), "got: {:?}", c.refused);
    }

    #[test]
    fn no_connector_may_claim_the_identity_the_guest_composes() {
        // An exec strips HOME and USER so the guest identity fills them. A placeholder refilling that slot tells the broker the author declared a home, and the exec lands on a marker string.
        let run = crate::run_registry::ExecEnvironment {
            session_env: vec!["HOME=/home/node".into(), "USER=node".into()],
            ..Default::default()
        };

        let joining = exec_session_env(&run, &[], &fills("HOME"));

        assert!(
            !joining.env.iter().any(|kv| kv.starts_with("HOME=")),
            "the guest identity owns this slot: {:?}",
            joining.env
        );
    }

    #[test]
    fn the_warning_says_the_variable_holds_a_placeholder_rather_than_calling_it_unset() {
        // The variable IS set — to the marker the boundary substitutes for. Telling the user it is not set sends someone debugging a tool that reads it to look for a variable that is there.
        let c = compose_workload_env(
            None,
            &["SOME_TOKEN=sk-live-real".into()],
            &fills("SOME_TOKEN"),
        );
        assert_eq!(env(&c), [format!("SOME_TOKEN={MARKER}")]);
        for source in [
            EnvSource::Flag,
            EnvSource::Definition,
            EnvSource::Run,
            EnvSource::Image,
        ] {
            let warning = refusal_warning(&refused_from("SOME_TOKEN", source));
            assert!(
                !warning.contains("not set"),
                "the variable holds the placeholder: {warning}"
            );
            assert!(
                warning.contains("placeholder"),
                "and the warning must say so: {warning}"
            );
        }
    }

    #[test]
    fn an_exec_joins_the_plain_env_a_granted_method_supplies() {
        // §3.2.4 promises a method's own `env` reaches the next exec, and an exec composes its environment here rather than from the boot frame that carries it.
        let run = crate::run_registry::ExecEnvironment::default();
        let connectors = ConnectorEnv {
            granted_env: [("SOME_PROVIDER_REGION".to_string(), "eu".to_string())].into(),
            ..Default::default()
        };

        let joining = exec_session_env(&run, &[], &connectors);

        assert!(
            joining.env.contains(&"SOME_PROVIDER_REGION=eu".to_string()),
            "got: {:?}",
            joining.env
        );
    }

    #[test]
    fn what_a_granted_method_sets_outranks_the_value_the_run_started_with() {
        // The boot frame layers this over the guest's own environment, so an exec that only filled an unset variable would disagree with the start of the same run.
        let run = crate::run_registry::ExecEnvironment {
            session_env: vec!["SOME_PROVIDER_REGION=us".into()],
            ..Default::default()
        };
        let connectors = ConnectorEnv {
            granted_env: [("SOME_PROVIDER_REGION".to_string(), "eu".to_string())].into(),
            ..Default::default()
        };

        let joining = exec_session_env(&run, &[], &connectors);

        assert!(
            joining.env.contains(&"SOME_PROVIDER_REGION=eu".to_string())
                && !joining.env.contains(&"SOME_PROVIDER_REGION=us".to_string()),
            "got: {:?}",
            joining.env
        );
    }

    #[test]
    fn a_granted_methods_env_may_not_claim_what_the_guest_composes_either() {
        // A grant consents to the method's config, not to the PATH that finds every binary the workload runs.
        let connectors = ConnectorEnv {
            granted_env: [("PATH".to_string(), "/only/this".to_string())].into(),
            ..Default::default()
        };

        let joining = exec_session_env(&Default::default(), &[], &connectors);

        assert!(
            !joining.env.iter().any(|kv| kv == "PATH=/only/this"),
            "the guest composes PATH: {:?}",
            joining.env
        );
    }

    #[test]
    fn the_runs_proxy_still_outranks_what_a_granted_method_sets() {
        // A route around the gate is not a connector's to choose, any more than an exec caller's.
        let connectors = ConnectorEnv {
            granted_env: [("HTTPS_PROXY".to_string(), "http://elsewhere".to_string())].into(),
            ..Default::default()
        };

        let joining = exec_session_env(&Default::default(), &[], &connectors);

        assert!(
            joining
                .env
                .contains(&format!("HTTPS_PROXY={}", lns_session::GUEST_PROXY_URL)),
            "got: {:?}",
            joining.env
        );
    }

    #[test]
    fn a_single_user_var_is_carried() {
        let c = compose_workload_env(
            None,
            &["CLAUDE_CODE_USE_BEDROCK=1".into()],
            &Default::default(),
        );
        assert_eq!(env(&c), ["CLAUDE_CODE_USE_BEDROCK=1"]);
    }

    #[test]
    fn multiple_user_vars_are_all_carried() {
        let c = compose_workload_env(None, &["A=1".into(), "B=2".into()], &Default::default());
        assert!(c.env.contains(&"A=1".to_string()));
        assert!(c.env.contains(&"B=2".to_string()));
    }

    #[test]
    fn value_is_split_on_the_first_equals_only() {
        let c = compose_workload_env(None, &["DSN=user=admin;pw=x".into()], &Default::default());
        assert_eq!(env(&c), ["DSN=user=admin;pw=x"]);
    }

    #[test]
    fn an_empty_value_is_preserved() {
        let c = compose_workload_env(None, &["FEATURE_X=".into()], &Default::default());
        assert_eq!(env(&c), ["FEATURE_X="]);
    }

    #[test]
    fn user_value_overrides_the_image_env() {
        let c = compose_workload_env(
            Some(&["PORT=3003".into()]),
            &["PORT=4000".into()],
            &Default::default(),
        );
        assert_eq!(env(&c), ["PORT=4000"]);
    }

    #[test]
    fn image_vars_not_overridden_are_kept() {
        let c = compose_workload_env(
            Some(&["FOO=bar".into()]),
            &["BAZ=1".into()],
            &Default::default(),
        );
        assert!(c.env.contains(&"FOO=bar".to_string()));
        assert!(c.env.contains(&"BAZ=1".to_string()));
    }

    #[test]
    fn a_malformed_user_entry_without_equals_is_ignored() {
        let c = compose_workload_env(None, &["NOTANASSIGNMENT".into()], &Default::default());
        assert!(c.env.is_empty());
    }

    #[test]
    fn a_malformed_image_entry_without_equals_is_ignored() {
        let c = compose_workload_env(Some(&["JUSTAKEY".into()]), &[], &Default::default());
        assert!(c.env.is_empty());
    }

    #[test]
    fn run_workload_env_carries_user_env_for_an_unsupervised_run() {
        let c = run_workload_env(
            None,
            &["FOO=bar".into()],
            None,
            None,
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(c.env, ["FOO=bar"], "user -e must reach a policy-less run");
    }

    #[test]
    fn run_workload_env_adds_no_supervisor_vars_when_unsupervised() {
        let c = run_workload_env(
            None,
            &["FOO=bar".into()],
            None,
            None,
            &Default::default(),
            &Default::default(),
        );
        assert!(
            !c.env
                .iter()
                .any(|e| e.starts_with("AGENT_COMMAND=") || e.starts_with("TERM=")),
            "no agent command means no supervisor vars: {:?}",
            c.env
        );
    }

    #[test]
    fn run_workload_env_appends_agent_command_and_term_when_supervised() {
        let c = run_workload_env(
            None,
            &["FOO=bar".into()],
            Some("echo hi"),
            None,
            &Default::default(),
            &Default::default(),
        );
        assert!(c.env.contains(&"FOO=bar".to_string()), "got: {:?}", c.env);
        assert!(c.env.contains(&"AGENT_COMMAND=echo hi".to_string()));
        assert!(c.env.contains(&"TERM=xterm-256color".to_string()));
    }

    #[test]
    fn run_workload_env_keeps_supervisor_term_after_a_user_term_so_it_cannot_be_clobbered() {
        let c = run_workload_env(
            None,
            &["TERM=dumb".into()],
            Some("sh"),
            None,
            &Default::default(),
            &Default::default(),
        );
        let last_term = c.env.iter().rposition(|e| e.starts_with("TERM=")).unwrap();
        assert_eq!(c.env[last_term], "TERM=xterm-256color");
    }

    #[test]
    fn run_workload_env_pins_workspace_path_for_a_supervised_run() {
        let c = run_workload_env(
            None,
            &[],
            Some("sh"),
            Some("/app"),
            &Default::default(),
            &Default::default(),
        );
        assert!(
            c.env.contains(&"WORKSPACE_PATH=/app".to_string()),
            "got: {:?}",
            c.env
        );
    }

    #[test]
    fn run_workload_env_omits_workspace_path_without_a_workdir() {
        let c = run_workload_env(
            None,
            &[],
            Some("sh"),
            None,
            &Default::default(),
            &Default::default(),
        );
        assert!(
            !c.env.iter().any(|e| e.starts_with("WORKSPACE_PATH=")),
            "got: {:?}",
            c.env
        );
    }

    #[test]
    fn run_workload_env_keeps_the_workdir_workspace_path_after_a_user_override() {
        let c = run_workload_env(
            None,
            &["WORKSPACE_PATH=/evil".into()],
            Some("sh"),
            Some("/app"),
            &Default::default(),
            &Default::default(),
        );
        let last = c
            .env
            .iter()
            .rposition(|e| e.starts_with("WORKSPACE_PATH="))
            .unwrap();
        assert_eq!(
            c.env[last], "WORKSPACE_PATH=/app",
            "the broker's last-wins putenv must land on the -w dir: {:?}",
            c.env
        );
    }

    #[test]
    fn run_workload_env_adds_no_workspace_path_when_unsupervised() {
        let c = run_workload_env(
            None,
            &[],
            None,
            Some("/app"),
            &Default::default(),
            &Default::default(),
        );
        assert!(
            c.env.is_empty(),
            "an unsupervised run's cwd travels via the session, not env: {:?}",
            c.env
        );
    }

    #[test]
    fn an_exec_cannot_set_from_outside_what_the_run_itself_refused() {
        // `lns exec` joins a live run; letting its caller replace a value the run displaced would put the real secret where the workload reads the placeholder.
        let run = crate::run_registry::ExecEnvironment::default();

        let joining = exec_session_env(
            &run,
            &["SOME_TOKEN=sk-live-real".into(), "SAFE=1".into()],
            &fills("SOME_TOKEN"),
        );

        let env = &joining.env;
        assert!(
            env.contains(&format!("SOME_TOKEN={MARKER}")),
            "the run refused the caller's value, and an exec is not a way around that: it reads the same marker the workload does: {env:?}"
        );
        assert!(env.contains(&"SAFE=1".to_string()));
        assert_eq!(
            joining.refused,
            [flag("SOME_TOKEN")],
            "and it says so, or the caller reads it as their own typo"
        );
    }

    #[test]
    fn an_exec_names_the_run_for_a_variable_the_run_itself_set() {
        // The run's own environment reaches the composer through its image slot, and no image set this: an exec caller sent to look at one finds nothing there.
        let run = crate::run_registry::ExecEnvironment {
            session_env: vec!["SOME_TOKEN=set-before-any-grant".into()],
            ..Default::default()
        };

        let joining = exec_session_env(&run, &[], &fills("SOME_TOKEN"));

        assert_eq!(
            joining.refused,
            [refused_from("SOME_TOKEN", EnvSource::Run)]
        );
        assert_eq!(
            refusal_warning(&joining.refused[0]),
            "SOME_TOKEN holds some-provider's placeholder for this run, not the value the run started with — run `lns connector forget some-provider --run <run>` and start the run again"
        );
    }

    #[test]
    fn the_ca_bundle_outranks_a_session_that_already_names_one() {
        // An image or `-e` that sets SSL_CERT_FILE must not outrank the proxy's bundle, or the exec session quietly stops trusting the interception CA.
        let run = crate::run_registry::ExecEnvironment {
            session_env: vec!["SSL_CERT_FILE=/etc/ssl/image-bundle.pem".into()],
            ..Default::default()
        };

        let env = exec_session_env(&run, &[], &Default::default()).env;

        let seen: Vec<&String> = env
            .iter()
            .filter(|kv| kv.starts_with("SSL_CERT_FILE="))
            .collect();
        assert_eq!(seen.len(), 1, "one variable holds one value: {env:?}");
        assert_eq!(
            seen[0],
            &format!("SSL_CERT_FILE={}", lns_session::SYSTEM_CA_BUNDLE_PATH),
            "got: {env:?}"
        );
    }

    #[test]
    fn an_exec_session_joins_the_runs_environment_and_keeps_the_definitions_own_values() {
        let run = crate::run_registry::ExecEnvironment {
            session_env: vec![
                "HOME=/workspace".into(),
                "SOME_TOOL_CACHE=/workspace/mine".into(),
            ],
            declared_identity_keys: vec!["HOME".into()],
            tools: ToolRuntime {
                bin_paths: vec!["/.lens/tools/some-tool/1.2.3/bin".into()],
                env: vec![
                    (
                        "SOME_TOOL_CACHE".to_string(),
                        "/.lens/tools/some-tool/1.2.3/home/.cache".to_string(),
                    ),
                    (
                        "SOME_TOOL_HOME".to_string(),
                        "/.lens/tools/some-tool/1.2.3/home".to_string(),
                    ),
                ],
            },
            ..Default::default()
        };

        let env = exec_session_env(&run, &[], &Default::default()).env;

        assert!(env.contains(&"HOME=/workspace".to_string()), "got: {env:?}");
        assert!(
            env.contains(&"SOME_TOOL_CACHE=/workspace/mine".to_string()),
            "the var the definition owns keeps the definition's value, not the tool tree's: {env:?}"
        );
        assert!(
            env.contains(&"SOME_TOOL_HOME=/.lens/tools/some-tool/1.2.3/home".to_string()),
            "a tool var nothing else set still arrives: {env:?}"
        );
        assert_eq!(
            env.iter().find(|kv| kv.starts_with("PATH=")),
            Some(&format!(
                "PATH=/.lens/tools/some-tool/1.2.3/bin:{GUEST_DEFAULT_PATH}"
            )),
            "the run's tool dirs still come first: {env:?}"
        );
    }

    #[test]
    fn an_image_declared_home_does_not_outrank_the_run_as_identity_in_an_exec_either() {
        let run = crate::run_registry::ExecEnvironment {
            session_env: vec![
                "HOME=/srv".into(),
                "USER=svc".into(),
                "MODE=research".into(),
            ],
            declared_identity_keys: Vec::new(),
            ..Default::default()
        };

        let env = exec_session_env(&run, &[], &Default::default()).env;

        assert!(
            !env.iter()
                .any(|kv| kv.starts_with("HOME=") || kv.starts_with("USER=")),
            "an image ENV HOME/USER the author never declared must leave the slot to the guest identity, as the supervisor does: {env:?}"
        );
        assert!(env.contains(&"MODE=research".to_string()), "got: {env:?}");
    }

    #[test]
    fn an_author_declared_home_survives_into_an_exec_session() {
        let run = crate::run_registry::ExecEnvironment {
            session_env: vec!["HOME=/srv".into(), "USER=svc".into()],
            declared_identity_keys: vec!["HOME".into()],
            ..Default::default()
        };

        let env = exec_session_env(&run, &[], &Default::default()).env;

        assert!(
            env.contains(&"HOME=/srv".to_string()),
            "a `-e HOME` is the author's decision, exactly like LENS_SANDBOX_WORKLOAD_HOME on the primary: {env:?}"
        );
        assert!(
            !env.iter().any(|kv| kv.starts_with("USER=")),
            "USER stays identity-owned when only HOME was declared: {env:?}"
        );
    }

    #[test]
    fn the_exec_callers_own_env_declares_identity_like_any_dash_e() {
        let env = exec_session_env(
            &Default::default(),
            &["HOME=/x".to_string()],
            &Default::default(),
        )
        .env;
        assert!(env.contains(&"HOME=/x".to_string()), "got: {env:?}");
    }

    #[test]
    fn declared_identity_keys_reports_only_the_identity_vars_the_author_set() {
        assert_eq!(
            declared_identity_keys(&["HOME=/h".into(), "A=1".into()]),
            vec!["HOME".to_string()]
        );
        assert!(declared_identity_keys(&["A=1".into()]).is_empty());
    }

    #[test]
    fn the_supervisors_own_handshake_does_not_travel_into_an_exec_session() {
        let run = crate::run_registry::ExecEnvironment {
            session_env: vec![
                "AGENT_COMMAND=agent --serve".into(),
                "WORKSPACE_PATH=/workspace".into(),
                format!("TERM={SUPERVISOR_PTY_OPT_IN_TERM}"),
                "LENS_SANDBOX_TOKEN=a-relay-token".into(),
                // A malformed entry still names an internal var, so the prefix decides, not the shape.
                "LENS_SANDBOX_BARE".into(),
                "MODE=research".into(),
            ],
            ..Default::default()
        };

        let env = exec_session_env(&run, &[], &Default::default()).env;

        for leaked in [
            "AGENT_COMMAND=",
            "WORKSPACE_PATH=",
            "TERM=",
            "LENS_SANDBOX_",
            "LENS_SANDBOX_BARE",
        ] {
            assert!(
                !env.iter().any(|kv| kv.starts_with(leaked)),
                "{leaked} must not reach an exec session: {env:?}"
            );
        }
        assert!(env.contains(&"MODE=research".to_string()), "got: {env:?}");
    }

    #[test]
    fn the_caller_of_an_exec_cannot_smuggle_an_internal_var_in_either() {
        let env = exec_session_env(
            &Default::default(),
            &["LENS_SANDBOX_TOKEN=a-forged-token".to_string()],
            &Default::default(),
        )
        .env;

        assert!(
            !env.iter().any(|kv| kv.starts_with("LENS_SANDBOX_")),
            "got: {env:?}"
        );
    }

    #[test]
    fn a_workloads_own_term_is_a_declaration_and_survives() {
        let run = crate::run_registry::ExecEnvironment {
            session_env: vec!["TERM=dumb".into()],
            ..Default::default()
        };

        assert!(
            exec_session_env(&run, &[], &Default::default())
                .env
                .contains(&"TERM=dumb".to_string()),
            "only the value the supervisor injected is internal"
        );
    }

    #[test]
    fn an_exec_session_trusts_the_same_ca_bundle_the_workload_does() {
        let env = exec_session_env(&Default::default(), &[], &Default::default()).env;

        for key in [
            "SSL_CERT_FILE",
            "REQUESTS_CA_BUNDLE",
            "NODE_EXTRA_CA_CERTS",
            "CURL_CA_BUNDLE",
            "GIT_SSL_CAINFO",
        ] {
            assert!(
                env.contains(&format!("{key}={}", lns_session::SYSTEM_CA_BUNDLE_PATH)),
                "a python or node command in an exec fails TLS without {key}: {env:?}"
            );
        }
    }

    #[test]
    fn an_exec_session_reaches_the_network_through_the_same_proxy_the_workload_does() {
        let env = exec_session_env(&Default::default(), &[], &Default::default()).env;

        for key in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
            assert!(
                env.contains(&format!("{key}={}", lns_session::GUEST_PROXY_URL)),
                "a curl in an exec would take a different route than the workload without {key}, so a probe would report on a path the agent never uses: {env:?}"
            );
        }
    }

    #[test]
    fn a_loopback_request_in_an_exec_session_stays_in_the_guest() {
        let env = exec_session_env(&Default::default(), &[], &Default::default()).env;

        for key in ["NO_PROXY", "no_proxy"] {
            assert!(
                env.contains(&format!("{key}={}", lns_session::GUEST_NO_PROXY)),
                "without {key} a curl in an exec sends a request for a server the run itself started to the egress proxy, which cannot connect back into the guest: {env:?}"
            );
        }
    }

    #[test]
    fn the_runs_proxy_outranks_a_proxy_the_exec_caller_names() {
        let run = crate::run_registry::ExecEnvironment {
            session_env: vec!["HTTPS_PROXY=http://an-image-baked-proxy".into()],
            ..Default::default()
        };

        let env = exec_session_env(
            &run,
            &["http_proxy=http://a-caller-named-proxy".to_string()],
            &Default::default(),
        )
        .env;

        assert!(
            env.contains(&format!("HTTPS_PROXY={}", lns_session::GUEST_PROXY_URL))
                && env.contains(&format!("http_proxy={}", lns_session::GUEST_PROXY_URL)),
            "the supervisor overrides a workload-set proxy for the same reason: a route around the gate is not the caller's to choose: {env:?}"
        );
    }

    #[test]
    fn tool_bin_paths_prepend_and_beat_the_image_path() {
        let c = run_workload_env(
            Some(&["PATH=/usr/bin".into()]),
            &[],
            None,
            None,
            &ToolRuntime {
                bin_paths: vec!["/.lens/tools/some-tool/1.2.3/bin".into()],
                env: Vec::new(),
            },
            &Default::default(),
        );
        assert_eq!(c.env, ["PATH=/.lens/tools/some-tool/1.2.3/bin:/usr/bin"]);
    }

    #[test]
    fn tool_bin_paths_extend_the_guest_default_when_the_image_sets_no_path() {
        let c = run_workload_env(
            None,
            &[],
            None,
            None,
            &ToolRuntime {
                bin_paths: vec!["/t/bin".into()],
                env: Vec::new(),
            },
            &Default::default(),
        );
        assert_eq!(c.env, [format!("PATH=/t/bin:{GUEST_DEFAULT_PATH}")]);
    }

    #[test]
    fn an_empty_image_path_never_leaves_the_current_directory_on_the_workload_path() {
        // A blank PATH segment is the cwd to execvp, and the workdir is usually a writable project bind.
        let c = run_workload_env(
            Some(&["PATH=".into()]),
            &[],
            None,
            None,
            &tools(&["/t/bin"]),
            &Default::default(),
        );
        assert_eq!(c.env, [format!("PATH=/t/bin:{GUEST_DEFAULT_PATH}")]);
    }

    #[test]
    fn blank_and_duplicate_path_segments_are_dropped() {
        let c = run_workload_env(
            Some(&["PATH=/usr/bin::/t/bin:/usr/bin".into()]),
            &[],
            None,
            None,
            &tools(&["/t/bin"]),
            &Default::default(),
        );
        assert_eq!(c.env, ["PATH=/t/bin:/usr/bin"]);
    }

    #[test]
    fn a_blank_image_path_segment_is_dropped_even_with_no_declared_tools() {
        // The cwd-on-PATH hazard comes from the image, so the sanitation cannot be a side effect of happening to declare a tool.
        let c = run_workload_env(
            Some(&["PATH=/usr/bin::/usr/bin".into()]),
            &[],
            None,
            None,
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(c.env, ["PATH=/usr/bin"]);
    }

    #[test]
    fn a_run_with_no_tools_and_no_image_path_is_left_to_the_kernel_default() {
        // Composing one here would put a second copy of the same value in the env for no reason; lns-init already exports it.
        let c = run_workload_env(
            None,
            &[],
            None,
            None,
            &Default::default(),
            &Default::default(),
        );
        assert!(
            !c.env.iter().any(|kv| kv.starts_with("PATH=")),
            "got: {:?}",
            c.env
        );
    }

    #[test]
    fn the_guest_default_path_carries_the_guest_tools_dir() {
        // The workload inherits the kernel PATH, so declaring a tool must not drop /.lens/bin as a side effect.
        assert!(GUEST_DEFAULT_PATH.starts_with("/.lens/bin:"));
        let c = run_workload_env(
            None,
            &[],
            None,
            None,
            &ToolRuntime {
                bin_paths: vec!["/t/bin".into()],
                env: Vec::new(),
            },
            &Default::default(),
        );
        assert!(c.env[0].contains("/.lens/bin"), "got: {:?}", c.env[0]);
    }

    #[test]
    fn tool_bin_paths_keep_declaration_order_and_precede_the_supervisor_appends() {
        let c = run_workload_env(
            None,
            &[],
            Some("sh"),
            None,
            &tools(&["/t/a/bin", "/t/b"]),
            &Default::default(),
        );
        let path = c.env.iter().position(|e| e.starts_with("PATH=")).unwrap();
        assert!(
            c.env[path].starts_with("PATH=/t/a/bin:/t/b:"),
            "got: {:?}",
            c.env
        );
        let agent = c
            .env
            .iter()
            .position(|e| e.starts_with("AGENT_COMMAND="))
            .unwrap();
        assert!(path < agent, "PATH must precede the last-wins appends");
    }

    #[test]
    fn injected_env_records_the_injected_var_names() {
        let env = injected_env(&["CLAUDE_CODE_USE_BEDROCK=1".into()]).expect("env built");
        assert_eq!(
            env.get("CLAUDE_CODE_USE_BEDROCK").unwrap(),
            REDACTED_ENV_VALUE
        );
    }

    #[test]
    fn injected_env_redacts_values_so_a_mistyped_secret_never_persists() {
        let env =
            injected_env(&["SOME_PRIVATE_TOKEN=super-secret-value".into()]).expect("env built");
        assert!(
            env.contains_key("SOME_PRIVATE_TOKEN"),
            "the var name is still recorded for the audit trail"
        );
        assert_eq!(
            env.get("SOME_PRIVATE_TOKEN").unwrap(),
            REDACTED_ENV_VALUE,
            "the raw -e value must never land in a durable audit record"
        );
    }

    #[test]
    fn injected_env_is_none_when_nothing_is_injected() {
        assert!(injected_env(&[]).is_none());
    }

    #[test]
    fn injected_env_skips_malformed_entries() {
        let env = injected_env(&["NOEQUALS".into(), "A=1".into()]).expect("env built");
        assert!(!env.contains_key("NOEQUALS"));
        assert!(env.contains_key("A"));
    }
}
