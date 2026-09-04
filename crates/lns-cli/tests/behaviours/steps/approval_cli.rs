use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_cli::approval::{self, ApprovalArgs, ApprovalService};
use lns_cli::command::parse_args;
use lns_cli::local_future::LocalBoxFuture;
use lns_ipc::{ApprovalAnswer, ApprovalEntryKind, ApprovalInfo, Request, Response};

fn fixture(id: &str, subject: &str, sandbox: &str, answer: &str) -> ApprovalInfo {
    ApprovalInfo {
        id: id.to_string(),
        sandbox: Some(sandbox.to_string()),
        subject: subject.to_string(),
        action: Some(format!("CONNECT {subject}:443")),
        kind: ApprovalEntryKind::Destination,
        answer: answer.to_string(),
        answerable: true,
    }
}

/// Stands in for the running service: answers each approval request from the rig's scripted entries.
struct FakeApprovalService {
    approvals: Vec<ApprovalInfo>,
    unknown: Vec<String>,
    not_written: Option<String>,
    unknown_sandbox: Option<String>,
    refuse_message: Option<String>,
    nonsense: bool,
    requests: std::sync::Arc<std::sync::Mutex<Vec<Request>>>,
}

impl FakeApprovalService {
    fn from_world(world: &BehaviourWorld) -> Self {
        Self {
            approvals: world.approval.approvals.clone(),
            unknown: world.approval.unknown.clone(),
            not_written: world.approval.not_written.clone(),
            unknown_sandbox: world.approval.unknown_sandbox.clone(),
            refuse_message: world.approval.refuse_message.clone(),
            nonsense: world.approval.nonsense,
            requests: world.approval.requests.clone(),
        }
    }

    fn answered(&self, id: &str, answer: ApprovalAnswer) -> Response {
        if self.unknown.iter().any(|missing| missing == id) {
            return Response::ApprovalUnknown { id: id.to_string() };
        }
        if let Some(reason) = &self.not_written {
            return Response::ApprovalNotWritten {
                id: id.to_string(),
                reason: reason.clone(),
            };
        }
        match self.approvals.iter().find(|held| held.id == id) {
            Some(held) => Response::ApprovalAnswered {
                approval: ApprovalInfo {
                    answer: words_for(answer).to_string(),
                    ..held.clone()
                },
            },
            None => Response::ApprovalUnknown { id: id.to_string() },
        }
    }

    fn listed(&self, sandbox: Option<&str>) -> Response {
        if let Some(unknown) = &self.unknown_sandbox
            && sandbox == Some(unknown.as_str())
        {
            return Response::RunUnknown {
                run: unknown.clone(),
            };
        }
        let approvals = self
            .approvals
            .iter()
            .filter(|held| sandbox.is_none_or(|want| held.sandbox.as_deref() == Some(want)))
            .cloned()
            .collect();
        Response::ApprovalList { approvals }
    }
}

fn words_for(answer: ApprovalAnswer) -> &'static str {
    match answer {
        ApprovalAnswer::AlwaysAllow => "always allow",
        ApprovalAnswer::AlwaysDeny => "always deny",
        ApprovalAnswer::AskAgain => "undecided",
    }
}

impl ApprovalService for FakeApprovalService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>> {
        self.requests.lock().unwrap().push(req.clone());
        if let Some(message) = &self.refuse_message {
            let refusal = Response::Error {
                message: message.clone(),
            };
            return Box::pin(async move { Some(refusal) });
        }
        if self.nonsense {
            return Box::pin(async move { Some(Response::Pong) });
        }
        let resp = match &req {
            Request::ListApprovals { sandbox } => self.listed(sandbox.as_deref()),
            Request::AnswerApproval { id, answer } => self.answered(id, *answer),
            other => panic!("unexpected approval request {other:?}"),
        };
        Box::pin(async move { Some(resp) })
    }
}

#[given(
    expr = "the service reports an undecided approval {string} for {string} raised by {string}"
)]
fn reports_undecided(world: &mut BehaviourWorld, id: String, subject: String, sandbox: String) {
    world
        .approval
        .approvals
        .push(fixture(&id, &subject, &sandbox, "undecided"));
}

