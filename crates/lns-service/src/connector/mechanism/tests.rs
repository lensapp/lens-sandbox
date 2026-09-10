use std::sync::Mutex;

use super::host::Host;
use super::traits::{Entropy, Exec, Http, Mechanism, Recorder};
use super::*;

#[derive(Default)]
pub struct Calls {
    pub reached: Vec<(String, bool)>,
    pub ran: Vec<(String, bool)>,
    /// lns's own entry, where one call wrote all it may.
    pub elided: Vec<u32>,
}

#[derive(Default)]
pub struct Spy {
    pub calls: Mutex<Calls>,
}

impl Spy {
    pub(crate) fn taken(&self) -> Calls {
        std::mem::take(&mut self.calls.lock().expect("spy lock"))
    }
}

impl Recorder for Spy {
    fn reached(&self, _connector: &str, host: &str, refused: bool) {
        self.calls
            .lock()
            .expect("spy lock")
            .reached
            .push((host.to_string(), refused));
    }

    fn ran(&self, _connector: &str, program: &str, refused: bool) {
        self.calls
            .lock()
            .expect("spy lock")
            .ran
            .push((program.to_string(), refused));
    }

    // no-op: a renewal is the refresh pass's to record, and nothing here runs one.
    fn renewed(&self, _connector: &str, _target: &str, _refused: bool) {}

    fn elided(&self, _connector: &str, after: u32) {
        self.calls.lock().expect("spy lock").elided.push(after);
    }
}

#[derive(Default)]
pub struct Reachable {
    pub seen: Mutex<Vec<String>>,
    /// The deadline the call carried, because an epoch tick cannot interrupt a host call already in flight.
    pub within: Mutex<std::time::Duration>,
    /// What the network did, rather than what the bound decided: a mechanism has to tell the two apart.
    pub unreachable: Mutex<bool>,
    /// One answer per call, in order, for a component whose rounds each need a different one. Empty means every call gets the same `ok`.
    pub scripted: Mutex<std::collections::VecDeque<(u16, Vec<u8>)>>,
}

impl Reachable {
    pub fn answering(answers: &[(u16, &str)]) -> Self {
        Self {
            scripted: Mutex::new(
                answers
                    .iter()
                    .map(|(status, body)| (*status, body.as_bytes().to_vec()))
                    .collect(),
            ),
            ..Self::default()
        }
    }
}

impl Http for Reachable {
    fn fetch(
        &self,
        request: &HttpRequest,
        within: std::time::Duration,
    ) -> Result<HttpResponse, CallError> {
        *self.within.lock().expect("http lock") = within;
        self.seen
            .lock()
            .expect("http lock")
            .push(request.url.clone());
        if *self.unreachable.lock().expect("http lock") {
            return Err(CallError::Failed("the network is down".to_string()));
        }
        let (status, body) = self
            .scripted
            .lock()
            .expect("http lock")
            .pop_front()
            .unwrap_or((200, b"ok".to_vec()));
        Ok(HttpResponse {
            status,
            headers: Vec::new(),
            body,
        })
    }
}

#[derive(Default)]
pub struct Runnable {
    pub seen: Mutex<Vec<Vec<String>>>,
    pub within: Mutex<std::time::Duration>,
    pub missing: Mutex<bool>,
}

impl Exec for Runnable {
    fn run(&self, argv: &[String], within: std::time::Duration) -> Result<ExecOutput, CallError> {
        *self.within.lock().expect("exec lock") = within;
        self.seen.lock().expect("exec lock").push(argv.to_vec());
        if *self.missing.lock().expect("exec lock") {
            return Err(CallError::Failed("no such program".to_string()));
        }
        Ok(ExecOutput {
            code: 0,
            stdout: b"ran".to_vec(),
            stderr: Vec::new(),
        })
    }
}

pub struct Counting;

impl Entropy for Counting {
    fn bytes(&self, count: u32) -> Vec<u8> {
        (0..count).map(|i| i as u8).collect()
    }
}

pub struct Parts {
    pub http: std::sync::Arc<Reachable>,
    pub exec: std::sync::Arc<Runnable>,
    pub recorder: std::sync::Arc<Spy>,
}

