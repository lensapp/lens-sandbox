mod connections;
mod events;
mod show;
mod store;
mod table;
pub mod verify;

use std::io::Write;

use anyhow::{Result, bail};
use clap::FromArgMatches;
use lns_ipc::AuthKind;

use crate::command::{CommandSpec, RunCtx, RunFuture, subcommand};
use crate::log;

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct AuditArgs {
    #[command(subcommand)]
    pub command: Option<AuditCommand>,
    #[arg(help = "Run id to show; shorthand for `lns audit show <id>`.")]
    pub run_id: Option<String>,
}

#[derive(clap::Subcommand)]
pub enum AuditCommand {
    #[command(about = "Show a run's audit timeline.")]
    Show(ShowArgs),
    #[command(about = "Verify the hash chain of a run's audit log, or the global ledger.")]
    Verify(VerifyArgs),
    #[command(about = "Show the global connection/approval ledger timeline.")]
    Log(LogArgs),
    #[command(about = "Summarize connections grouped by integration.")]
    Connections(ConnectionsArgs),
}

#[derive(clap::Args)]
pub struct ShowArgs {
    #[arg(help = "Run identifier surfaced by `lns run` as `✓ started run #<id>`.")]
    pub run_id: String,
    #[arg(
        long,
        help = "Emit the raw JSONL log instead of the rendered timeline."
    )]
    pub json: bool,
}

#[derive(clap::Args)]
pub struct VerifyArgs {
    #[arg(help = "Run id to verify; omit to verify the global connection ledger.")]
    pub run_id: Option<String>,
    #[arg(
        long,
        help = "Treat a missing anchor as non-fatal. Without it, an absent anchor exits non-zero because truncation and rollback cannot be detected."
    )]
    pub allow_missing_anchor: bool,
}

#[derive(clap::Args)]
pub struct LogArgs {
    #[arg(long, help = "Only show events for this integration.")]
    pub integration: Option<String>,
    #[arg(long, help = "Only show events for this run id.")]
    pub run: Option<u32>,
    #[arg(long, value_enum, help = "Only show events of this kind.")]
    pub kind: Option<KindArg>,
    #[arg(long, help = "Emit raw JSONL instead of the rendered timeline.")]
    pub json: bool,
}

#[derive(clap::Args)]
pub struct ConnectionsArgs {
    #[arg(help = "Integration to drill into; omit for the summary across all.")]
    pub integration: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum KindArg {
    Approval,
    Connection,
    CredentialUse,
}

impl KindArg {
    pub(super) fn event_name(self) -> &'static str {
        match self {
            KindArg::Approval => "approval",
            KindArg::Connection => "connection",
            KindArg::CredentialUse => "credential_use",
        }
    }
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<AuditArgs>("audit").about("Inspect audit logs and the connection ledger."),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "audit",
    augment,
    run,
    announces_update_check: true,
    owns_terminal: false,
};

pub fn run<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = AuditArgs::from_arg_matches(matches)?;
        dispatch(args, ctx.out)
    })
}

fn dispatch(args: AuditArgs, out: &mut dyn Write) -> Result<i32> {
    match args.command {
        Some(AuditCommand::Show(args)) => show::run(&args, out),
        Some(AuditCommand::Verify(args)) => run_verify(&args, out),
        Some(AuditCommand::Log(args)) => events::run(&args, out),
        Some(AuditCommand::Connections(args)) => connections::run(&args, out),
        None => match args.run_id {
            Some(run_id) => show::run(
                &ShowArgs {
                    run_id,
                    json: false,
                },
                out,
            ),
            None => bail!("specify a run id, or a subcommand: show, verify, log, connections"),
        },
    }
}

fn run_verify(args: &VerifyArgs, out: &mut dyn Write) -> Result<i32> {
    let (path, anchor) = match &args.run_id {
        Some(run_id) => (
            lns_ipc::audit_log_for_run(run_id)?,
            lns_ipc::audit_anchor_for_run(run_id)?,
        ),
        None => (
            lns_ipc::connection_ledger()?,
            lns_ipc::connection_ledger_anchor()?,
        ),
    };
    log::info!("Verifying", "hash chain at {}", path.display());
    let outcome = verify::verify_chain_with_anchor(&path, &anchor)?;
    report_outcome(outcome, args.allow_missing_anchor, out)
}

