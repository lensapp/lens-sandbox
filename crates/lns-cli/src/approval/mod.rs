use std::io::Write;

use anyhow::{Result, bail};
use lns_ipc::{ApprovalAnswer, ApprovalInfo, Request, Response};

use crate::command::{CommandSpec, subcommand};
use crate::local_future::LocalBoxFuture;

mod real;

pub use real::RealApprovalService;

#[derive(clap::Args)]
pub struct ApprovalArgs {
    #[command(subcommand)]
    pub command: ApprovalCommand,
}

#[derive(clap::Subcommand)]
pub enum ApprovalCommand {
    #[command(about = "List what each run has been asked, and the answer it has.")]
    Ls(ApprovalLsArgs),
    #[command(about = "Answer one destination entry, or answer it again.")]
    Answer(ApprovalAnswerArgs),
}

#[derive(clap::Args)]
pub struct ApprovalLsArgs {
    #[arg(help = "Sandbox id or name, to list what that run alone was asked.")]
    pub sandbox: Option<String>,

    #[command(flatten)]
    pub output: crate::output::OutputArgs,
}

#[derive(clap::Args)]
pub struct ApprovalAnswerArgs {
    #[arg(help = "Entry id, as `lns approval ls` prints it.")]
    pub id: String,

    #[arg(
        value_enum,
        help = "The verdict. A once verdict answers a request the guest still holds, which the approval window is for."
    )]
    pub answer: AnswerArg,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum AnswerArg {
    AlwaysAllow,
    AlwaysDeny,
    AskAgain,
}

impl From<AnswerArg> for ApprovalAnswer {
    fn from(arg: AnswerArg) -> Self {
        match arg {
            AnswerArg::AlwaysAllow => Self::AlwaysAllow,
            AnswerArg::AlwaysDeny => Self::AlwaysDeny,
            AnswerArg::AskAgain => Self::AskAgain,
        }
    }
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<ApprovalArgs>("approval")
            .about("Read and answer what the approval window asked."),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "approval",
    augment,
    run: real::run,
    announces_update_check: true,
    owns_terminal: crate::command::never_owns_terminal,
};

/// Sends one approval request to the running service; `None` means the service did not answer.
pub trait ApprovalService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>>;
}

pub async fn run(
    cmd: &ApprovalCommand,
    svc: &dyn ApprovalService,
    writer: &mut impl Write,
) -> Result<i32> {
    match cmd {
        ApprovalCommand::Ls(args) => ls(svc, args, writer).await,
        ApprovalCommand::Answer(args) => answer(svc, args, writer).await,
    }
}

async fn send(svc: &dyn ApprovalService, req: Request) -> Result<Response> {
    let response = svc
        .request(req)
        .await
        .ok_or_else(|| anyhow::anyhow!("no response from lns-service (is it running?)"))?;
    if let Response::Error { message } = response {
        bail!("{message}");
    }
    Ok(response)
}

async fn ls(
    svc: &dyn ApprovalService,
    args: &ApprovalLsArgs,
    writer: &mut impl Write,
) -> Result<i32> {
    let req = Request::ListApprovals {
        sandbox: args.sandbox.clone(),
    };
    match send(svc, req).await? {
        Response::ApprovalList { approvals } => {
            let rows: Vec<ApprovalRow> = approvals.iter().map(ApprovalRow::new).collect();
            crate::output::emit(args.output.format, &rows, "Nothing has been asked.", writer)?;
            Ok(0)
        }
        Response::RunUnknown { run } => bail!("no sandbox named {run}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn answer(
    svc: &dyn ApprovalService,
    args: &ApprovalAnswerArgs,
    writer: &mut impl Write,
) -> Result<i32> {
    let req = Request::AnswerApproval {
        id: args.id.clone(),
        answer: args.answer.into(),
    };
    match send(svc, req).await? {
        Response::ApprovalAnswered { approval } => {
            writeln!(writer, "{} {}", approval.subject, approval.answer)?;
            Ok(0)
        }
        Response::ApprovalUnknown { id } => bail!("no approval entry with id {id}"),
        Response::ApprovalNotWritten { id, reason } => bail!("{id} was not answered: {reason}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalRow {
    id: String,
    sandbox: Option<String>,
    subject: String,
    action: Option<String>,
    /// What was asked about, so a script can tell a notice from a destination without reading `answer`.
    kind: lns_ipc::ApprovalEntryKind,
    answer: String,
    answerable: bool,
}

impl ApprovalRow {
    fn new(approval: &ApprovalInfo) -> Self {
        Self {
            id: approval.id.clone(),
            sandbox: approval.sandbox.clone(),
            subject: approval.subject.clone(),
            action: approval.action.clone(),
            kind: approval.kind,
            answer: approval.answer.clone(),
            answerable: approval.answerable,
        }
    }
}

impl crate::output::TableRow for ApprovalRow {
    const HEADERS: &'static [&'static str] = &["ID", "SANDBOX", "ASKED ABOUT", "ANSWER"];

    fn cells(&self) -> Vec<String> {
        vec![
            self.id.clone(),
            self.sandbox.clone().unwrap_or_else(|| "-".to_string()),
            self.subject.clone(),
            self.answer.clone(),
        ]
    }
}
