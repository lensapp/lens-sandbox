use std::io::Write;

use anyhow::{Result, bail};
use lns_ipc::{Request, Response, VolumeInfo};

use crate::command::{CommandSpec, subcommand};
use crate::local_future::LocalBoxFuture;
use crate::terminal::Terminal;

mod real;

pub use real::RealVolumeService;

#[derive(clap::Args)]
pub struct VolumeArgs {
    #[command(subcommand)]
    pub command: VolumeCommand,
}

#[derive(clap::Subcommand)]
pub enum VolumeCommand {
    #[command(about = "List named volumes with their on-disk size, age, and holder.")]
    Ls(VolumeLsArgs),
    #[command(about = "Create a named volume ahead of its first `lns run -v` attach.")]
    Create(VolumeNameArg),
    #[command(about = "Show a volume's capacity, on-disk bytes, age, and holder.")]
    Inspect(VolumeInspectArgs),
    #[command(about = "Remove a named volume; refused while a run holds it.")]
    Rm(VolumeNameArg),
    #[command(about = "Remove every volume no sandbox holds.")]
    Prune(VolumePruneArgs),
}

#[derive(clap::Args)]
pub struct VolumeLsArgs {
    #[command(flatten)]
    pub output: crate::output::OutputArgs,
}

#[derive(clap::Args)]
pub struct VolumeInspectArgs {
    #[arg(
        value_parser = parse_volume_name,
        help = "Volume name, as used with `lns run -v name:/path`."
    )]
    pub name: String,

    #[command(flatten)]
    pub output: crate::output::OutputArgs,
}

#[derive(clap::Args)]
pub struct VolumeNameArg {
    #[arg(
        value_parser = parse_volume_name,
        help = "Volume name, as used with `lns run -v name:/path`."
    )]
    pub name: String,
}

#[derive(clap::Args)]
pub struct VolumePruneArgs {
    #[arg(
        short = 'f',
        long,
        default_value_t = false,
        help = "Skip the confirmation prompt."
    )]
    pub force: bool,
}

fn parse_volume_name(s: &str) -> Result<String, String> {
    lns_ipc::validate_volume_name(s)?;
    Ok(s.to_string())
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<VolumeArgs>("volume")
            .about("Manage the named volumes used with `lns run -v`."),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "volume",
    augment,
    run: real::run,
    announces_update_check: true,
    owns_terminal: crate::command::never_owns_terminal,
};

/// Sends one volume request to the running service; `None` means the service did not answer.
pub trait VolumeService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>>;
}

pub async fn run(
    cmd: &VolumeCommand,
    svc: &dyn VolumeService,
    terminal: &mut dyn Terminal,
    writer: &mut impl Write,
    err: &mut (impl tokio::io::AsyncWriteExt + Unpin),
) -> Result<i32> {
    match cmd {
        VolumeCommand::Ls(args) => ls(svc, args, writer).await,
        VolumeCommand::Create(args) => create(svc, &args.name, writer).await,
        VolumeCommand::Inspect(args) => inspect(svc, &args.name, args.output.format, writer).await,
        VolumeCommand::Rm(args) => rm(svc, &args.name, writer).await,
        VolumeCommand::Prune(args) => prune(svc, args.force, terminal, writer, err).await,
    }
}

async fn send(svc: &dyn VolumeService, req: Request) -> Result<Response> {
    let response = svc
        .request(req)
        .await
        .ok_or_else(|| anyhow::anyhow!("no response from lns-service (is it running?)"))?;
    if let Response::Error { message } = response {
        bail!("{message}");
    }
    Ok(response)
}

