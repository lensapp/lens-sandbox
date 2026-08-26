use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::FromArgMatches;

use super::{ArtifactArgs, ArtifactCommand, run_with_writers};
use crate::command::{RunCtx, RunFuture};
use crate::service::client::TermInfo;
use crate::service::real::RealSandboxService;

pub fn run<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let mut args = ArtifactArgs::from_arg_matches(matches)?;
        super::apply_registry_default(&mut args.command, configured_registry()?.as_deref());
        if super::author::is_offline(&args.command) {
            return run_author(&args.command, ctx);
        }
        if let ArtifactCommand::Push(push) = &args.command {
            let cwd = ctx.cwd()?;
            return push_local(
                &push.reference.clone(),
                push.dry_run,
                push.assume_yes,
                push.file.clone().as_deref(),
                cwd,
                ctx.input,
            )
            .await;
        }
        if let ArtifactCommand::Inspect(inspect) = &mut args.command
            && !inspect.mixins.is_empty()
        {
            inspect.root_mixins(&ctx.cwd()?)?;
        }
        crate::service::require_running().await?;
        dispatch(args.command, ctx.input).await
    })
}

pub(crate) fn configured_registry() -> Result<Option<String>> {
    let path = crate::config::default_config_path()?;
    Ok(crate::config::load_run_defaults(&path)?.registry)
}

fn qualified_reference(reference: &str) -> Result<String> {
    Ok(crate::config::resolve_default_registry(
        reference,
        configured_registry()?.as_deref(),
    ))
}

pub fn run_init<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::InitArgs::from_arg_matches(matches)?;
        run_author(&ArtifactCommand::Init(args), ctx)
    })
}

pub fn run_push<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::PushArgs::from_arg_matches(matches)?;
        push_local(
            &qualified_reference(&args.reference)?,
            args.dry_run,
            args.assume_yes,
            args.file.as_deref(),
            ctx.cwd()?,
            ctx.input,
        )
        .await
    })
}

pub fn run_pull<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let mut args = super::PullArgs::from_arg_matches(matches)?;
        args.reference = qualified_reference(&args.reference)?;
        run_after_gate(ArtifactCommand::Pull(args), ctx.input).await
    })
}

pub fn run_tag<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::TagArgs::from_arg_matches(matches)?;
        let mut command = ArtifactCommand::Tag(args);
        super::apply_registry_default(&mut command, configured_registry()?.as_deref());
        run_after_gate(command, ctx.input).await
    })
}

/// The shortcut settles a path-shaped operand itself, so a local document never reaches the service.
pub fn run_inspect_offline(args: super::InspectArgs, ctx: RunCtx<'_>) -> Result<i32> {
    run_author(&ArtifactCommand::Inspect(args), ctx)
}

pub fn run_rm<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::RmArgs::from_arg_matches(matches)?;
        let mut command = ArtifactCommand::Rm(args);
        super::apply_registry_default(&mut command, configured_registry()?.as_deref());
        crate::service::require_running().await?;
        dispatch(command, ctx.input).await
    })
}

async fn run_after_gate(command: ArtifactCommand, input: &mut dyn std::io::BufRead) -> Result<i32> {
    crate::service::require_running().await?;
    dispatch(command, input).await
}

async fn push_local(
    reference: &str,
    dry_run: bool,
    assume_yes: bool,
    file: Option<&std::path::Path>,
    cwd: PathBuf,
    input: &mut dyn std::io::BufRead,
) -> Result<i32> {
    let path = super::author::selected_definition_path(file, &cwd);
    let project_dir = path.parent().unwrap_or(&cwd).to_path_buf();
    let doc = super::author::load_definition_json_at(&RealFs, &path)?;
    let mut out = std::io::stdout();
    if dry_run {
        return super::distribute::push_dry_run(&RealFs, &project_dir, &doc, reference, &mut out);
    }
    super::distribute::push(
        super::distribute::PushPorts {
            fs: &RealFs,
            cwd: &project_dir,
            producer: &RealProducer,
            resolver: &RealToolResolver,
        },
        &doc,
        reference,
        super::distribute::Confirm {
            assume_yes,
            interactive: crate::raw_mode::stdin_is_tty(),
            input,
        },
        &mut out,
    )
    .await
}

struct RealToolResolver;

/// A push must fail with a diagnostic rather than hang against a blackholing proxy, and the index body is a version list, not a download.
const TOOL_INDEX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const MAX_TOOL_INDEX_BYTES: usize = 8 * 1024 * 1024;