fn report_outcome(
    outcome: verify::VerifyOutcome,
    allow_missing_anchor: bool,
    out: &mut dyn Write,
) -> Result<i32> {
    Ok(match outcome {
        verify::VerifyOutcome::Ok { line_count } => {
            writeln!(out, "Verified {line_count} audit events")?;
            0
        }
        verify::VerifyOutcome::NoAnchor { line_count } => {
            report_no_anchor(line_count, allow_missing_anchor, out)?
        }
        verify::VerifyOutcome::AnchorUnreadable { line_count, reason } => {
            log::error!(
                "audit anchor present but unreadable ({reason}) — truncation or rollback cannot be verified for {line_count} events"
            );
            1
        }
        verify::VerifyOutcome::Broken { at_line, reason } => {
            log::error!("audit chain TAMPERED at line {at_line}: {reason}");
            1
        }
        verify::VerifyOutcome::Truncated { reason } => {
            log::error!("audit chain TRUNCATED: {reason}");
            1
        }
    })
}

fn report_no_anchor(
    line_count: usize,
    allow_missing_anchor: bool,
    out: &mut dyn Write,
) -> Result<i32> {
    writeln!(out, "Verified {line_count} audit events")?;
    log::warn!(
        "no anchor beside the log — chain integrity was checked, but truncation or rollback cannot be detected"
    );
    Ok(if allow_missing_anchor { 0 } else { 1 })
}

pub(super) fn friendly_when(ts: &str) -> String {
    ts.trim_end_matches('Z').replacen('T', " ", 1)
}

