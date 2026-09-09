//! The Containerfile a path-form `spec.image` names, parsed and held to the v1
//! instruction subset here rather than in a caller, because the CLI that refuses
//! an instruction and the executor that runs one must agree on which is which.

use std::collections::BTreeMap;

mod upstream;

/// A Containerfile lns can build: one stage's instructions, in the order they were written (§3.1.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Containerfile {
    pub instructions: Vec<Instruction>,
}

/// One instruction and the line it was written on, so a refusal and an audit entry can both name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    pub line: usize,
    pub kind: InstructionKind,
}

/// The v1 instruction subset — every spelling lns builds; anything else is refused at validate (§3.1.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstructionKind {
    From {
        image: String,
        flags: Vec<Flag>,
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
        here_docs: Vec<String>,
        flags: Vec<Flag>,
    },
    Copy(Transfer),
    Add(Transfer),
    Entrypoint(Command),
    Cmd(Command),
    Shell(Vec<String>),
    Expose(Vec<String>),
    Volume(Vec<String>),
}

/// A `RUN`, `CMD` or `ENTRYPOINT` argument in the two forms Docker defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Shell(String),
    Exec(Vec<String>),
}

/// What one `COPY` or `ADD` moves from the context into the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub sources: Vec<String>,
    pub destination: String,
    pub flags: Vec<Flag>,
}

/// An instruction option as written, such as `--chown=node:node`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flag {
    pub name: String,
    pub value: Option<String>,
}

/// The issue that decides what the subset grows to hold, named by every refusal that has no alternative today.
const SUBSET_ISSUE: &str = "https://github.com/lensapp/lens-sandbox/issues/393";

/// The suffixes Docker unpacks an `ADD` source by, which is a second behaviour behind one instruction.
const ARCHIVE_SUFFIXES: [&str; 9] = [
    ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz", ".tar.zst", ".tzst",
];

/// Parse a Containerfile and hold it to the v1 subset; each refusal names its instruction, its line and the alternative to use (§3.1.1).
pub fn parse(text: &str) -> Result<Containerfile, Vec<String>> {
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
            Ok(kind) => instructions.push(Instruction { line, kind }),
            Err(refusal) => refusals.push(refusal),
        }
    }
    if refusals.is_empty() {
        Ok(Containerfile { instructions })
    } else {
        Err(refusals)
    }
}

/// The parser reports the position twice — in the message and in `line()`/`column()` — and a refusal states it once.
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
) -> Result<InstructionKind, String> {
    use parse_dockerfile::Instruction as I;
    match instruction {
        I::From(from) => accept_from(from, line, seen_from),
        I::Arg(arg) => Ok(accept_arg(&arg.arguments.value)),
        I::Env(env) => Ok(InstructionKind::Env(key_values(&env.arguments.value))),
        I::Label(label) => Ok(InstructionKind::Label(key_values(&label.arguments.value))),
        I::User(user) => Ok(InstructionKind::User(user.arguments.value.to_string())),
        I::Workdir(workdir) => Ok(InstructionKind::Workdir(
            workdir.arguments.value.to_string(),
        )),
        I::Run(run) => accept_run(run, line),
        I::Copy(copy) => accept_copy(copy, line),
        I::Add(add) => accept_add(add, line),
        I::Entrypoint(entrypoint) => Ok(InstructionKind::Entrypoint(upstream::command(
            &entrypoint.arguments,
        ))),
        I::Cmd(cmd) => Ok(InstructionKind::Cmd(upstream::command(&cmd.arguments))),
        I::Shell(shell) => Ok(InstructionKind::Shell(strings(&shell.arguments))),
        I::Expose(expose) => Ok(InstructionKind::Expose(strings(&expose.arguments))),
        I::Volume(volume) => Ok(InstructionKind::Volume(volume_targets(&volume.arguments))),
        other => Err(unsupported(other, line)),
    }
}

fn unsupported(instruction: &parse_dockerfile::Instruction<'_>, line: usize) -> String {
    match upstream::unsupported_keyword(instruction) {
        "MAINTAINER" => format!(
            "line {line}: MAINTAINER is not supported; write LABEL org.opencontainers.image.authors instead"
        ),
        keyword => no_alternative(keyword, line),
    }
}