async fn ls(svc: &dyn VolumeService, args: &VolumeLsArgs, writer: &mut impl Write) -> Result<i32> {
    match send(svc, Request::ListVolumes).await? {
        Response::VolumeList { volumes } => {
            let rows: Vec<VolumeRow> = volumes.iter().map(VolumeRow::new).collect();
            crate::output::emit(args.output.format, &rows, "No volumes.", writer)?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct VolumeRow {
    name: String,
    size_bytes: u64,
    disk_bytes: u64,
    created: String,
    in_use_by: Vec<String>,
}

impl VolumeRow {
    fn new(volume: &VolumeInfo) -> Self {
        Self {
            name: volume.name.clone(),
            size_bytes: volume.size_bytes,
            disk_bytes: volume.disk_bytes,
            created: volume.created.clone(),
            in_use_by: volume.in_use_by.clone(),
        }
    }
}

impl VolumeRow {
    fn fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("NAME", self.name.clone()),
            ("CAPACITY", format_size(self.size_bytes)),
            ("ON DISK", format_size(self.disk_bytes)),
            ("CREATED", crate::service::friendly_started(&self.created)),
            ("IN USE", in_use_str(&self.in_use_by)),
        ]
    }
}

impl crate::output::TableRow for VolumeRow {
    const HEADERS: &'static [&'static str] = &["NAME", "ON DISK", "CREATED", "IN USE"];

    fn cells(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            format_size(self.disk_bytes),
            crate::service::friendly_started(&self.created),
            in_use_str(&self.in_use_by),
        ]
    }
}

