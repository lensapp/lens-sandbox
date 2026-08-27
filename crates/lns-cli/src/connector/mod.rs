use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};
use lns_ipc::{ConnectorView, Request, Response};

use crate::command::{CommandSpec, subcommand};
use crate::local_future::LocalBoxFuture;

mod real;

pub use real::RealConnectorService;

#[derive(clap::Args)]
pub struct ConnectorArgs {
    #[command(subcommand)]
    pub command: ConnectorCommand,
}

#[derive(clap::Subcommand)]
pub enum ConnectorCommand {
    #[command(
        about = "Make a pulled or local connector available on this machine. Installing grants nothing."
    )]
    Install(InstallArgs),
    #[command(
        about = "Remove a connector from this machine, with every profile it held. Grants stay."
    )]
    Uninstall(NameArg),
    #[command(about = "List what is installed: what each connector serves, and the profiles held.")]
    List(ListArgs),
}

#[derive(clap::Args)]
pub struct InstallArgs {
    #[arg(
        help = "A published connector reference, or a path to a directory holding its lns.yaml."
    )]
    pub source: String,
}

#[derive(clap::Args)]
pub struct NameArg {
    #[arg(help = "The connector's name, as `lns connector list` shows it.")]
    pub name: String,
}

#[derive(clap::Args)]
pub struct ListArgs {
    #[command(flatten)]
    pub output: crate::output::OutputArgs,
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<ConnectorArgs>("connector").about(
            "Decide what this machine offers: install a connector, or see what is installed.",
        ),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "connector",
    augment,
    run: real::run,
    announces_update_check: true,
    owns_terminal: crate::command::never_owns_terminal,
};

/// Sends one connector request to the running service; `None` means the service did not answer.
pub trait ConnectorService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>>;
}

pub async fn run(
    cmd: &ConnectorCommand,
    svc: &dyn ConnectorService,
    cwd: &Path,
    writer: &mut impl Write,
) -> Result<i32> {
    match cmd {
        ConnectorCommand::Install(args) => install(svc, &args.source, cwd, writer).await,
        ConnectorCommand::Uninstall(args) => uninstall(svc, &args.name, writer).await,
        ConnectorCommand::List(args) => list(svc, args.output.format, writer).await,
    }
}

async fn send(svc: &dyn ConnectorService, req: Request) -> Result<Response> {
    let response = svc
        .request(req)
        .await
        .ok_or_else(|| anyhow::anyhow!("no response from lns-service (is it running?)"))?;
    if let Response::Error { message } = response {
        bail!("{message}");
    }
    Ok(response)
}