impl Parts {
    pub fn new() -> Self {
        Self {
            http: std::sync::Arc::new(Reachable::default()),
            exec: std::sync::Arc::new(Runnable::default()),
            recorder: std::sync::Arc::new(Spy::default()),
        }
    }

    /// Parts whose network answers each call with the next scripted response, for a component whose rounds each need a different one.
    pub fn answering(answers: &[(u16, &str)]) -> Self {
        Self {
            http: std::sync::Arc::new(Reachable::answering(answers)),
            ..Self::new()
        }
    }

    pub fn host(&self, bounds: Bounds) -> Host {
        Host::new(
            "some-provider",
            bounds,
            self.http.clone(),
            self.exec.clone(),
            std::sync::Arc::new(Counting),
            self.recorder.clone(),
        )
    }
}

fn get(url: &str) -> HttpRequest {
    HttpRequest {
        method: "GET".to_string(),
        url: url.to_string(),
        headers: Vec::new(),
        body: Vec::new(),
    }
}

fn declaring(hosts: &[&str]) -> Bounds {
    Bounds {
        hosts: hosts.iter().map(|h| (*h).to_string()).collect(),
        ..Bounds::default()
    }
}

#[test]
fn a_declared_host_is_reached_and_the_call_is_written_down() {
    let parts = Parts::new();
    let host = parts.host(declaring(&["auth.some-provider.example"]));

    let response = host
        .fetch(&get("https://auth.some-provider.example/token"))
        .expect("a declared host is reachable");

    assert_eq!(response.status, 200);
    assert_eq!(
        *parts.http.seen.lock().expect("http lock"),
        ["https://auth.some-provider.example/token"]
    );
    assert_eq!(
        parts.recorder.taken().reached,
        [("auth.some-provider.example".to_string(), false)]
    );
}

#[test]
fn an_undeclared_host_is_refused_before_anything_leaves_the_machine() {
    let parts = Parts::new();
    let host = parts.host(declaring(&["auth.some-provider.example"]));

    let refusal = host
        .fetch(&get("https://other.some-provider.example/token"))
        .expect_err("an undeclared host is refused");

    assert!(
        matches!(refusal, CallError::Refused(ref why) if why.contains("other.some-provider.example"))
    );
    assert!(parts.http.seen.lock().expect("http lock").is_empty());
    assert_eq!(
        parts.recorder.taken().reached,
        [("other.some-provider.example".to_string(), true)]
    );
}

#[test]
fn a_bound_holds_against_the_host_the_request_reaches_and_not_a_spelling_beside_it() {
    // A backslash ends the authority of a special-scheme URL, so this reaches evil.some-provider.example whatever the userinfo before it reads.
    let parts = Parts::new();
    let host = parts.host(declaring(&["auth.some-provider.example"]));

    let refusal = host
        .fetch(&get(
            r"https://evil.some-provider.example\@auth.some-provider.example/token",
        ))
        .expect_err("the bound holds against the host the request would reach");

    assert!(
        matches!(refusal, CallError::Refused(ref why) if why.contains("evil.some-provider.example")),
        "{refusal:?}"
    );
    assert!(parts.http.seen.lock().expect("http lock").is_empty());
    assert_eq!(
        parts.recorder.taken().reached,
        [("evil.some-provider.example".to_string(), true)],
        "and the record names the host it would have reached, not the one it was dressed as"
    );
}

#[test]
fn a_bound_naming_an_address_literal_holds_against_the_url_that_reaches_it() {
    // The pattern grammar carries an IPv6 literal unbracketed, so a host read out bracketed would match no spelling of one and the bound would silently hold nothing.
    for declared in ["[2001:db8::1]:8443", "[2001:db8::1]", "2001:db8::1"] {
        let parts = Parts::new();
        let host = parts.host(declaring(&[declared]));

        host.fetch(&get("https://[2001:db8::1]:8443/token"))
            .unwrap_or_else(|e| panic!("{declared} names the host this reaches: {e:?}"));

        assert_eq!(
            parts.recorder.taken().reached,
            [("2001:db8::1".to_string(), false)],
            "and it is written down the way the pattern grammar spells it"
        );
    }
}

