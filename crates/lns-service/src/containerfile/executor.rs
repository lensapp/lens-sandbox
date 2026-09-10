//! The instruction loop: slice 3 of lensapp/lens-sandbox#393.
//!
//! One instruction at a time, in the order it was written. A `RUN` runs in a build guest booted
//! from the image the build has so far and what it wrote becomes one layer; a `COPY` or an `ADD`
//! becomes one layer of the context's files; everything else writes the image config.

use anyhow::{Context, Result, bail};

use super::cache::Kind;
use super::exclude::only_the_workloads_writes;
use super::key;
use super::upper::ChangeSet;
use lns_artifact::containerfile::{Command, Containerfile, HereDoc, InstructionKind, Transfer};

/// The shell a `RUN` in shell form is run through until a `SHELL` instruction says otherwise.
const DEFAULT_SHELL: [&str; 2] = ["/bin/sh", "-c"];

/// The identity a `RUN` runs as until a `USER` instruction says otherwise: a build installs into
/// the image, which nothing else in the guest may write.
const DEFAULT_USER: &str = "root";

/// The directory a `RUN` runs in until a `WORKDIR` instruction says otherwise.
const DEFAULT_WORKDIR: &str = "/";

/// The delimiter a here-document is handed to the shell with, extended until the body cannot end it early.
const HERE_DOC_DELIMITER: &str = "LNSHEREDOC";

/// One `RUN` as the build guest is asked to run it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunStep {
    pub parent: String,
    pub argv: Vec<String>,
    pub env: Vec<String>,
    pub user: String,
    pub workdir: String,
    pub line: usize,
}

/// One `COPY` or `ADD` as the context is asked for it, with every path already expanded and rooted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CopyStep {
    pub sources: Vec<String>,
    pub destination: String,
    /// Whether the destination names a directory the sources land in, rather than the path one source is written at.
    pub into_directory: bool,
    pub owner: Option<String>,
    /// The octal mode `--chmod` gives every entry the copy writes.
    pub mode: Option<u32>,
    pub line: usize,
}

/// What the Containerfile declares about the built image, over whatever its base already said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ConfigDraft {
    pub env: Vec<(String, String)>,
    pub labels: Vec<(String, String)>,
    pub user: Option<String>,
    pub workdir: Option<String>,
    pub entrypoint: Option<Vec<String>>,
    pub cmd: Option<Vec<String>>,
    pub shell: Option<Vec<String>>,
    pub exposed_ports: Vec<String>,
    pub volumes: Vec<String>,
}

/// One instruction's result: the layer it produced, if it produced one, over the image so far.
pub(crate) struct Commit<'a> {
    pub parent: &'a str,
    pub layer: Option<&'a ChangeSet>,
    pub config: &'a ConfigDraft,
    pub created_by: &'a str,
}

/// What one build guest wrote, and every path a fileset seeded into it — neither belongs in a layer.
pub(crate) struct RunOutcome {
    pub changes: ChangeSet,
    pub fileset_paths: Vec<String>,
}

/// The image a `FROM` resolved to: the reference the build stands on, and the environment its config already declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Base {
    pub reference: String,
    pub env: Vec<(String, String)>,
}

/// What the loop needs of the world: a registry, a build guest, the build context, the local store,
/// and what this machine built for a key it has seen before.
pub(crate) trait BuildHost {
    /// The digest-pinned reference the `FROM` resolved to, pulled so the build can stand on its config.
    async fn resolve_base(&self, image: &str) -> Result<Base>;
    /// The same reference and config, read off the registry with no layer fetched, for a caller that will build nothing.
    async fn peek_base(&self, image: &str) -> Result<Base>;
    async fn run(&self, step: &RunStep) -> Result<RunOutcome>;
    /// Whether the image the build has so far holds a directory at this path, which is what decides where a single source lands.
    async fn holds_a_directory_at(&self, parent: &str, path: &str) -> Result<bool>;
    async fn copy(&self, step: &CopyStep) -> Result<ChangeSet>;
    async fn commit(&self, commit: &Commit<'_>) -> Result<String>;
    /// The image this machine built for the key and still holds, if it built one.
    async fn cached(&self, kind: Kind, key: &str) -> Option<Cached>;
    async fn remember(&self, kind: Kind, key: &str, built: &Cached);
}

/// What this machine already built for a key: the image, and whether the engine that built it stood outside the gate (§3.1.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cached {
    pub reference: String,
    pub built_outside_the_gate: bool,
}

/// What one build is asked for: the file, the bytes behind it, and whether the cache may answer.
pub(crate) struct BuildPlan<'a> {
    pub file: &'a Containerfile,
    /// What `spec.image` named, as every line about this build spells it.
    pub label: &'a str,
    /// The Containerfile as it was written, comments and all — the key measures the file, not the parse.
    pub text: &'a str,
    pub context_hash: &'a str,
    pub arch: &'a str,
    pub rebuild: bool,
    /// What the document declares a step may reach and carry, as `key::policy_fingerprint` spells it.
    pub policy: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Built {
    pub reference: String,
    pub layers: usize,
    /// The image key, which names this build wherever it is reported.
    pub key: String,
    /// True when the key answered outright, so nothing was run and nothing was committed.
    pub reused: bool,
    /// How many leading instructions stood on an image this machine already had.
    pub reused_steps: usize,
    /// True when the image this build answers with was built by the host Docker daemon, whichever engine this build was asked for.
    pub built_outside_the_gate: bool,
}

/// What the instructions have decided so far: the image to stand on, the scopes a `RUN` is given, and the config the next commit writes.
struct Build {
    parent: String,
    /// What the base image's own config already puts in the environment, which `ENV PATH=…:$PATH` expands against.
    base_env: Vec<(String, String)>,
    /// The `ARG`s declared before `FROM`: Docker keeps them out of the stage until it declares them again.
    global_args: Vec<(String, String)>,
    args: Vec<(String, String)>,
    config: ConfigDraft,
    user: String,
    workdir: String,
    shell: Vec<String>,
    layers: usize,
    /// A build the user asked to rebuild reads no key, and still writes every one it produces.
    rebuild: bool,
    reused_steps: usize,
    /// The policy every step of this build is keyed under, so no step of another document answers for one of these.
    policy: String,
}

impl Build {
    fn steps(&self) -> key::Steps<'_> {
        key::Steps::under(&self.policy)
    }
}

/// Everything the `FROM` decided: the base the build stands on, the key that names it, the image this machine already answers that key with, and the `ARG`s declared before the stage.
pub(crate) struct Opening {
    pub key: String,
    pub base: Base,
    /// What this machine built for the key and still holds; a `--rebuild` never reads one.
    pub cached: Option<Cached>,
    global_args: Vec<(String, String)>,
    /// How far into the file the `FROM` sat, so the loop resumes after it.
    after_from: usize,
}

/// Whether the base's layers have to be on this machine when the `FROM` is done: a build stands on them, a plan only needs the digest its key is taken over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BaseNeed {
    Ingested,
    PeekedOnly,
}

/// What the file says before anything is resolved: the `ARG`s declared before `FROM`, the line that `FROM` sits on, the image it names with those args expanded, and where the loop resumes.
pub(crate) struct Preamble {
    pub global_args: Vec<(String, String)>,
    pub line: usize,
    pub image: String,
    pub after_from: usize,
}

/// Read the file up to and including its `FROM`; every engine starts here, because the base is what a build stands on and what its key is taken over.
pub(crate) fn preamble(file: &Containerfile) -> Result<Preamble> {
    let mut global_args = Vec::new();
    let mut after_from = 0;
    let (line, image) = loop {
        let Some(instruction) = file.instructions.get(after_from) else {
            bail!(
                "a Containerfile must name what it builds on with FROM, and this one names nothing"
            );
        };
        after_from += 1;
        match &instruction.kind {
            InstructionKind::From { image, .. } => break (instruction.line, image),
            InstructionKind::Arg { name, default } => {
                if let Some(value) = default {
                    let value = expand(value, &global_args)?;
                    set(&mut global_args, name, &value);
                }
            }
            other => bail!(
                "line {}: {} comes before FROM, and a build has nothing to run it on yet",
                instruction.line,
                label(other),
            ),
        }
    };
    let image = expand(image, &global_args)?;
    Ok(Preamble {
        global_args,
        line,
        image,
        after_from,
    })
}

/// Every `ARG` the file gives a default, in the order it declares them: the build arguments another engine is handed, because lns states them and does not let a daemon guess.
pub(crate) fn arg_defaults(file: &Containerfile) -> Result<Vec<(String, String)>> {
    let mut args: Vec<(String, String)> = Vec::new();
    for instruction in &file.instructions {
        if let InstructionKind::Arg {
            name,
            default: Some(default),
        } = &instruction.kind
        {
            let value = expand(default, &args)?;
            set(&mut args, name, &value);
        }
    }
    Ok(args)
}

