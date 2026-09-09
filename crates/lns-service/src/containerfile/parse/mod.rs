//! The Containerfile the executor runs, parsed behind one adapter.
//!
//! Slice 2 of lensapp/lens-sandbox#393 owns this type in `lns-artifact`
//! (`lns_artifact::containerfile`), where the CLI's `validate` reads it too. That branch is not
//! in this one's base, so the executor parses through the same upstream crate here and keeps the
//! same type shape: when slice 2 lands, this module goes and the executor imports theirs.

mod upstream;

/// A Containerfile lns can build: one stage's instructions, in the order they were written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Containerfile {
    pub instructions: Vec<Instruction>,
}

/// One instruction and the line it was written on, so a refusal and an audit entry can both name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Instruction {
    pub line: usize,
    pub kind: InstructionKind,
}

/// The v1 instruction subset — every spelling lns builds; anything else stops the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstructionKind {
    From {
        image: String,
    },
    Arg {
        name: String,
        default: Option<String>,
    },
    Env(Vec<(String, String)>),
    Label(Vec<(String, String)>),
    User(String),
    Workdir(String),
    Run {
        command: Command,
        here_docs: Vec<HereDoc>,
    },
    Copy(Transfer),
    Add(Transfer),
    Entrypoint(Command),
    Cmd(Command),
    Shell(Vec<String>),
    Expose(Vec<String>),
    Volume(Vec<String>),
}

/// One here-document body of a `RUN`, and whether the shell expands variables in it — a quoted delimiter (`<<'EOF'`) says it does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HereDoc {
    pub expand: bool,
    pub body: String,
}

/// A `RUN`, `CMD` or `ENTRYPOINT` argument in the two forms Docker defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    Shell(String),
    Exec(Vec<String>),
}

/// What one `COPY` or `ADD` moves from the context into the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Transfer {
    pub sources: Vec<String>,
    pub destination: String,
    pub owner: Option<String>,
    /// The octal mode `--chmod` gives every entry the transfer writes.
    pub mode: Option<u32>,
}

/// The issue that decides what the subset grows to hold, named by every refusal that has no alternative today.
const SUBSET_ISSUE: &str = "https://github.com/lensapp/lens-sandbox/issues/393";

/// The suffixes Docker unpacks an `ADD` source by, which is a second behaviour behind one instruction.
const ARCHIVE_SUFFIXES: [&str; 9] = [
    ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz", ".tar.zst", ".tzst",
];

/// Parse a Containerfile and hold it to the v1 subset; each refusal names its instruction and its line.
pub(crate) fn parse(text: &str) -> Result<Containerfile, Vec<String>> {
    let parsed = parse_dockerfile::parse(text).map_err(|e| {
        vec![format!(
            "line {}: {}",
            e.line(),
            first_sentence(&e.to_string())
        )]
    })?;
    let lines = LineIndex::of(text);
    let mut instructions = Vec::new();
    let mut refusals = Vec::new();
    let mut seen_from = false;
    for instruction in &parsed.instructions {
        let line = lines.line_of(upstream::keyword_start(instruction));
        match accept(instruction, line, &mut seen_from) {
            Ok(kinds) => {
                instructions.extend(kinds.into_iter().map(|kind| Instruction { line, kind }))
            }
            Err(refusal) => refusals.push(refusal),
        }
    }
    if refusals.is_empty() {
        Ok(Containerfile { instructions })
    } else {
        Err(refusals)
    }
}

/// The parser reports the position twice — in the message and in `line()` — and a refusal states it once.
fn first_sentence(message: &str) -> String {
    match message.split_once(" at line ") {
        Some((head, _)) => head.to_string(),
        None => message.to_string(),
    }
}

/// Byte offsets are what the parser reports and lines are what an author reads.
struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn of(text: &str) -> Self {
        let starts = std::iter::once(0)
            .chain(
                text.match_indices('\n')
                    .map(|(offset, newline)| offset + newline.len()),
            )
            .collect();
        Self { starts }
    }

    fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|start| *start <= offset)
    }
}

