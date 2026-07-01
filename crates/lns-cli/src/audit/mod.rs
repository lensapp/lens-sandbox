mod events;
mod show;
mod store;
mod table;
mod timeline;
pub mod verify;

use std::path::Path;

use clap::FromArgMatches;
use lns_ipc::AuthKind;

use crate::command::{CommandSpec, RunCtx, RunFuture, subcommand};
use crate::log;

#[derive(clap::Args)]
pub struct AuditArgs {
    #[arg(
        help = "Sandbox to scope to: a run id or a unique run-id prefix. Omit for every sandbox."
    )]
    pub sandbox: Option<String>,
    #[arg(long, help = "Only show events for this integration.")]
    pub integration: Option<String>,
    #[arg(long, value_enum, help = "Only show events of this kind.")]
    pub kind: Option<KindArg>,
    #[arg(long, help = "Emit one raw JSON event per line instead of the table.")]
    pub json: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum KindArg {
    Egress,
    Env,
    Volume,
    Bind,
    Approval,
    Connection,
    Credential,
}

impl KindArg {
    pub(super) fn label(self) -> &'static str {
        match self {
            KindArg::Egress => "egress",
            KindArg::Env => "env",
            KindArg::Volume => "volume",
            KindArg::Bind => "bind",
            KindArg::Approval => "approval",
            KindArg::Connection => "connection",
            KindArg::Credential => "credential",
        }
    }
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(subcommand::<AuditArgs>("audit").about(
        "Show one chronological timeline of every audit event across all sandboxes, or scope it to one.",
    ))
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
        timeline::run(&args, ctx.out)
    })
}

pub(super) fn warn_if_compromised(path: &Path, anchor: &Path) {
    if let Ok(outcome) = verify::verify_chain_with_anchor(path, anchor)
        && let Some(advisory) = integrity_advisory(&outcome)
    {
        log::warn!("{advisory}");
    }
}

fn integrity_advisory(outcome: &verify::VerifyOutcome) -> Option<String> {
    match outcome {
        verify::VerifyOutcome::Ok { .. } => None,
        verify::VerifyOutcome::Broken { at_line, reason } => Some(format!(
            "audit integrity: chain broken at line {at_line} ({reason}) — entries shown may have been altered"
        )),
        verify::VerifyOutcome::Truncated { reason } => Some(format!(
            "audit integrity: log truncated or rolled back ({reason}) — entries may be missing"
        )),
        verify::VerifyOutcome::AnchorUnreadable { reason, .. } => Some(format!(
            "audit integrity: anchor unreadable ({reason}) — the log cannot be confirmed intact"
        )),
        verify::VerifyOutcome::NoAnchor { .. } => Some(
            "audit integrity: no anchor beside the log — truncation or rollback cannot be detected"
                .to_string(),
        ),
    }
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
    use anyhow::Result;

    fn write_run_chain(home: &std::path::Path, run_id: &str) {
        for cache in ["Library/Caches", ".cache"] {
            let dir = home.join(cache).join("lns").join("runs").join(run_id);
            std::fs::create_dir_all(&dir).unwrap();
            let mut chain = lns_ipc::AuditChain::new();
            let mut payload = String::new();
            for line in [
                r#"{"ts":"2026-06-29T13:00:00Z","type":"volume_attached","name":"data","target":"/data"}"#,
                r#"{"ts":"2026-06-29T13:00:01Z","event":"run_env","env":{"FOO":"bar"}}"#,
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
            run: "5e6f7a8b0000000000000000000000bb".into(),
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
    fn kind_arg_labels_match_the_event_kind_names() {
        assert_eq!(KindArg::Egress.label(), "egress");
        assert_eq!(KindArg::Env.label(), "env");
        assert_eq!(KindArg::Volume.label(), "volume");
        assert_eq!(KindArg::Bind.label(), "bind");
        assert_eq!(KindArg::Approval.label(), "approval");
        assert_eq!(KindArg::Connection.label(), "connection");
        assert_eq!(KindArg::Credential.label(), "credential");
    }

    #[test]
    fn integrity_advisory_is_silent_only_for_an_intact_chain() {
        assert_eq!(
            integrity_advisory(&verify::VerifyOutcome::Ok { line_count: 3 }),
            None
        );
        let broken = integrity_advisory(&verify::VerifyOutcome::Broken {
            at_line: 7,
            reason: "prev_hash mismatch".into(),
        })
        .expect("a broken chain advises");
        assert!(
            broken.contains("line 7") && broken.contains("altered"),
            "{broken}"
        );
        assert!(
            integrity_advisory(&verify::VerifyOutcome::Truncated {
                reason: "tail".into()
            })
            .expect("truncation advises")
            .contains("truncated")
        );
        assert!(
            integrity_advisory(&verify::VerifyOutcome::AnchorUnreadable {
                line_count: 2,
                reason: "corrupt".into(),
            })
            .expect("an unreadable anchor advises")
            .contains("anchor unreadable")
        );
        assert!(
            integrity_advisory(&verify::VerifyOutcome::NoAnchor { line_count: 2 })
                .expect("a missing anchor advises")
                .contains("no anchor")
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn the_bare_timeline_merges_run_and_ledger_events() {
        let home = tempfile::TempDir::new().unwrap();
        write_run_chain(home.path(), "424242");
        write_ledger(home.path());
        let _env = home_env(home.path());
        let mut out = Vec::new();
        let code = dispatch_argv(&["lns", "audit"], &mut out).await.unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("WHEN") && text.contains("DETAIL"), "{text}");
        assert!(text.contains("data → /data"), "{text}");
        assert!(text.contains("injected: FOO"), "{text}");
        assert!(
            text.contains("connect some-oauth (oauth) @hchen [repo]"),
            "{text}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn the_json_flag_emits_one_raw_event_per_line() {
        let home = tempfile::TempDir::new().unwrap();
        write_ledger(home.path());
        let _env = home_env(home.path());
        let mut out = Vec::new();
        let code = dispatch_argv(&["lns", "audit", "--json"], &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        let line = text
            .lines()
            .find(|l| !l.trim().is_empty())
            .expect("a json line");
        let v: serde_json::Value = serde_json::from_str(line).expect("each line is a json object");
        assert_eq!(v["event"], "connection", "got: {text}");
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn a_bare_sandbox_id_scopes_the_timeline_to_that_run() {
        let home = tempfile::TempDir::new().unwrap();
        write_run_chain(home.path(), "424242");
        write_ledger(home.path());
        let _env = home_env(home.path());
        let mut out = Vec::new();
        let code = dispatch_argv(&["lns", "audit", "424242"], &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("data → /data"), "got: {text}");
        assert!(
            !text.contains("some-oauth"),
            "scoped out the ledger: {text}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn an_unknown_sandbox_reports_no_events_without_erroring() {
        let home = tempfile::TempDir::new().unwrap();
        write_run_chain(home.path(), "424242");
        let _env = home_env(home.path());
        let mut out = Vec::new();
        let code = dispatch_argv(&["lns", "audit", "nope"], &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.trim(), "No audit events for sandbox nope.");
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn an_empty_world_reports_no_events() {
        let home = tempfile::TempDir::new().unwrap();
        let _env = home_env(home.path());
        let mut out = Vec::new();
        let code = dispatch_argv(&["lns", "audit"], &mut out).await.unwrap();
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).unwrap().trim(), "No audit events.");
    }
}
