use std::io::{BufRead, Write};

use anyhow::{Result, bail};
use lns_ipc::{Request, Response, VolumeInfo};

use crate::cli::VolumeCommand;
use crate::command::{CommandSpec, subcommand};
use crate::integration::LocalBoxFuture;

mod real;

pub use real::RealVolumeService;

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<crate::cli::VolumeArgs>("volume")
            .about("Manage the named volumes used with `lns run -v` (`docker volume`-style)."),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "volume",
    augment,
    run: real::run,
    announces_update_check: true,
};

/// Sends one volume request to the running service; `None` means the service did not answer.
pub trait VolumeService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>>;
}

pub async fn run(
    cmd: &VolumeCommand,
    svc: &dyn VolumeService,
    input: &mut dyn BufRead,
    writer: &mut impl Write,
) -> Result<i32> {
    match cmd {
        VolumeCommand::Ls => ls(svc, writer).await,
        VolumeCommand::Create(args) => create(svc, &args.name, writer).await,
        VolumeCommand::Inspect(args) => inspect(svc, &args.name, writer).await,
        VolumeCommand::Rm(args) => rm(svc, &args.name, writer).await,
        VolumeCommand::Prune(args) => prune(svc, args.force, input, writer).await,
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

async fn ls(svc: &dyn VolumeService, writer: &mut impl Write) -> Result<i32> {
    match send(svc, Request::ListVolumes).await? {
        Response::VolumeList { volumes } => {
            render_volume_table(writer, &volumes)?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
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

async fn inspect(svc: &dyn VolumeService, name: &str, writer: &mut impl Write) -> Result<i32> {
    let req = Request::InspectVolume {
        name: name.to_string(),
    };
    match send(svc, req).await? {
        Response::VolumeInspect { volume } => {
            writeln!(writer, "{}", serde_json::to_string_pretty(&volume)?)?;
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
    input: &mut dyn BufRead,
    writer: &mut impl Write,
) -> Result<i32> {
    if !force && !confirm_prune(input, writer)? {
        return Ok(0);
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

fn confirm_prune(input: &mut dyn BufRead, writer: &mut impl Write) -> Result<bool> {
    write!(
        writer,
        "This removes every volume not attached to a running sandbox. Continue? [y/N] "
    )?;
    writer.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    let answer = line.trim();
    let yes = answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes");
    if !yes {
        writeln!(writer, "Aborted.")?;
    }
    Ok(yes)
}

fn render_volume_table<W: Write>(out: &mut W, rows: &[VolumeInfo]) -> std::io::Result<()> {
    let cells: Vec<(String, String, String, String)> = rows
        .iter()
        .map(|v| {
            (
                v.name.clone(),
                format_size(v.disk_bytes),
                crate::service::friendly_started(&v.created),
                in_use_str(v.in_use_by),
            )
        })
        .collect();
    let width = |header: &str, pick: fn(&(String, String, String, String)) -> &String| {
        header
            .len()
            .max(cells.iter().map(|c| pick(c).len()).max().unwrap_or(0))
    };
    let name_w = width("NAME", |c| &c.0);
    let disk_w = width("ON DISK", |c| &c.1);
    let created_w = width("CREATED", |c| &c.2);

    writeln!(
        out,
        "{:<name_w$}  {:<disk_w$}  {:<created_w$}  IN USE",
        "NAME", "ON DISK", "CREATED",
    )?;
    for (name, disk, created, in_use) in &cells {
        writeln!(
            out,
            "{name:<name_w$}  {disk:<disk_w$}  {created:<created_w$}  {in_use}",
        )?;
    }
    Ok(())
}

fn in_use_str(holder: Option<u32>) -> String {
    match holder {
        Some(run_id) => format!("run #{run_id}"),
        None => "-".to_string(),
    }
}

fn format_size(bytes: u64) -> String {
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

    async fn run_cmd(cmd: &VolumeCommand, svc: &dyn VolumeService) -> Result<(i32, String)> {
        let mut input = Cursor::new(String::new());
        let mut buf = Vec::new();
        let code = run(cmd, svc, &mut input, &mut buf).await?;
        Ok((code, String::from_utf8(buf).unwrap()))
    }

    fn name_arg(name: &str) -> crate::cli::VolumeNameArg {
        crate::cli::VolumeNameArg {
            name: name.to_string(),
        }
    }

    #[tokio::test]
    async fn each_verb_rejects_a_mismatched_response_kind() {
        for cmd in [
            VolumeCommand::Ls,
            VolumeCommand::Create(name_arg("v")),
            VolumeCommand::Inspect(name_arg("v")),
            VolumeCommand::Rm(name_arg("v")),
            VolumeCommand::Prune(crate::cli::VolumePruneArgs { force: true }),
        ] {
            let svc = CannedService::with([Some(Response::Pong)]);
            let err = run_cmd(&cmd, &svc).await.unwrap_err().to_string();
            assert!(err.contains("unexpected response"), "got: {err}");
        }
    }

    #[tokio::test]
    async fn prune_without_force_reads_eof_as_decline() {
        let svc = CannedService::with([]);
        let (code, out) = run_cmd(
            &VolumeCommand::Prune(crate::cli::VolumePruneArgs { force: false }),
            &svc,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert!(out.contains("Aborted."), "got: {out}");
    }

    #[tokio::test]
    async fn confirm_prune_accepts_yes_in_any_case() {
        for answer in ["y\n", "Y\n", "yes\n", "YES\n"] {
            let mut input = Cursor::new(answer.to_string());
            let mut buf = Vec::new();
            assert!(confirm_prune(&mut input, &mut buf).unwrap(), "{answer:?}");
        }
    }

    #[tokio::test]
    async fn confirm_prune_treats_anything_else_as_decline() {
        for answer in ["n\n", "no\n", "\n", "yep\n"] {
            let mut input = Cursor::new(answer.to_string());
            let mut buf = Vec::new();
            assert!(!confirm_prune(&mut input, &mut buf).unwrap(), "{answer:?}");
            let out = String::from_utf8(buf).unwrap();
            assert!(out.contains("Aborted."), "got: {out}");
        }
    }

    #[tokio::test]
    async fn ls_renders_an_empty_table_with_headers_only() {
        let svc = CannedService::with([Some(Response::VolumeList {
            volumes: Vec::new(),
        })]);
        let (code, out) = run_cmd(&VolumeCommand::Ls, &svc).await.unwrap();
        assert_eq!(code, 0);
        for header in ["NAME", "ON DISK", "CREATED", "IN USE"] {
            assert!(out.contains(header), "missing {header} in {out:?}");
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
    async fn inspect_emits_the_volume_as_pretty_json() {
        let svc = CannedService::with([Some(Response::VolumeInspect {
            volume: volume("prism-data"),
        })]);
        let (_, out) = run_cmd(&VolumeCommand::Inspect(name_arg("prism-data")), &svc)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["name"], "prism-data");
        assert_eq!(parsed["size_bytes"], 10_737_418_240_u64);
        assert_eq!(parsed["disk_bytes"], 33_554_432);
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
        assert_eq!(in_use_str(Some(7)), "run #7");
        assert_eq!(in_use_str(None), "-");
    }
}