#[test]
fn a_bound_pinned_to_a_port_reads_it_as_a_number_and_not_as_its_spelling() {
    // `unusable_port` accepts `0443`, so a bound could be validated and then hold against nothing a URL could ever say.
    let parts = Parts::new();
    let host = parts.host(declaring(&["auth.some-provider.example:0443"]));

    host.fetch(&get("https://auth.some-provider.example:443/token"))
        .expect("a port is the number it names, however the author wrote it");
}

#[test]
fn what_is_sent_is_the_url_the_bound_was_decided_from() {
    // One parse decides both, so no spelling can be read one way by the bound and another on the wire.
    let parts = Parts::new();
    let host = parts.host(declaring(&["auth.some-provider.example"]));

    host.fetch(&get("https://auth.some-provider.example"))
        .expect("a declared host is reachable");

    assert_eq!(
        *parts.http.seen.lock().expect("http lock"),
        ["https://auth.some-provider.example/"]
    );
}

#[test]
fn a_url_no_bound_could_be_held_against_is_refused_rather_than_reached() {
    // Each half a bound needs gets its own refusal, because "reread the host" is no help to an author whose scheme has no port.
    for (url, said) in [
        ("auth.some-provider.example/token", "is not a URL"),
        ("foo://auth.some-provider.example/token", "names no port"),
    ] {
        let parts = Parts::new();
        let host = parts.host(declaring(&["auth.some-provider.example"]));

        let refusal = host
            .fetch(&get(url))
            .expect_err("a bound cannot be held against this");

        assert!(
            matches!(refusal, CallError::Refused(ref why) if why.contains(said)),
            "{url}: {refusal:?}"
        );
        assert!(
            parts.recorder.taken().reached.is_empty(),
            "there is no host to write down"
        );
    }
}

#[test]
fn a_method_declaring_no_hosts_reaches_nothing() {
    let parts = Parts::new();
    let host = parts.host(Bounds::default());

    host.fetch(&get("https://auth.some-provider.example/token"))
        .expect_err("a method declaring no hosts reaches nothing");
}

#[test]
fn a_wildcard_bound_covers_the_hosts_below_it() {
    let parts = Parts::new();
    let host = parts.host(declaring(&["*.some-provider.example"]));

    host.fetch(&get("https://auth.some-provider.example/token"))
        .expect("a wildcard bound covers what is below it");
}

#[test]
fn every_call_carries_the_deadline_the_method_declared() {
    // An epoch tick cannot interrupt a host call already in flight, so the call has to carry it (§3.2.6).
    let parts = Parts::new();
    let host = parts.host(Bounds {
        hosts: vec!["auth.some-provider.example".to_string()],
        exec: true,
        call_seconds: 12,
        session_seconds: 900,
    });

    host.fetch(&get("https://auth.some-provider.example/token"))
        .expect("a declared host is reachable");
    host.run(&["claude".to_string()])
        .expect("host execution was declared");

    assert_eq!(
        *parts.http.within.lock().expect("http lock"),
        std::time::Duration::from_secs(12)
    );
    assert_eq!(
        *parts.exec.within.lock().expect("exec lock"),
        std::time::Duration::from_secs(12)
    );
}

#[test]
fn a_bound_naming_a_port_holds_against_that_port_alone() {
    let parts = Parts::new();
    let host = parts.host(declaring(&["auth.some-provider.example:8443"]));

    host.fetch(&get("https://auth.some-provider.example:8443/token"))
        .expect("the port the bound names is reachable");
    host.fetch(&get("https://auth.some-provider.example/token"))
        .expect_err("a bound naming a port does not hold against another");
}

#[test]
fn a_bound_naming_no_port_holds_against_whichever_the_url_reaches() {
    let parts = Parts::new();
    let host = parts.host(declaring(&["auth.some-provider.example"]));

    host.fetch(&get("https://auth.some-provider.example:8443/token"))
        .expect("a bound naming no port holds against any");
}

#[test]
fn a_call_that_is_not_over_tls_is_refused_however_the_host_was_declared() {
    let parts = Parts::new();
    let host = parts.host(declaring(&["auth.some-provider.example"]));

    let refusal = host
        .fetch(&get("http://auth.some-provider.example/token"))
        .expect_err("lns carries a mechanism's calls over TLS or not at all");

    assert!(matches!(refusal, CallError::Refused(ref why) if why.contains("not https")));
    assert!(parts.http.seen.lock().expect("http lock").is_empty());
    assert_eq!(
        parts.recorder.taken().reached,
        [("auth.some-provider.example".to_string(), true)]
    );
}

