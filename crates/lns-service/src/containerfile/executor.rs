//! The instruction loop: slice 3 of lensapp/lens-sandbox#393.
//!
//! One instruction at a time, in the order it was written. A `RUN` runs in a build guest booted
//! from the image the build has so far and what it wrote becomes one layer; a `COPY` or an `ADD`
//! becomes one layer of the context's files; everything else writes the image config.

use anyhow::{Context, Result, bail};

use super::exclude::only_the_workloads_writes;
use super::parse::{Command, Containerfile, InstructionKind, Transfer};
use super::upper::ChangeSet;

/// The shell a `RUN` in shell form is run through until a `SHELL` instruction says otherwise.
const DEFAULT_SHELL: [&str; 2] = ["/bin/sh", "-c"];

/// The identity a `RUN` runs as until a `USER` instruction says otherwise: a build installs into
/// the image, which nothing else in the guest may write.
const DEFAULT_USER: &str = "root";

/// The directory a `RUN` runs in until a `WORKDIR` instruction says otherwise.
const DEFAULT_WORKDIR: &str = "/";

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
    pub owner: Option<String>,
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

/// What the loop needs of the world: a registry, a build guest, the build context, and the local store.
pub(crate) trait BuildHost {
    /// The digest-pinned reference the `FROM` resolved to, pulled so the build can stand on its config.
    async fn resolve_base(&self, image: &str) -> Result<String>;
    async fn run(&self, step: &RunStep) -> Result<RunOutcome>;
    async fn copy(&self, step: &CopyStep) -> Result<ChangeSet>;
    async fn commit(&self, commit: &Commit<'_>) -> Result<String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Built {
    pub reference: String,
    pub layers: usize,
}

/// What the instructions have decided so far: the image to stand on, the scopes a `RUN` is given, and the config the next commit writes.
struct Build {
    parent: String,
    /// The `ARG`s declared before `FROM`: Docker keeps them out of the stage until it declares them again.
    global_args: Vec<(String, String)>,
    args: Vec<(String, String)>,
    config: ConfigDraft,
    user: String,
    workdir: String,
    shell: Vec<String>,
    layers: usize,
}

pub(crate) async fn build<H: BuildHost>(host: &H, file: &Containerfile) -> Result<Built> {
    let mut global_args = Vec::new();
    let mut instructions = file.instructions.iter();
    let from = loop {
        let Some(instruction) = instructions.next() else {
            bail!(
                "a Containerfile must name what it builds on with FROM, and this one names nothing"
            );
        };
        match &instruction.kind {
            InstructionKind::From { image } => break (instruction.line, image),
            InstructionKind::Arg { name, default } => {
                if let Some(value) = default {
                    let value = expand(value, &global_args);
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
    let (line, image) = from;
    let image = expand(image, &global_args);
    let parent = host
        .resolve_base(&image)
        .await
        .with_context(|| format!("line {line}: FROM {image}"))?;

    let mut build = Build {
        parent,
        global_args,
        args: Vec::new(),
        config: ConfigDraft::default(),
        user: DEFAULT_USER.to_string(),
        workdir: DEFAULT_WORKDIR.to_string(),
        shell: DEFAULT_SHELL.map(str::to_string).to_vec(),
        layers: 0,
    };
    for instruction in instructions {
        let created_by = label(&instruction.kind);
        step(host, &mut build, instruction.line, &instruction.kind)
            .await
            .with_context(|| format!("line {}: {created_by}", instruction.line))?;
    }
    Ok(Built {
        reference: build.parent,
        layers: build.layers,
    })
}

async fn step<H: BuildHost>(
    host: &H,
    build: &mut Build,
    line: usize,
    kind: &InstructionKind,
) -> Result<()> {
    match kind {
        InstructionKind::From { image } => {
            bail!("a second FROM ({image}) is not supported; lns builds one stage")
        }
        InstructionKind::Arg { name, default } => {
            declare_arg(build, name, default.as_deref());
            return Ok(());
        }
        InstructionKind::Run { command, here_docs } => {
            let outcome = host.run(&run_step(build, line, command, here_docs)).await?;
            let (changes, dropped) =
                only_the_workloads_writes(outcome.changes, &outcome.fileset_paths);
            if dropped.total() > 0 {
                crate::log::debug!(
                    boot = dropped.boot,
                    fileset = dropped.fileset,
                    "the captured layer drops what this boot wrote for the guest",
                );
            }
            return commit(host, build, Some(&changes), kind).await;
        }
        InstructionKind::Copy(transfer) | InstructionKind::Add(transfer) => {
            let changes = host.copy(&copy_step(build, line, transfer)).await?;
            return commit(host, build, Some(&changes), kind).await;
        }
        InstructionKind::Env(pairs) => {
            for (key, value) in pairs {
                let value = expand(value, &scope(build));
                set(&mut build.config.env, key, &value);
            }
        }
        InstructionKind::Label(pairs) => {
            for (key, value) in pairs {
                let value = expand(value, &scope(build));
                set(&mut build.config.labels, key, &value);
            }
        }
        InstructionKind::User(user) => {
            build.user = expand(user, &scope(build));
            build.config.user = Some(build.user.clone());
        }
        InstructionKind::Workdir(dir) => {
            build.workdir = join_workdir(&build.workdir, &expand(dir, &scope(build)));
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
                let port = expand(port, &scope(build));
                if !build.config.exposed_ports.contains(&port) {
                    build.config.exposed_ports.push(port);
                }
            }
        }
        InstructionKind::Volume(targets) => {
            for target in targets {
                let target = expand(target, &scope(build));
                if !build.config.volumes.contains(&target) {
                    build.config.volumes.push(target);
                }
            }
        }
    }
    commit(host, build, None, kind).await
}

async fn commit<H: BuildHost>(
    host: &H,
    build: &mut Build,
    layer: Option<&ChangeSet>,
    kind: &InstructionKind,
) -> Result<()> {
    build.parent = host
        .commit(&Commit {
            parent: &build.parent,
            layer,
            config: &build.config,
            created_by: &label(kind),
        })
        .await?;
    if layer.is_some() {
        build.layers += 1;
    }
    Ok(())
}

/// A `RUN` is given the build arguments and the environment as one scope, the way Docker's shell sees them.
fn run_step(build: &Build, line: usize, command: &Command, here_docs: &[String]) -> RunStep {
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

fn run_argv(build: &Build, command: &Command, here_docs: &[String]) -> Vec<String> {
    match command {
        Command::Exec(argv) => argv.clone(),
        Command::Shell(text) => {
            let mut argv = build.shell.clone();
            argv.push(if here_docs.is_empty() {
                text.clone()
            } else {
                here_docs.concat()
            });
            argv
        }
    }
}

fn copy_step(build: &Build, line: usize, transfer: &Transfer) -> CopyStep {
    let scope = scope(build);
    CopyStep {
        sources: transfer
            .sources
            .iter()
            .map(|source| expand(source, &scope))
            .collect(),
        destination: join_workdir(&build.workdir, &expand(&transfer.destination, &scope)),
        owner: transfer.owner.clone(),
        line,
    }
}

/// A stage inherits a global `ARG` only where it declares the name again, which is Docker's rule.
fn declare_arg(build: &mut Build, name: &str, default: Option<&str>) {
    let value = match default {
        Some(default) => Some(expand(default, &scope(build))),
        None => value_of(&build.global_args, name).map(str::to_string),
    };
    if let Some(value) = value {
        set(&mut build.args, name, &value);
    }
}

/// What a `RUN` and every expansion see: the build arguments, with the environment over them.
fn scope(build: &Build) -> Vec<(String, String)> {
    let mut scope = build.args.clone();
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

/// `$NAME` and `${NAME}`, the two spellings Docker expands in an instruction's arguments; a name nothing declared expands to nothing.
fn expand(value: &str, scope: &[(String, String)]) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(dollar) = rest.find('$') {
        out.push_str(&rest[..dollar]);
        let after = &rest[dollar + 1..];
        let (name, tail) = match after.strip_prefix('{') {
            Some(braced) => match braced.split_once('}') {
                Some((name, tail)) => (name, tail),
                None => {
                    out.push('$');
                    rest = after;
                    continue;
                }
            },
            None => {
                let end = after
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .unwrap_or(after.len());
                (&after[..end], &after[end..])
            }
        };
        if name.is_empty() {
            out.push('$');
        } else {
            out.push_str(value_of(scope, name).unwrap_or_default());
        }
        rest = tail;
    }
    out.push_str(rest);
    out
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
        InstructionKind::From { image } => format!("FROM {image}"),
        InstructionKind::Arg { name, default } => match default {
            Some(default) => format!("ARG {name}={default}"),
            None => format!("ARG {name}"),
        },
        InstructionKind::Env(pairs) => format!("ENV {}", assignments(pairs)),
        InstructionKind::Label(pairs) => format!("LABEL {}", assignments(pairs)),
        InstructionKind::User(user) => format!("USER {user}"),
        InstructionKind::Workdir(dir) => format!("WORKDIR {dir}"),
        InstructionKind::Run { command, here_docs } => match command {
            Command::Shell(text) if text.is_empty() => format!("RUN {}", here_docs.concat().trim()),
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
    use crate::containerfile::parse::parse;
    use crate::containerfile::upper::{Change, ChangeSet};
    use std::sync::Mutex;

    /// What the loop asked the host to do, in the order it asked.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Base(String),
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
        wrote: Mutex<Option<ChangeSet>>,
        fileset_paths: Vec<String>,
        run_fails_on_line: Option<usize>,
        /// One way for each of the other three answers to fail, so no test needs a host of its own.
        base_fails: bool,
        copy_fails: bool,
        commit_fails: bool,
    }

    impl FakeHost {
        fn new() -> Self {
            Self::default()
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
        async fn resolve_base(&self, image: &str) -> Result<String> {
            self.calls.lock().unwrap().push(Call::Base(image.into()));
            if self.base_fails {
                anyhow::bail!("no such image {image}");
            }
            Ok(format!("registry.test/{image}@sha256:base"))
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
                    bytes: b"context\n".to_vec(),
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
    }

    fn containerfile(text: &str) -> crate::containerfile::parse::Containerfile {
        parse(text).expect("the subset accepts this file")
    }

    async fn built(host: &FakeHost, text: &str) -> Built {
        build(host, &containerfile(text))
            .await
            .expect("this Containerfile builds")
    }

    async fn refused(host: &FakeHost, text: &str) -> String {
        format!(
            "{:#}",
            build(host, &containerfile(text))
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

        let order: Vec<String> = host
            .calls()
            .into_iter()
            .map(|call| match call {
                Call::Base(image) => format!("base {image}"),
                Call::Run(step) => format!("run {}", step.argv.join(" ")),
                Call::Copy(step) => format!("copy {}", step.destination),
                Call::Commit { parent, .. } => format!("commit over {parent}"),
            })
            .collect();

        assert_eq!(
            order,
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
    async fn a_here_document_run_is_run_as_the_script_it_holds() {
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
                crate::containerfile::parse::Instruction {
                    line: 1,
                    kind: InstructionKind::Run {
                        command: Command::Shell("echo hi".into()),
                        here_docs: Vec::new(),
                    },
                },
                crate::containerfile::parse::Instruction {
                    line: 2,
                    kind: InstructionKind::From {
                        image: "alpine".into(),
                    },
                },
            ],
        };

        let refusal = format!("{:#}", build(&host, &file).await.unwrap_err());
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
            instructions: vec![crate::containerfile::parse::Instruction {
                line: 1,
                kind: InstructionKind::Arg {
                    name: "VERSION".into(),
                    default: Some("1".into()),
                },
            }],
        };

        let refusal = format!("{:#}", build(&host, &file).await.unwrap_err());
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
                    },
                ),
                instruction(
                    2,
                    InstructionKind::From {
                        image: "node:24".into(),
                    },
                ),
            ],
        };

        let refusal = format!("{:#}", build(&host, &file).await.unwrap_err());
        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("a second FROM"), "{refusal}");
        assert!(
            parse("FROM alpine\nFROM node:24\n").is_err(),
            "the parser refuses the same file first",
        );
    }

    fn instruction(line: usize, kind: InstructionKind) -> crate::containerfile::parse::Instruction {
        crate::containerfile::parse::Instruction { line, kind }
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
}
