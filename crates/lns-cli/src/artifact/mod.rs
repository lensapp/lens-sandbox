use std::path::PathBuf;

use anyhow::{Result, bail};
use lns_ipc::{Request, Response};
use tokio::io::AsyncWriteExt;

use crate::command::{CommandSpec, subcommand};
use crate::service::client::{SandboxService, TermInfo};

pub mod author;
pub mod distribute;
pub mod fileset;
pub mod mixin_plan;
pub mod real;
#[cfg(test)]
pub(crate) mod test_support;

/// `--mixin` composes a document before it boots, and neither a sandbox that has already booted nor a file rendered offline can honour it.
pub fn refuse_mixins_unless_published(mixins: &[String]) -> Result<()> {
    if mixins.is_empty() {
        return Ok(());
    }
    bail!(
        "--mixin applies to a published reference: a live sandbox has already booted with what it merged, and a local document renders offline, without resolving anything"
    )
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<ArtifactArgs>("artifact")
            .about("The document — author, validate, publish, fetch, and cache it, of any kind."),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "artifact",
    augment,
    run: real::run,
    announces_update_check: true,
    owns_terminal: crate::command::never_owns_terminal,
};

macro_rules! shortcut_spec {
    ($augment:ident, $const_name:ident, $args:ty, $name:literal, $run:path, $about:literal) => {
        pub fn $augment(app: clap::Command) -> clap::Command {
            app.subcommand(subcommand::<$args>($name).about($about))
        }
        pub const $const_name: CommandSpec = CommandSpec {
            name: $name,
            augment: $augment,
            run: $run,
            announces_update_check: true,
            owns_terminal: crate::command::never_owns_terminal,
        };
    };
}

shortcut_spec!(
    augment_init,
    INIT_SPEC,
    InitArgs,
    "init",
    real::run_init,
    "Scaffold a document in this directory (shortcut for `lns artifact init`)."
);
shortcut_spec!(
    augment_push,
    PUSH_SPEC,
    PushArgs,
    "push",
    real::run_push,
    "Build and publish ./lns.yaml as one artifact (shortcut for `lns artifact push`)."
);
shortcut_spec!(
    augment_pull,
    PULL_SPEC,
    PullArgs,
    "pull",
    real::run_pull,
    "Fetch a published artifact into the cache (shortcut for `lns artifact pull`)."
);
shortcut_spec!(
    augment_tag,
    TAG_SPEC,
    TagArgs,
    "tag",
    real::run_tag,
    "Re-reference a cached artifact (shortcut for `lns artifact tag`)."
);

/// Qualifies the registry coordinate a distribution verb addresses with `registry`, or the built-in default when nothing is configured; the local-cache verbs take a reference the service resolves, so they are left alone.
pub fn apply_registry_default(command: &mut ArtifactCommand, registry: Option<&str>) {
    let reference = match command {
        ArtifactCommand::Push(args) => &mut args.reference,
        ArtifactCommand::Pull(args) => &mut args.reference,
        _ => return,
    };
    *reference = crate::config::resolve_default_registry(reference, registry);
}

#[derive(clap::Args)]
pub struct ArtifactArgs {
    #[command(subcommand)]
    pub command: ArtifactCommand,
}

/// Every verb here works on any kind, because each reads the kind from the document itself; only `init`, which writes one that does not exist yet, is told.
#[derive(clap::Subcommand)]
pub enum ArtifactCommand {
    #[command(about = "Scaffold a document in this directory; `--kind` chooses which.")]
    Init(InitArgs),
    #[command(about = "Validate a document — schema, cross-field, and secret checks, offline.")]
    Validate(ValidateArgs),
    #[command(about = "Build ./lns.yaml and upload it to a registry as one artifact, in one step.")]
    Push(PushArgs),
    #[command(about = "Fetch a published artifact and its base image into the local cache.")]
    Pull(PullArgs),
    #[command(about = "Add a tag to a cached artifact within its current repository.")]
    Tag(TagArgs),
    #[command(
        about = "List what the local store holds: reference, kind, digest, size, and holder."
    )]
    Ls(LsArgs),
    #[command(
        about = "Render one artifact's resolved content: a cached reference, or a local document (offline)."
    )]
    Inspect(InspectArgs),
    #[command(
        about = "Remove a cached artifact and free its now-unreferenced layers; refuses one a sandbox holds."
    )]
    Rm(RmArgs),
    #[command(
        about = "Remove every cached artifact nothing holds, and the provisioned tool cache when no sandbox is live."
    )]
    Prune(PruneArgs),
}

#[derive(clap::Args)]
pub struct LsArgs {
    #[arg(
        long,
        value_enum,
        value_name = "KIND",
        help = "List only the cached entries of this kind."
    )]
    pub kind: Option<CachedKindFilter>,

    #[command(flatten)]
    pub output: crate::output::OutputArgs,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "lower")]
pub enum CachedKindFilter {
    Image,
    Sandbox,
}

