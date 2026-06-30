use std::io::{BufRead, Write};

use anyhow::{Result, bail};
use lns_ipc::{ImageInfo, Request, Response};

use crate::command::{CommandSpec, subcommand};
use crate::integration::LocalBoxFuture;

mod real;

pub use real::RealImageService;

#[derive(clap::Args)]
pub struct ImageArgs {
    #[command(subcommand)]
    pub command: ImageCommand,
}

#[derive(clap::Subcommand)]
pub enum ImageCommand {
    #[command(about = "Pull an image into the cache ahead of `lns run`, resolving its digest.")]
    Pull(ImageRefArg),
    #[command(about = "List cached images with their digest, size, age, and user.")]
    Ls,
    #[command(about = "Remove a cached image; refused while a run uses it.")]
    Rm(ImageRefArg),
    #[command(about = "Remove every cached image not used by a running sandbox.")]
    Prune(ImagePruneArgs),
}

#[derive(clap::Args)]
pub struct ImageRefArg {
    #[arg(help = "Image reference, e.g. `alpine:3.20` or `alpine@sha256:…`.")]
    pub image: String,
}

#[derive(clap::Args)]
pub struct ImagePruneArgs {
    #[arg(
        short = 'f',
        long,
        default_value_t = false,
        help = "Skip the confirmation prompt."
    )]
    pub force: bool,
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<ImageArgs>("image").about(
            "Manage the cached OCI images that `lns run` boots from (`docker image`-style).",
        ),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "image",
    augment,
    run: real::run,
    announces_update_check: true,
    owns_terminal: false,
};

/// Sends one image request to the running service; `None` means the service did not answer.
pub trait ImageService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>>;
}

pub async fn run(
    cmd: &ImageCommand,
    svc: &dyn ImageService,
    input: &mut dyn BufRead,
    writer: &mut impl Write,
) -> Result<i32> {
    match cmd {
        ImageCommand::Pull(args) => pull(svc, &args.image, writer).await,
        ImageCommand::Ls => ls(svc, writer).await,
        ImageCommand::Rm(args) => rm(svc, &args.image, writer).await,
        ImageCommand::Prune(args) => prune(svc, args.force, input, writer).await,
    }
}

async fn send(svc: &dyn ImageService, req: Request) -> Result<Response> {
    let response = svc
        .request(req)
        .await
        .ok_or_else(|| anyhow::anyhow!("no response from lns-service (is it running?)"))?;
    if let Response::Error { message } = response {
        bail!("{message}");
    }
    Ok(response)
}