#[test]
fn a_network_that_is_down_reads_differently_from_a_bound_that_was_crossed() {
    let parts = Parts::new();
    *parts.http.unreachable.lock().expect("http lock") = true;
    let host = parts.host(declaring(&["auth.some-provider.example"]));

    let failure = host
        .fetch(&get("https://auth.some-provider.example/token"))
        .expect_err("the network is down");

    assert!(matches!(failure, CallError::Failed(_)));
    assert_eq!(
        parts.recorder.taken().reached,
        [("auth.some-provider.example".to_string(), false)],
        "the call was allowed, so it is written down as made"
    );
}

#[test]
fn a_program_that_is_not_on_the_machine_fails_the_call_rather_than_refusing_it() {
    let parts = Parts::new();
    *parts.exec.missing.lock().expect("exec lock") = true;
    let host = parts.host(Bounds {
        exec: true,
        ..Bounds::default()
    });

    let failure = host
        .run(&["claude".to_string()])
        .expect_err("there is no such program");

    assert!(matches!(failure, CallError::Failed(_)));
}

#[test]
fn a_program_runs_only_where_the_method_declared_host_execution() {
    let parts = Parts::new();
    let host = parts.host(Bounds {
        exec: true,
        ..Bounds::default()
    });

    let output = host
        .run(&["claude".to_string(), "auth".to_string()])
        .expect("a method declaring host execution may run one");

    assert_eq!(output.code, 0);
    assert_eq!(parts.recorder.taken().ran, [("claude".to_string(), false)]);
}

#[test]
fn a_method_declaring_no_host_execution_runs_nothing_and_the_attempt_is_written_down() {
    let parts = Parts::new();
    let host = parts.host(Bounds::default());

    let refusal = host
        .run(&["claude".to_string()])
        .expect_err("a method that did not declare host execution runs nothing");

    assert!(matches!(refusal, CallError::Refused(ref why) if why.contains("host execution")));
    assert!(parts.exec.seen.lock().expect("exec lock").is_empty());
    assert_eq!(parts.recorder.taken().ran, [("claude".to_string(), true)]);
}

#[test]
fn a_host_written_down_is_cut_to_the_ceiling_like_every_other_target() {
    // A URL parser accepts a host far longer than anything that resolves, and a refused reach is written down as readily as one that landed.
    let parts = Parts::new();
    let host = parts.host(declaring(&["auth.some-provider.example"]));
    let sprawling = format!("{}.example.com", "a".repeat(300));

    host.fetch(&get(&format!("https://{sprawling}/token")))
        .expect_err("that host is not among the ones this method declares");

    let reached = parts.recorder.taken().reached;
    assert_eq!(
        (reached[0].0.len(), reached[0].1),
        (super::host::MAX_RECORDED_TARGET_BYTES, true)
    );
}

#[test]
fn what_a_component_is_written_down_as_having_run_is_rendered_on_lnss_own_terms() {
    // The ledger is the only account of a component nobody can read, and `lns audit` prints it: a program name that could redraw the line would forge that account. A refused attempt is recorded too, so this holds for a method that declared no host execution at all.
    let parts = Parts::new();
    let host = parts.host(Bounds::default());

    host.run(&["\u{1b}[2Kclaude\u{202e}auth".to_string()])
        .expect_err("a method that did not declare host execution runs nothing");

    assert_eq!(
        parts.recorder.taken().ran,
        [(" [2Kclaude auth".to_string(), true)],
        "every character that does not draw in the place lns drew it is replaced"
    );
}

#[test]
fn a_program_name_past_the_ceiling_is_cut_rather_than_written_down_whole() {
    // A component may call until its fuel runs out, and each call leaves a durable line; an unbounded name makes the ledger the place a connector writes megabytes to.
    let parts = Parts::new();
    let host = parts.host(Bounds::default());

    host.run(&["a".repeat(super::host::MAX_RECORDED_TARGET_BYTES + 1)])
        .expect_err("a method that did not declare host execution runs nothing");

    assert_eq!(
        parts.recorder.taken().ran[0].0.len(),
        super::host::MAX_RECORDED_TARGET_BYTES
    );
}