/// Read the file up to and including its `FROM`, resolve that base, and ask the key — everything a build and a plan agree on before they part.
pub(crate) async fn open<H: BuildHost>(
    host: &H,
    plan: &BuildPlan<'_>,
    need: BaseNeed,
) -> Result<Opening> {
    let Preamble {
        global_args,
        line,
        image,
        after_from,
    } = preamble(plan.file)?;
    let base = match need {
        BaseNeed::Ingested => host.resolve_base(&image).await,
        BaseNeed::PeekedOnly => host.peek_base(&image).await,
    }
    .with_context(|| format!("line {line}: FROM {image}"))?;
    let key = key::image_key(&base.reference, plan.text, plan.context_hash, plan.arch);
    let cached = match plan.rebuild {
        true => None,
        false => host.cached(Kind::Image, &key).await,
    };
    Ok(Opening {
        key,
        base,
        cached,
        global_args,
        after_from,
    })
}

/// The key a build is remembered under, and the image behind it when this machine has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Planned {
    pub key: String,
    pub reference: Option<String>,
    /// Whether the image the key already answers with was built outside the gate; a key nothing answers records nothing.
    pub built_outside_the_gate: bool,
}

/// What `lns push --dry-run` asks for: the key, and the digest only where the key already answers. Nothing is built.
pub(crate) async fn plan<H: BuildHost>(host: &H, request: &BuildPlan<'_>) -> Result<Planned> {
    let opening = open(host, request, BaseNeed::PeekedOnly).await?;
    Ok(Planned {
        key: opening.key,
        built_outside_the_gate: opening
            .cached
            .as_ref()
            .is_some_and(|cached| cached.built_outside_the_gate),
        reference: opening.cached.map(|cached| cached.reference),
    })
}

pub(crate) async fn build<H: BuildHost>(host: &H, plan: &BuildPlan<'_>) -> Result<Built> {
    let Opening {
        key,
        base,
        cached,
        global_args,
        after_from,
    } = open(host, plan, BaseNeed::Ingested).await?;
    let instructions = plan.file.instructions[after_from..].iter();
    if let Some(cached) = cached {
        return Ok(Built {
            reference: cached.reference,
            layers: 0,
            key,
            reused: true,
            reused_steps: plan.file.instructions.len(),
            built_outside_the_gate: cached.built_outside_the_gate,
        });
    }

    let ignoring = match plan.rebuild {
        true => ", ignoring the cache",
        false => "",
    };
    let count = plan.file.instructions.len();
    crate::log::info!(
        "Building",
        "{} ({count} instructions){ignoring}",
        plan.label
    );
    let mut build = Build {
        parent: base.reference,
        base_env: base.env,
        global_args,
        args: Vec::new(),
        config: ConfigDraft::default(),
        user: DEFAULT_USER.to_string(),
        workdir: DEFAULT_WORKDIR.to_string(),
        shell: DEFAULT_SHELL.map(str::to_string).to_vec(),
        layers: 0,
        rebuild: plan.rebuild,
        reused_steps: 0,
        policy: plan.policy.to_string(),
    };
    for instruction in instructions {
        let created_by = label(&instruction.kind);
        step(host, &mut build, instruction.line, &instruction.kind)
            .await
            .with_context(|| format!("line {}: {created_by}", instruction.line))?;
    }
    host.remember(Kind::Image, &key, &built_in_a_guest(&build.parent))
        .await;
    Ok(Built {
        reference: build.parent,
        layers: build.layers,
        key,
        reused: false,
        reused_steps: build.reused_steps,
        built_outside_the_gate: false,
    })
}

/// Every key the executor writes was filled by a build the document's egress and credentials decided.
fn built_in_a_guest(reference: &str) -> Cached {
    Cached {
        reference: reference.to_string(),
        built_outside_the_gate: false,
    }
}

async fn step<H: BuildHost>(
    host: &H,
    build: &mut Build,
    line: usize,
    kind: &InstructionKind,
) -> Result<()> {
    match kind {
        InstructionKind::From { image, .. } => {
            bail!("a second FROM ({image}) is not supported; lns builds one stage")
        }
        InstructionKind::Arg { name, default } => {
            declare_arg(build, name, default.as_deref())?;
            return Ok(());
        }
        InstructionKind::Run {
            command, here_docs, ..
        } => {
            let step = run_step(build, line, command, here_docs);
            let key = build.steps().run(&build.parent, &step);
            if reuse(host, build, &key, true).await {
                return Ok(());
            }
            let outcome = host.run(&step).await?;
            let (changes, dropped) =
                only_the_workloads_writes(outcome.changes, &outcome.fileset_paths);
            if dropped.total() > 0 {
                crate::log::debug!(
                    boot = dropped.boot,
                    fileset = dropped.fileset,
                    "the captured layer drops what this boot wrote for the guest",
                );
            }
            return commit(host, build, Some(&changes), kind, &key).await;
        }
        // The copied bytes are half of this instruction's key, so the context is read before the
        // cache is asked; reading it costs no guest.
        InstructionKind::Copy(transfer) | InstructionKind::Add(transfer) => {
            let step = copy_step(host, build, line, transfer).await?;
            let changes = host.copy(&step).await?;
            let key = build
                .steps()
                .transfer(&build.parent, &label(kind), &changes);
            if reuse(host, build, &key, true).await {
                return Ok(());
            }
            return commit(host, build, Some(&changes), kind, &key).await;
        }
        InstructionKind::Env(pairs) => {
            for (key, value) in pairs {
                let value = expand(value, &scope(build))?;
                set(&mut build.config.env, key, &value);
            }
        }
        InstructionKind::Label(pairs) => {
            for (key, value) in pairs {
                let value = expand(value, &scope(build))?;
                set(&mut build.config.labels, key, &value);
            }
        }
        InstructionKind::User(user) => {
            build.user = expand(user, &scope(build))?;
            build.config.user = Some(build.user.clone());
        }
        InstructionKind::Workdir(dir) => {
            build.workdir = join_workdir(&build.workdir, &expand(dir, &scope(build))?);
            build.config.workdir = Some(build.workdir.clone());
        }
        InstructionKind::Entrypoint(command) => {
            build.config.entrypoint = Some(argv_of(command, &build.shell));
        }
        InstructionKind::Cmd(command) => {
            build.config.cmd = Some(argv_of(command, &build.shell));
        }
        InstructionKind::Shell(shell) => {
            build.shell = shell.clone();
            build.config.shell = Some(shell.clone());
        }
        InstructionKind::Expose(ports) => {
            for port in ports {
                let port = expand(port, &scope(build))?;
                if !build.config.exposed_ports.contains(&port) {
                    build.config.exposed_ports.push(port);
                }
            }
        }
        InstructionKind::Volume(targets) => {
            for target in targets {
                let target = expand(target, &scope(build))?;
                if !build.config.volumes.contains(&target) {
                    build.config.volumes.push(target);
                }
            }
        }
    }
    let key = build
        .steps()
        .config(&build.parent, &label(kind), &build.config);
    if reuse(host, build, &key, false).await {
        return Ok(());
    }
    commit(host, build, None, kind, &key).await
}

/// A step this machine already has stands where the build was going to run one, and the build goes on from it.
async fn reuse<H: BuildHost>(host: &H, build: &mut Build, key: &str, layer: bool) -> bool {
    if build.rebuild {
        return false;
    }
    let Some(cached) = host.cached(Kind::Step, key).await else {
        return false;
    };
    build.parent = cached.reference;
    build.reused_steps += 1;
    if layer {
        build.layers += 1;
    }
    true
}

async fn commit<H: BuildHost>(
    host: &H,
    build: &mut Build,
    layer: Option<&ChangeSet>,
    kind: &InstructionKind,
    key: &str,
) -> Result<()> {
    build.parent = host
        .commit(&Commit {
            parent: &build.parent,
            layer,
            config: &build.config,
            created_by: &label(kind),
        })
        .await?;
    host.remember(Kind::Step, key, &built_in_a_guest(&build.parent))
        .await;
    if layer.is_some() {
        build.layers += 1;
    }
    Ok(())
}

/// A `RUN` is given the build arguments and the environment as one scope, the way Docker's shell sees them.
fn run_step(build: &Build, line: usize, command: &Command, here_docs: &[HereDoc]) -> RunStep {
    RunStep {
        parent: build.parent.clone(),
        argv: run_argv(build, command, here_docs),
        env: scope(build)
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect(),
        user: build.user.clone(),
        workdir: build.workdir.clone(),
        line,
    }
}

fn run_argv(build: &Build, command: &Command, here_docs: &[HereDoc]) -> Vec<String> {
    match command {
        Command::Exec(argv) => argv.clone(),
        Command::Shell(text) => {
            let mut argv = build.shell.clone();
            argv.push(shell_text(text, here_docs));
            argv
        }
    }
}