fn no_alternative(instruction: &str, line: usize) -> String {
    format!(
        "line {line}: {instruction} is not supported and has no alternative in v1; ask for it on {SUBSET_ISSUE}"
    )
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
        flags: flags(&from.options),
    })
}

fn accept_arg(arguments: &str) -> InstructionKind {
    match arguments.split_once('=') {
        Some((name, default)) => InstructionKind::Arg {
            name: name.trim().to_string(),
            default: Some(unquote(default.trim()).to_string()),
        },
        None => InstructionKind::Arg {
            name: arguments.trim().to_string(),
            default: None,
        },
    }
}

fn accept_run(
    run: &parse_dockerfile::RunInstruction<'_>,
    line: usize,
) -> Result<InstructionKind, String> {
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
            .map(|doc| doc.value.to_string())
            .collect(),
        flags: flags(&run.options),
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
    let moved = transfer("ADD", &add.src, &add.dest, &add.options, line)?;
    for source in &moved.sources {
        if is_remote(source) {
            return Err(format!(
                "line {line}: ADD source {source:?} is a URL, which is not supported; RUN curl to fetch it, so the fetch is decided by the document's egress"
            ));
        }
        if ARCHIVE_SUFFIXES
            .iter()
            .any(|suffix| source.ends_with(suffix))
        {
            return Err(format!(
                "line {line}: ADD source {source:?} is an archive ADD would unpack, which is not supported; COPY it and RUN tar to unpack it"
            ));
        }
    }
    Ok(InstructionKind::Add(moved))
}

fn is_remote(source: &str) -> bool {
    source.contains("://") || source.starts_with("git@")
}