#[test]
fn one_call_writes_down_only_so_much_of_itself_and_says_where_it_stopped() {
    // A component may call until its fuel runs out. Thousands of entries bury the neighbouring ones as surely as one unbounded name would, and those are the only evidence anything happened (§3.2.6).
    let parts = Parts::new();
    let host = parts.host(Bounds::default());

    for _ in 0..super::host::MAX_RECORDED_ENTRIES_PER_CALL + 10 {
        host.run(&["claude".to_string()])
            .expect_err("this method declares no host execution");
    }

    let calls = parts.recorder.taken();
    assert_eq!(
        calls.ran.len(),
        super::host::MAX_RECORDED_ENTRIES_PER_CALL as usize
    );
    assert_eq!(
        calls.elided,
        [super::host::MAX_RECORDED_ENTRIES_PER_CALL],
        "lns says once that it stopped writing, so a silent stop cannot read as a call that did nothing more"
    );
}

#[test]
fn the_next_call_may_write_down_as_much_as_the_first() {
    // The ceiling is what one call may write, not what a component may write for as long as it is installed.
    let parts = Parts::new();
    let host = parts.host(Bounds::default());
    for _ in 0..super::host::MAX_RECORDED_ENTRIES_PER_CALL + 1 {
        let _ = host.run(&["claude".to_string()]);
    }
    parts.recorder.taken();

    host.begins_a_call();
    let _ = host.run(&["claude".to_string()]);

    assert_eq!(parts.recorder.taken().ran.len(), 1);
}

#[test]
fn an_empty_argv_names_no_program_and_is_refused_before_the_capability_is_read() {
    let parts = Parts::new();
    let host = parts.host(Bounds {
        exec: true,
        ..Bounds::default()
    });

    let refusal = host
        .run(&[])
        .expect_err("there is no program to run and none to name");

    assert!(matches!(refusal, CallError::Refused(ref why) if why.contains("not named")));
    assert!(parts.recorder.taken().ran.is_empty());
}

#[test]
fn entropy_is_capped_so_one_call_draws_a_verifier_and_not_a_keystream() {
    let parts = Parts::new();
    let host = parts.host(Bounds::default());

    assert_eq!(host.bytes(8).len(), 8);
    assert_eq!(host.bytes(u32::MAX).len(), 1024);
}

#[test]
fn bounds_come_from_the_method_that_declared_the_mechanism() {
    let auth: lns_artifact::connector::Auth = serde_json::from_value(serde_json::json!({
        "kind": "code",
        "component": "./sign-in.wasm",
        "outputs": ["access_token"],
        "hosts": ["auth.some-provider.example"],
        "exec": true,
        "limits": {"callSeconds": 10, "sessionSeconds": 60},
    }))
    .expect("a code auth reads");
    let code = auth
        .code()
        .expect("the kind is code")
        .expect("the block reads");

    let bounds = Bounds::of(&code);

    assert_eq!(
        bounds,
        Bounds {
            hosts: vec!["auth.some-provider.example".to_string()],
            exec: true,
            call_seconds: 10,
            session_seconds: 60,
        }
    );
    assert!(bounds.allows("auth.some-provider.example", 443));
    assert!(!bounds.allows("other.some-provider.example", 443));
}

#[test]
fn a_debug_of_a_step_cannot_print_the_values_or_the_state_it_carries() {
    // Anything on a connect path may be rendered by `log::debug!`, and both a produced value and a device code are secret material (§3.2.6).
    let asked = Step::Ask {
        message: "open the picker".to_string(),
        fields: vec![Field {
            name: "access_token".to_string(),
            label: "access token".to_string(),
            secret: true,
        }],
        state: b"a device code".to_vec(),
    };
    let rendered = format!("{asked:?}");
    assert!(!rendered.contains("a device code"), "{rendered}");
    assert!(rendered.contains("redacted"), "{rendered}");
    assert!(
        rendered.contains("open the picker") && rendered.contains("access token"),
        "what the user is about to read is not a secret, and has to be diagnosable: {rendered}"
    );

    let done = Step::Done(Outcome {
        values: Answers::from([("access_token".to_string(), "sk-live-real".to_string())]),
        authority: std::collections::BTreeSet::from(["repo:read".to_string()]),
        expires_at_millis: Some(1_000),
    });
    let rendered = format!("{done:?}");
    assert!(!rendered.contains("sk-live-real"), "{rendered}");
    assert!(rendered.contains("redacted"), "{rendered}");
    assert!(
        rendered.contains("repo:read") && rendered.contains("1000"),
        "the authority and the expiry are what a connection records, not what it hides: {rendered}"
    );

    assert!(
        format!("{:?}", Step::Failed("the provider said no".to_string()))
            .contains("the provider said no")
    );
}