async fn create(svc: &dyn VolumeService, name: &str, writer: &mut impl Write) -> Result<i32> {
    let req = Request::CreateVolume {
        name: name.to_string(),
    };
    match send(svc, req).await? {
        Response::VolumeCreated { volume } => {
            writeln!(writer, "{}", volume.name)?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn inspect(
    svc: &dyn VolumeService,
    name: &str,
    format: crate::output::Format,
    writer: &mut impl Write,
) -> Result<i32> {
    let req = Request::InspectVolume {
        name: name.to_string(),
    };
    match send(svc, req).await? {
        Response::VolumeInspect { volume } => {
            let row = VolumeRow::new(&volume);
            crate::output::emit_fields(format, &row.fields(), &row, writer)?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn rm(svc: &dyn VolumeService, name: &str, writer: &mut impl Write) -> Result<i32> {
    let req = Request::RemoveVolume {
        name: name.to_string(),
    };
    match send(svc, req).await? {
        Response::VolumeRemoved { name } => {
            writeln!(writer, "{name}")?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn prune(
    svc: &dyn VolumeService,
    force: bool,
    terminal: &mut dyn Terminal,
    writer: &mut impl Write,
    err: &mut (impl tokio::io::AsyncWriteExt + Unpin),
) -> Result<i32> {
    if !force {
        if !terminal.is_available() {
            bail!(
                "this removes every volume no sandbox holds; there is no terminal to ask at, so pass --force to confirm"
            );
        }
        // The question has to be about the sweep that follows it, so the service answers both.
        let planned = prune_report(svc, true).await?;
        if planned.removed.is_empty() {
            return report_prune(writer, &planned);
        }
        crate::output::announce_prune_candidates(&planned.removed, err).await?;
        if !confirm_prune(terminal, err).await? {
            return Ok(0);
        }
    }
    let pruned = prune_report(svc, false).await?;
    report_prune(writer, &pruned)
}

#[derive(Debug)]
struct PruneOutcome {
    removed: Vec<String>,
    reclaimed_bytes: u64,
    failed: Vec<lns_ipc::VolumePruneFailure>,
}

async fn prune_report(svc: &dyn VolumeService, dry_run: bool) -> Result<PruneOutcome> {
    match send(svc, Request::PruneVolumes { dry_run }).await? {
        Response::VolumesPruned {
            removed,
            reclaimed_bytes,
            failed,
        } => Ok(PruneOutcome {
            removed,
            reclaimed_bytes,
            failed,
        }),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

fn report_prune(writer: &mut impl Write, outcome: &PruneOutcome) -> Result<i32> {
    for name in &outcome.removed {
        writeln!(writer, "{name}")?;
    }
    if !outcome.removed.is_empty() {
        writeln!(
            writer,
            "Total reclaimed space: {}",
            format_size(outcome.reclaimed_bytes)
        )?;
    }
    for f in &outcome.failed {
        writeln!(writer, "Failed to remove {}: {}", f.name, f.error)?;
    }
    if outcome.removed.is_empty() && outcome.failed.is_empty() {
        writeln!(writer, "No unused volumes.")?;
    }
    Ok(if outcome.failed.is_empty() { 0 } else { 1 })
}

async fn confirm_prune(
    terminal: &mut dyn Terminal,
    err: &mut (impl tokio::io::AsyncWriteExt + Unpin),
) -> Result<bool> {
    err.write_all(b"This removes every volume no sandbox holds. Continue? [y/N] ")
        .await?;
    err.flush().await?;
    let yes = crate::terminal::is_affirmative(&terminal.read_answer()?);
    if !yes {
        err.write_all(b"Aborted.\n").await?;
        err.flush().await?;
    }
    Ok(yes)
}

fn in_use_str(holders: &[String]) -> String {
    if holders.is_empty() {
        return "-".to_string();
    }
    holders.join(", ")
}

pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value.fract() == 0.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::ScriptedTerminal;
    use std::collections::VecDeque;
    use std::sync::Mutex;

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

    impl VolumeService for CannedService {
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

    fn volume(name: &str) -> VolumeInfo {
        VolumeInfo {
            name: name.to_string(),
            size_bytes: 10 * 1024_u64.pow(3),
            disk_bytes: 32 * 1024 * 1024,
            created: "2026-06-01T12:00:00Z".to_string(),
            in_use_by: Vec::new(),
        }
    }

    async fn run_cmd(cmd: &VolumeCommand, svc: &dyn VolumeService) -> Result<(i32, String)> {
        run_cmd_at(cmd, svc, ScriptedTerminal::answering(&[])).await
    }

    async fn run_cmd_at(
        cmd: &VolumeCommand,
        svc: &dyn VolumeService,
        mut terminal: ScriptedTerminal,
    ) -> Result<(i32, String)> {
        let mut buf = Vec::new();
        let mut err_buf = Vec::new();
        let code = run(cmd, svc, &mut terminal, &mut buf, &mut err_buf).await?;
        buf.extend_from_slice(&err_buf);
        Ok((code, String::from_utf8(buf).unwrap()))
    }

    fn ls_args() -> VolumeLsArgs {
        VolumeLsArgs {
            output: crate::output::OutputArgs {
                format: crate::output::Format::Table,
            },
        }
    }

    fn name_arg(name: &str) -> VolumeNameArg {
        VolumeNameArg {
            name: name.to_string(),
        }
    }

    fn inspect_arg(name: &str, format: crate::output::Format) -> VolumeInspectArgs {
        VolumeInspectArgs {
            name: name.to_string(),
            output: crate::output::OutputArgs { format },
        }
    }

    #[tokio::test]
    async fn each_verb_rejects_a_mismatched_response_kind() {
        for cmd in [
            VolumeCommand::Ls(ls_args()),
            VolumeCommand::Create(name_arg("v")),
            VolumeCommand::Inspect(inspect_arg("v", crate::output::Format::Json)),
            VolumeCommand::Rm(name_arg("v")),
            VolumeCommand::Prune(VolumePruneArgs { force: true }),
        ] {
            let svc = CannedService::with([Some(Response::Pong)]);
            let err = run_cmd(&cmd, &svc).await.unwrap_err().to_string();
            assert!(err.contains("unexpected response"), "got: {err}");
        }
    }

    #[tokio::test]
    async fn prune_without_a_terminal_refuses_and_names_the_flag_that_answers_it() {
        let svc = CannedService::with([]);
        let err = run_cmd_at(
            &VolumeCommand::Prune(VolumePruneArgs { force: false }),
            &svc,
            ScriptedTerminal::absent(),
        )
        .await
        .unwrap_err();
        let err = format!("{err:#}");
        assert!(err.contains("--force"), "got: {err}");
        assert!(err.contains("no terminal to ask at"), "got: {err}");
    }

    #[tokio::test]
    async fn a_prune_with_nothing_to_remove_says_so_without_asking() {
        let svc = CannedService::with([Some(Response::VolumesPruned {
            removed: Vec::new(),
            reclaimed_bytes: 0,
            failed: Vec::new(),
        })]);
        let (code, out) = run_cmd(
            &VolumeCommand::Prune(VolumePruneArgs { force: false }),
            &svc,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert!(out.contains("No unused volumes."), "got: {out}");
        assert!(!out.contains("Continue?"), "got: {out}");
    }

    #[tokio::test]
    async fn a_prune_that_would_remove_nothing_reports_what_stands_in_the_way_without_asking() {
        let svc = CannedService::with([Some(Response::VolumesPruned {
            removed: Vec::new(),
            reclaimed_bytes: 0,
            failed: vec![lns_ipc::VolumePruneFailure {
                name: "scratch".into(),
                error: "run aa07's record cannot be read".into(),
            }],
        })]);
        let (code, out) = run_cmd(
            &VolumeCommand::Prune(VolumePruneArgs { force: false }),
            &svc,
        )
        .await
        .unwrap();
        assert_eq!(
            code, 1,
            "a prune that removed nothing it was asked to is a failure"
        );
        assert!(out.contains("Failed to remove scratch"), "got: {out}");
        assert!(
            !out.contains("Continue?"),
            "there is nothing to consent to, so the question must not be asked: {out}"
        );
    }

    #[tokio::test]
    async fn the_prune_plan_rejects_an_unrelated_variant() {
        let svc = CannedService::with([Some(Response::Pong)]);
        let err = prune_report(&svc, true).await.unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn confirm_prune_accepts_yes_in_any_case() {
        for answer in ["y\n", "Y\n", "yes\n", "YES\n"] {
            let mut terminal = ScriptedTerminal::answering(&[answer]);
            let mut buf = Vec::new();
            assert!(
                confirm_prune(&mut terminal, &mut buf).await.unwrap(),
                "{answer:?}"
            );
        }
    }

    #[tokio::test]
    async fn confirm_prune_treats_anything_else_as_decline() {
        for answer in ["n\n", "no\n", "\n", "yep\n"] {
            let mut terminal = ScriptedTerminal::answering(&[answer]);
            let mut buf = Vec::new();
            assert!(
                !confirm_prune(&mut terminal, &mut buf).await.unwrap(),
                "{answer:?}"
            );
            let out = String::from_utf8(buf).unwrap();
            assert!(out.contains("Aborted."), "got: {out}");
        }
    }

    #[tokio::test]
    async fn rm_echoes_the_name_the_service_confirmed() {
        let svc = CannedService::with([Some(Response::VolumeRemoved {
            name: "prism-data".into(),
        })]);
        let (code, out) = run_cmd(&VolumeCommand::Rm(name_arg("prism-data")), &svc)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(out, "prism-data\n");
    }

    #[tokio::test]
    async fn inspect_emits_the_volume_as_one_object_not_a_list() {
        let svc = CannedService::with([Some(Response::VolumeInspect {
            volume: volume("prism-data"),
        })]);
        let (_, out) = run_cmd(
            &VolumeCommand::Inspect(inspect_arg("prism-data", crate::output::Format::Json)),
            &svc,
        )
        .await
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed.is_object(), "one thing is one object: {parsed}");
        assert_eq!(parsed["name"], "prism-data");
        assert_eq!(parsed["sizeBytes"], 10_737_418_240_u64);
        assert_eq!(parsed["diskBytes"], 33_554_432);
    }

    #[tokio::test]
    async fn inspect_as_a_table_names_the_capacity_the_json_carries_raw() {
        let svc = CannedService::with([Some(Response::VolumeInspect {
            volume: volume("prism-data"),
        })]);
        let (_, out) = run_cmd(
            &VolumeCommand::Inspect(inspect_arg("prism-data", crate::output::Format::Table)),
            &svc,
        )
        .await
        .unwrap();
        assert!(out.contains("CAPACITY"), "got: {out}");
        assert!(out.contains("10 GiB"), "got: {out}");
        assert!(out.contains("ON DISK"), "got: {out}");
    }

    #[test]
    fn format_size_picks_the_largest_fitting_binary_unit() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1 KiB");
        assert_eq!(format_size(1536), "1.5 KiB");
        assert_eq!(format_size(32 * 1024 * 1024), "32 MiB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3 GiB");
        assert_eq!(format_size(2 * 1024_u64.pow(4)), "2 TiB");
        assert_eq!(format_size(2048 * 1024_u64.pow(4)), "2048 TiB");
    }

    #[test]
    fn in_use_str_names_every_holder_or_dashes() {
        assert_eq!(in_use_str(&["reviewer".to_string()]), "reviewer");
        assert_eq!(
            in_use_str(&["reviewer".to_string(), "auditor".to_string()]),
            "reviewer, auditor",
            "a stopped sandbox holds a volume too, so several can hold one at once"
        );
        assert_eq!(in_use_str(&[]), "-");
    }
}