fn transfer(
    instruction: &str,
    src: &[parse_dockerfile::Source<'_>],
    dest: &parse_dockerfile::UnescapedString<'_>,
    options: &[parse_dockerfile::Flag<'_>],
    line: usize,
) -> Result<Transfer, String> {
    let mut sources = Vec::new();
    for source in src {
        match source {
            parse_dockerfile::Source::Path(path) => {
                let path = path.value.to_string();
                leaves_the_context(&path, instruction, line)?;
                sources.push(path);
            }
            _ => {
                return Err(format!(
                    "line {line}: {instruction} from a here-document is not supported; write the file into the context and {instruction} it"
                ));
            }
        }
    }
    Ok(Transfer {
        sources,
        destination: dest.value.to_string(),
        flags: flags(options),
    })
}

/// A source is read from the context the artifact ships, so one that reaches outside it names something no consumer receives.
fn leaves_the_context(path: &str, instruction: &str, line: usize) -> Result<(), String> {
    let escapes = path.split('/').any(|segment| segment == "..");
    if escapes || path.starts_with('/') {
        return Err(format!(
            "line {line}: {instruction} source {path:?} leaves the build context; the artifact ships the context, so name a path inside it"
        ));
    }
    Ok(())
}

fn flags(options: &[parse_dockerfile::Flag<'_>]) -> Vec<Flag> {
    options
        .iter()
        .map(|flag| Flag {
            name: flag.name.value.to_string(),
            value: flag.value.as_ref().map(|v| v.value.to_string()),
        })
        .collect()
}

fn strings(arguments: &[parse_dockerfile::UnescapedString<'_>]) -> Vec<String> {
    arguments
        .iter()
        .map(|word| word.value.to_string())
        .collect()
}

fn volume_targets(arguments: &parse_dockerfile::JsonOrStringArray<'_, 1>) -> Vec<String> {
    match arguments {
        parse_dockerfile::JsonOrStringArray::Json(json) => strings(&json.value),
        parse_dockerfile::JsonOrStringArray::String(words) => strings(words),
    }
}

/// `ENV` and `LABEL` take pairs in one line, quoted where a value holds a space, plus the legacy `ENV KEY value` spelling.
fn key_values(arguments: &str) -> Vec<(String, String)> {
    let words = split_words(arguments);
    if words.first().is_some_and(|word| !word.contains('=')) {
        let mut words = words.into_iter();
        let key = words.next().unwrap_or_default();
        let value = words.collect::<Vec<_>>().join(" ");
        return vec![(key, unquote(&value).to_string())];
    }
    words
        .iter()
        .filter_map(|word| word.split_once('='))
        .map(|(key, value)| (key.to_string(), unquote(value).to_string()))
        .collect()
}

/// Splitting on whitespace outside quotes, because `ENV A="one two" B=3` is one instruction with two pairs.
fn split_words(arguments: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    for c in arguments.chars() {
        match (quote, c) {
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

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|v| v.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// The `ARG` defaults a build starts with, which the executor substitutes and the cache key holds.
pub fn arg_defaults(containerfile: &Containerfile) -> BTreeMap<String, String> {
    containerfile
        .instructions
        .iter()
        .filter_map(|instruction| match &instruction.kind {
            InstructionKind::Arg {
                name,
                default: Some(default),
            } => Some((name.clone(), default.clone())),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(text: &str) -> String {
        parse(text)
            .map(|_| String::new())
            .expect_err("the subset gate refuses this Containerfile")
            .join("\n")
    }

    fn built(text: &str) -> Containerfile {
        parse(text).unwrap_or_else(|refusals| panic!("expected the subset to accept: {refusals:?}"))
    }

    #[test]
    fn a_second_stage_is_refused_with_its_line_and_the_one_stage_alternative() {
        let refusal = refusal("FROM alpine\nRUN one\nFROM alpine\nRUN two\n");
        assert!(refusal.contains("line 3"), "got: {refusal}");
        assert!(refusal.contains("FROM"), "got: {refusal}");
        assert!(
            refusal.contains("name it in FROM"),
            "the author needs the alternative: {refusal}"
        );
    }

    #[test]
    fn copy_from_another_stage_is_refused_with_its_line_and_the_one_stage_alternative() {
        let refusal = refusal("FROM alpine\nCOPY --from=build /a /b\n");
        assert!(
            refusal.contains("line 2") && refusal.contains("COPY --from"),
            "got: {refusal}"
        );
        assert!(refusal.contains("name it in FROM"), "got: {refusal}");
    }

    #[test]
    fn a_run_mount_is_refused_with_its_line_and_the_copy_alternative() {
        let refusal = refusal("FROM alpine\nRUN --mount=type=cache,target=/x npm i\n");
        assert!(
            refusal.contains("line 2") && refusal.contains("RUN --mount"),
            "got: {refusal}"
        );
        assert!(refusal.contains("COPY"), "got: {refusal}");
    }

    #[test]
    fn an_add_from_a_url_is_refused_with_its_line_and_the_run_curl_alternative() {
        let refusal = refusal("FROM alpine\nADD https://example.com/x /x\n");
        assert!(
            refusal.contains("line 2") && refusal.contains("ADD"),
            "got: {refusal}"
        );
        assert!(refusal.contains("RUN curl"), "got: {refusal}");
    }

    #[test]
    fn an_add_that_would_unpack_an_archive_is_refused_with_its_line_and_the_tar_alternative() {
        let refusal = refusal("FROM alpine\nRUN true\nADD ./bundle.tar.gz /opt\n");
        assert!(
            refusal.contains("line 3") && refusal.contains("ADD"),
            "got: {refusal}"
        );
        assert!(refusal.contains("RUN tar"), "got: {refusal}");
    }

    #[test]
    fn onbuild_healthcheck_and_stopsignal_are_refused_with_their_line_and_the_issue_that_decides() {
        for (text, instruction, line) in [
            ("FROM alpine\nONBUILD RUN x\n", "ONBUILD", "line 2"),
            (
                "FROM alpine\nRUN true\nHEALTHCHECK CMD y\n",
                "HEALTHCHECK",
                "line 3",
            ),
            ("FROM alpine\nSTOPSIGNAL SIGTERM\n", "STOPSIGNAL", "line 2"),
        ] {
            let refusal = refusal(text);
            assert!(
                refusal.contains(instruction) && refusal.contains(line),
                "got: {refusal}"
            );
            assert!(
                refusal.contains("no alternative in v1") && refusal.contains("issues/393"),
                "a refusal with no alternative names where the subset grows: {refusal}"
            );
        }
    }

    #[test]
    fn a_copy_source_that_leaves_the_context_is_refused_naming_the_source_it_read() {
        for source in ["../secret", "./a/../../secret", "/etc/shadow"] {
            let refusal = refusal(&format!("FROM alpine\nCOPY {source} /app\n"));
            assert!(
                refusal.contains("source") && refusal.contains(source),
                "the author has to see which source is refused: {refusal}"
            );
            assert!(refusal.contains("line 2"), "got: {refusal}");
        }
    }

    #[test]
    fn an_add_source_that_leaves_the_context_is_refused_the_same_way() {
        let refusal = refusal("FROM alpine\nADD ../secret /app\n");
        assert!(
            refusal.contains("ADD") && refusal.contains("../secret"),
            "got: {refusal}"
        );
    }

    #[test]
    fn a_parse_error_is_refused_naming_the_line_it_sits_on() {
        let refusal = refusal("FROM alpine\nRUN ok\nBOGUS wat\n");
        assert!(refusal.contains("line 3"), "got: {refusal}");
        assert!(refusal.contains("BOGUS"), "got: {refusal}");
    }

    #[test]
    fn a_containerfile_with_no_from_is_refused_because_a_build_starts_from_an_image() {
        let refusal = refusal("RUN echo hi\n");
        assert!(refusal.contains("FROM"), "got: {refusal}");
    }

    #[test]
    fn every_refusal_of_one_file_is_reported_at_once_so_the_author_fixes_them_together() {
        let refusals = parse("FROM alpine\nHEALTHCHECK CMD y\nSTOPSIGNAL SIGTERM\n")
            .expect_err("both are outside the subset");
        assert_eq!(refusals.len(), 2, "got: {refusals:?}");
    }

    #[test]
    fn a_run_written_as_a_here_document_is_accepted_with_its_body() {
        let built = built("FROM alpine\nRUN <<EOF\necho one\necho two\nEOF\n");
        assert_eq!(
            built.instructions[1].kind,
            InstructionKind::Run {
                command: Command::Shell(String::new()),
                here_docs: vec!["echo one\necho two\n".to_string()],
                flags: Vec::new(),
            }
        );
    }

    #[test]
    fn maintainer_is_refused_with_the_label_that_replaced_it() {
        let refusal = refusal("FROM alpine\nMAINTAINER bob\n");
        assert!(
            refusal.contains("line 2") && refusal.contains("MAINTAINER"),
            "got: {refusal}"
        );
        assert!(
            refusal.contains("LABEL org.opencontainers.image.authors"),
            "got: {refusal}"
        );
    }

    #[test]
    fn a_copy_written_as_a_here_document_is_refused_because_a_build_copies_from_the_context() {
        let refusal = refusal("FROM alpine\nCOPY <<EOF /srv/hello\nhello\nEOF\n");
        assert!(
            refusal.contains("here-document") && refusal.contains("COPY"),
            "got: {refusal}"
        );
    }

    #[test]
    fn a_volume_written_as_a_json_array_names_the_same_target_a_bare_word_does() {
        let json = built("FROM alpine\nVOLUME [\"/data\", \"/cache\"]\n");
        assert_eq!(
            json.instructions[1].kind,
            InstructionKind::Volume(vec!["/data".to_string(), "/cache".to_string()])
        );
    }

    #[test]
    fn a_single_quoted_value_reaches_the_build_without_its_quotes() {
        let built = built("FROM alpine\nARG GREETING='good morning'\n");
        assert_eq!(
            arg_defaults(&built),
            [("GREETING".to_string(), "good morning".to_string())]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn a_parse_error_that_states_no_position_is_still_reported_whole() {
        assert_eq!(
            first_sentence("something went wrong"),
            "something went wrong"
        );
    }

    #[test]
    fn run_is_accepted_in_shell_and_exec_form() {
        let built = built("FROM alpine\nRUN echo hi\nRUN [\"/bin/sh\", \"-c\", \"echo hi\"]\n");
        let kinds: Vec<&InstructionKind> = built.instructions.iter().map(|i| &i.kind).collect();
        assert!(
            matches!(kinds[1], InstructionKind::Run { command: Command::Shell(c), .. } if c == "echo hi"),
            "got: {kinds:?}"
        );
        assert!(
            matches!(kinds[2], InstructionKind::Run { command: Command::Exec(words), .. } if words.len() == 3),
            "got: {kinds:?}"
        );
    }

    #[test]
    fn an_arg_default_is_recorded_so_the_build_and_the_cache_key_can_read_it() {
        let built = built("FROM alpine\nARG CLAUDE_CODE_VERSION=2.1.263\nARG NO_DEFAULT\n");
        assert_eq!(
            arg_defaults(&built),
            [("CLAUDE_CODE_VERSION".to_string(), "2.1.263".to_string())]
                .into_iter()
                .collect(),
            "an ARG without a default contributes none"
        );
        assert_eq!(
            built.instructions[2].kind,
            InstructionKind::Arg {
                name: "NO_DEFAULT".to_string(),
                default: None
            }
        );
    }

    #[test]
    fn every_instruction_of_the_v1_subset_is_accepted_with_the_line_it_was_written_on() {
        let built = built(concat!(
            "FROM docker.io/library/node:24-bookworm@sha256:abc\n",
            "ARG V=1\n",
            "ENV A=b C=\"d e\"\n",
            "USER node\n",
            "WORKDIR /srv\n",
            "RUN npm ci\n",
            "COPY --chown=node:node ./app /srv/app\n",
            "ADD ./extra /srv/extra\n",
            "LABEL org.opencontainers.image.title=hermes\n",
            "ENTRYPOINT [\"/bin/sh\"]\n",
            "CMD [\"-c\", \"node .\"]\n",
            "SHELL [\"/bin/bash\", \"-c\"]\n",
            "EXPOSE 8080\n",
            "VOLUME /data\n",
        ));
        assert_eq!(built.instructions.len(), 14);
        let lines: Vec<usize> = built.instructions.iter().map(|i| i.line).collect();
        assert_eq!(lines, (1..=14).collect::<Vec<usize>>());
        assert_eq!(
            built.instructions[2].kind,
            InstructionKind::Env(vec![
                ("A".to_string(), "b".to_string()),
                ("C".to_string(), "d e".to_string()),
            ]),
            "a quoted value holding a space is one pair"
        );
        assert_eq!(
            built.instructions[6].kind,
            InstructionKind::Copy(Transfer {
                sources: vec!["./app".to_string()],
                destination: "/srv/app".to_string(),
                flags: vec![Flag {
                    name: "chown".to_string(),
                    value: Some("node:node".to_string())
                }],
            })
        );
        assert_eq!(
            built.instructions[12].kind,
            InstructionKind::Expose(vec!["8080".to_string()])
        );
        assert_eq!(
            built.instructions[13].kind,
            InstructionKind::Volume(vec!["/data".to_string()])
        );
    }

    #[test]
    fn a_legacy_env_pair_written_without_an_equals_sign_is_one_pair() {
        let built = built("FROM alpine\nENV PATH /usr/local/bin:/usr/bin\n");
        assert_eq!(
            built.instructions[1].kind,
            InstructionKind::Env(vec![(
                "PATH".to_string(),
                "/usr/local/bin:/usr/bin".to_string()
            )])
        );
    }

    #[test]
    fn a_line_continuation_keeps_the_line_of_the_instruction_that_follows_it() {
        let built = built("FROM alpine\nRUN echo a \\\n && echo b\nRUN echo c\n");
        assert_eq!(
            built.instructions[2].line, 4,
            "the second RUN is on line 4: {built:?}"
        );
    }

    #[test]
    fn a_comment_and_a_blank_line_shift_the_line_a_refusal_names() {
        let refusal = refusal("# base\n\nFROM alpine\n\n# the check\nHEALTHCHECK CMD y\n");
        assert!(refusal.contains("line 6"), "got: {refusal}");
    }
}
