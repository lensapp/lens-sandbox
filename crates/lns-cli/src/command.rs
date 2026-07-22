use std::collections::HashSet;
use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::{Context, Result};
use clap::{ArgMatches, CommandFactory};

use crate::cli::Cli;

pub type RunFuture<'a> = Pin<Box<dyn Future<Output = Result<i32>> + 'a>>;

/// Real-I/O primitives a command's wiring draws from; faked in unit tests of the measured commands.
pub struct RunCtx<'a> {
    pub debug: bool,
    pub cwd: Option<PathBuf>,
    pub input: &'a mut dyn std::io::BufRead,
    pub out: &'a mut dyn std::io::Write,
}

impl RunCtx<'_> {
    /// The directory a command edits relative to: the injected one in tests, else the process cwd.
    pub fn cwd(&self) -> Result<PathBuf> {
        match &self.cwd {
            Some(cwd) => Ok(cwd.clone()),
            None => std::env::current_dir().context("reading current directory"),
        }
    }
}

/// One `lns <name>` subcommand, declared entirely in its own module and registered in `registry()`.
pub struct CommandSpec {
    pub name: &'static str,
    pub augment: fn(clap::Command) -> clap::Command,
    pub run: for<'a> fn(&'a ArgMatches, RunCtx<'a>) -> RunFuture<'a>,
    pub announces_update_check: bool,
    /// True for commands that drive the tty over async tokio stdin/stdout, so the dispatcher must not hold the blocking std stdin/stdout locks.
    pub owns_terminal: bool,
}

/// Add a derive-`Args` subcommand named `name` to `app`.
pub fn subcommand<A: clap::Args>(name: &'static str) -> clap::Command {
    A::augment_args(clap::Command::new(name))
}

pub fn registry() -> Vec<CommandSpec> {
    vec![
        crate::run::RUN_SPEC,
        crate::run::EXEC_SPEC,
        crate::volume::SPEC,
        crate::sandbox::SPEC,
        crate::sandbox::INIT_SPEC,
        crate::sandbox::PS_SPEC,
        crate::sandbox::KILL_SPEC,
        crate::sandbox::PUSH_SPEC,
        crate::sandbox::PULL_SPEC,
        crate::sandbox::TAG_SPEC,
        crate::sandbox::STOP_SPEC,
        crate::sandbox::RM_SPEC,
        crate::sandbox::INSPECT_SPEC,
        crate::sandbox::LOGS_SPEC,
        crate::sandbox::ATTACH_SPEC,
        crate::audit::SPEC,
        crate::service::SPEC,
        crate::update::SPEC,
        crate::uninstall::SPEC,
        crate::policy::SPEC,
        crate::connector::SPEC,
        crate::config::SPEC,
        crate::login::LOGIN_SPEC,
        crate::login::LOGOUT_SPEC,
    ]
}

pub fn build_cli() -> clap::Command {
    let mut app = Cli::command()
        .subcommand_required(true)
        .arg_required_else_help(true);
    for spec in registry() {
        app = (spec.augment)(app);
    }
    app
}

pub fn try_get_matches_from<I, T>(argv: I) -> clap::error::Result<ArgMatches>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let app = build_cli();
    let normalized = normalize_argv(&app, argv);
    app.try_get_matches_from(normalized)
}

pub fn spec_for(name: &str) -> Option<CommandSpec> {
    registry().into_iter().find(|spec| spec.name == name)
}

/// Parse `argv` (program name first) and decode the matched subcommand into its typed args.
pub fn parse_args<A, I, T>(argv: I) -> clap::error::Result<A>
where
    A: clap::FromArgMatches,
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let matches = try_get_matches_from(argv)?;
    let (_, sub) = matches
        .subcommand()
        .expect("build_cli requires a subcommand, so a successful parse always has one");
    A::from_arg_matches(sub)
}

fn normalize_argv<I, T>(app: &clap::Command, argv: I) -> Vec<OsString>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let raw: Vec<OsString> = argv.into_iter().map(Into::into).collect();
    match workload_position(app, &raw) {
        Some((idx, cmd)) => normalize_workload_argv(app, cmd, &raw, idx),
        None => raw,
    }
}

fn workload_position<'a>(
    app: &'a clap::Command,
    raw: &[OsString],
) -> Option<(usize, &'a clap::Command)> {
    let (idx, name) = first_subcommand(app, raw)?;
    if name != "sandbox" {
        return app.find_subcommand(name).map(|cmd| (idx, cmd));
    }
    let sandbox = app.find_subcommand("sandbox")?;
    let (nested_idx, nested) = nested_workload(app, raw, idx + 1)?;
    sandbox.find_subcommand(nested).map(|cmd| (nested_idx, cmd))
}