impl super::distribute::ToolResolver for RealToolResolver {
    fn resolve<'a>(
        &'a self,
        tool: &'a lns_artifact::tools::ToolRef,
    ) -> crate::connector::LocalBoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let url = lns_artifact::tools::version_index_url(&tool.name);
            let response = reqwest::Client::builder()
                .timeout(TOOL_INDEX_TIMEOUT)
                .build()
                .context("building the tool version index client")?
                .get(&url)
                .send()
                .await
                .with_context(|| format!("querying the tool version index at {url}"))?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                anyhow::bail!(
                    "tool {:?} is unknown to the version index ({url}); check the name against mise's registry",
                    tool.name
                );
            }
            let response = response
                .error_for_status()
                .with_context(|| format!("tool version index at {url}"))?;
            // Stop at the limit mid-stream: checking a fully buffered body cannot prevent the allocation it names.
            let body = capped_body(response, &url, MAX_TOOL_INDEX_BYTES).await?;
            lns_artifact::tools::resolve_from_index(&tool.name, &tool.version, &body)
        })
    }

    fn verify<'a>(
        &'a self,
        tool: &'a lns_artifact::tools::ToolRef,
    ) -> crate::connector::LocalBoxFuture<'a, super::distribute::IndexVerification> {
        use super::distribute::IndexVerification;
        Box::pin(async move {
            let url = lns_artifact::tools::version_index_url(&tool.name);
            let Ok(client) = reqwest::Client::builder()
                .timeout(TOOL_INDEX_TIMEOUT)
                .build()
            else {
                return IndexVerification::Unavailable;
            };
            match client.get(&url).send().await {
                Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => {
                    IndexVerification::Absent
                }
                Ok(response) => match response.error_for_status() {
                    Ok(response) => match capped_body(response, &url, MAX_TOOL_INDEX_BYTES).await {
                        Ok(body)
                            if lns_artifact::tools::index_lists_exact(&body, &tool.version) =>
                        {
                            IndexVerification::Confirmed
                        }
                        Ok(_) => IndexVerification::Absent,
                        Err(_) => IndexVerification::Unavailable,
                    },
                    Err(_) => IndexVerification::Unavailable,
                },
                Err(_) => IndexVerification::Unavailable,
            }
        })
    }
}

async fn capped_body(response: reqwest::Response, url: &str, max_bytes: usize) -> Result<String> {
    use futures_util::StreamExt;
    let too_big = |seen: usize| {
        anyhow::anyhow!(
            "the version index at {url} returned at least {seen} bytes, over the {max_bytes}-byte limit"
        )
    };
    if let Some(declared) = response.content_length()
        && declared > max_bytes as u64
    {
        return Err(too_big(declared as usize));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("reading the version index at {url}"))?;
        if body.len() + chunk.len() > max_bytes {
            return Err(too_big(body.len() + chunk.len()));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).with_context(|| format!("the version index at {url} is not text"))
}

struct RealProducer;

impl super::distribute::Producer for RealProducer {
    fn push_built<'a>(
        &'a self,
        built: &'a lns_artifact::build::BuiltArtifact,
        reference: &'a str,
    ) -> crate::connector::LocalBoxFuture<'a, Result<()>> {
        Box::pin(async move { crate::build::push::push_artifact(built, reference).await })
    }
}

fn run_author(command: &ArtifactCommand, ctx: RunCtx<'_>) -> Result<i32> {
    let cwd = ctx.cwd()?;
    let mut out = std::io::stdout();
    match command {
        ArtifactCommand::Init(args) => {
            super::author::init(&RealFs, &cwd, args.kind, args.file.as_deref(), &mut out)
        }
        ArtifactCommand::Validate(args) => {
            super::author::validate(&RealFs, &cwd, args.kind, args.file.as_deref(), &mut out)
        }
        ArtifactCommand::Inspect(args) => super::author::inspect_local(
            &RealFs,
            &cwd,
            args.reference.as_deref(),
            args.file.as_deref(),
            &args.mixins,
            &mut out,
        ),
        _ => unreachable!("run_author is only called for offline author verbs"),
    }
}

pub struct RealFs;

impl super::author::Fs for RealFs {
    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }
    fn write(&self, path: &Path, contents: &str) -> std::io::Result<()> {
        std::fs::write(path, contents)
    }
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn read_limited(&self, path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    fn dir_entries(&self, dir: &Path) -> std::io::Result<Vec<super::author::DirEntry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().into_string().map_err(|name| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("non-utf8 file name {name:?}"),
                )
            })?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("symlink {name} — filesets carry only regular files"),
                ));
            }
            use std::os::unix::fs::PermissionsExt;
            entries.push(super::author::DirEntry {
                name,
                dir: file_type.is_dir(),
                mode: entry.metadata()?.permissions().mode() & 0o777,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
}

// The caller already holds the process-wide stdin lock (run_from_matches), so this must borrow it — a second Stdin::lock on the same thread deadlocks every dispatched verb.
pub async fn dispatch(command: ArtifactCommand, input: &mut dyn std::io::BufRead) -> Result<i32> {
    let svc = RealSandboxService::new(crate::service::socket_path()?);
    let term = TermInfo {
        stdin_is_tty: crate::raw_mode::stdin_is_tty(),
        stdout_is_terminal: std::io::stdout().is_terminal(),
    };
    let mut out = std::io::stdout();
    let mut stderr = tokio::io::stderr();
    let mut input = input;
    run_with_writers(&command, &svc, term, &mut input, &mut out, &mut stderr).await
}