#[given(
    expr = "the service reports an approval {string} for {string} raised by {string} answered always allow"
)]
fn reports_always_allowed(
    world: &mut BehaviourWorld,
    id: String,
    subject: String,
    sandbox: String,
) {
    world
        .approval
        .approvals
        .push(fixture(&id, &subject, &sandbox, "always allow"));
}

#[given(expr = "the service reports no approval {string}")]
fn reports_no_approval(world: &mut BehaviourWorld, id: String) {
    world.approval.unknown.push(id);
}

#[given(expr = "the service will not write the rule, saying {string}")]
fn service_will_not_write(world: &mut BehaviourWorld, reason: String) {
    world.approval.not_written = Some(reason);
}

#[given(expr = "the service knows no sandbox {string}")]
fn service_knows_no_sandbox(world: &mut BehaviourWorld, handle: String) {
    world.approval.unknown_sandbox = Some(handle);
}

#[given(expr = "the service refuses approvals with {string}")]
fn service_refuses_approvals(world: &mut BehaviourWorld, message: String) {
    world.approval.refuse_message = Some(message);
}

#[given("the service answers approvals with something else")]
fn service_answers_nonsense(world: &mut BehaviourWorld) {
    world.approval.nonsense = true;
}

#[then(expr = "the service is asked to answer {string} with always deny")]
fn asked_to_always_deny(world: &mut BehaviourWorld, id: String) {
    assert_answered(world, &id, ApprovalAnswer::AlwaysDeny);
}

#[when(expr = "the user runs approval command {string}")]
async fn run_approval(world: &mut BehaviourWorld, tail: String) {
    let mut argv = vec!["lns".to_string(), "approval".to_string()];
    argv.extend(tail.split_whitespace().map(str::to_string));
    let run = match parse_args::<ApprovalArgs, _, _>(&argv) {
        Ok(args) => {
            let svc = FakeApprovalService::from_world(world);
            let mut buf = Vec::<u8>::new();
            match approval::run(&args.command, &svc, &mut buf).await {
                Ok(exit_code) => CliRun {
                    exit_code,
                    output: String::from_utf8_lossy(&buf).into_owned(),
                },
                Err(e) => CliRun {
                    exit_code: 1,
                    output: format!("{}{e:#}", String::from_utf8_lossy(&buf)),
                },
            }
        }
        Err(e) => CliRun {
            exit_code: e.exit_code(),
            output: e.to_string(),
        },
    };
    world.result = Some(run);
}

#[then(expr = "the service is asked to answer {string} with always allow")]
fn asked_to_always_allow(world: &mut BehaviourWorld, id: String) {
    assert_answered(world, &id, ApprovalAnswer::AlwaysAllow);
}

#[then(expr = "the service is asked to answer {string} with ask again")]
fn asked_to_ask_again(world: &mut BehaviourWorld, id: String) {
    assert_answered(world, &id, ApprovalAnswer::AskAgain);
}

fn assert_answered(world: &BehaviourWorld, id: &str, want: ApprovalAnswer) {
    let sent = world.approval.requests.lock().unwrap();
    assert!(
        sent.iter().any(|req| matches!(
            req,
            Request::AnswerApproval { id: sent, answer } if sent == id && *answer == want
        )),
        "expected an answer of {want:?} for {id:?}, got {sent:?}"
    );
}

#[then("the output is valid JSON")]
fn output_is_json(world: &mut BehaviourWorld) {
    let output = &world.result.as_ref().expect("a CLI run").output;
    serde_json::from_str::<serde_json::Value>(output)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {output:?}"));
}

#[then(expr = "the JSON output contains {string}")]
fn json_output_contains(world: &mut BehaviourWorld, needle: String) {
    let output = &world.result.as_ref().expect("a CLI run").output;
    let parsed: serde_json::Value =
        serde_json::from_str(output).unwrap_or_else(|e| panic!("not JSON ({e}): {output:?}"));
    assert!(
        parsed.to_string().contains(&needle),
        "expected {needle:?} in {parsed}"
    );
}