fn accept(
    instruction: &parse_dockerfile::Instruction<'_>,
    line: usize,
    seen_from: &mut bool,
) -> Result<Vec<InstructionKind>, String> {
    use parse_dockerfile::Instruction as I;
    match instruction {
        I::From(from) => one(accept_from(from, line, seen_from)?),
        I::Arg(arg) => Ok(accept_arg(&arg.arguments.value)),
        I::Env(env) => one(InstructionKind::Env(key_values(&env.arguments.value))),
        I::Label(label) => one(InstructionKind::Label(key_values(&label.arguments.value))),
        I::User(user) => one(InstructionKind::User(user.arguments.value.to_string())),
        I::Workdir(workdir) => one(InstructionKind::Workdir(
            workdir.arguments.value.to_string(),
        )),
        I::Run(run) => one(accept_run(run, line)?),
        I::Copy(copy) => one(accept_copy(copy, line)?),
        I::Add(add) => one(accept_add(add, line)?),
        I::Entrypoint(entrypoint) => one(InstructionKind::Entrypoint(upstream::command(
            &entrypoint.arguments,
        ))),
        I::Cmd(cmd) => one(InstructionKind::Cmd(upstream::command(&cmd.arguments))),
        I::Shell(shell) => one(InstructionKind::Shell(
            shell
                .arguments
                .iter()
                .map(|word| word.value.to_string())
                .collect(),
        )),
        I::Expose(expose) => one(InstructionKind::Expose(
            expose
                .arguments
                .iter()
                .map(|port| port.value.to_string())
                .collect(),
        )),
        I::Volume(volume) => one(InstructionKind::Volume(volume_targets(&volume.arguments))),
        other => Err(format!(
            "line {line}: {} is not supported and has no alternative in v1; ask for it on {SUBSET_ISSUE}",
            upstream::unsupported_keyword(other)
        )),
    }
}

fn accept_from(
    from: &parse_dockerfile::FromInstruction<'_>,
    line: usize,
    seen_from: &mut bool,
) -> Result<InstructionKind, String> {
    if *seen_from {
        return Err(format!(
            "line {line}: a second FROM is not supported; lns builds one stage, so build the earlier stage as its own image and name it in FROM"
        ));
    }
    *seen_from = true;
    Ok(InstructionKind::From {
        image: from.image.value.to_string(),
    })
}

/// `ARG <name>[=<default>] [<name>[=<default>]...]` names several variables on one line, and each is its own `Arg` so an expansion reads one name at a time.
fn accept_arg(arguments: &str) -> Vec<InstructionKind> {
    split_words(arguments)
        .iter()
        .map(|word| match word.split_once('=') {
            Some((name, default)) => InstructionKind::Arg {
                name: name.to_string(),
                default: Some(unquote(default).to_string()),
            },
            None => InstructionKind::Arg {
                name: word.to_string(),
                default: None,
            },
        })
        .collect()
}

fn one(kind: InstructionKind) -> Result<Vec<InstructionKind>, String> {
    Ok(vec![kind])
}

fn split_words(arguments: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut chars = arguments.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), c) => {
                if c == '\'' {
                    quote = None;
                } else {
                    word.push(c);
                }
            }
            (_, '\\') => word.push(chars.next().unwrap_or('\\')),
            (Some(open), c) if c == open => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            (None, c) => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn accept_run(
    run: &parse_dockerfile::RunInstruction<'_>,
    line: usize,
) -> Result<InstructionKind, String> {
    for name in ["network", "security"] {
        if let Some(flag) = run.options.iter().find(|flag| flag.name.value == name) {
            let value = flag.value.as_ref().map(|v| v.value.as_ref()).unwrap_or("");
            return Err(format!(
                "line {line}: RUN --{name}={value} is not supported; a build step runs under the document's own network and confinement, so there is nothing to widen it to"
            ));
        }
    }
    if let Some(mount) = run.options.iter().find(|flag| flag.name.value == "mount") {
        let target = mount.value.as_ref().map(|v| v.value.as_ref()).unwrap_or("");
        return Err(format!(
            "line {line}: RUN --mount={target} is not supported; COPY the file into the image and RUN against it"
        ));
    }
    Ok(InstructionKind::Run {
        command: upstream::command(&run.arguments),
        here_docs: run
            .here_docs
            .iter()
            .map(|doc| HereDoc {
                expand: doc.expand,
                body: doc.value.to_string(),
            })
            .collect(),
    })
}