/// A here-document with nothing before it is the script itself; with a command before it, it is
/// that command's standard input, which is what handing the shell the here-document back does.
fn shell_text(text: &str, here_docs: &[HereDoc]) -> String {
    if here_docs.is_empty() {
        return text.to_string();
    }
    if text.is_empty() {
        return bodies(here_docs);
    }
    let mut script = text.to_string();
    let mut fed = String::new();
    for doc in here_docs {
        let delimiter = delimiter_for(&doc.body);
        let opener = match doc.expand {
            true => format!(" <<{delimiter}"),
            false => format!(" <<'{delimiter}'"),
        };
        script.push_str(&opener);
        fed.push_str(&ending_in_a_newline(&doc.body));
        fed.push_str(&delimiter);
        fed.push('\n');
    }
    format!("{script}\n{fed}")
}

fn bodies(here_docs: &[HereDoc]) -> String {
    here_docs
        .iter()
        .map(|doc| doc.body.clone())
        .collect::<Vec<String>>()
        .concat()
}

/// A body holding the delimiter on a line of its own would end the document early, so the delimiter grows until it cannot.
fn delimiter_for(body: &str) -> String {
    let mut delimiter = HERE_DOC_DELIMITER.to_string();
    while body.lines().any(|line| line == delimiter) {
        delimiter.push('X');
    }
    delimiter
}

fn ending_in_a_newline(body: &str) -> String {
    match body.ends_with('\n') || body.is_empty() {
        true => body.to_string(),
        false => format!("{body}\n"),
    }
}

/// Where a copy lands: a trailing slash and more than one source both name a directory, and for a
/// single source the image the build has so far is what says whether one already stands there.
async fn copy_step<H: BuildHost>(
    host: &H,
    build: &Build,
    line: usize,
    transfer: &Transfer,
) -> Result<CopyStep> {
    let scope = scope(build);
    let sources: Vec<String> = transfer
        .sources
        .iter()
        .map(|source| expand(source, &scope))
        .collect::<Result<_>>()?;
    let destination = join_workdir(&build.workdir, &expand(&transfer.destination, &scope)?);
    let into_directory = destination.ends_with('/')
        || sources.len() > 1
        || host
            .holds_a_directory_at(&build.parent, &destination)
            .await?;
    Ok(CopyStep {
        sources,
        destination,
        into_directory,
        owner: transfer.owner().map(str::to_string),
        mode: transfer.mode(),
        line,
    })
}

/// A stage inherits a global `ARG` only where it declares the name again, which is Docker's rule.
fn declare_arg(build: &mut Build, name: &str, default: Option<&str>) -> Result<()> {
    let value = match default {
        Some(default) => Some(expand(default, &scope(build))?),
        None => value_of(&build.global_args, name).map(str::to_string),
    };
    if let Some(value) = value {
        set(&mut build.args, name, &value);
    }
    Ok(())
}

/// What a `RUN` and every expansion see: the base image's environment, the build arguments over it, and this file's own environment over both.
fn scope(build: &Build) -> Vec<(String, String)> {
    let mut scope = build.base_env.clone();
    for (key, value) in &build.args {
        set(&mut scope, key, value);
    }
    for (key, value) in &build.config.env {
        set(&mut scope, key, value);
    }
    scope
}

fn set(pairs: &mut Vec<(String, String)>, key: &str, value: &str) {
    match pairs.iter_mut().find(|(existing, _)| existing == key) {
        Some(pair) => pair.1 = value.to_string(),
        None => pairs.push((key.to_string(), value.to_string())),
    }
}

fn value_of<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(existing, _)| existing == key)
        .map(|(_, value)| value.as_str())
}

/// `$NAME`, `${NAME}`, `${NAME:-default}` and `${NAME:+alt}`, the spellings Docker expands in an instruction's arguments; a name nothing declared expands to nothing.
pub(crate) fn expand(value: &str, scope: &[(String, String)]) -> Result<String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(dollar) = rest.find('$') {
        out.push_str(&rest[..dollar]);
        let after = &rest[dollar + 1..];
        rest = match after.strip_prefix('{') {
            Some(braced) => match braced.split_once('}') {
                Some((inside, tail)) => {
                    out.push_str(&braced_value(inside, scope)?);
                    tail
                }
                None => {
                    out.push('$');
                    after
                }
            },
            None => {
                let end = after
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .unwrap_or(after.len());
                match &after[..end] {
                    "" => out.push('$'),
                    name => out.push_str(value_of(scope, name).unwrap_or_default()),
                }
                &after[end..]
            }
        };
    }
    out.push_str(rest);
    Ok(out)
}

/// What `${…}` holds: a name, and at most one of the two modifiers that decide what an unset or a set name expands to.
fn braced_value(inside: &str, scope: &[(String, String)]) -> Result<String> {
    let end = inside
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(inside.len());
    let (name, modifier) = inside.split_at(end);
    if !names_a_variable(name) {
        bail!(
            "${{{inside}}} does not name a variable; a name is a letter or _ followed by letters, digits or _"
        );
    }
    let held = value_of(scope, name);
    match modifier {
        "" => Ok(held.unwrap_or_default().to_string()),
        _ => match modifier.split_at(2) {
            (":-", default) => match held {
                Some(value) if !value.is_empty() => Ok(value.to_string()),
                _ => expand(default, scope),
            },
            (":+", alternative) => match held {
                Some(value) if !value.is_empty() => expand(alternative, scope),
                _ => Ok(String::new()),
            },
            _ => bail!(
                "${{{inside}}} is not a variable lns expands; the forms are ${{NAME}}, ${{NAME:-default}} and ${{NAME:+alternative}}"
            ),
        },
    }
}