async fn pull(svc: &dyn ImageService, image: &str, writer: &mut impl Write) -> Result<i32> {
    let req = Request::PullImage {
        image: image.to_string(),
    };
    match send(svc, req).await? {
        Response::ImagePulled { image } => {
            let layers = if image.layers == 1 { "layer" } else { "layers" };
            writeln!(writer, "Pulled {}", image.reference)?;
            writeln!(writer, "Digest: {}", image.digest)?;
            writeln!(
                writer,
                "Size: {} ({} {layers})",
                format_size(image.size_bytes),
                image.layers
            )?;
            writeln!(
                writer,
                "Pin: {}",
                pin_reference(&image.reference, &image.digest)
            )?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

fn pin_reference(reference: &str, digest: &str) -> String {
    if let Some((repo, _)) = reference.split_once('@') {
        return format!("{repo}@{digest}");
    }
    let repo = match reference.rfind(':') {
        Some(i) if i > reference.rfind('/').unwrap_or(0) => &reference[..i],
        _ => reference,
    };
    format!("{repo}@{digest}")
}

async fn ls(svc: &dyn ImageService, writer: &mut impl Write) -> Result<i32> {
    match send(svc, Request::ListImages).await? {
        Response::ImageList { images } => {
            render_image_table(writer, &images)?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn rm(svc: &dyn ImageService, image: &str, writer: &mut impl Write) -> Result<i32> {
    let req = Request::RemoveImage {
        image: image.to_string(),
    };
    match send(svc, req).await? {
        Response::ImageRemoved {
            reference,
            reclaimed_bytes,
        } => {
            writeln!(writer, "{reference}")?;
            writeln!(
                writer,
                "Total reclaimed space: {}",
                format_size(reclaimed_bytes)
            )?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn prune(
    svc: &dyn ImageService,
    force: bool,
    input: &mut dyn BufRead,
    writer: &mut impl Write,
) -> Result<i32> {
    if !force && !confirm_prune(input, writer)? {
        return Ok(0);
    }
    match send(svc, Request::PruneImages).await? {
        Response::ImagesPruned {
            removed,
            reclaimed_bytes,
        } => {
            if removed.is_empty() && reclaimed_bytes == 0 {
                writeln!(writer, "No unused images.")?;
                return Ok(0);
            }
            for reference in &removed {
                writeln!(writer, "{reference}")?;
            }
            writeln!(
                writer,
                "Total reclaimed space: {}",
                format_size(reclaimed_bytes)
            )?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

fn confirm_prune(input: &mut dyn BufRead, writer: &mut impl Write) -> Result<bool> {
    write!(
        writer,
        "This removes every cached image not used by a running sandbox. Continue? [y/N] "
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

fn render_image_table<W: Write>(out: &mut W, rows: &[ImageInfo]) -> std::io::Result<()> {
    let cells: Vec<(String, String, String, String, String)> = rows
        .iter()
        .map(|i| {
            (
                i.reference.clone(),
                short_digest(&i.digest),
                format_size(i.size_bytes),
                crate::service::friendly_started(&i.pulled),
                in_use_str(i.in_use_by.as_deref()),
            )
        })
        .collect();
    let width = |header: &str, pick: fn(&(String, String, String, String, String)) -> &String| {
        header
            .len()
            .max(cells.iter().map(|c| pick(c).len()).max().unwrap_or(0))
    };
    let ref_w = width("REFERENCE", |c| &c.0);
    let digest_w = width("DIGEST", |c| &c.1);
    let size_w = width("SIZE", |c| &c.2);
    let pulled_w = width("PULLED", |c| &c.3);

    writeln!(
        out,
        "{:<ref_w$}  {:<digest_w$}  {:<size_w$}  {:<pulled_w$}  IN USE",
        "REFERENCE", "DIGEST", "SIZE", "PULLED",
    )?;
    for (reference, digest, size, pulled, in_use) in &cells {
        writeln!(
            out,
            "{reference:<ref_w$}  {digest:<digest_w$}  {size:<size_w$}  {pulled:<pulled_w$}  {in_use}",
        )?;
    }
    Ok(())
}

fn short_digest(digest: &str) -> String {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    hex.chars().take(12).collect()
}

fn in_use_str(holder: Option<&str>) -> String {
    match holder {
        Some(run_id) => format!("run {}", lns_ipc::short_run_id(run_id)),
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

    impl ImageService for CannedService {
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

    fn image(reference: &str) -> ImageInfo {
        ImageInfo {
            reference: reference.to_string(),
            digest: format!("sha256:{}", "a".repeat(64)),
            size_bytes: 3 * 1024 * 1024,
            layers: 2,
            pulled: "2026-06-01T12:00:00Z".to_string(),
            in_use_by: None,
        }
    }

    async fn run_cmd(cmd: &ImageCommand, svc: &dyn ImageService) -> Result<(i32, String)> {
        let mut input = Cursor::new(String::new());
        let mut buf = Vec::new();
        let code = run(cmd, svc, &mut input, &mut buf).await?;
        Ok((code, String::from_utf8(buf).unwrap()))
    }

    fn ref_arg(image: &str) -> ImageRefArg {
        ImageRefArg {
            image: image.to_string(),
        }
    }

    #[tokio::test]
    async fn each_verb_rejects_a_mismatched_response_kind() {
        for cmd in [
            ImageCommand::Pull(ref_arg("some-image:1.0")),
            ImageCommand::Ls,
            ImageCommand::Rm(ref_arg("some-image:1.0")),
            ImageCommand::Prune(ImagePruneArgs { force: true }),
        ] {
            let svc = CannedService::with([Some(Response::Pong)]);
            let err = run_cmd(&cmd, &svc).await.unwrap_err().to_string();
            assert!(err.contains("unexpected response"), "got: {err}");
        }
    }

    #[tokio::test]
    async fn a_service_refusal_surfaces_its_message() {
        let svc = CannedService::with([Some(Response::Error {
            message: "no such image: docker.io/library/some-image:1.0".into(),
        })]);
        let err = run_cmd(&ImageCommand::Rm(ref_arg("some-image:1.0")), &svc)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no such image"), "got: {err}");
    }

    #[tokio::test]
    async fn pull_reports_digest_size_and_a_pinnable_reference() {
        let svc = CannedService::with([Some(Response::ImagePulled {
            image: ImageInfo {
                reference: "docker.io/library/some-image:1.0".into(),
                digest: format!("sha256:{}", "a".repeat(64)),
                size_bytes: 3 * 1024 * 1024,
                layers: 1,
                pulled: "2026-06-01T12:00:00Z".into(),
                in_use_by: None,
            },
        })]);
        let (code, out) = run_cmd(&ImageCommand::Pull(ref_arg("some-image:1.0")), &svc)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert!(
            out.contains("Pulled docker.io/library/some-image:1.0"),
            "got: {out}"
        );
        assert!(
            out.contains(&format!("Digest: sha256:{}", "a".repeat(64))),
            "got: {out}"
        );
        assert!(out.contains("Size: 3 MiB (1 layer)"), "got: {out}");
        assert!(
            out.contains(&format!(
                "Pin: docker.io/library/some-image@sha256:{}",
                "a".repeat(64)
            )),
            "got: {out}"
        );
    }

    #[tokio::test]
    async fn pull_pluralizes_the_layer_count() {
        let svc = CannedService::with([Some(Response::ImagePulled {
            image: image("docker.io/library/some-image:1.0"),
        })]);
        let (_, out) = run_cmd(&ImageCommand::Pull(ref_arg("some-image:1.0")), &svc)
            .await
            .unwrap();
        assert!(out.contains("(2 layers)"), "got: {out}");
    }

    #[test]
    fn pin_reference_swaps_a_tag_for_the_digest() {
        assert_eq!(
            pin_reference("docker.io/library/some-image:1.0", "sha256:abc"),
            "docker.io/library/some-image@sha256:abc"
        );
    }

    #[test]
    fn pin_reference_is_not_fooled_by_a_registry_port() {
        assert_eq!(
            pin_reference("registry.example.test:5000/some-image", "sha256:abc"),
            "registry.example.test:5000/some-image@sha256:abc"
        );
        assert_eq!(
            pin_reference("registry.example.test:5000/some-image:1.0", "sha256:abc"),
            "registry.example.test:5000/some-image@sha256:abc"
        );
    }

    #[test]
    fn pin_reference_keeps_an_already_pinned_reference_pinned() {
        assert_eq!(
            pin_reference("registry.example.test/some-image@sha256:abc", "sha256:abc"),
            "registry.example.test/some-image@sha256:abc"
        );
    }

    #[tokio::test]
    async fn ls_renders_an_empty_table_with_headers_only() {
        let svc = CannedService::with([Some(Response::ImageList { images: Vec::new() })]);
        let (code, out) = run_cmd(&ImageCommand::Ls, &svc).await.unwrap();
        assert_eq!(code, 0);
        for header in ["REFERENCE", "DIGEST", "SIZE", "PULLED", "IN USE"] {
            assert!(out.contains(header), "missing {header} in {out:?}");
        }
    }

    #[tokio::test]
    async fn ls_renders_a_row_with_short_digest_and_holder() {
        let svc = CannedService::with([Some(Response::ImageList {
            images: vec![ImageInfo {
                in_use_by: Some("1a2b3c4d0000000000000000000000aa".to_string()),
                ..image("registry.example.test/some-image:1.0")
            }],
        })]);
        let (_, out) = run_cmd(&ImageCommand::Ls, &svc).await.unwrap();
        assert!(
            out.contains("registry.example.test/some-image:1.0"),
            "got: {out}"
        );
        assert!(out.contains(&"a".repeat(12)), "got: {out}");
        assert!(!out.contains(&"a".repeat(64)), "got: {out}");
        assert!(out.contains("3 MiB"), "got: {out}");
        assert!(out.contains("run 1a2b3c4d0000"), "got: {out}");
    }

    #[tokio::test]
    async fn rm_echoes_the_reference_and_the_space_reclaimed() {
        let svc = CannedService::with([Some(Response::ImageRemoved {
            reference: "registry.example.test/some-image:1.0".into(),
            reclaimed_bytes: 3 * 1024 * 1024,
        })]);
        let (code, out) = run_cmd(&ImageCommand::Rm(ref_arg("some-image:1.0")), &svc)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert!(
            out.contains("registry.example.test/some-image:1.0"),
            "got: {out}"
        );
        assert!(out.contains("Total reclaimed space: 3 MiB"), "got: {out}");
    }

    #[tokio::test]
    async fn prune_without_force_reads_eof_as_decline() {
        let svc = CannedService::with([]);
        let (code, out) = run_cmd(&ImageCommand::Prune(ImagePruneArgs { force: false }), &svc)
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
    async fn prune_with_nothing_to_remove_says_so() {
        let svc = CannedService::with([Some(Response::ImagesPruned {
            removed: Vec::new(),
            reclaimed_bytes: 0,
        })]);
        let (code, out) = run_cmd(&ImageCommand::Prune(ImagePruneArgs { force: true }), &svc)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert!(out.contains("No unused images."), "got: {out}");
    }

    #[tokio::test]
    async fn prune_reports_orphaned_blob_cleanup_even_with_no_image_removed() {
        let svc = CannedService::with([Some(Response::ImagesPruned {
            removed: Vec::new(),
            reclaimed_bytes: 2048,
        })]);
        let (_, out) = run_cmd(&ImageCommand::Prune(ImagePruneArgs { force: true }), &svc)
            .await
            .unwrap();
        assert!(out.contains("Total reclaimed space: 2 KiB"), "got: {out}");
    }

    #[tokio::test]
    async fn an_unanswered_request_is_reported_plainly() {
        let svc = CannedService::with([None]);
        let err = run_cmd(&ImageCommand::Ls, &svc)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no response from lns-service"), "got: {err}");
    }

    #[test]
    fn short_digest_takes_the_first_twelve_hex_chars() {
        assert_eq!(
            short_digest(&format!("sha256:{}", "ab".repeat(32))),
            "abababababab"
        );
        assert_eq!(short_digest("plain"), "plain");
    }

    #[test]
    fn in_use_str_names_the_holder_or_dashes() {
        assert_eq!(
            in_use_str(Some("1a2b3c4d0000000000000000000000aa")),
            "run 1a2b3c4d0000"
        );
        assert_eq!(in_use_str(None), "-");
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
}