fn accept_copy(
    copy: &parse_dockerfile::CopyInstruction<'_>,
    line: usize,
) -> Result<InstructionKind, String> {
    if let Some(stage) = copy.options.iter().find(|flag| flag.name.value == "from") {
        let stage = stage.value.as_ref().map(|v| v.value.as_ref()).unwrap_or("");
        return Err(format!(
            "line {line}: COPY --from={stage} is not supported; lns builds one stage, so build the earlier stage as its own image and name it in FROM"
        ));
    }
    if copy.options.iter().any(|flag| flag.name.value == "link") {
        return Err(format!(
            "line {line}: COPY --link is not supported; lns commits every step as its own layer already, so drop the flag"
        ));
    }
    Ok(InstructionKind::Copy(transfer(
        "COPY",
        &copy.src,
        &copy.dest,
        &copy.options,
        line,
    )?))
}

fn accept_add(
    add: &parse_dockerfile::AddInstruction<'_>,
    line: usize,
) -> Result<InstructionKind, String> {
    if add.options.iter().any(|flag| flag.name.value == "link") {
        return Err(format!(
            "line {line}: ADD --link is not supported; lns commits every step as its own layer already, so drop the flag"
        ));
    }
    let moved = transfer("ADD", &add.src, &add.dest, &add.options, line)?;
    for source in &moved.sources {
        if source.contains("://") {
            return Err(format!(
                "line {line}: ADD source {source:?} is a URL, which is not supported; RUN curl to fetch it, so the fetch is decided by the document's egress"
            ));
        }
        if ARCHIVE_SUFFIXES
            .iter()
            .any(|suffix| source.ends_with(suffix))
        {
            return Err(format!(
                "line {line}: ADD source {source:?} would be unpacked, which is not supported; COPY the archive in and RUN tar to unpack it"
            ));
        }
    }
    Ok(InstructionKind::Add(moved))
}