impl CachedKindFilter {
    fn matches(self, kind: lns_ipc::CachedKind) -> bool {
        matches!(
            (self, kind),
            (CachedKindFilter::Image, lns_ipc::CachedKind::Image)
                | (CachedKindFilter::Sandbox, lns_ipc::CachedKind::Sandbox)
        )
    }
}

#[derive(clap::Args)]
pub struct InitArgs {
    #[arg(
        long,
        value_enum,
        default_value_t = DocumentKind::Sandbox,
        value_name = "KIND",
        help = "Which document to scaffold."
    )]
    pub kind: DocumentKind,

    #[arg(
        short = 'f',
        long = "file",
        value_name = "FILE",
        help = "File to write instead of ./lns.yaml. Refuses to overwrite either way."
    )]
    pub file: Option<PathBuf>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "lower")]
pub enum DocumentKind {
    Sandbox,
    Mixin,
}

impl DocumentKind {
    pub fn as_artifact_kind(self) -> lns_artifact::spec::Kind {
        match self {
            DocumentKind::Sandbox => lns_artifact::spec::Kind::Sandbox,
            DocumentKind::Mixin => lns_artifact::spec::Kind::Mixin,
        }
    }
}

#[derive(clap::Args)]
pub struct ValidateArgs {
    #[arg(
        long,
        value_enum,
        value_name = "KIND",
        help = "Also require the document to be this kind."
    )]
    pub kind: Option<DocumentKind>,

    #[arg(
        short = 'f',
        long = "file",
        value_name = "FILE",
        help = "Definition file to validate instead of ./lns.yaml; its directory roots the definition's relative filesets."
    )]
    pub file: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct PushArgs {
    #[arg(
        value_name = "REF",
        help = "Registry reference to publish the sandbox at, e.g. ghcr.io/team/hermes:1.4.0; a bare reference resolves against the `run.registry` default, else hub.lns.run."
    )]
    pub reference: String,
    #[arg(
        long = "dry-run",
        help = "Validate, pack, and build everything push would upload, print the digests, and upload nothing."
    )]
    pub dry_run: bool,
    #[arg(
        long = "yes",
        help = "Publish the local mixins this document names without prompting."
    )]
    pub assume_yes: bool,
    #[arg(
        short = 'f',
        long = "file",
        value_name = "FILE",
        help = "Definition file to publish instead of ./lns.yaml; its directory roots the definition's relative filesets."
    )]
    pub file: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct PullArgs {
    #[arg(
        value_name = "REF",
        help = "Published sandbox reference to fetch, e.g. ghcr.io/team/hermes:1.4.0; a bare reference resolves against the `run.registry` default, else hub.lns.run."
    )]
    pub reference: String,
    #[arg(
        long = "yes",
        help = "Accept publisher-declared tool installation without prompting."
    )]
    pub assume_yes: bool,
}

#[derive(clap::Args)]
pub struct TagArgs {
    #[arg(value_name = "SOURCE", help = "Cached sandbox to re-reference.")]
    pub from: String,

    #[arg(
        value_name = "TARGET",
        help = "New tag in the source sandbox's registry and repository."
    )]
    pub to: String,
}

#[derive(clap::Args)]
pub struct PruneArgs {
    #[arg(
        short = 'f',
        long,
        default_value_t = false,
        help = "Remove unused cached sandboxes, and the provisioned tool cache when none is live, without asking."
    )]
    pub force: bool,
}

#[derive(clap::Args)]
pub struct InspectArgs {
    #[arg(
        value_name = "REF",
        help = "A cached artifact reference, or a path to a local document (., lns.yaml, ./dir, ./lns.dev.yaml — rendered offline). Omit to render ./lns.yaml."
    )]
    pub reference: Option<String>,

    #[arg(
        short = 'f',
        long = "file",
        value_name = "FILE",
        conflicts_with = "reference",
        help = "Document to render instead of ./lns.yaml, offline. Cannot be combined with REF."
    )]
    pub file: Option<PathBuf>,

    #[arg(
        long = "mixin",
        value_name = "REF",
        help = "Resolve this mixin into the artifact before rendering it, as `lns run --mixin` would. Repeatable."
    )]
    pub mixins: Vec<String>,
}

#[derive(clap::Args)]
pub struct RmArgs {
    #[arg(value_name = "REF", help = "Cached artifact reference to remove.")]
    pub reference: String,
}