pub(super) fn auth_word(auth: AuthKind) -> &'static str {
    match auth {
        AuthKind::Oauth => "oauth",
        AuthKind::Apikey => "apikey",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(outcome: verify::VerifyOutcome, allow_missing_anchor: bool) -> i32 {
        report_outcome(outcome, allow_missing_anchor, &mut Vec::new()).unwrap()
    }

    fn write_run_chain(home: &std::path::Path, run_id: &str) {
        for cache in ["Library/Caches", ".cache"] {
            let dir = home.join(cache).join("lns").join("runs").join(run_id);
            std::fs::create_dir_all(&dir).unwrap();
            let mut chain = lns_ipc::AuditChain::new();
            let mut payload = String::new();
            for line in [
                r#"{"type":"volume_attached","name":"data","target":"/data"}"#,
                r#"{"event":"run_env"}"#,
            ] {
                let aug = chain.augment(line).unwrap();
                payload.push_str(std::str::from_utf8(&aug).unwrap());
                payload.push('\n');
            }
            std::fs::write(dir.join("audit.jsonl"), payload).unwrap();
            std::fs::write(
                dir.join("audit.anchor"),
                chain.anchor().expect("chain has events").to_line(),
            )
            .unwrap();
        }
    }

    fn write_ledger(home: &std::path::Path) {
        let record = lns_ipc::LedgerRecord {
            ts: "2026-06-29T14:02:11Z".into(),
            run: 49,
            microvm: "calm-finch".into(),
            event: lns_ipc::LedgerEvent::Connection {
                integration: "some-oauth".into(),
                auth: AuthKind::Oauth,
                account: Some("@hchen".into()),
                scopes: vec!["repo".into()],
                expires: None,
            },
        };
        for data in ["Library/Application Support", ".local/share"] {
            let dir = home.join(data).join("lns");
            std::fs::create_dir_all(&dir).unwrap();
            let mut chain = lns_ipc::AuditChain::new();
            let mut line = chain
                .augment(&serde_json::to_string(&record).unwrap())
                .unwrap();
            line.push(b'\n');
            std::fs::write(dir.join("ledger.jsonl"), line).unwrap();
            std::fs::write(
                dir.join("ledger.anchor"),
                chain.anchor().expect("chain has events").to_line(),
            )
            .unwrap();
        }
    }

    fn home_env(home: &std::path::Path) -> Vec<crate::test_env::EnvScope> {
        vec![
            crate::test_env::EnvScope::set("HOME", home),
            crate::test_env::EnvScope::set("XDG_CACHE_HOME", home.join(".cache")),
            crate::test_env::EnvScope::set("XDG_DATA_HOME", home.join(".local/share")),
        ]
    }

    async fn dispatch_argv(argv: &[&str], out: &mut Vec<u8>) -> Result<i32> {
        let matches = crate::command::build_cli()
            .try_get_matches_from(argv)
            .unwrap();
        let (_, sub) = matches.subcommand().unwrap();
        let mut input: &[u8] = b"";
        let ctx = RunCtx {
            debug: false,
            cwd: None,
            input: &mut input,
            out,
        };
        run(sub, ctx).await
    }

    #[test]
    fn report_outcome_clean_chain_returns_zero_and_prints_the_count() {
        let mut out = Vec::new();
        let code =
            report_outcome(verify::VerifyOutcome::Ok { line_count: 3 }, false, &mut out).unwrap();
        assert_eq!(code, 0);
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("Verified 3 audit events")
        );
    }

    #[test]
    fn report_outcome_tampered_and_truncated_and_unreadable_return_one() {
        assert_eq!(
            report(
                verify::VerifyOutcome::Broken {
                    at_line: 7,
                    reason: "prev_hash mismatch".into(),
                },
                false,
            ),
            1
        );
        assert_eq!(
            report(
                verify::VerifyOutcome::Truncated {
                    reason: "tail truncated".into(),
                },
                false,
            ),
            1
        );
        assert_eq!(
            report(
                verify::VerifyOutcome::AnchorUnreadable {
                    line_count: 2,
                    reason: "corrupt".into(),
                },
                true,
            ),
            1
        );
    }

    #[test]
    fn report_outcome_missing_anchor_fails_by_default_but_is_tolerated_when_allowed() {
        assert_eq!(
            report(verify::VerifyOutcome::NoAnchor { line_count: 2 }, false),
            1
        );
        assert_eq!(
            report(verify::VerifyOutcome::NoAnchor { line_count: 2 }, true),
            0
        );
    }

    #[test]
    fn friendly_when_strips_the_zulu_marker_and_spaces_the_date() {
        assert_eq!(friendly_when("2026-06-29T14:02:11Z"), "2026-06-29 14:02:11");
        assert_eq!(friendly_when(""), "");
    }

    #[test]
    fn auth_word_maps_both_kinds() {
        assert_eq!(auth_word(AuthKind::Oauth), "oauth");
        assert_eq!(auth_word(AuthKind::Apikey), "apikey");
    }

    #[test]
    fn kind_arg_names_match_the_ledger_event_names() {
        assert_eq!(KindArg::Approval.event_name(), "approval");
        assert_eq!(KindArg::Connection.event_name(), "connection");
        assert_eq!(KindArg::CredentialUse.event_name(), "credential_use");
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn a_bare_run_id_is_shorthand_for_show() {
        let home = tempfile::TempDir::new().unwrap();
        write_run_chain(home.path(), "424242");
        let _env = home_env(home.path());
        let mut out = Vec::new();
        let code = dispatch_argv(&["lns", "audit", "424242"], &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("volume_attached"), "got: {text}");
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn the_show_subcommand_renders_a_run_timeline() {
        let home = tempfile::TempDir::new().unwrap();
        write_run_chain(home.path(), "424242");
        let _env = home_env(home.path());
        let mut out = Vec::new();
        let code = dispatch_argv(&["lns", "audit", "show", "424242"], &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert!(String::from_utf8(out).unwrap().contains("volume_attached"));
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn verify_with_a_run_id_checks_that_runs_chain() {
        let home = tempfile::TempDir::new().unwrap();
        write_run_chain(home.path(), "424242");
        let _env = home_env(home.path());
        let mut out = Vec::new();
        let code = dispatch_argv(&["lns", "audit", "verify", "424242"], &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("Verified 2 audit events")
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn verify_without_a_run_id_checks_the_global_ledger() {
        let home = tempfile::TempDir::new().unwrap();
        write_ledger(home.path());
        let _env = home_env(home.path());
        let mut out = Vec::new();
        let code = dispatch_argv(&["lns", "audit", "verify"], &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("Verified 1 audit events")
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn log_renders_the_global_ledger_timeline() {
        let home = tempfile::TempDir::new().unwrap();
        write_ledger(home.path());
        let _env = home_env(home.path());
        let mut out = Vec::new();
        let code = dispatch_argv(&["lns", "audit", "log"], &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("some-oauth"), "got: {text}");
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn connections_summarizes_the_global_ledger() {
        let home = tempfile::TempDir::new().unwrap();
        write_ledger(home.path());
        let _env = home_env(home.path());
        let mut out = Vec::new();
        let code = dispatch_argv(&["lns", "audit", "connections"], &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("@hchen"), "got: {text}");
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn a_bare_audit_with_no_target_explains_the_choices() {
        let home = tempfile::TempDir::new().unwrap();
        let _env = home_env(home.path());
        let mut out = Vec::new();
        let err = dispatch_argv(&["lns", "audit"], &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("show, verify, log, connections"));
    }
}