fn transfer(
    keyword: &str,
    sources: &[parse_dockerfile::Source<'_>],
    destination: &parse_dockerfile::UnescapedString<'_>,
    options: &[parse_dockerfile::Flag<'_>],
    line: usize,
) -> Result<Transfer, String> {
    let mut paths = Vec::with_capacity(sources.len());
    for source in sources {
        match source {
            parse_dockerfile::Source::Path(path) => paths.push(path.value.to_string()),
            _ => {
                return Err(format!(
                    "line {line}: a here-document source for {keyword} is not supported; write the file with RUN instead"
                ));
            }
        }
    }
    Ok(Transfer {
        sources: paths,
        destination: destination.value.to_string(),
        owner: flag_value(options, "chown"),
        mode: match flag_value(options, "chmod") {
            None => None,
            Some(written) => Some(u32::from_str_radix(&written, 8).map_err(|_| {
                format!(
                    "line {line}: {keyword} --chmod={written} is not an octal mode; write it as --chmod=755"
                )
            })?),
        },
    })
}

fn flag_value(options: &[parse_dockerfile::Flag<'_>], name: &str) -> Option<String> {
    options
        .iter()
        .find(|flag| flag.name.value == name)
        .and_then(|flag| flag.value.as_ref())
        .map(|value| value.value.to_string())
}

/// `KEY=VALUE` pairs, or the one-key form Docker keeps for `ENV KEY the rest of the line`.
fn key_values(arguments: &str) -> Vec<(String, String)> {
    let words = shlex::split(arguments).unwrap_or_else(|| {
        arguments
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<String>>()
    });
    match words.split_first() {
        Some((first, rest)) if first.contains('=') => std::iter::once(first)
            .chain(rest)
            .filter_map(|word| word.split_once('='))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        Some((first, rest)) => vec![(first.clone(), rest.join(" "))],
        None => Vec::new(),
    }
}

fn volume_targets(arguments: &parse_dockerfile::JsonOrStringArray<'_, 1>) -> Vec<String> {
    match arguments {
        parse_dockerfile::JsonOrStringArray::Json(json) => {
            json.value.iter().map(|v| v.value.to_string()).collect()
        }
        parse_dockerfile::JsonOrStringArray::String(words) => {
            words.iter().map(|v| v.value.to_string()).collect()
        }
    }
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<InstructionKind> {
        parse(text)
            .expect("the subset accepts this file")
            .instructions
            .into_iter()
            .map(|instruction| instruction.kind)
            .collect()
    }

    fn refusal(text: &str) -> String {
        parse(text)
            .expect_err("this file is outside the subset")
            .join("; ")
    }

    #[test]
    fn every_instruction_in_the_subset_parses_in_the_order_it_was_written() {
        let kinds = kinds(
            "FROM alpine:3.20\n\
             ARG VERSION=1.2.3\n\
             ENV PATH=/usr/bin HOME=/root\n\
             LABEL org.opencontainers.image.title=agent\n\
             USER node\n\
             WORKDIR /srv\n\
             RUN echo hi\n\
             COPY --chown=node:node app /srv/app\n\
             ADD extra /srv/extra\n\
             ENTRYPOINT [\"/bin/agent\"]\n\
             CMD [\"--serve\"]\n\
             SHELL [\"/bin/bash\", \"-c\"]\n\
             EXPOSE 8080\n\
             VOLUME /data\n",
        );

        assert_eq!(
            kinds,
            vec![
                InstructionKind::From {
                    image: "alpine:3.20".into()
                },
                InstructionKind::Arg {
                    name: "VERSION".into(),
                    default: Some("1.2.3".into()),
                },
                InstructionKind::Env(vec![
                    ("PATH".into(), "/usr/bin".into()),
                    ("HOME".into(), "/root".into()),
                ]),
                InstructionKind::Label(vec![(
                    "org.opencontainers.image.title".into(),
                    "agent".into()
                )]),
                InstructionKind::User("node".into()),
                InstructionKind::Workdir("/srv".into()),
                InstructionKind::Run {
                    command: Command::Shell("echo hi".into()),
                    here_docs: Vec::new(),
                },
                InstructionKind::Copy(Transfer {
                    sources: vec!["app".into()],
                    destination: "/srv/app".into(),
                    owner: Some("node:node".into()),
                    mode: None,
                }),
                InstructionKind::Add(Transfer {
                    sources: vec!["extra".into()],
                    destination: "/srv/extra".into(),
                    owner: None,
                    mode: None,
                }),
                InstructionKind::Entrypoint(Command::Exec(vec!["/bin/agent".into()])),
                InstructionKind::Cmd(Command::Exec(vec!["--serve".into()])),
                InstructionKind::Shell(vec!["/bin/bash".into(), "-c".into()]),
                InstructionKind::Expose(vec!["8080".into()]),
                InstructionKind::Volume(vec!["/data".into()]),
            ]
        );
    }

    #[test]
    fn an_instruction_records_the_line_it_was_written_on() {
        let parsed = parse("FROM alpine:3.20\n\nRUN echo one\n\nRUN echo two\n").unwrap();
        let lines: Vec<usize> = parsed.instructions.iter().map(|i| i.line).collect();
        assert_eq!(lines, vec![1, 3, 5]);
    }

    #[test]
    fn an_arg_naming_several_variables_records_one_arg_for_each_of_them() {
        let parsed =
            parse("FROM alpine\nARG NODE_VERSION=24 CLAUDE_CODE_VERSION=2.1.263 BARE\n").unwrap();
        let kinds: Vec<InstructionKind> = parsed.instructions[1..]
            .iter()
            .map(|i| i.kind.clone())
            .collect();

        assert_eq!(
            kinds,
            vec![
                InstructionKind::Arg {
                    name: "NODE_VERSION".into(),
                    default: Some("24".into()),
                },
                InstructionKind::Arg {
                    name: "CLAUDE_CODE_VERSION".into(),
                    default: Some("2.1.263".into()),
                },
                InstructionKind::Arg {
                    name: "BARE".into(),
                    default: None,
                },
            ],
            "each name carries its own default, not the rest of the line"
        );
        assert!(
            parsed.instructions[1..].iter().all(|i| i.line == 2),
            "every one of them was written on line 2: {parsed:?}"
        );
    }

    #[test]
    fn an_arg_with_no_default_carries_none() {
        assert_eq!(
            kinds("FROM alpine\nARG VERSION\n")[1],
            InstructionKind::Arg {
                name: "VERSION".into(),
                default: None,
            },
        );
    }

    #[test]
    fn a_quoted_arg_default_keeps_the_value_and_not_the_quotes() {
        assert_eq!(
            kinds("FROM alpine\nARG GREETING=\"hello world\"\n")[1],
            InstructionKind::Arg {
                name: "GREETING".into(),
                default: Some("hello world".into()),
            },
        );
    }

    #[test]
    fn the_one_key_env_form_takes_the_rest_of_the_line_as_the_value() {
        assert_eq!(
            kinds("FROM alpine\nENV GREETING hello world\n")[1],
            InstructionKind::Env(vec![("GREETING".into(), "hello world".into())]),
        );
    }

    #[test]
    fn a_here_doc_run_keeps_its_body_and_the_rest_of_its_line() {
        assert_eq!(
            kinds("FROM alpine\nRUN <<EOF cat > /out\necho hi\nEOF\n")[1],
            InstructionKind::Run {
                command: Command::Shell("cat > /out".into()),
                here_docs: vec![HereDoc {
                    expand: true,
                    body: "echo hi\n".into(),
                }],
            },
        );
    }

    /// A quoted delimiter is Docker's spelling for "do not expand this body", and the build owes it.
    #[test]
    fn a_quoted_here_doc_delimiter_says_the_body_is_not_expanded() {
        let InstructionKind::Run { here_docs, .. } =
            kinds("FROM alpine\nRUN <<'EOF'\necho $HOME\nEOF\n")[1].clone()
        else {
            panic!("this is a RUN");
        };

        assert_eq!(
            here_docs,
            vec![HereDoc {
                expand: false,
                body: "echo $HOME\n".into(),
            }],
        );
    }

    #[test]
    fn a_volume_in_json_form_and_in_string_form_read_alike() {
        assert_eq!(
            kinds("FROM alpine\nVOLUME [\"/data\", \"/cache\"]\n")[1],
            InstructionKind::Volume(vec!["/data".into(), "/cache".into()]),
        );
        assert_eq!(
            kinds("FROM alpine\nVOLUME /data /cache\n")[1],
            InstructionKind::Volume(vec!["/data".into(), "/cache".into()]),
        );
    }

    #[test]
    fn a_run_in_exec_form_keeps_its_argv() {
        assert_eq!(
            kinds("FROM alpine\nRUN [\"/bin/sh\", \"-c\", \"echo hi\"]\n")[1],
            InstructionKind::Run {
                command: Command::Exec(vec!["/bin/sh".into(), "-c".into(), "echo hi".into()]),
                here_docs: Vec::new(),
            },
        );
    }

    /// The two shapes an assignment list can take that no Containerfile the parser accepts can
    /// reach: an unbalanced quote, which shell splitting refuses, and no arguments at all.
    #[test]
    fn an_assignment_list_falls_back_to_words_when_a_quote_is_unbalanced_and_is_empty_for_nothing()
    {
        assert_eq!(
            key_values("GREETING=\"unclosed value"),
            vec![("GREETING".to_string(), "\"unclosed".to_string())],
            "an unbalanced quote must still yield the assignments it can, not nothing",
        );
        assert_eq!(key_values("   "), Vec::new());
    }

    #[test]
    fn a_second_from_is_refused_with_its_line_and_the_alternative() {
        let refusal = refusal("FROM alpine\nRUN echo one\nFROM node:24 AS build\n");
        assert!(refusal.contains("line 3"), "{refusal}");
        assert!(refusal.contains("a second FROM"), "{refusal}");
        assert!(refusal.contains("one stage"), "{refusal}");
    }

    #[test]
    fn a_copy_from_another_stage_is_refused_with_its_line() {
        let refusal = refusal("FROM alpine\nCOPY --from=build /out /srv\n");
        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("COPY --from=build"), "{refusal}");
    }

    #[test]
    fn a_run_mount_is_refused_and_names_the_target_it_asked_for() {
        let refusal = refusal("FROM alpine\nRUN --mount=type=cache,target=/root/.npm npm i\n");
        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("RUN --mount="), "{refusal}");
        assert!(
            refusal.contains("COPY the file into the image"),
            "{refusal}"
        );
    }

    #[test]
    fn a_copy_chmod_is_read_as_the_octal_mode_it_names() {
        assert_eq!(
            kinds("FROM alpine\nCOPY --chmod=755 entrypoint.sh /usr/local/bin/entrypoint.sh\n")[1],
            InstructionKind::Copy(Transfer {
                sources: vec!["entrypoint.sh".into()],
                destination: "/usr/local/bin/entrypoint.sh".into(),
                owner: None,
                mode: Some(0o755),
            })
        );
    }

    #[test]
    fn a_chmod_that_is_not_an_octal_mode_is_refused_with_the_line_it_was_written_on() {
        let refusal = refusal("FROM alpine\nCOPY --chmod=rwx entrypoint.sh /srv/entrypoint.sh\n");
        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("COPY --chmod=rwx"), "{refusal}");
        assert!(refusal.contains("--chmod=755"), "{refusal}");
    }

    #[test]
    fn a_copy_link_is_refused_by_name_with_the_line_it_was_written_on() {
        let refusal = refusal("FROM alpine\nCOPY --link app /srv/app\n");
        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("COPY --link"), "{refusal}");
    }

    #[test]
    fn an_add_link_is_refused_by_name_with_the_line_it_was_written_on() {
        let refusal = refusal("FROM alpine\nADD --link extra /srv/extra\n");
        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("ADD --link"), "{refusal}");
    }

    #[test]
    fn a_run_network_is_refused_by_name_with_the_line_it_was_written_on() {
        let refusal = refusal("FROM alpine\nRUN --network=none npm ci\n");
        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("RUN --network=none"), "{refusal}");
        assert!(refusal.contains("the document's own network"), "{refusal}");
    }

    #[test]
    fn a_run_security_is_refused_by_name_with_the_line_it_was_written_on() {
        let refusal = refusal("FROM alpine\nRUN --security=insecure make\n");
        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("RUN --security=insecure"), "{refusal}");
    }

    #[test]
    fn an_add_from_a_url_is_refused_and_names_run_curl_as_the_alternative() {
        let refusal = refusal("FROM alpine\nADD https://example.test/x.bin /opt/x\n");
        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("is a URL"), "{refusal}");
        assert!(refusal.contains("egress"), "{refusal}");
    }

    #[test]
    fn an_add_that_would_unpack_an_archive_is_refused_for_every_suffix_docker_unpacks() {
        for suffix in ARCHIVE_SUFFIXES {
            let refusal = refusal(&format!("FROM alpine\nADD payload{suffix} /opt/payload\n"));
            assert!(refusal.contains("would be unpacked"), "{refusal}");
        }
    }

    #[test]
    fn an_instruction_outside_the_subset_is_refused_with_its_line_and_the_issue() {
        for (text, keyword) in [
            ("FROM alpine\nHEALTHCHECK CMD /bin/true\n", "HEALTHCHECK"),
            ("FROM alpine\nMAINTAINER someone\n", "MAINTAINER"),
            ("FROM alpine\nONBUILD RUN echo hi\n", "ONBUILD"),
            ("FROM alpine\nSTOPSIGNAL SIGTERM\n", "STOPSIGNAL"),
        ] {
            let refusal = refusal(text);
            assert!(refusal.contains("line 2"), "{refusal}");
            assert!(refusal.contains(keyword), "{refusal}");
            assert!(refusal.contains(SUBSET_ISSUE), "{refusal}");
        }
    }

    #[test]
    fn a_here_document_source_for_copy_is_refused_by_name() {
        let refusal = refusal("FROM alpine\nCOPY <<EOF /srv/x\nbody\nEOF\n");
        assert!(
            refusal.contains("here-document source for COPY"),
            "{refusal}"
        );
    }

    #[test]
    fn every_refusal_in_one_file_is_reported_at_once() {
        let refusals = parse("FROM alpine\nMAINTAINER a\nSTOPSIGNAL SIGTERM\n").unwrap_err();
        assert_eq!(refusals.len(), 2, "{refusals:?}");
    }

    #[test]
    fn a_file_the_parser_cannot_read_at_all_is_refused_with_the_line_it_stopped_on() {
        let refusals = parse("FROM alpine\nCOPY only-one-argument\n").unwrap_err();
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(refusals[0].starts_with("line 2"), "{refusals:?}");
        assert!(
            !refusals[0].contains(" at line "),
            "the position is stated once: {refusals:?}"
        );
    }
}