#[test]
fn a_refused_call_reads_differently_from_one_that_failed() {
    assert_eq!(
        CallError::Refused("no".to_string()).to_string(),
        "refused: no"
    );
    assert_eq!(
        CallError::Failed("down".to_string()).to_string(),
        "failed: down"
    );
}

#[test]
fn the_token_mechanism_asks_for_its_one_output_by_the_authors_word_for_it() {
    let parts = Parts::new();
    let host = parts.host(Bounds::default());
    let token = token::Token::new(vec!["token".to_string()], "personal access token");

    let step = token.connect(&host, 0).expect("token asks");

    assert_eq!(
        step,
        Step::Ask {
            message: String::new(),
            fields: vec![Field {
                name: "token".to_string(),
                label: "personal access token".to_string(),
                secret: true,
            }],
            state: Vec::new(),
        }
    );
}

#[test]
fn a_mechanism_producing_several_values_names_each_output_rather_than_the_auth() {
    let parts = Parts::new();
    let host = parts.host(Bounds::default());
    let token = token::Token::new(vec!["id".to_string(), "secret".to_string()], "credentials");

    let Step::Ask { fields, .. } = token.connect(&host, 0).expect("token asks") else {
        panic!("a token mechanism asks before it is done");
    };

    assert_eq!(
        fields.iter().map(|f| f.label.as_str()).collect::<Vec<_>>(),
        ["id", "secret"]
    );
}

#[test]
fn the_token_mechanism_is_done_with_what_the_user_gave_it() {
    let parts = Parts::new();
    let host = parts.host(Bounds::default());
    let token = token::Token::new(vec!["token".to_string()], "token");
    let answers = Answers::from([("token".to_string(), "ghp_x".to_string())]);

    let step = token
        .resume(&host, &[], &answers, 0)
        .expect("token finishes");

    assert_eq!(
        step,
        Step::Done(Outcome {
            values: answers,
            ..Outcome::default()
        })
    );
}

#[test]
fn the_token_mechanism_keeps_only_what_its_auth_says_it_produces() {
    let parts = Parts::new();
    let host = parts.host(Bounds::default());
    let token = token::Token::new(vec!["token".to_string()], "token");
    let answers = Answers::from([
        ("token".to_string(), "ghp_x".to_string()),
        ("smuggled".to_string(), "not mine to answer for".to_string()),
    ]);

    let step = token
        .resume(&host, &[], &answers, 0)
        .expect("token finishes");

    assert_eq!(
        step,
        Step::Done(Outcome {
            values: Answers::from([("token".to_string(), "ghp_x".to_string())]),
            ..Outcome::default()
        })
    );
}

#[test]
fn the_token_mechanism_refuses_an_output_left_empty_rather_than_storing_a_blank() {
    let parts = Parts::new();
    let host = parts.host(Bounds::default());
    let token = token::Token::new(vec!["token".to_string()], "token");

    for answers in [
        Answers::new(),
        Answers::from([("token".to_string(), String::new())]),
    ] {
        let step = token
            .resume(&host, &[], &answers, 0)
            .expect("token answers");
        assert_eq!(
            step,
            Step::Failed("token was not given a value".to_string())
        );
    }
}

#[test]
fn a_token_this_machine_was_given_cannot_be_renewed_without_the_user() {
    let parts = Parts::new();
    let host = parts.host(Bounds::default());
    let token = token::Token::new(vec!["token".to_string()], "token");

    let refusal = token
        .refresh(&host, &Answers::new(), 0)
        .expect_err("a pasted value is not one lns can fetch again");

    assert!(refusal.to_string().contains("cannot be renewed"));
    token
        .revoke(&host, &Answers::new(), 0)
        .expect("dropping a pasted value tells nobody");
}
