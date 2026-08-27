use std::io::{BufRead, Write};

use anyhow::{Result, bail};
use lns_ipc::{Request, Response, VolumeInfo};

use crate::command::{CommandSpec, subcommand};
use crate::connector::LocalBoxFuture;

mod real;

pub use crate::service::TermInfo;
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
    #[command(about = "Remove every volume not attached to a running sandbox.")]
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
            .about("Manage the named volumes used with `lns run -v` (`docker volume`-style)."),
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
    term: TermInfo,
    input: &mut dyn BufRead,
    writer: &mut impl Write,
    err: &mut (impl tokio::io::AsyncWriteExt + Unpin),
) -> Result<i32> {
    match cmd {
        VolumeCommand::Ls(args) => ls(svc, args, writer).await,
        VolumeCommand::Create(args) => create(svc, &args.name, writer).await,
        VolumeCommand::Inspect(args) => inspect(svc, &args.name, args.output.format, writer).await,
        VolumeCommand::Rm(args) => rm(svc, &args.name, writer).await,
        VolumeCommand::Prune(args) => prune(svc, args.force, term, input, writer, err).await,
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
    in_use_by: Option<String>,
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
            ("IN USE", in_use_str(self.in_use_by.as_deref())),
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
            in_use_str(self.in_use_by.as_deref()),
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
    term: TermInfo,
    input: &mut dyn BufRead,
    writer: &mut impl Write,
    err: &mut (impl tokio::io::AsyncWriteExt + Unpin),
) -> Result<i32> {
    if !force {
        if !term.stdin_is_tty {
            bail!(
                "this removes every volume not attached to a running sandbox; there is no terminal to ask at, so pass --force to confirm"
            );
        }
        let unused = unused_volume_names(svc).await?;
        if unused.is_empty() {
            writeln!(writer, "No unused volumes.")?;
            return Ok(0);
        }
        crate::output::announce_prune_candidates(&unused, err).await?;
        if !confirm_prune(input, err).await? {
            return Ok(0);
        }
    }
    match send(svc, Request::PruneVolumes).await? {
        Response::VolumesPruned {
            removed,
            reclaimed_bytes,
            failed,
        } => {
            for name in &removed {
                writeln!(writer, "{name}")?;
            }
            if !removed.is_empty() {
                writeln!(
                    writer,
                    "Total reclaimed space: {}",
                    format_size(reclaimed_bytes)
                )?;
            }
            for f in &failed {
                writeln!(writer, "Failed to remove {}: {}", f.name, f.error)?;
            }
            if removed.is_empty() && failed.is_empty() {
                writeln!(writer, "No unused volumes.")?;
            }
            Ok(if failed.is_empty() { 0 } else { 1 })
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn unused_volume_names(svc: &dyn VolumeService) -> Result<Vec<String>> {
    match send(svc, Request::ListVolumes).await? {
        Response::VolumeList { volumes } => {
            let mut names: Vec<String> = volumes
                .into_iter()
                .filter(|volume| volume.in_use_by.is_none())
                .map(|volume| volume.name)
                .collect();
            names.sort_unstable();
            Ok(names)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn confirm_prune(
    input: &mut dyn BufRead,
    err: &mut (impl tokio::io::AsyncWriteExt + Unpin),
) -> Result<bool> {
    err.write_all(b"This removes every volume not attached to a running sandbox. Continue? [y/N] ")
        .await?;
    err.flush().await?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    let answer = line.trim();
    let yes = answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes");
    if !yes {
        err.write_all(b"Aborted.\n").await?;
        err.flush().await?;
    }
    Ok(yes)
}

fn in_use_str(holder: Option<&str>) -> String {
    match holder {
        Some(run_id) => format!("run {}", lns_ipc::short_run_id(run_id)),
        None => "-".to_string(),
    }
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
    use std::collections::VecDeque;
    use std::io::Cursor;
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
            in_use_by: None,
        }
    }

    fn at_a_terminal() -> TermInfo {
        TermInfo {
            stdin_is_tty: true,
            stdout_is_terminal: true,
        }
    }

    async fn run_cmd(cmd: &VolumeCommand, svc: &dyn VolumeService) -> Result<(i32, String)> {
        run_cmd_at(cmd, svc, at_a_terminal()).await
    }

    async fn run_cmd_at(
        cmd: &VolumeCommand,
        svc: &dyn VolumeService,
        term: TermInfo,
    ) -> Result<(i32, String)> {
        let mut input = Cursor::new(String::new());
        let mut buf = Vec::new();
        let mut err_buf = Vec::new();
        let code = run(cmd, svc, term, &mut input, &mut buf, &mut err_buf).await?;
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
            TermInfo::default(),
        )
        .await
        .unwrap_err();
        let err = format!("{err:#}");
        assert!(err.contains("--force"), "got: {err}");
        assert!(err.contains("no terminal to ask at"), "got: {err}");
    }

    #[tokio::test]
    async fn prune_without_force_says_so_when_every_volume_is_held() {
        let svc = CannedService::with([Some(Response::VolumeList {
            volumes: vec![VolumeInfo {
                in_use_by: Some("aa07".into()),
                ..volume("held")
            }],
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
    async fn unused_volume_listing_rejects_an_unrelated_variant() {
        let svc = CannedService::with([Some(Response::Pong)]);
        let err = unused_volume_names(&svc).await.unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn confirm_prune_accepts_yes_in_any_case() {
        for answer in ["y\n", "Y\n", "yes\n", "YES\n"] {
            let mut input = Cursor::new(answer.to_string());
            let mut buf = Vec::new();
            assert!(
                confirm_prune(&mut input, &mut buf).await.unwrap(),
                "{answer:?}"
            );
        }
    }

    #[tokio::test]
    async fn confirm_prune_treats_anything_else_as_decline() {
        for answer in ["n\n", "no\n", "\n", "yep\n"] {
            let mut input = Cursor::new(answer.to_string());
            let mut buf = Vec::new();
            assert!(
                !confirm_prune(&mut input, &mut buf).await.unwrap(),
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
    fn in_use_str_names_the_holder_or_dashes() {
        assert_eq!(
            in_use_str(Some("1a2b3c4d0000000000000000000000aa")),
            "run 1a2b3c4d0000"
        );
        assert_eq!(in_use_str(None), "-");
    }
}