fn names_a_variable(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A relative `WORKDIR` or `COPY` destination joins the directory in force, as Docker defines it.
fn join_workdir(workdir: &str, path: &str) -> String {
    if path.starts_with('/') {
        return path.to_string();
    }
    format!("{}/{path}", workdir.trim_end_matches('/'))
}

fn argv_of(command: &Command, shell: &[String]) -> Vec<String> {
    match command {
        Command::Exec(argv) => argv.clone(),
        Command::Shell(text) => {
            let mut argv = shell.to_vec();
            argv.push(text.clone());
            argv
        }
    }
}

/// One instruction as its history entry and its refusal both name it.
fn label(kind: &InstructionKind) -> String {
    match kind {
        InstructionKind::From { image, .. } => format!("FROM {image}"),
        InstructionKind::Arg { name, default } => match default {
            Some(default) => format!("ARG {name}={default}"),
            None => format!("ARG {name}"),
        },
        InstructionKind::Env(pairs) => format!("ENV {}", assignments(pairs)),
        InstructionKind::Label(pairs) => format!("LABEL {}", assignments(pairs)),
        InstructionKind::User(user) => format!("USER {user}"),
        InstructionKind::Workdir(dir) => format!("WORKDIR {dir}"),
        InstructionKind::Run {
            command, here_docs, ..
        } => match command {
            Command::Shell(text) if text.is_empty() => format!("RUN {}", bodies(here_docs).trim()),
            Command::Shell(text) => format!("RUN {text}"),
            Command::Exec(argv) => format!("RUN {}", json_argv(argv)),
        },
        InstructionKind::Copy(transfer) => format!("COPY {}", transferred(transfer)),
        InstructionKind::Add(transfer) => format!("ADD {}", transferred(transfer)),
        InstructionKind::Entrypoint(command) => format!("ENTRYPOINT {}", commanded(command)),
        InstructionKind::Cmd(command) => format!("CMD {}", commanded(command)),
        InstructionKind::Shell(shell) => format!("SHELL {}", json_argv(shell)),
        InstructionKind::Expose(ports) => format!("EXPOSE {}", ports.join(" ")),
        InstructionKind::Volume(targets) => format!("VOLUME {}", targets.join(" ")),
    }
}

fn assignments(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<String>>()
        .join(" ")
}

fn transferred(transfer: &Transfer) -> String {
    format!("{} {}", transfer.sources.join(" "), transfer.destination)
}

fn commanded(command: &Command) -> String {
    match command {
        Command::Shell(text) => text.clone(),
        Command::Exec(argv) => json_argv(argv),
    }
}

fn json_argv(argv: &[String]) -> String {
    serde_json::to_string(argv).unwrap_or_else(|_| argv.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::containerfile::upper::{Change, ChangeSet};
    use lns_artifact::containerfile::parse;
    use std::sync::Mutex;

    /// What the loop asked the host to do, in the order it asked.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Base(String),
        Peek(String),
        Run(RunStep),
        Copy(CopyStep),
        Commit {
            parent: String,
            layer: Option<ChangeSet>,
            config: ConfigDraft,
            created_by: String,
        },
    }

    #[derive(Default)]
    struct FakeHost {
        calls: Mutex<Vec<Call>>,
        commits: Mutex<usize>,
        /// What this machine remembers, which survives from one build to the next of the same host.
        remembered: Mutex<std::collections::BTreeMap<(String, String), Cached>>,
        /// What the context answers a COPY with, so a test can edit a file between two builds.
        copied_bytes: Mutex<Vec<u8>>,
        wrote: Mutex<Option<ChangeSet>>,
        fileset_paths: Vec<String>,
        base_env: Vec<(String, String)>,
        /// The paths the image the build stands on holds a directory at, as the parent tree answers.
        directories: Vec<String>,
        run_fails_on_line: Option<usize>,
        /// One way for each of the other three answers to fail, so no test needs a host of its own.
        base_fails: bool,
        copy_fails: bool,
        commit_fails: bool,
    }

    impl FakeHost {
        fn new() -> Self {
            Self {
                copied_bytes: Mutex::new(b"context\n".to_vec()),
                ..Self::default()
            }
        }

        /// What every RUN's guest leaves in its upper, so one build can be read as one change set.
        fn writing(mut self, changes: ChangeSet) -> Self {
            self.wrote = Mutex::new(Some(changes));
            self
        }

        fn seeding(mut self, paths: &[&str]) -> Self {
            self.fileset_paths = paths.iter().map(|p| p.to_string()).collect();
            self
        }

        fn holding_directories(mut self, paths: &[&str]) -> Self {
            self.directories = paths.iter().map(|p| p.to_string()).collect();
            self
        }

        /// What the image the `FROM` names already declares in its config, the way a registry serves it.
        fn based_on(mut self, env: &[(&str, &str)]) -> Self {
            self.base_env = env
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            self
        }

        fn failing_on_line(mut self, line: usize) -> Self {
            self.run_fails_on_line = Some(line);
            self
        }

        fn with_no_base(mut self) -> Self {
            self.base_fails = true;
            self
        }

        fn with_no_context(mut self) -> Self {
            self.copy_fails = true;
            self
        }

        fn with_no_store(mut self) -> Self {
            self.commit_fails = true;
            self
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        fn commits(&self) -> Vec<(Option<ChangeSet>, ConfigDraft, String)> {
            self.calls()
                .into_iter()
                .filter_map(|call| match call {
                    Call::Commit {
                        layer,
                        config,
                        created_by,
                        ..
                    } => Some((layer, config, created_by)),
                    _ => None,
                })
                .collect()
        }

        /// The config the finished image carries, which is the last one committed.
        fn final_config(&self) -> ConfigDraft {
            self.commits()
                .pop()
                .expect("a build commits at least once")
                .1
        }

        fn copies(&self) -> Vec<CopyStep> {
            self.calls()
                .into_iter()
                .filter_map(|call| match call {
                    Call::Copy(step) => Some(step),
                    _ => None,
                })
                .collect()
        }

        fn runs(&self) -> Vec<RunStep> {
            self.calls()
                .into_iter()
                .filter_map(|call| match call {
                    Call::Run(step) => Some(step),
                    _ => None,
                })
                .collect()
        }
    }

    impl BuildHost for FakeHost {
        async fn resolve_base(&self, image: &str) -> Result<Base> {
            self.calls.lock().unwrap().push(Call::Base(image.into()));
            if self.base_fails {
                anyhow::bail!("no such image {image}");
            }
            Ok(Base {
                reference: format!("registry.test/{image}@sha256:base"),
                env: self.base_env.clone(),
            })
        }

        async fn peek_base(&self, image: &str) -> Result<Base> {
            self.calls.lock().unwrap().push(Call::Peek(image.into()));
            if self.base_fails {
                anyhow::bail!("no such image {image}");
            }
            Ok(Base {
                reference: format!("registry.test/{image}@sha256:base"),
                env: self.base_env.clone(),
            })
        }

        async fn run(&self, step: &RunStep) -> Result<RunOutcome> {
            self.calls.lock().unwrap().push(Call::Run(step.clone()));
            if self.run_fails_on_line == Some(step.line) {
                anyhow::bail!("the build guest's command exited 2");
            }
            Ok(RunOutcome {
                changes: self.wrote.lock().unwrap().clone().unwrap_or_default(),
                fileset_paths: self.fileset_paths.clone(),
            })
        }

        async fn holds_a_directory_at(&self, _parent: &str, path: &str) -> Result<bool> {
            Ok(self.directories.iter().any(|held| held == path))
        }

        async fn copy(&self, step: &CopyStep) -> Result<ChangeSet> {
            self.calls.lock().unwrap().push(Call::Copy(step.clone()));
            if self.copy_fails {
                anyhow::bail!("no such file in the build context");
            }
            Ok(ChangeSet {
                changes: vec![Change::Regular {
                    path: step.destination.trim_start_matches('/').into(),
                    mode: 0o644,
                    uid: 0,
                    gid: 0,
                    bytes: self.copied_bytes.lock().unwrap().clone(),
                }],
            })
        }

        async fn commit(&self, commit: &Commit<'_>) -> Result<String> {
            if self.commit_fails {
                anyhow::bail!("the layer cache is not writable");
            }
            let mut committed = self.commits.lock().unwrap();
            *committed += 1;
            self.calls.lock().unwrap().push(Call::Commit {
                parent: commit.parent.into(),
                layer: commit.layer.cloned(),
                config: commit.config.clone(),
                created_by: commit.created_by.into(),
            });
            Ok(format!("lns-build.local/built@sha256:step{committed}"))
        }

        async fn cached(&self, kind: Kind, key: &str) -> Option<Cached> {
            self.remembered
                .lock()
                .unwrap()
                .get(&(format!("{kind:?}"), key.to_string()))
                .cloned()
        }

        async fn remember(&self, kind: Kind, key: &str, built: &Cached) {
            self.remembered
                .lock()
                .unwrap()
                .insert((format!("{kind:?}"), key.to_string()), built.clone());
        }
    }

    fn containerfile(text: &str) -> lns_artifact::containerfile::Containerfile {
        parse(text).expect("the subset accepts this file")
    }

    fn build_plan<'a>(file: &'a Containerfile, text: &'a str) -> BuildPlan<'a> {
        BuildPlan {
            file,
            label: "./image/Containerfile",
            text,
            context_hash: "sha256:context",
            arch: "arm64",
            rebuild: false,
            policy: "sha256:the-policy-this-document-declares",
        }
    }

    async fn built(host: &FakeHost, text: &str) -> Built {
        let file = containerfile(text);
        build(host, &build_plan(&file, text))
            .await
            .expect("this Containerfile builds")
    }

    async fn refused(host: &FakeHost, text: &str) -> String {
        let file = containerfile(text);
        format!(
            "{:#}",
            build(host, &build_plan(&file, text))
                .await
                .expect_err("this Containerfile must stop the build")
        )
    }

    #[tokio::test]
    async fn the_instructions_run_in_the_order_they_were_written_and_each_stands_on_the_last() {
        let host = FakeHost::new();
        let built = built(
            &host,
            "FROM alpine:3.20\n\
             RUN echo one\n\
             COPY app /srv/app\n\
             RUN echo two\n",
        )
        .await;

        assert_eq!(
            order(&host),
            vec![
                "base alpine:3.20".to_string(),
                "run /bin/sh -c echo one".to_string(),
                "commit over registry.test/alpine:3.20@sha256:base".to_string(),
                "copy /srv/app".to_string(),
                "commit over lns-build.local/built@sha256:step1".to_string(),
                "run /bin/sh -c echo two".to_string(),
                "commit over lns-build.local/built@sha256:step2".to_string(),
            ],
        );
        assert_eq!(built.reference, "lns-build.local/built@sha256:step3");
        assert_eq!(built.layers, 3);
    }

    #[tokio::test]
    async fn a_run_runs_in_a_guest_booted_from_the_image_the_build_has_so_far() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nRUN echo one\nRUN echo two\n").await;

        let parents: Vec<String> = host.runs().into_iter().map(|step| step.parent).collect();
        assert_eq!(
            parents,
            vec![
                "registry.test/alpine@sha256:base".to_string(),
                "lns-build.local/built@sha256:step1".to_string(),
            ],
            "the next RUN boots the image the last instruction produced",
        );
    }

    #[tokio::test]
    async fn only_the_filesystem_instructions_produce_a_layer_and_the_rest_write_config() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\n\
             ENV MODE=research\n\
             RUN echo one\n\
             LABEL org.opencontainers.image.title=agent\n\
             COPY app /srv/app\n\
             USER node\n\
             WORKDIR /srv\n\
             ADD extra /srv/extra\n\
             ENTRYPOINT [\"/bin/agent\"]\n\
             CMD [\"--serve\"]\n\
             SHELL [\"/bin/bash\", \"-c\"]\n\
             EXPOSE 8080\n\
             VOLUME /data\n",
        )
        .await;

        let produced: Vec<(bool, String)> = host
            .commits()
            .into_iter()
            .map(|(layer, _, created_by)| (layer.is_some(), created_by))
            .collect();

        assert_eq!(
            produced,
            vec![
                (false, "ENV MODE=research".to_string()),
                (true, "RUN echo one".to_string()),
                (
                    false,
                    "LABEL org.opencontainers.image.title=agent".to_string()
                ),
                (true, "COPY app /srv/app".to_string()),
                (false, "USER node".to_string()),
                (false, "WORKDIR /srv".to_string()),
                (true, "ADD extra /srv/extra".to_string()),
                (false, "ENTRYPOINT [\"/bin/agent\"]".to_string()),
                (false, "CMD [\"--serve\"]".to_string()),
                (false, "SHELL [\"/bin/bash\",\"-c\"]".to_string()),
                (false, "EXPOSE 8080".to_string()),
                (false, "VOLUME /data".to_string()),
            ],
        );
    }

    #[tokio::test]
    async fn every_config_instruction_writes_the_field_it_owns() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\n\
             ENV MODE=research HOME=/home/node\n\
             LABEL org.opencontainers.image.title=agent\n\
             USER node\n\
             WORKDIR /srv\n\
             ENTRYPOINT [\"/bin/agent\"]\n\
             CMD [\"--serve\"]\n\
             SHELL [\"/bin/bash\", \"-c\"]\n\
             EXPOSE 8080\n\
             VOLUME /data\n",
        )
        .await;

        assert_eq!(
            host.final_config(),
            ConfigDraft {
                env: vec![
                    ("MODE".into(), "research".into()),
                    ("HOME".into(), "/home/node".into()),
                ],
                labels: vec![("org.opencontainers.image.title".into(), "agent".into())],
                user: Some("node".into()),
                workdir: Some("/srv".into()),
                entrypoint: Some(vec!["/bin/agent".into()]),
                cmd: Some(vec!["--serve".into()]),
                shell: Some(vec!["/bin/bash".into(), "-c".into()]),
                exposed_ports: vec!["8080".into()],
                volumes: vec!["/data".into()],
            },
        );
    }

    #[tokio::test]
    async fn a_later_env_replaces_the_value_the_earlier_one_set() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nENV MODE=first\nENV MODE=second\n").await;

        assert_eq!(
            host.final_config().env,
            vec![("MODE".to_string(), "second".to_string())],
            "one key holds one value, and the last instruction decides it",
        );
    }

    #[tokio::test]
    async fn an_arg_reaches_the_next_run_and_never_the_image_config() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nARG CLAUDE_CODE_VERSION=2.1.263\nRUN npm i -g claude@$CLAUDE_CODE_VERSION\n",
        )
        .await;

        assert_eq!(
            host.runs()[0].env,
            vec!["CLAUDE_CODE_VERSION=2.1.263".to_string()],
            "a build argument is what the RUN's shell expands",
        );
        assert!(
            host.final_config().env.is_empty(),
            "an ARG is a build-time value, so the built image must not carry it",
        );
    }

    #[tokio::test]
    async fn an_env_reaches_the_next_run_as_well_as_the_image_config() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nENV MODE=research\nRUN echo $MODE\n").await;

        assert_eq!(host.runs()[0].env, vec!["MODE=research".to_string()]);
        assert_eq!(
            host.final_config().env,
            vec![("MODE".to_string(), "research".to_string())],
        );
    }

    /// `FROM node:24-alpine` then `ENV PATH=/opt/agent/bin:$PATH` must keep the base's PATH, not truncate it.
    #[tokio::test]
    async fn the_base_image_s_environment_is_what_an_instruction_expands_against() {
        let host = FakeHost::new().based_on(&[("PATH", "/usr/local/bin:/usr/bin")]);
        built(
            &host,
            "FROM node:24-alpine\nENV PATH=/opt/agent/bin:$PATH\nRUN which agent\n",
        )
        .await;

        assert_eq!(
            host.final_config().env,
            vec![(
                "PATH".to_string(),
                "/opt/agent/bin:/usr/local/bin:/usr/bin".to_string()
            )],
        );
    }

    #[tokio::test]
    async fn a_run_carries_the_base_image_s_environment_even_where_the_file_declares_none() {
        let host = FakeHost::new().based_on(&[("PATH", "/usr/bin"), ("NODE_VERSION", "24.0.0")]);
        built(&host, "FROM node:24-alpine\nRUN node --version\n").await;

        assert_eq!(
            host.runs()[0].env,
            vec![
                "PATH=/usr/bin".to_string(),
                "NODE_VERSION=24.0.0".to_string()
            ],
        );
    }

    #[tokio::test]
    async fn an_env_outranks_an_arg_of_the_same_name_the_way_docker_defines_it() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nARG MODE=from-arg\nENV MODE=from-env\nRUN echo $MODE\n",
        )
        .await;

        assert_eq!(host.runs()[0].env, vec!["MODE=from-env".to_string()]);
    }

    #[tokio::test]
    async fn an_arg_declared_before_from_belongs_to_no_stage_until_it_is_declared_again() {
        let host = FakeHost::new();
        built(
            &host,
            "ARG BASE_TAG=3.20\nFROM alpine:$BASE_TAG\nRUN echo $BASE_TAG\n",
        )
        .await;

        assert_eq!(
            host.calls()[0],
            Call::Base("alpine:3.20".into()),
            "an ARG before FROM is what the base reference expands with",
        );
        assert!(
            host.runs()[0].env.is_empty(),
            "Docker keeps a global ARG out of the stage until the stage declares it again",
        );
    }

    #[tokio::test]
    async fn a_stage_that_declares_a_global_arg_again_inherits_its_value() {
        let host = FakeHost::new();
        built(
            &host,
            "ARG BASE_TAG=3.20\nFROM alpine:$BASE_TAG\nARG BASE_TAG\nRUN echo $BASE_TAG\n",
        )
        .await;

        assert_eq!(host.runs()[0].env, vec!["BASE_TAG=3.20".to_string()]);
    }

    #[tokio::test]
    async fn an_arg_with_no_value_anywhere_reaches_the_run_as_nothing_at_all() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nARG UNSET\nRUN echo $UNSET\n").await;

        assert!(host.runs()[0].env.is_empty());
    }

    #[tokio::test]
    async fn a_user_and_a_workdir_apply_to_every_run_after_them() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nRUN echo first\nUSER node\nWORKDIR /srv\nRUN echo second\n",
        )
        .await;

        let steps = host.runs();
        assert_eq!(
            (steps[0].user.as_str(), steps[0].workdir.as_str()),
            ("root", "/")
        );
        assert_eq!(
            (steps[1].user.as_str(), steps[1].workdir.as_str()),
            ("node", "/srv"),
            "a RUN takes the identity and the directory the instructions before it set",
        );
    }

    #[tokio::test]
    async fn a_relative_workdir_joins_the_one_in_force() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nWORKDIR /srv\nWORKDIR app\nRUN echo hi\n",
        )
        .await;

        assert_eq!(host.runs()[0].workdir, "/srv/app");
        assert_eq!(host.final_config().workdir, Some("/srv/app".to_string()));
    }

    #[tokio::test]
    async fn a_shell_instruction_decides_how_the_next_run_in_shell_form_is_run() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nRUN echo first\nSHELL [\"/bin/bash\", \"-lc\"]\nRUN echo second\n",
        )
        .await;

        let argv: Vec<Vec<String>> = host.runs().into_iter().map(|step| step.argv).collect();
        assert_eq!(
            argv,
            vec![
                vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "echo first".to_string()
                ],
                vec![
                    "/bin/bash".to_string(),
                    "-lc".to_string(),
                    "echo second".to_string()
                ],
            ],
        );
    }

    #[tokio::test]
    async fn a_run_in_exec_form_is_run_without_any_shell() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nRUN [\"/bin/agent\", \"--check\"]\n").await;

        assert_eq!(
            host.runs()[0].argv,
            vec!["/bin/agent".to_string(), "--check".to_string()],
        );
    }

    #[tokio::test]
    async fn a_here_document_run_with_nothing_before_it_is_run_as_the_script_it_holds() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nRUN <<EOF\necho one\necho two\nEOF\n").await;

        assert_eq!(
            host.runs()[0].argv,
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo one\necho two\n".to_string()
            ],
        );
    }

    /// `RUN <<EOF cat > /entry` writes the file: the rest of the line is the command, and the body
    /// is what it reads, which is what handing the shell the here-document back does.
    #[tokio::test]
    async fn a_here_document_run_keeps_the_rest_of_its_line_and_feeds_the_body_on_stdin() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nRUN <<EOF cat > /usr/local/bin/entry\n#!/bin/sh\necho hi\nEOF\n",
        )
        .await;

        assert_eq!(
            host.runs()[0].argv,
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "cat > /usr/local/bin/entry <<LNSHEREDOC\n#!/bin/sh\necho hi\nLNSHEREDOC\n"
                    .to_string(),
            ],
        );
    }

    /// The delimiter ends the here-document only where it stands at the start of a line.
    #[test]
    fn a_body_the_author_left_unterminated_gets_the_newline_the_delimiter_stands_on() {
        assert_eq!(ending_in_a_newline("echo hi"), "echo hi\n");
        assert_eq!(ending_in_a_newline("echo hi\n"), "echo hi\n");
        assert_eq!(ending_in_a_newline(""), "");
    }

    #[tokio::test]
    async fn a_quoted_here_document_delimiter_keeps_the_shell_out_of_the_body() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nRUN <<'EOF' cat > /entry\necho $HOME\nEOF\n",
        )
        .await;

        assert_eq!(
            host.runs()[0].argv[2],
            "cat > /entry <<'LNSHEREDOC'\necho $HOME\nLNSHEREDOC\n",
        );
    }

    #[tokio::test]
    async fn a_body_that_spells_the_delimiter_gets_one_the_body_cannot_end() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nRUN <<EOF cat > /entry\nLNSHEREDOC\nEOF\n",
        )
        .await;

        assert_eq!(
            host.runs()[0].argv[2],
            "cat > /entry <<LNSHEREDOCX\nLNSHEREDOC\nLNSHEREDOCX\n",
        );
    }

    #[tokio::test]
    async fn what_this_boot_wrote_for_the_run_is_kept_out_of_every_captured_layer() {
        let host = FakeHost::new()
            .writing(ChangeSet {
                changes: vec![
                    change("opt/tool/bin"),
                    change(".lens/.cmdline"),
                    change("etc/resolv.conf"),
                    change("opt/agent-skills/prompts.md"),
                ],
            })
            .seeding(&["/opt/agent-skills"]);

        built(
            &host,
            "FROM alpine\nRUN install-a-tool\nRUN install-another\n",
        )
        .await;

        for (layer, _, created_by) in host.commits() {
            let paths: Vec<String> = layer
                .expect("a RUN commits a layer")
                .changes
                .iter()
                .map(|change| change.path().to_string())
                .collect();
            assert_eq!(
                paths,
                vec!["opt/tool/bin".to_string()],
                "{created_by} carried what lns wrote for the guest, or what a fileset seeded",
            );
        }
    }

    fn change(path: &str) -> Change {
        Change::Regular {
            path: path.into(),
            mode: 0o644,
            uid: 0,
            gid: 0,
            bytes: b"x".to_vec(),
        }
    }

    #[tokio::test]
    async fn a_copy_lands_where_the_workdir_in_force_puts_it() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nWORKDIR /srv\nCOPY app dist\n").await;

        let step = host.copies().pop().expect("the COPY reached the host");
        assert_eq!(step.destination, "/srv/dist");
        assert_eq!(step.sources, vec!["app".to_string()]);
    }

    /// `COPY entrypoint.sh /usr/local/bin` where the image has that directory writes into it; a
    /// regular file at that path would shadow every binary the base image shipped there.
    #[tokio::test]
    async fn a_copy_onto_a_directory_the_image_already_has_lands_inside_it() {
        let host = FakeHost::new().holding_directories(&["/usr/local/bin"]);
        built(&host, "FROM alpine\nCOPY entrypoint.sh /usr/local/bin\n").await;

        let step = host.copies().pop().expect("the COPY reached the host");
        assert!(
            step.into_directory,
            "the image holds a directory at the destination, so the source lands in it",
        );
    }

    #[tokio::test]
    async fn a_copy_onto_a_path_the_image_has_no_directory_at_writes_the_file_there() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nCOPY entrypoint.sh /usr/local/bin/entry\n",
        )
        .await;

        let step = host.copies().pop().expect("the COPY reached the host");
        assert!(!step.into_directory);
    }

    #[tokio::test]
    async fn a_trailing_slash_and_several_sources_each_name_a_directory_without_asking_the_image() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nCOPY skills/ /opt/agent-skills/\nCOPY one two /srv/both\n",
        )
        .await;

        let steps = host.copies();
        assert!(
            steps[0].into_directory,
            "a trailing slash names a directory"
        );
        assert!(
            steps[1].into_directory,
            "more than one source can only land in a directory",
        );
    }

    #[tokio::test]
    async fn a_copy_expands_a_build_argument_in_its_paths() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nARG VERSION=1.2.3\nCOPY dist/$VERSION /srv/app\n",
        )
        .await;

        let step = host.copies().pop().expect("the COPY reached the host");
        assert_eq!(step.sources, vec!["dist/1.2.3".to_string()]);
    }

    #[tokio::test]
    async fn a_braced_name_a_lone_dollar_and_an_unterminated_brace_each_expand_as_docker_does() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nARG V=1.2.3\nENV BRACED=x${V}y PLAIN=$V$ UNCLOSED=${V\n",
        )
        .await;

        assert_eq!(
            host.final_config().env,
            vec![
                ("BRACED".to_string(), "x1.2.3y".to_string()),
                ("PLAIN".to_string(), "1.2.3$".to_string()),
                ("UNCLOSED".to_string(), "${V".to_string()),
            ],
        );
    }

    #[tokio::test]
    async fn a_default_stands_in_for_a_name_that_holds_nothing() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nARG SET=1.2.3\nARG EMPTY=\nENV HELD=${SET:-fallback} MISSING=${NOTHING:-fallback} BLANK=${EMPTY:-fallback} NESTED=${NOTHING:-$SET}\n",
        )
        .await;

        assert_eq!(
            host.final_config().env,
            vec![
                ("HELD".to_string(), "1.2.3".to_string()),
                ("MISSING".to_string(), "fallback".to_string()),
                ("BLANK".to_string(), "fallback".to_string()),
                ("NESTED".to_string(), "1.2.3".to_string()),
            ],
        );
    }

    #[tokio::test]
    async fn an_alternative_stands_in_only_where_the_name_holds_something() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nARG SET=1.2.3\nENV HELD=${SET:+--version=$SET} MISSING=${NOTHING:+--version}\n",
        )
        .await;

        assert_eq!(
            host.final_config().env,
            vec![
                ("HELD".to_string(), "--version=1.2.3".to_string()),
                ("MISSING".to_string(), String::new()),
            ],
        );
    }

    #[tokio::test]
    async fn a_braced_expansion_that_names_no_variable_stops_the_build() {
        let host = FakeHost::new();

        let refusal = refused(&host, "FROM alpine\nENV OUT=${8ball}\n").await;

        assert!(refusal.contains("${8ball}"), "{refusal}");
        assert!(refusal.contains("does not name a variable"), "{refusal}");
    }

    #[tokio::test]
    async fn a_braced_modifier_lns_does_not_expand_stops_the_build_naming_the_ones_it_does() {
        let host = FakeHost::new();

        let refusal = refused(&host, "FROM alpine\nARG V=1\nENV OUT=${V#prefix}\n").await;

        assert!(refusal.contains("${V#prefix}"), "{refusal}");
        assert!(refusal.contains("${NAME:-default}"), "{refusal}");
    }

    #[tokio::test]
    async fn a_name_nothing_declared_expands_to_nothing() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nWORKDIR /srv/$NOTHING\n").await;

        assert_eq!(host.final_config().workdir, Some("/srv/".to_string()));
    }

    /// Docker keeps the shell form of `CMD` and `ENTRYPOINT`, so the config carries what the shell
    /// in force would have run rather than the words the author wrote.
    #[tokio::test]
    async fn a_cmd_and_an_entrypoint_in_shell_form_are_run_through_the_shell_in_force() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nSHELL [\"/bin/bash\", \"-lc\"]\nENTRYPOINT /bin/agent --serve\nCMD --check\n",
        )
        .await;

        let config = host.final_config();
        assert_eq!(
            config.entrypoint,
            Some(vec![
                "/bin/bash".to_string(),
                "-lc".to_string(),
                "/bin/agent --serve".to_string()
            ]),
        );
        assert_eq!(
            config.cmd,
            Some(vec![
                "/bin/bash".to_string(),
                "-lc".to_string(),
                "--check".to_string()
            ]),
        );
        let created_by: Vec<String> = host
            .commits()
            .into_iter()
            .map(|(_, _, created_by)| created_by)
            .collect();
        assert!(
            created_by.contains(&"ENTRYPOINT /bin/agent --serve".to_string()),
            "the history names the instruction as it was written: {created_by:?}",
        );
    }

    #[tokio::test]
    async fn a_run_that_fails_stops_the_build_naming_its_line_and_what_it_was() {
        let host = FakeHost::new().failing_on_line(3);
        let refusal = refused(
            &host,
            "FROM alpine\nRUN echo one\nRUN reach-an-undeclared-host\n",
        )
        .await;

        assert!(refusal.contains("line 3"), "{refusal}");
        assert!(
            refusal.contains("RUN reach-an-undeclared-host"),
            "{refusal}"
        );
        assert!(refusal.contains("exited 2"), "{refusal}");
        assert_eq!(
            host.runs().len(),
            2,
            "the instruction after the failure must not run",
        );
    }

    /// The parser refuses both of these before the loop sees them; the loop still answers for a
    /// parsed file it is handed, because slice 2's type is what it takes and any caller can build one.
    #[tokio::test]
    async fn an_instruction_before_from_stops_the_build_naming_its_line() {
        let host = FakeHost::new();
        let file = Containerfile {
            instructions: vec![
                lns_artifact::containerfile::Instruction {
                    line: 1,
                    kind: InstructionKind::Run {
                        command: Command::Shell("echo hi".into()),
                        here_docs: Vec::new(),
                        flags: Vec::new(),
                    },
                },
                lns_artifact::containerfile::Instruction {
                    line: 2,
                    kind: InstructionKind::From {
                        image: "alpine".into(),
                        flags: Vec::new(),
                    },
                },
            ],
        };

        let refusal = format!(
            "{:#}",
            build(&host, &build_plan(&file, "")).await.unwrap_err()
        );
        assert!(refusal.contains("line 1"), "{refusal}");
        assert!(refusal.contains("RUN echo hi"), "{refusal}");
        assert!(refusal.contains("FROM"), "{refusal}");
        assert!(
            host.calls().is_empty(),
            "a build that cannot start must reach no guest and no store",
        );
        assert!(
            parse("RUN echo hi\nFROM alpine\n").is_err(),
            "the parser refuses the same file first",
        );
    }

    #[tokio::test]
    async fn a_file_with_no_from_at_all_stops_the_build() {
        let host = FakeHost::new();
        let file = Containerfile {
            instructions: vec![lns_artifact::containerfile::Instruction {
                line: 1,
                kind: InstructionKind::Arg {
                    name: "VERSION".into(),
                    default: Some("1".into()),
                },
            }],
        };

        let refusal = format!(
            "{:#}",
            build(&host, &build_plan(&file, "")).await.unwrap_err()
        );
        assert!(refusal.contains("FROM"), "{refusal}");
        assert!(parse("# nothing but a comment\n").is_err());
    }

    #[tokio::test]
    async fn a_second_from_stops_the_build_where_the_loop_meets_it() {
        let host = FakeHost::new();
        let file = Containerfile {
            instructions: vec![
                instruction(
                    1,
                    InstructionKind::From {
                        image: "alpine".into(),
                        flags: Vec::new(),
                    },
                ),
                instruction(
                    2,
                    InstructionKind::From {
                        image: "node:24".into(),
                        flags: Vec::new(),
                    },
                ),
            ],
        };

        let refusal = format!(
            "{:#}",
            build(&host, &build_plan(&file, "")).await.unwrap_err()
        );
        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("a second FROM"), "{refusal}");
        assert!(
            parse("FROM alpine\nFROM node:24\n").is_err(),
            "the parser refuses the same file first",
        );
    }

    fn instruction(line: usize, kind: InstructionKind) -> lns_artifact::containerfile::Instruction {
        lns_artifact::containerfile::Instruction { line, kind }
    }

    #[tokio::test]
    async fn a_containerfile_that_only_names_its_base_boots_that_base() {
        let host = FakeHost::new();
        let built = built(&host, "FROM alpine:3.20\n").await;

        assert_eq!(built.reference, "registry.test/alpine:3.20@sha256:base");
        assert_eq!(built.layers, 0);
        assert!(
            host.commits().is_empty(),
            "a file that changes nothing about the base has nothing to commit",
        );
    }

    #[tokio::test]
    async fn a_base_the_build_cannot_resolve_stops_it_before_any_instruction_runs() {
        let host = FakeHost::new().with_no_base();
        let refusal = refused(&host, "FROM missing:1\nRUN echo hi\n").await;

        assert!(refusal.contains("line 1"), "{refusal}");
        assert!(refusal.contains("FROM missing:1"), "{refusal}");
        assert!(
            host.runs().is_empty(),
            "an instruction must not run on an image the build has not got",
        );
    }

    #[tokio::test]
    async fn a_copy_the_context_cannot_answer_stops_the_build_naming_its_line() {
        let host = FakeHost::new().with_no_context();
        let refusal = refused(&host, "FROM alpine\nCOPY missing /srv\n").await;

        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("COPY missing /srv"), "{refusal}");
        assert!(refusal.contains("build context"), "{refusal}");
    }

    #[tokio::test]
    async fn a_commit_the_store_refuses_stops_the_build_naming_the_instruction() {
        let host = FakeHost::new().with_no_store();
        let refusal = refused(&host, "FROM alpine\nENV MODE=research\n").await;

        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("ENV MODE=research"), "{refusal}");
        assert!(refusal.contains("layer cache"), "{refusal}");
    }

    /// The whole image answers for the key, so a second build of an untouched document boots no
    /// guest, commits nothing and says it did neither.
    #[tokio::test]
    async fn a_second_build_of_an_unchanged_containerfile_runs_nothing_at_all() {
        let host = FakeHost::new();
        let text = "FROM alpine\nRUN echo one\nUSER node\n";
        let first = built(&host, text).await;

        let second = built(&host, text).await;

        assert_eq!(second.reference, first.reference);
        assert_eq!(second.key, first.key);
        assert!(second.reused, "an unchanged build is a no-op");
        assert_eq!(host.runs().len(), 1, "the second build boots no guest");
        assert_eq!(host.commits().len(), 2, "the second build commits nothing");
    }

    /// What the host was asked for, in the order it was asked, as one readable line each.
    fn order(host: &FakeHost) -> Vec<String> {
        host.calls()
            .into_iter()
            .map(|call| match call {
                Call::Base(image) => format!("base {image}"),
                Call::Peek(image) => format!("peek {image}"),
                Call::Run(step) => format!("run {}", step.argv.join(" ")),
                Call::Copy(step) => format!("copy {}", step.destination),
                Call::Commit { parent, .. } => format!("commit over {parent}"),
            })
            .collect()
    }

    async fn planned(host: &FakeHost, text: &str) -> Planned {
        let file = containerfile(text);
        plan(host, &build_plan(&file, text))
            .await
            .expect("a plan reads the key of a Containerfile the subset accepts")
    }

    /// What `lns push --dry-run` answers with: a key always, and a digest only where the key already answers.
    #[tokio::test]
    async fn a_plan_answers_with_the_key_and_builds_nothing() {
        let host = FakeHost::new();
        let text = "FROM alpine\nRUN echo one\n";

        let planned = planned(&host, text).await;

        assert!(planned.key.starts_with("sha256:"), "{}", planned.key);
        assert_eq!(
            planned.reference, None,
            "a digest this machine has never built can only be known by building"
        );
        assert!(host.runs().is_empty(), "a plan boots no guest");
        assert!(host.commits().is_empty(), "a plan commits nothing");
    }

    /// A dry run says it builds nothing, so its FROM must cost a manifest read rather than the base's every layer.
    #[tokio::test]
    async fn a_plan_reads_its_base_without_ingesting_it() {
        let host = FakeHost::new();

        let planned = planned(&host, "FROM alpine\nRUN echo one\n").await;

        assert!(planned.key.starts_with("sha256:"));
        assert_eq!(
            order(&host),
            vec!["peek alpine".to_string()],
            "a plan peeks the base; only a build pulls it"
        );
    }

    #[tokio::test]
    async fn a_plan_of_a_base_no_registry_answers_for_is_refused_where_a_build_would_be() {
        let host = FakeHost::new().with_no_base();
        let file = containerfile("FROM alpine\nRUN echo one\n");

        let refusal = format!(
            "{:#}",
            plan(&host, &build_plan(&file, "FROM alpine\nRUN echo one\n"))
                .await
                .expect_err("a plan whose FROM resolves to nothing has no key to answer with")
        );
        assert!(refusal.contains("line 1: FROM alpine"), "{refusal}");
        assert!(refusal.contains("no such image"), "{refusal}");
    }

    #[tokio::test]
    async fn a_plan_of_a_build_this_machine_already_made_names_its_image() {
        let host = FakeHost::new();
        let text = "FROM alpine\nRUN echo one\n";
        let built = built(&host, text).await;

        let planned = planned(&host, text).await;

        assert_eq!(planned.key, built.key);
        assert_eq!(planned.reference, Some(built.reference));
        assert_eq!(host.runs().len(), 1, "the plan boots no second guest");
    }

    #[tokio::test]
    async fn a_plan_that_ignores_the_cache_names_no_image_although_one_is_held() {
        let host = FakeHost::new();
        let text = "FROM alpine\nRUN echo one\n";
        built(&host, text).await;
        let file = containerfile(text);

        let planned = plan(
            &host,
            &BuildPlan {
                rebuild: true,
                ..build_plan(&file, text)
            },
        )
        .await
        .unwrap();

        assert_eq!(
            planned.reference, None,
            "a --rebuild reads no key, so a preview must not promise the digest it would drop"
        );
    }

    #[tokio::test]
    async fn a_plan_of_a_file_with_no_from_is_refused_the_way_a_build_is() {
        let host = FakeHost::new();
        let file = Containerfile {
            instructions: vec![lns_artifact::containerfile::Instruction {
                line: 1,
                kind: InstructionKind::Arg {
                    name: "VERSION".into(),
                    default: Some("1".into()),
                },
            }],
        };
        let refusal = format!(
            "{:#}",
            plan(&host, &build_plan(&file, ""))
                .await
                .expect_err("a file that names nothing to build on cannot be planned either")
        );
        assert!(refusal.contains("FROM"), "{refusal}");
    }

    #[tokio::test]
    async fn a_build_reuses_every_leading_step_and_rebuilds_from_the_first_that_differs() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nRUN echo one\nRUN echo two\nRUN echo three\n",
        )
        .await;

        let again = built(
            &host,
            "FROM alpine\nRUN echo one\nRUN echo changed\nRUN echo three\n",
        )
        .await;

        let commands: Vec<String> = host
            .runs()
            .into_iter()
            .map(|step| step.argv.join(" "))
            .collect();
        assert_eq!(
            commands,
            [
                "/bin/sh -c echo one",
                "/bin/sh -c echo two",
                "/bin/sh -c echo three",
                "/bin/sh -c echo changed",
                "/bin/sh -c echo three",
            ],
            "only the first RUN is reused; the one that changed and the one after it run again",
        );
        assert_eq!(again.reused_steps, 1);
        assert_eq!(
            again.layers, 3,
            "a reused step still put a layer in the image"
        );
        assert!(!again.reused);
    }

    /// An `ARG` commits nothing, so the image the next `RUN` stands on is the same one, and the
    /// line is the same line; only the environment the command resolves against tells them apart.
    #[tokio::test]
    async fn an_arg_the_run_reads_makes_it_a_different_step_although_the_line_is_unchanged() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nARG V=1\nRUN install agent@$V\n").await;

        let again = built(&host, "FROM alpine\nARG V=2\nRUN install agent@$V\n").await;

        let envs: Vec<Vec<String>> = host.runs().into_iter().map(|step| step.env).collect();
        assert_eq!(envs, [[String::from("V=1")], [String::from("V=2")]]);
        assert_eq!(
            again.reused_steps, 0,
            "the second RUN reaches the guest, because the value it reads is not the first one",
        );
    }

    #[tokio::test]
    async fn a_copy_whose_file_changed_is_a_different_step_although_the_instruction_is_the_same() {
        let host = FakeHost::new();
        let text = "FROM alpine\nCOPY app /srv/app\nRUN echo after\n";
        let file = containerfile(text);
        build(&host, &build_plan(&file, text)).await.unwrap();
        let before = host.commits().len();
        *host.copied_bytes.lock().unwrap() = b"edited\n".to_vec();

        let again = build(
            &host,
            &BuildPlan {
                context_hash: "sha256:edited",
                ..build_plan(&file, text)
            },
        )
        .await
        .unwrap();

        assert_eq!(
            again.reused_steps, 0,
            "the copy's own bytes are half of its key, so it is not the step it was",
        );
        assert_eq!(
            host.commits().len(),
            before + 2,
            "the copy and the instruction after it are both committed again",
        );
    }

    /// A comment above the instructions is a different file and so a different image, and every
    /// instruction is still the instruction it was: the build commits a new image and runs nothing.
    #[tokio::test]
    async fn a_containerfile_whose_instructions_are_unchanged_reuses_every_step_of_them() {
        let host = FakeHost::new();
        let text = "FROM alpine\nRUN echo one\nCOPY app /srv/app\nUSER node\n";
        let file = containerfile(text);
        build(&host, &build_plan(&file, text)).await.unwrap();
        let runs = host.runs().len();
        let commits = host.commits().len();

        let again = build(
            &host,
            &build_plan(&file, "# what this image is for\nFROM alpine\nRUN echo one\nCOPY app /srv/app\nUSER node\n"),
        )
        .await
        .unwrap();

        assert_eq!(
            again.reused_steps, 3,
            "every instruction stands where it stood"
        );
        assert_eq!(
            again.layers, 2,
            "the two reused steps still put their layers in the image"
        );
        assert!(
            !again.reused,
            "the image key is a new one, so the image is remembered again"
        );
        assert_eq!(
            host.runs().len(),
            runs,
            "no guest boots for a step a key answers"
        );
        assert_eq!(
            host.commits().len(),
            commits,
            "and nothing is committed twice"
        );
    }

    #[tokio::test]
    async fn a_rebuild_reads_no_key_and_still_writes_every_one_it_produces() {
        let host = FakeHost::new();
        let text = "FROM alpine\nRUN echo one\n";
        built(&host, text).await;

        let file = containerfile(text);
        let forced = build(
            &host,
            &BuildPlan {
                rebuild: true,
                ..build_plan(&file, text)
            },
        )
        .await
        .expect("a forced build builds");

        assert!(!forced.reused);
        assert_eq!(host.runs().len(), 2, "--rebuild boots the guest again");
        assert_eq!(
            built(&host, text).await.reference,
            forced.reference,
            "the build it forced is the one the next build finds",
        );
    }

    #[tokio::test]
    async fn a_build_whose_context_changed_is_a_different_image_although_the_file_is_the_same() {
        let host = FakeHost::new();
        let text = "FROM alpine\nRUN echo one\n";
        let file = containerfile(text);
        let first = build(&host, &build_plan(&file, text)).await.unwrap();

        let second = build(
            &host,
            &BuildPlan {
                context_hash: "sha256:edited",
                ..build_plan(&file, text)
            },
        )
        .await
        .unwrap();

        assert_ne!(second.key, first.key);
        assert!(!second.reused);
    }

    #[tokio::test]
    async fn the_key_a_build_reports_is_the_one_the_next_build_of_it_answers() {
        let host = FakeHost::new();
        let text = "FROM alpine\nUSER node\n";
        let key = built(&host, text).await.key;

        assert!(key.starts_with("sha256:"), "{key}");
        assert_eq!(
            host.cached(Kind::Image, &key)
                .await
                .map(|cached| cached.reference),
            Some("lns-build.local/built@sha256:step1".to_string()),
        );
    }
    /// The switch says `lns` again, but the image this key answers with is the daemon's, so the build reports the engine that made it (§3.1.1).
    #[tokio::test]
    async fn a_key_the_daemon_filled_answers_a_build_in_a_guest_and_still_says_so() {
        let host = FakeHost::new();
        let text = "FROM alpine\nRUN echo one\n";
        let file = containerfile(text);
        let key = key::image_key(
            "registry.test/alpine@sha256:base",
            text,
            "sha256:context",
            "arm64",
        );
        host.remember(
            Kind::Image,
            &key,
            &Cached {
                reference: "lns-build.local/built@sha256:daemon".to_string(),
                built_outside_the_gate: true,
            },
        )
        .await;

        let reused = build(&host, &build_plan(&file, text)).await.unwrap();

        assert!(reused.reused, "the key answers, so nothing is run");
        assert!(
            reused.built_outside_the_gate,
            "the gate did not apply to the image this run boots",
        );
    }

    #[tokio::test]
    async fn a_build_in_a_guest_is_remembered_as_one_the_gate_applied_to() {
        let host = FakeHost::new();
        let text = "FROM alpine\nRUN echo one\n";
        let built = built(&host, text).await;

        assert!(!built.built_outside_the_gate);
        assert_eq!(
            host.cached(Kind::Image, &built.key)
                .await
                .map(|cached| cached.built_outside_the_gate),
            Some(false),
        );
    }

    /// Another engine is handed what the file declares, not what it happens to default to, and a later default may stand on an earlier one.
    #[test]
    fn every_arg_with_a_default_is_a_build_argument_expanded_against_the_ones_before_it() {
        let file = lns_artifact::containerfile::parse(
            "ARG MAJOR=24\nARG TAG=${MAJOR}-bookworm\nFROM node:$TAG\nARG BARE\nARG V=2.1.263\n",
        )
        .expect("a Containerfile lns builds");
        assert_eq!(
            arg_defaults(&file).expect("reading the defaults"),
            vec![
                ("MAJOR".to_string(), "24".to_string()),
                ("TAG".to_string(), "24-bookworm".to_string()),
                ("V".to_string(), "2.1.263".to_string()),
            ],
            "an ARG with no default states nothing",
        );
    }
}