async fn pull<I, W, E>(
    svc: &impl SandboxService,
    args: &PullArgs,
    term: TermInfo,
    input: &mut I,
    out: &mut W,
    stderr: &mut E,
) -> Result<i32>
where
    I: std::io::BufRead,
    W: std::io::Write,
    E: AsyncWriteExt + Unpin,
{
    let inspection = svc
        .one_shot(Request::InspectImage {
            image: args.reference.clone(),
            mixins: Vec::new(),
            decisions: None,
        })
        .await?;
    // A mixin pull installs nothing: it caches documents, so there is no effect to consent to and its tools are disclosed where they are installed.
    let (digest, tools) = match inspection {
        Response::ImageInspected {
            inspection: lns_ipc::ArtifactInspection::Sandbox(view),
        } if !view.digest.is_empty() => (view.digest, view.tools),
        Response::ImageInspected {
            inspection: lns_ipc::ArtifactInspection::Mixin(view),
        } if !view.digest.is_empty() => (view.digest, Vec::new()),
        Response::ImageInspected {
            inspection:
                lns_ipc::ArtifactInspection::Mixin(_) | lns_ipc::ArtifactInspection::Sandbox(_),
        } => bail!(
            "the registry did not provide a digest for {}",
            args.reference
        ),
        Response::ImageInspected {
            inspection: lns_ipc::ArtifactInspection::Image(_),
        } => bail!(
            "{} is an OCI image, not a published sandbox",
            args.reference
        ),
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    };
    crate::run::pull_confirm::confirm_pulled_effects(
        &crate::run::pull_confirm::PulledEffects {
            reference: &args.reference,
            binds: &[],
            volumes: &[],
            filesets: &[],
            tools: &tools,
            // A pull runs no script: scripts run at boot, and `lns run` discloses them there.
            scripts: &[],
        },
        args.assume_yes,
        term.stdin_is_tty,
        input,
        out,
    )?;
    let response = svc
        .one_shot(Request::PullImage {
            image: args.reference.clone(),
            expected_digest: digest,
        })
        .await?;
    match response {
        Response::MixinPulled {
            reference,
            digest,
            cached_mixins,
        } => {
            writeln!(out, "pulled {reference}")?;
            writeln!(out, "digest: {digest}")?;
            if cached_mixins > 0 {
                writeln!(out, "cached {cached_mixins} mixin(s) it layers on")?;
            }
            Ok(0)
        }
        Response::ImagePulled { image, warnings } => {
            writeln!(out, "pulled {}", image.reference)?;
            writeln!(out, "digest: {}", image.digest)?;
            for warning in warnings {
                stderr
                    .write_all(format!("warning: {warning}\n").as_bytes())
                    .await?;
                stderr.flush().await?;
            }
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn tag<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &TagArgs,
    out: &mut W,
) -> Result<i32> {
    let response = svc
        .one_shot(Request::TagImage {
            from: args.from.clone(),
            to: args.to.clone(),
        })
        .await?;
    match response {
        Response::ImageTagged { from, to } => {
            writeln!(out, "tagged {from} as {to}")?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn ls<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &LsArgs,
    out: &mut W,
) -> Result<i32> {
    match svc.one_shot(Request::ListImages).await? {
        Response::ImageList { mut images } => {
            images.sort_by(|a, b| a.reference.cmp(&b.reference));
            let rows: Vec<ArtifactRow> = images
                .iter()
                .filter(|image| args.kind.is_none_or(|kind| kind.matches(image.kind)))
                .map(ArtifactRow::new)
                .collect();
            crate::output::emit(args.output.format, &rows, out)?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactRow {
    reference: String,
    kind: String,
    digest: String,
    size_bytes: u64,
    layers: u32,
    pulled: String,
    in_use_by: Option<String>,
}

impl ArtifactRow {
    fn new(image: &lns_ipc::ImageInfo) -> Self {
        Self {
            reference: image.reference.clone(),
            kind: image.kind.as_str().to_string(),
            digest: image.digest.clone(),
            size_bytes: image.size_bytes,
            layers: image.layers,
            pulled: image.pulled.clone(),
            in_use_by: image.in_use_by.clone(),
        }
    }
}

/// The digest column is the short form the tables elsewhere use, since the whole one crowds out every other column and the JSON carries it in full.
fn short_digest(digest: &str) -> String {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    hex.char_indices()
        .nth(12)
        .map_or(hex, |(i, _)| &hex[..i])
        .to_string()
}

impl crate::output::TableRow for ArtifactRow {
    const HEADERS: &'static [&'static str] = &["ARTIFACT", "KIND", "DIGEST", "SIZE", "HOLDER"];

    fn cells(&self) -> Vec<String> {
        vec![
            self.reference.clone(),
            self.kind.clone(),
            short_digest(&self.digest),
            crate::output::format_bytes(self.size_bytes),
            match &self.in_use_by {
                Some(run) => format!("run {}", lns_ipc::short_run_id(run)),
                None => "-".to_string(),
            },
        ]
    }
}

pub(crate) async fn remove_cached<W: std::io::Write>(
    svc: &impl SandboxService,
    reference: &str,
    out: &mut W,
) -> Result<i32> {
    match svc
        .one_shot(Request::RemoveImage {
            image: reference.to_string(),
        })
        .await?
    {
        Response::ImageRemoved {
            reference,
            reclaimed_bytes,
        } => {
            writeln!(out, "removed {reference}")?;
            writeln!(
                out,
                "freed {} of base-image layers",
                crate::output::format_bytes(reclaimed_bytes)
            )?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn prune<I: std::io::BufRead, W: std::io::Write, E: AsyncWriteExt + Unpin>(
    svc: &impl SandboxService,
    args: &PruneArgs,
    term: TermInfo,
    input: &mut I,
    out: &mut W,
    stderr: &mut E,
) -> Result<i32> {
    if !args.force {
        if !term.stdin_is_tty {
            bail!(
                "this removes every cached sandbox not held by a running one and, when none is live, the provisioned tool cache; there is no terminal to ask at, so pass --force to confirm"
            );
        }
        if !confirm_prune(input, stderr).await? {
            return Ok(0);
        }
    }
    match svc.one_shot(Request::PruneImages).await? {
        Response::ImagesPruned {
            mut removed,
            reclaimed_bytes,
        } => {
            removed.sort_unstable();
            for reference in &removed {
                writeln!(out, "removed {reference}")?;
            }
            writeln!(
                out,
                "reclaimed {}",
                crate::output::format_bytes(reclaimed_bytes)
            )?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn confirm_prune<I: std::io::BufRead, E: AsyncWriteExt + Unpin>(
    input: &mut I,
    err: &mut E,
) -> Result<bool> {
    err.write_all(
        b"This removes every cached sandbox not held by a running one and, when none is live, the provisioned tool cache. Continue? [y/N] ",
    )
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

pub(crate) async fn inspect_cached<W: std::io::Write>(
    svc: &impl SandboxService,
    reference: &str,
    mixins: &[String],
    out: &mut W,
) -> Result<i32> {
    match svc
        .one_shot(Request::InspectImage {
            image: reference.to_string(),
            mixins: mixins.to_vec(),
            decisions: None,
        })
        .await?
    {
        Response::ImageInspected { inspection } => {
            render_cached_inspect(&inspection, mixins, out)?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

fn render_cached_inspect<W: std::io::Write>(
    inspection: &lns_ipc::ArtifactInspection,
    typed: &[String],
    out: &mut W,
) -> Result<()> {
    match inspection {
        lns_ipc::ArtifactInspection::Sandbox(view) => {
            writeln!(out, "kind: sandbox")?;
            writeln!(out, "reference: {}", view.reference)?;
            if !view.digest.is_empty() {
                writeln!(out, "digest: {}", view.digest)?;
            }
            writeln!(out, "image: {}", view.image)?;
            for mixin in
                crate::run::summary::mixin_display(&view.mixins, typed, &view.pinned_mixins)
            {
                writeln!(out, "mixin: {mixin}")?;
            }
            if let Some(user) = &view.user {
                writeln!(out, "user: {user}")?;
            }
            if let Some(workdir) = &view.workdir {
                writeln!(out, "workdir: {workdir}")?;
            }
            for entry in &view.env {
                writeln!(out, "env: {entry}")?;
            }
            render_mounts(out, &view.mounts)?;
            if !view.ports.is_empty() {
                writeln!(out, "ports: {}", declared_ports_line(&view.ports))?;
            }
            render_filesets(out, &view.filesets)?;
            render_connectors(out, &view.connectors)?;
            for credential in &view.credentials {
                writeln!(out, "credential: {}", credential_disclosure(credential))?;
            }
            for tool in &view.tools {
                writeln!(out, "tool: {tool}")?;
            }
            render_scripts(out, &view.scripts)?;
            render_policy_flags(out, &view.policy_flags)?;
        }
        lns_ipc::ArtifactInspection::Mixin(view) => {
            writeln!(out, "kind: mixin")?;
            writeln!(out, "reference: {}", view.reference)?;
            if !view.digest.is_empty() {
                writeln!(out, "digest: {}", view.digest)?;
            }
            for mixin in &view.mixins {
                writeln!(out, "mixin: {mixin}")?;
            }
            for entry in &view.env {
                writeln!(out, "env: {entry}")?;
            }
            render_mounts(out, &view.mounts)?;
            if !view.ports.is_empty() {
                writeln!(out, "ports: {}", declared_ports_line(&view.ports))?;
            }
            render_filesets(out, &view.filesets)?;
            for credential in &view.credentials {
                writeln!(out, "credential: {}", credential_disclosure(credential))?;
            }
            for tool in &view.tools {
                writeln!(out, "tool: {tool}")?;
            }
            render_scripts(out, &view.scripts)?;
            render_policy_flags(out, &view.policy_flags)?;
        }
        lns_ipc::ArtifactInspection::Image(view) => {
            writeln!(out, "kind: image")?;
            writeln!(out, "reference: {}", view.reference)?;
            writeln!(out, "digest: {}", view.digest)?;
        }
    }
    Ok(())
}

/// One renderer for both kinds, since a mixin's mounts read exactly as a sandbox's do.
fn render_mounts<W: std::io::Write>(out: &mut W, mounts: &[lns_ipc::SandboxMount]) -> Result<()> {
    for mount in mounts {
        let kind = match mount.kind {
            lns_ipc::SandboxMountKind::Bind => "bind",
            lns_ipc::SandboxMountKind::Volume => "volume",
        };
        let mode = if mount.read_only { " (read-only)" } else { "" };
        writeln!(
            out,
            "mount: {kind} {} -> {}{mode}",
            mount.source, mount.target
        )?;
    }
    Ok(())
}

fn render_filesets<W: std::io::Write>(
    out: &mut W,
    filesets: &[lns_ipc::SandboxFileset],
) -> Result<()> {
    for fileset in filesets {
        let source = crate::run::summary::fileset_view_source_display(fileset);
        let owner = crate::run::summary::fileset_view_owner_display(fileset.owner);
        writeln!(
            out,
            "fileset: {source} -> {} (owner: {owner})",
            fileset.guest_path
        )?;
    }
    Ok(())
}

fn declared_ports_line(ports: &[lns_ipc::SandboxPort]) -> String {
    ports
        .iter()
        .map(|port| match port.host {
            Some(host) => format!("{host}:{}", port.container),
            None => port.container.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a declared credential discloses: the variable the workload reads, and every destination its value may travel to — the two things a reader has to weigh before running it.
pub(crate) fn credential_disclosure(credential: &lns_spec::Credential) -> String {
    if credential.injections.is_empty() {
        return format!("{} (travels nowhere)", credential.env_var);
    }
    let domains: Vec<&str> = credential
        .injections
        .iter()
        .map(|injection| injection.domain.as_str())
        .collect();
    format!("{} -> {}", credential.env_var, domains.join(", "))
}

fn render_connectors<W: std::io::Write>(out: &mut W, connectors: &[String]) -> Result<()> {
    for id in connectors {
        writeln!(out, "connector: {id}")?;
    }
    Ok(())
}

/// Inspect is where a script's body is printed whole; the launch summary elides it so a long one cannot bury the rest of the approval.
fn render_scripts<W: std::io::Write>(
    out: &mut W,
    scripts: &[lns_ipc::SandboxScript],
) -> Result<()> {
    for script in scripts {
        let user = script.user.as_deref().unwrap_or("the workload user");
        writeln!(out, "script: {} (runs as {user})", script.when)?;
        if let Some(description) = &script.description {
            writeln!(out, "  {description}")?;
        }
        for line in script.run.lines() {
            writeln!(out, "  | {line}")?;
        }
    }
    Ok(())
}

fn render_policy_flags<W: std::io::Write>(out: &mut W, flags: &[String]) -> Result<()> {
    for flag in flags {
        writeln!(out, "⚠ {flag}")?;
    }
    Ok(())
}

pub async fn run_with_writers<S, I, W, E>(
    cmd: &ArtifactCommand,
    svc: &S,
    term: TermInfo,
    input: &mut I,
    out: &mut W,
    stderr: &mut E,
) -> Result<i32>
where
    S: SandboxService,
    I: std::io::BufRead,
    W: std::io::Write,
    E: AsyncWriteExt + Unpin,
{
    match cmd {
        ArtifactCommand::Init(_) | ArtifactCommand::Validate(_) => {
            bail!("the author verbs run offline, not through the service dispatch")
        }
        ArtifactCommand::Push(_) => {
            bail!("push builds and uploads locally, not through the service dispatch")
        }
        ArtifactCommand::Pull(args) => pull(svc, args, term, input, out, stderr).await,
        ArtifactCommand::Tag(args) => tag(svc, args, out).await,
        ArtifactCommand::Ls(args) => ls(svc, args, out).await,
        ArtifactCommand::Inspect(args) => {
            let Some(reference) = &args.reference else {
                bail!("a local document renders offline, not through the service dispatch")
            };
            inspect_cached(svc, reference, &args.mixins, out).await
        }
        ArtifactCommand::Rm(args) => remove_cached(svc, &args.reference, out).await,
        ArtifactCommand::Prune(args) => prune(svc, args, term, input, out, stderr).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_service::{
        CannedService, pulled_response, sandbox_inspection, sandbox_inspection_with_digest,
    };

    #[tokio::test]
    async fn run_with_writers_refuses_the_offline_author_verbs() {
        let svc = CannedService::new(Response::Pong);
        for cmd in [
            ArtifactCommand::Init(InitArgs {
                kind: DocumentKind::Sandbox,
                file: None,
            }),
            ArtifactCommand::Validate(ValidateArgs {
                kind: None,
                file: None,
            }),
            ArtifactCommand::Inspect(InspectArgs {
                reference: None,
                mixins: Vec::new(),
                file: None,
            }),
        ] {
            let mut out = Vec::new();
            let mut stderr = Vec::new();
            let err = run_with_writers(
                &cmd,
                &svc,
                TermInfo::default(),
                &mut std::io::Cursor::new(""),
                &mut out,
                &mut stderr,
            )
            .await
            .unwrap_err();
            assert!(format!("{err:#}").contains("offline"), "got: {err:#}");
        }
    }

    #[tokio::test]
    async fn run_with_writers_refuses_push_which_runs_locally() {
        let svc = CannedService::new(Response::Pong);
        let cmd = ArtifactCommand::Push(PushArgs {
            reference: "ghcr.io/team/hermes:1.4.0".into(),
            dry_run: false,
            assume_yes: false,
            file: None,
        });
        let mut out = Vec::new();
        let mut stderr = Vec::new();
        let err = run_with_writers(
            &cmd,
            &svc,
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut out,
            &mut stderr,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("locally"), "got: {err:#}");
    }

    #[tokio::test]
    async fn pull_reports_the_pulled_reference_and_digest() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let svc = CannedService::with_inspect_image(
            Response::ImagePulled {
                image: lns_ipc::ImageInfo {
                    kind: lns_ipc::CachedKind::Sandbox,
                    reference: "ghcr.io/team/hermes:1.4.0".into(),
                    digest: digest.clone(),
                    size_bytes: 1024,
                    layers: 1,
                    pulled: "2026-01-01T00:00:00Z".into(),
                    in_use_by: None,
                },
                warnings: vec!["the sandbox is cached, but its first run needs the network".into()],
            },
            sandbox_inspection(Vec::new()),
        );
        let mut out = Vec::new();
        let mut stderr = Vec::new();
        let code = run_with_writers(
            &ArtifactCommand::Pull(PullArgs {
                reference: "ghcr.io/team/hermes:1.4.0".into(),
                assume_yes: false,
            }),
            &svc,
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut out,
            &mut stderr,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("sha256:"), "got: {text}");
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "warning: the sandbox is cached, but its first run needs the network\n"
        );
        assert!(matches!(
            svc.requests.lock().unwrap().as_slice(),
            [
                Request::InspectImage { image, .. },
                Request::PullImage {
                    image: pulled,
                    expected_digest
                }
            ] if image == "ghcr.io/team/hermes:1.4.0"
                && pulled == image
                && expected_digest == &digest
        ));
    }

    #[tokio::test]
    async fn pull_discloses_declared_tools_before_requesting_provisioning() {
        let svc = CannedService::with_inspect_image(
            pulled_response(),
            sandbox_inspection(vec!["node@22".into()]),
        );
        let mut out = Vec::new();

        let code = pull(
            &svc,
            &PullArgs {
                reference: "ghcr.io/team/hermes:1.4.0".into(),
                assume_yes: false,
            },
            TermInfo {
                stdin_is_tty: true,
                stdout_is_terminal: false,
            },
            &mut std::io::Cursor::new("yes\n"),
            &mut out,
            &mut Vec::new(),
        )
        .await
        .unwrap();

        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Tool:       node@22"), "got: {text}");
        assert!(
            matches!(
                svc.requests.lock().unwrap().as_slice(),
                [Request::InspectImage { .. }, Request::PullImage { .. }]
            ),
            "inspection must precede the provisioning pull"
        );
    }

    #[tokio::test]
    async fn declining_pulled_tools_sends_no_provisioning_request() {
        let svc = CannedService::with_inspect_image(
            pulled_response(),
            sandbox_inspection(vec!["node@22".into()]),
        );

        let err = pull(
            &svc,
            &PullArgs {
                reference: "ghcr.io/team/hermes:1.4.0".into(),
                assume_yes: false,
            },
            TermInfo {
                stdin_is_tty: true,
                stdout_is_terminal: false,
            },
            &mut std::io::Cursor::new("n\n"),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("declined"), "got: {err}");
        assert!(matches!(
            svc.requests.lock().unwrap().as_slice(),
            [Request::InspectImage { .. }]
        ));
    }

    #[tokio::test]
    async fn noninteractive_tool_pull_requires_yes_and_yes_skips_the_prompt() {
        let refused = CannedService::with_inspect_image(
            pulled_response(),
            sandbox_inspection(vec!["node@22".into()]),
        );
        let err = pull(
            &refused,
            &PullArgs {
                reference: "ghcr.io/team/hermes:1.4.0".into(),
                assume_yes: false,
            },
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("--yes"), "got: {err}");
        assert!(matches!(
            refused.requests.lock().unwrap().as_slice(),
            [Request::InspectImage { .. }]
        ));

        let accepted = CannedService::with_inspect_image(
            pulled_response(),
            sandbox_inspection(vec!["node@22".into()]),
        );
        let mut out = Vec::new();
        pull(
            &accepted,
            &PullArgs {
                reference: "ghcr.io/team/hermes:1.4.0".into(),
                assume_yes: true,
            },
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut out,
            &mut Vec::new(),
        )
        .await
        .unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(!out.contains("Continue?"), "got: {out}");
        assert!(matches!(
            accepted.requests.lock().unwrap().as_slice(),
            [Request::InspectImage { .. }, Request::PullImage { .. }]
        ));
    }

    #[tokio::test]
    async fn pull_surfaces_a_daemon_error_and_rejects_an_unrelated_variant() {
        let err = pull(
            &CannedService::new(Response::Error {
                message: "registry unreachable".into(),
            }),
            &PullArgs {
                reference: "x:1".into(),
                assume_yes: false,
            },
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("registry unreachable"));

        let err = pull(
            &CannedService::new(Response::Pong),
            &PullArgs {
                reference: "x:1".into(),
                assume_yes: false,
            },
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn pull_refuses_an_unpinned_sandbox_and_a_plain_image() {
        let err = pull(
            &CannedService::with_inspect_image(
                Response::Pong,
                sandbox_inspection_with_digest(Vec::new(), String::new()),
            ),
            &PullArgs {
                reference: "x:1".into(),
                assume_yes: false,
            },
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("did not provide a digest"));

        let err = pull(
            &CannedService::with_inspect_image(
                Response::Pong,
                Response::ImageInspected {
                    inspection: lns_ipc::ArtifactInspection::Image(lns_ipc::ImageView {
                        reference: "x:1".into(),
                        digest: format!("sha256:{}", "a".repeat(64)),
                    }),
                },
            ),
            &PullArgs {
                reference: "x:1".into(),
                assume_yes: false,
            },
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("OCI image"));
    }

    #[tokio::test]
    async fn pull_surfaces_a_post_consent_daemon_error_and_unexpected_response() {
        for (response, expected) in [
            (
                Response::Error {
                    message: "registry changed".into(),
                },
                "registry changed",
            ),
            (Response::Pong, "unexpected response"),
        ] {
            let err = pull(
                &CannedService::with_inspect_image(response, sandbox_inspection(Vec::new())),
                &PullArgs {
                    reference: "x:1".into(),
                    assume_yes: false,
                },
                TermInfo::default(),
                &mut std::io::Cursor::new(""),
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .await
            .unwrap_err();
            assert!(err.to_string().contains(expected), "got: {err}");
        }
    }

    #[tokio::test]
    async fn tag_confirms_the_new_reference() {
        let svc = CannedService::new(Response::ImageTagged {
            from: "hermes:1.4.0".into(),
            to: "hermes:latest".into(),
        });
        let mut out = Vec::new();
        let code = tag(
            &svc,
            &TagArgs {
                from: "hermes:1.4.0".into(),
                to: "hermes:latest".into(),
            },
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "tagged hermes:1.4.0 as hermes:latest\n"
        );
    }

    #[tokio::test]
    async fn tag_surfaces_a_daemon_error_and_rejects_an_unrelated_variant() {
        let err = tag(
            &CannedService::new(Response::Error {
                message: "no such cached sandbox".into(),
            }),
            &TagArgs {
                from: "a:1".into(),
                to: "a:2".into(),
            },
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("no such cached sandbox"));

        let err = tag(
            &CannedService::new(Response::Pong),
            &TagArgs {
                from: "a:1".into(),
                to: "a:2".into(),
            },
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn ls_surfaces_a_daemon_error() {
        let svc = CannedService::new(Response::Error {
            message: "registry poisoned".into(),
        });
        let mut out = Vec::new();
        let err = ls(&svc, &ls_args(), &mut out).await.unwrap_err();
        assert!(format!("{err:#}").contains("registry poisoned"));
    }

    #[tokio::test]
    async fn ls_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = ls(&svc, &ls_args(), &mut out).await.unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    fn ls_args() -> LsArgs {
        LsArgs {
            kind: None,
            output: crate::output::OutputArgs {
                format: crate::output::Format::Table,
            },
        }
    }

    #[tokio::test]
    async fn inspect_renders_a_cached_sandboxs_connectors_tools_and_scripts() {
        let svc = CannedService::with_inspect_image(
            Response::Error {
                message: "no such run: hermes:1.4.0".into(),
            },
            Response::ImageInspected {
                inspection: lns_ipc::ArtifactInspection::Sandbox(Box::new(lns_ipc::SandboxView {
                    mixins: Vec::new(),
                    pinned_mixins: Vec::new(),
                    contributions: Vec::new(),
                    reference: "hermes:1.4.0".into(),
                    digest: format!("sha256:{}", "a".repeat(64)),
                    image: "docker.io/library/alpine@sha256:abc".into(),
                    workdir: None,
                    user: None,
                    mounts: Vec::new(),
                    ports: Vec::new(),
                    filesets: Vec::new(),
                    connectors: vec!["some-provider".into()],
                    env: Vec::new(),
                    credentials: Vec::new(),
                    tools: vec!["node@22.11.0".into()],
                    scripts: vec![
                        lns_ipc::SandboxScript {
                            when: "pre-start".into(),
                            run: "apt-get update\napt-get install -y psql".into(),
                            user: Some("root".into()),
                            description: Some("the psql the prompts assume".into()),
                        },
                        lns_ipc::SandboxScript {
                            when: "pre-start".into(),
                            run: "npm ci".into(),
                            user: None,
                            description: None,
                        },
                    ],
                    policy_flags: Vec::new(),
                    cpus: None,
                    mem_mib: None,
                    disk_bytes: None,
                })),
            },
        );
        let mut out = Vec::new();
        let code = inspect_cached(&svc, "hermes:1.4.0", &[], &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("kind: sandbox"), "got: {text}");
        assert!(
            text.contains("image: docker.io/library/alpine"),
            "got: {text}"
        );
        assert!(text.contains("connector: some-provider"), "got: {text}");
        assert!(text.contains("tool: node@22.11.0"), "got: {text}");
        assert!(
            text.contains("script: pre-start (runs as root)")
                && text.contains("  the psql the prompts assume")
                && text.contains("  | apt-get update")
                && text.contains("  | apt-get install -y psql"),
            "inspect answers \"so what does it actually do\", so every line of the body has to appear; got: {text}"
        );
        assert!(
            text.contains("script: pre-start (runs as the workload user)"),
            "a script naming no user runs as the workload does, and a reader should not have to infer that from a blank; got: {text}"
        );
    }

    #[tokio::test]
    async fn inspect_cached_renders_the_image_kind() {
        let image = CannedService::with_inspect_image(
            Response::Error {
                message: "no such run: x".into(),
            },
            Response::ImageInspected {
                inspection: lns_ipc::ArtifactInspection::Image(lns_ipc::ImageView {
                    reference: "alpine:3.20".into(),
                    digest: "sha256:abc".into(),
                }),
            },
        );
        let mut out = Vec::new();
        inspect_cached(&image, "x", &[], &mut out).await.unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("kind: image"), "got: {text}");
        assert!(text.contains("digest: sha256:abc"), "got: {text}");
    }

    #[tokio::test]
    async fn inspect_cached_rejects_an_unrelated_image_response() {
        let svc = CannedService::with_inspect_image(
            Response::Error {
                message: "no such run: x".into(),
            },
            Response::Pong,
        );
        let mut out = Vec::new();
        let err = inspect_cached(&svc, "x", &[], &mut out).await.unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn rm_removes_a_cached_sandbox_and_reports_freed_layers() {
        let svc = CannedService::with_remove_image(
            Response::Error {
                message: "no such run: hermes:1.4.0".into(),
            },
            Response::ImageRemoved {
                reference: "hermes:1.4.0".into(),
                reclaimed_bytes: 3 * 1024 * 1024,
            },
        );
        let mut out = Vec::new();
        let code = remove_cached(&svc, "hermes:1.4.0", &mut out).await.unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("removed hermes:1.4.0"), "got: {text}");
        assert!(text.contains("freed 3.0 MiB"), "got: {text}");
    }

    #[tokio::test]
    async fn rm_of_a_cached_sandbox_surfaces_the_daemon_error() {
        let svc = CannedService::with_remove_image(
            Response::Error {
                message: "no such run: ghcr.io/team/x:1".into(),
            },
            Response::Error {
                message: "no such image: ghcr.io/team/x:1".into(),
            },
        );
        let mut out = Vec::new();
        let err = remove_cached(&svc, "ghcr.io/team/x:1", &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("no such image"));
    }

    #[tokio::test]
    async fn rm_rejects_an_unrelated_remove_response() {
        let svc = CannedService::with_remove_image(
            Response::Error {
                message: "no such run: reviewer".into(),
            },
            Response::Pong,
        );
        let mut out = Vec::new();
        let err = remove_cached(&svc, "reviewer", &mut out).await.unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn declining_the_prune_prompt_reaches_no_service() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = prune(
            &svc,
            &PruneArgs { force: false },
            TermInfo {
                stdin_is_tty: true,
                stdout_is_terminal: false,
            },
            &mut std::io::Cursor::new(b"n\n".to_vec()),
            &mut out,
            &mut err,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert!(String::from_utf8(err).unwrap().contains("Aborted."));
        assert!(out.is_empty());
        assert!(svc.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn prune_with_force_lists_removed_sandboxes_and_reclaimed_bytes() {
        let svc = CannedService::new(Response::ImagesPruned {
            removed: vec!["b:2".into(), "a:1".into()],
            reclaimed_bytes: 64 * 1024 * 1024,
        });
        let mut out = Vec::new();
        let code = prune(
            &svc,
            &PruneArgs { force: true },
            TermInfo::default(),
            &mut std::io::empty(),
            &mut out,
            &mut Vec::new(),
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("removed a:1") && text.contains("removed b:2"),
            "got: {text}"
        );
        assert!(text.contains("reclaimed 64.0 MiB"), "got: {text}");
    }

    #[tokio::test]
    async fn prune_surfaces_a_daemon_error_and_rejects_an_unrelated_variant() {
        let err = prune(
            &CannedService::new(Response::Error {
                message: "registry poisoned".into(),
            }),
            &PruneArgs { force: true },
            TermInfo::default(),
            &mut std::io::empty(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("registry poisoned"));

        let err = prune(
            &CannedService::new(Response::Pong),
            &PruneArgs { force: true },
            TermInfo::default(),
            &mut std::io::empty(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[test]
    fn format_bytes_picks_the_natural_unit() {
        assert_eq!(crate::output::format_bytes(512), "512 B");
        assert_eq!(crate::output::format_bytes(2048), "2.0 KiB");
        assert_eq!(crate::output::format_bytes(92_274_688), "88.0 MiB");
        assert_eq!(
            crate::output::format_bytes(2 * 1024 * 1024 * 1024),
            "2.0 GiB"
        );
    }
}