fn nested_workload(
    app: &clap::Command,
    raw: &[OsString],
    from: usize,
) -> Option<(usize, &'static str)> {
    let global = value_consuming_options(app);
    let mut idx = from;
    while idx < raw.len() {
        let Some(arg) = raw[idx].to_str() else {
            idx += 1;
            continue;
        };
        if arg == "--" {
            return None;
        }
        if arg.starts_with('-') && global.contains(arg) && !arg.contains('=') {
            idx += 2;
        } else if arg.starts_with('-') {
            idx += 1;
        } else {
            return workload_subcommand(arg).map(|name| (idx, name));
        }
    }
    None
}

fn first_subcommand(app: &clap::Command, raw: &[OsString]) -> Option<(usize, &'static str)> {
    let global = value_consuming_options(app);
    let mut idx = 1;
    while idx < raw.len() {
        let Some(arg) = raw[idx].to_str() else {
            idx += 1;
            continue;
        };
        if arg == "--" {
            return None;
        }
        if arg.starts_with('-') && global.contains(arg) && !arg.contains('=') {
            idx += 2;
        } else if arg.starts_with('-') {
            idx += 1;
        } else {
            return normalized_subcommand(arg).map(|name| (idx, name));
        }
    }
    None
}

fn normalized_subcommand(arg: &str) -> Option<&'static str> {
    match arg {
        "sandbox" => Some("sandbox"),
        _ => workload_subcommand(arg),
    }
}

fn workload_subcommand(arg: &str) -> Option<&'static str> {
    match arg {
        "run" => Some("run"),
        "exec" => Some("exec"),
        _ => None,
    }
}

fn normalize_workload_argv(
    app: &clap::Command,
    cmd: &clap::Command,
    raw: &[OsString],
    idx: usize,
) -> Vec<OsString> {
    let mut consumes = value_consuming_options(app);
    consumes.extend(value_consuming_options(cmd));
    let mut out = raw[..=idx].to_vec();
    let rest = &raw[idx + 1..];
    let mut positional_seen = false;
    let mut i = 0;
    while i < rest.len() {
        let arg = &rest[i];
        if arg == "--" {
            out.extend(rest[i..].iter().cloned());
            return out;
        }
        if positional_seen {
            out.push(OsString::from("--"));
            out.extend(rest[i..].iter().cloned());
            return out;
        }
        if let Some(expanded) = expand_it(arg) {
            out.extend(expanded);
            i += 1;
            continue;
        }
        out.push(arg.clone());
        let s = arg.to_string_lossy();
        if consumes.contains(s.as_ref())
            && !s.contains('=')
            && let Some(value) = rest.get(i + 1)
        {
            out.push(value.clone());
            i += 2;
            continue;
        }
        if !s.starts_with('-') {
            positional_seen = true;
        }
        i += 1;
    }
    out
}

fn expand_it(arg: &OsString) -> Option<Vec<OsString>> {
    match arg.to_str() {
        Some("-it") => Some(vec![OsString::from("-i"), OsString::from("-t")]),
        Some("-ti") => Some(vec![OsString::from("-t"), OsString::from("-i")]),
        _ => None,
    }
}