async fn install(
    svc: &dyn ConnectorService,
    source: &str,
    cwd: &Path,
    writer: &mut impl Write,
) -> Result<i32> {
    let req = Request::InstallConnector {
        source: crate::run::target::root_named_directory(source, cwd, "connector directory")?,
    };
    match send(svc, req).await? {
        Response::ConnectorInstalled { connector } => {
            report_installed(&connector, writer)?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

/// Says what installing did and did not do, because §7.1's "installing grants nothing" is the thing a user is most likely to assume otherwise.
fn report_installed(connector: &ConnectorView, writer: &mut impl Write) -> std::io::Result<()> {
    writeln!(
        writer,
        "installed {} ({})",
        connector.name, connector.digest
    )?;
    writeln!(writer, "  serves: {}", connector.serves.join(", "))?;
    for method in &connector.methods {
        let needs = if method.needs_connect {
            "connect to use"
        } else {
            "no connect needed"
        };
        let unsupported = if method.offerable {
            ""
        } else {
            " — needs a newer lns"
        };
        writeln!(writer, "  method {}: {needs}{unsupported}", method.label)?;
    }
    writeln!(writer, "nothing is granted yet")
}

async fn uninstall(svc: &dyn ConnectorService, name: &str, writer: &mut impl Write) -> Result<i32> {
    let req = Request::UninstallConnector {
        name: name.to_string(),
    };
    match send(svc, req).await? {
        Response::ConnectorUninstalled {
            name,
            dropped_profiles,
        } => {
            writeln!(writer, "uninstalled {name}")?;
            if dropped_profiles > 0 {
                writeln!(writer, "  dropped {dropped_profiles} profile(s)")?;
            }
            writeln!(
                writer,
                "  projects that granted it keep that decision; reinstalling the same bytes resumes it"
            )?;
            Ok(0)
        }
        Response::ConnectorUnknown { name } => {
            writeln!(writer, "no connector named {name} is installed")?;
            Ok(1)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn list(
    svc: &dyn ConnectorService,
    format: crate::output::Format,
    writer: &mut impl Write,
) -> Result<i32> {
    match send(svc, Request::ListConnectors).await? {
        Response::ConnectorList { connectors } => {
            let rows: Vec<ConnectorRow> = connectors.iter().map(ConnectorRow::new).collect();
            crate::output::emit(format, &rows, "No connectors installed.", writer)?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectorRow {
    name: String,
    digest: String,
    serves: Vec<String>,
    methods: Vec<String>,
    profiles: Vec<String>,
}

impl ConnectorRow {
    fn new(connector: &ConnectorView) -> Self {
        Self {
            name: connector.name.clone(),
            digest: connector.digest.clone(),
            serves: connector.serves.clone(),
            methods: connector
                .methods
                .iter()
                .map(|m| {
                    if m.needs_connect {
                        m.label.clone()
                    } else {
                        format!("{} (no connect)", m.label)
                    }
                })
                .collect(),
            profiles: connector.profiles.iter().map(|p| p.label.clone()).collect(),
        }
    }
}

impl crate::output::TableRow for ConnectorRow {
    const HEADERS: &'static [&'static str] = &["NAME", "SERVES", "METHODS", "PROFILES"];

    fn cells(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.serves.join(", "),
            self.methods.join(", "),
            none_when_empty(&self.profiles),
        ]
    }
}

/// A connector with no profile is the normal state after an install, so the column says so rather than sitting blank.
fn none_when_empty(profiles: &[String]) -> String {
    if profiles.is_empty() {
        return "none".to_string();
    }
    profiles.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Answers with whatever the test queued, so the arms that reject a wrong answer are reachable.
    struct CannedService {
        responses: Mutex<VecDeque<Option<Response>>>,
    }

    impl CannedService {
        fn with(responses: impl IntoIterator<Item = Option<Response>>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
            }
        }
    }

    impl ConnectorService for CannedService {
        fn request(&self, _req: Request) -> LocalBoxFuture<'_, Option<Response>> {
            let resp = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("CannedService: no canned response left");
            Box::pin(async move { resp })
        }
    }

    fn cwd() -> std::path::PathBuf {
        std::path::PathBuf::from("/work")
    }

    #[tokio::test]
    async fn a_path_that_is_not_utf8_is_refused_rather_than_mangled() {
        // The absolutised path is the operand the service opens, so a lossy conversion would send it a path naming a directory nobody has.
        use std::os::unix::ffi::OsStrExt;
        let odd = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/work/\xff"));
        let svc = CannedService::with([]);
        let mut out = Vec::new();
        let err = run(
            &ConnectorCommand::Install(InstallArgs {
                source: "./some-provider".into(),
            }),
            &svc,
            &odd,
            &mut out,
        )
        .await
        .expect_err("a non-utf8 path must be refused before anything is sent");
        assert!(format!("{err:#}").contains("utf-8"), "got: {err:#}");
    }

    /// A verb that answers with another verb's response is a protocol mismatch, not a refusal, so each verb says so rather than treating it as success.
    #[tokio::test]
    async fn every_verb_refuses_a_response_meant_for_another() {
        let cases: Vec<(ConnectorCommand, Response)> = vec![
            (
                ConnectorCommand::Install(InstallArgs {
                    source: "ghcr.io/acme/c:1".into(),
                }),
                Response::Pong,
            ),
            (
                ConnectorCommand::Uninstall(NameArg {
                    name: "some-provider".into(),
                }),
                Response::Pong,
            ),
            (
                ConnectorCommand::List(ListArgs {
                    output: crate::output::OutputArgs {
                        format: crate::output::Format::Table,
                    },
                }),
                Response::Pong,
            ),
        ];
        for (cmd, wrong) in cases {
            let svc = CannedService::with([Some(wrong)]);
            let mut out = Vec::new();
            let err = run(&cmd, &svc, &cwd(), &mut out)
                .await
                .expect_err("a mismatched response must not read as success");
            assert!(
                format!("{err:#}").contains("unexpected response"),
                "got: {err:#}"
            );
        }
    }

    #[tokio::test]
    async fn an_install_naming_a_method_this_version_cannot_offer_says_so() {
        // §3.2.2: an unknown auth.kind parses and leaves the method unofferable, so the card cannot offer it and the install has to explain the gap.
        let connector = ConnectorView {
            name: "some-provider".into(),
            digest: "sha256:abc".into(),
            serves: vec!["api.some-provider.example".into()],
            methods: vec![lns_ipc::ConnectorMethodView {
                name: "future".into(),
                label: "Future sign-in".into(),
                needs_connect: true,
                offerable: false,
            }],
            profiles: Vec::new(),
        };
        let svc = CannedService::with([Some(Response::ConnectorInstalled { connector })]);
        let mut out = Vec::new();
        let code = run(
            &ConnectorCommand::Install(InstallArgs {
                source: "ghcr.io/acme/c:1".into(),
            }),
            &svc,
            &cwd(),
            &mut out,
        )
        .await
        .expect("the install itself succeeds");
        assert_eq!(code, 0);
        let text = String::from_utf8(out).expect("utf-8");
        assert!(text.contains("needs a newer lns"), "got: {text}");
    }

    #[test]
    fn the_profiles_column_lists_what_the_machine_holds() {
        let row = ConnectorRow::new(&ConnectorView {
            name: "some-provider".into(),
            digest: "sha256:abc".into(),
            serves: vec!["api.some-provider.example".into()],
            methods: Vec::new(),
            profiles: vec![
                lns_ipc::ConnectorProfileView {
                    label: "work".into(),
                    method: "token".into(),
                    authority: Vec::new(),
                },
                lns_ipc::ConnectorProfileView {
                    label: "personal".into(),
                    method: "token".into(),
                    authority: Vec::new(),
                },
            ],
        });
        use crate::output::TableRow;
        assert_eq!(row.cells()[3], "work, personal");
    }
}