fn value_consuming_options(cmd: &clap::Command) -> HashSet<String> {
    let mut set = HashSet::new();
    for arg in cmd.get_arguments() {
        if arg.is_positional()
            || arg.is_require_equals_set()
            || !matches!(
                arg.get_action(),
                clap::ArgAction::Set | clap::ArgAction::Append
            )
        {
            continue;
        }
        for long in arg.get_long_and_visible_aliases().into_iter().flatten() {
            set.insert(format!("--{long}"));
        }
        for short in arg.get_short_and_visible_aliases().into_iter().flatten() {
            set.insert(format!("-{short}"));
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn cwd_prefers_an_injected_directory() {
        let mut input: &[u8] = b"";
        let mut out: Vec<u8> = Vec::new();
        let ctx = RunCtx {
            debug: false,
            cwd: Some(PathBuf::from("/injected")),
            input: &mut input,
            out: &mut out,
        };
        assert_eq!(ctx.cwd().unwrap(), PathBuf::from("/injected"));
    }

    #[test]
    fn cwd_falls_back_to_the_process_directory_when_unset() {
        let mut input: &[u8] = b"";
        let mut out: Vec<u8> = Vec::new();
        let ctx = RunCtx {
            debug: false,
            cwd: None,
            input: &mut input,
            out: &mut out,
        };
        assert_eq!(ctx.cwd().unwrap(), std::env::current_dir().unwrap());
    }

    #[test]
    fn every_spec_name_is_unique() {
        let mut seen = BTreeSet::new();
        for spec in registry() {
            assert!(
                seen.insert(spec.name),
                "duplicate command name {}",
                spec.name
            );
        }
    }

    #[test]
    fn build_cli_exposes_one_subcommand_per_registered_spec() {
        let app = build_cli();
        let names: BTreeSet<&str> = app.get_subcommands().map(|c| c.get_name()).collect();
        for spec in registry() {
            assert!(
                names.contains(spec.name),
                "build_cli is missing subcommand {}",
                spec.name
            );
        }
    }

    #[test]
    fn only_update_and_uninstall_opt_out_of_the_update_check_announce() {
        for spec in registry() {
            let opts_in = spec.announces_update_check;
            if matches!(spec.name, "update" | "uninstall") {
                assert!(
                    !opts_in,
                    "{} must skip the announce so it does not nag while changing the install",
                    spec.name
                );
            } else {
                assert!(opts_in, "{} should announce available updates", spec.name);
            }
        }
    }

    #[test]
    fn spec_for_finds_a_registered_command_and_rejects_an_unknown_one() {
        assert!(spec_for("volume").is_some());
        assert!(spec_for("does-not-exist").is_none());
    }

    #[test]
    fn every_spec_that_can_stream_over_tokio_stdio_owns_the_terminal() {
        for spec in registry() {
            if matches!(spec.name, "run" | "exec" | "sandbox" | "logs" | "attach") {
                assert!(
                    spec.owns_terminal,
                    "{} can drive the tty over tokio stdin/stdout; the dispatcher must not hold the std locks for it",
                    spec.name
                );
            } else {
                assert!(
                    !spec.owns_terminal,
                    "{} runs synchronously and relies on the dispatcher holding the std stdin/stdout locks",
                    spec.name
                );
            }
        }
    }

    #[test]
    fn run_command_expands_it_and_inserts_the_command_separator() {
        let args: crate::cli::RunArgs =
            parse_args(["lns", "run", "--rm", "-it", "alpine", "sh"]).unwrap();
        assert!(args.auto_remove);
        assert!(args.interactive);
        assert!(args.tty);
        assert_eq!(args.image.as_deref(), Some("alpine"));
        assert_eq!(args.cmd, ["sh"]);
    }

    #[test]
    fn run_command_still_normalizes_after_global_options() {
        let args: crate::cli::RunArgs =
            parse_args(["lns", "--log-level", "debug", "run", "-it", "alpine", "sh"]).unwrap();
        assert!(args.interactive);
        assert!(args.tty);
        assert_eq!(args.image.as_deref(), Some("alpine"));
        assert_eq!(args.cmd, ["sh"]);
    }

    #[test]
    fn normalization_ignores_run_and_exec_after_another_subcommand() {
        let raw = ["lns", "sandbox", "logs", "exec", "-it"];
        let normalized = normalize_argv(&build_cli(), raw);
        assert_eq!(
            normalized,
            raw.into_iter().map(OsString::from).collect::<Vec<_>>()
        );
    }

    fn sandbox_run_args(argv: &[&str]) -> crate::cli::RunArgs {
        use crate::sandbox::SandboxCommand;
        let args: crate::sandbox::SandboxArgs = parse_args(argv.iter().copied()).unwrap();
        let SandboxCommand::Run(run) = args.command else {
            panic!("expected the run variant")
        };
        *run
    }

    #[test]
    #[should_panic(expected = "expected the run variant")]
    fn sandbox_run_args_guards_against_decoding_another_verb() {
        sandbox_run_args(&["lns", "sandbox", "ps"]);
    }

    #[test]
    fn sandbox_run_expands_it_and_inserts_the_command_separator() {
        let args = sandbox_run_args(&["lns", "sandbox", "run", "--rm", "-it", "alpine", "sh"]);
        assert!(args.auto_remove);
        assert!(args.interactive);
        assert!(args.tty);
        assert_eq!(args.image.as_deref(), Some("alpine"));
        assert_eq!(args.cmd, ["sh"]);
    }

    #[test]
    fn sandbox_run_treats_a_value_options_argument_as_its_value_not_the_image() {
        let args = sandbox_run_args(&["lns", "sandbox", "run", "--workdir", "/w", "alpine", "sh"]);
        assert_eq!(args.workdir.as_deref(), Some("/w"));
        assert_eq!(args.image.as_deref(), Some("alpine"));
        assert_eq!(args.cmd, ["sh"]);
    }

    #[test]
    fn sandbox_run_uses_short_h_for_hostname_while_long_help_still_works() {
        let args = sandbox_run_args(&["lns", "sandbox", "run", "-h", "demo", "alpine"]);
        assert_eq!(args.hostname.as_deref(), Some("demo"));
        let err = try_get_matches_from(["lns", "sandbox", "run", "--help"])
            .expect_err("--help exits through clap");
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    fn sandbox_exec_args(argv: &[&str]) -> crate::cli::ExecArgs {
        use crate::sandbox::SandboxCommand;
        let args: crate::sandbox::SandboxArgs = parse_args(argv.iter().copied()).unwrap();
        let SandboxCommand::Exec(exec) = args.command else {
            panic!("expected the exec variant")
        };
        exec
    }

    #[test]
    #[should_panic(expected = "expected the exec variant")]
    fn sandbox_exec_args_guards_against_decoding_another_verb() {
        sandbox_exec_args(&["lns", "sandbox", "ps"]);
    }

    #[test]
    fn sandbox_exec_expands_a_leading_it_cluster_into_session_flags() {
        let exec = sandbox_exec_args(&["lns", "sandbox", "exec", "-it", "demo", "sh"]);
        assert!(exec.interactive);
        assert!(exec.tty);
        assert_eq!(exec.run, "demo");
        assert_eq!(exec.cmd, ["sh"]);
    }

    #[test]
    fn nested_run_normalizes_after_a_value_consuming_global_between_sandbox_and_run() {
        let run = sandbox_run_args(&[
            "lns",
            "sandbox",
            "--log-level",
            "debug",
            "run",
            "-it",
            "alpine",
            "sh",
        ]);
        assert!(run.interactive, "the -it cluster must still expand");
        assert!(run.tty);
        assert_eq!(run.image.as_deref(), Some("alpine"));
        assert_eq!(run.cmd, ["sh"]);
    }

    #[test]
    fn nested_run_normalizes_past_an_equals_form_global_between_sandbox_and_run() {
        let run = sandbox_run_args(&[
            "lns",
            "sandbox",
            "--log-level=debug",
            "run",
            "-it",
            "alpine",
            "sh",
        ]);
        assert!(
            run.interactive,
            "an =-form global carries its own value, not the next token"
        );
        assert_eq!(run.image.as_deref(), Some("alpine"));
        assert_eq!(run.cmd, ["sh"]);
    }

    #[test]
    fn normalization_stops_at_a_sandbox_level_separator() {
        let raw = ["lns", "sandbox", "--", "run", "-it"];
        let normalized = normalize_argv(&build_cli(), raw);
        assert_eq!(
            normalized,
            raw.into_iter().map(OsString::from).collect::<Vec<_>>()
        );
    }

    #[cfg(unix)]
    #[test]
    fn normalization_skips_non_utf8_arguments_between_sandbox_and_run() {
        use std::os::unix::ffi::OsStringExt;
        let raw = vec![
            OsString::from("lns"),
            OsString::from("sandbox"),
            OsString::from_vec(vec![0xff]),
            OsString::from("run"),
            OsString::from("alpine"),
            OsString::from("sh"),
        ];
        let normalized = normalize_argv(&build_cli(), raw);
        let expected = vec![
            OsString::from("lns"),
            OsString::from("sandbox"),
            OsString::from_vec(vec![0xff]),
            OsString::from("run"),
            OsString::from("alpine"),
            OsString::from("--"),
            OsString::from("sh"),
        ];
        assert_eq!(normalized, expected);
    }

    #[test]
    fn normalization_ignores_a_sandbox_namespace_with_no_nested_workload() {
        let raw = ["lns", "sandbox", "--help"];
        let normalized = normalize_argv(&build_cli(), raw);
        assert_eq!(
            normalized,
            raw.into_iter().map(OsString::from).collect::<Vec<_>>()
        );
    }

    #[test]
    fn normalization_stops_at_a_top_level_separator() {
        let raw = ["lns", "--", "run", "-it"];
        let normalized = normalize_argv(&build_cli(), raw);
        assert_eq!(
            normalized,
            raw.into_iter().map(OsString::from).collect::<Vec<_>>()
        );
    }

    #[cfg(unix)]
    #[test]
    fn normalization_skips_non_utf8_arguments_before_the_subcommand() {
        use std::os::unix::ffi::OsStringExt;
        let raw = vec![
            OsString::from("lns"),
            OsString::from_vec(vec![0xff]),
            OsString::from("run"),
            OsString::from("-it"),
            OsString::from("alpine"),
            OsString::from("sh"),
        ];
        let normalized = normalize_argv(&build_cli(), raw);
        let expected = vec![
            OsString::from("lns"),
            OsString::from_vec(vec![0xff]),
            OsString::from("run"),
            OsString::from("-i"),
            OsString::from("-t"),
            OsString::from("alpine"),
            OsString::from("--"),
            OsString::from("sh"),
        ];
        assert_eq!(normalized, expected);
    }

    #[test]
    fn run_command_preserves_command_flags_after_the_image() {
        let args: crate::cli::RunArgs =
            parse_args(["lns", "run", "alpine", "sh", "-c", "echo hi"]).unwrap();
        assert_eq!(args.image.as_deref(), Some("alpine"));
        assert_eq!(args.cmd, ["sh", "-c", "echo hi"]);
    }

    #[test]
    fn explicit_separator_still_supports_imageless_runs() {
        let args: crate::cli::RunArgs = parse_args(["lns", "run", "--", "echo", "hi"]).unwrap();
        assert!(args.image.is_none());
        assert_eq!(args.cmd, ["echo", "hi"]);
    }

    #[test]
    fn run_preserves_an_it_token_inside_the_workload_command() {
        let args: crate::cli::RunArgs = parse_args(["lns", "run", "alpine", "sh", "-it"]).unwrap();
        assert_eq!(args.image.as_deref(), Some("alpine"));
        assert_eq!(args.cmd, ["sh", "-it"]);
    }

    #[test]
    fn exec_preserves_an_it_token_inside_the_workload_command() {
        let args: crate::cli::ExecArgs =
            parse_args(["lns", "exec", "demo", "--", "tool", "-ti"]).unwrap();
        assert_eq!(args.run, "demo");
        assert_eq!(args.cmd, ["tool", "-ti"]);
    }

    #[test]
    fn exec_expands_a_leading_it_cluster_into_session_flags() {
        let args: crate::cli::ExecArgs =
            parse_args(["lns", "exec", "-it", "demo", "--", "sh"]).unwrap();
        assert!(args.interactive);
        assert!(args.tty);
        assert_eq!(args.run, "demo");
        assert_eq!(args.cmd, ["sh"]);
    }

    #[test]
    fn exec_without_a_separator_normalizes_to_the_run_name_only() {
        let args: crate::cli::ExecArgs = parse_args(["lns", "exec", "demo"]).unwrap();
        assert_eq!(args.run, "demo");
        assert!(args.cmd.is_empty());
    }

    #[test]
    fn exec_inserts_the_command_separator_after_the_run_name() {
        let args: crate::cli::ExecArgs =
            parse_args(["lns", "exec", "demo", "sh", "-c", "echo hi"]).unwrap();
        assert_eq!(args.run, "demo");
        assert_eq!(args.cmd, ["sh", "-c", "echo hi"]);
    }

    #[test]
    fn normalization_ignores_a_top_level_flag_with_no_subcommand() {
        let raw = ["lns", "--help"];
        let normalized = normalize_argv(&build_cli(), raw);
        assert_eq!(
            normalized,
            raw.into_iter().map(OsString::from).collect::<Vec<_>>()
        );
    }

    #[test]
    fn run_treats_a_value_options_argument_as_its_value_not_the_image() {
        let args: crate::cli::RunArgs =
            parse_args(["lns", "run", "--workdir", "/w", "alpine", "sh"]).unwrap();
        assert_eq!(args.workdir.as_deref(), Some("/w"));
        assert_eq!(args.image.as_deref(), Some("alpine"));
        assert_eq!(args.cmd, ["sh"]);
    }

    #[test]
    fn run_uses_short_h_for_hostname_while_long_help_still_works() {
        let args: crate::cli::RunArgs = parse_args(["lns", "run", "-h", "demo", "alpine"]).unwrap();
        assert_eq!(args.hostname.as_deref(), Some("demo"));
        let err =
            try_get_matches_from(["lns", "run", "--help"]).expect_err("--help exits through clap");
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    }
}
