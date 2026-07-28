mod timeline;

use clap::FromArgMatches;

use crate::command::{CommandSpec, RunCtx, RunFuture, subcommand};

#[derive(clap::Args)]
pub struct AuditArgs {
    #[arg(
        help = "Sandbox to scope to: a run id or a unique run-id prefix. Omit for every sandbox."
    )]
    pub sandbox: Option<String>,
    #[arg(long, help = "Only show events for this connector.")]
    pub connector: Option<String>,
    #[arg(long, value_enum, help = "Only show events of this kind.")]
    pub kind: Option<KindArg>,
    #[arg(long, help = "Emit one raw JSON event per line instead of the table.")]
    pub json: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum KindArg {
    Launch,
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
            KindArg::Launch => "launch",
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

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    fn octx<'a>(run: &'a str, ts: &'a str) -> lns_ocsf::Context<'a> {
        lns_ocsf::Context {
            time_unix_secs: 1_780_000_000,
            ts_rfc3339: ts,
            run,
            microvm: "calm-finch",
        }
    }

    fn write_run_chain(home: &std::path::Path, run_id: &str) {
        let volume = lns_ocsf::volume_mount(&octx(run_id, "2026-06-29T13:00:00Z"), "data", "/data")
            .to_string();
        let mut env = serde_json::Map::new();
        env.insert("FOO".into(), "bar".into());
        let run_env = lns_ocsf::run_env(&octx(run_id, "2026-06-29T13:00:01Z"), &env).to_string();
        for data in ["Library/Application Support", ".local/share"] {
            let dir = home.join(data).join("lns").join("runs").join(run_id);
            std::fs::create_dir_all(&dir).unwrap();
            let mut chain = lns_ipc::AuditChain::new();
            let mut payload = String::new();
            for line in [&volume, &run_env] {
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

    fn write_sandbox_run_chain(home: &std::path::Path, run_id: &str) {
        let event = lns_ocsf::sandbox_run(
            &octx(run_id, "2026-06-29T13:00:00Z"),
            "ghcr.io/team/hermes:1.4.0",
            "sha256:abc",
            &[],
            "policyhash",
        )
        .to_string();
        for data in ["Library/Application Support", ".local/share"] {
            let dir = home.join(data).join("lns").join("runs").join(run_id);
            std::fs::create_dir_all(&dir).unwrap();
            let mut chain = lns_ipc::AuditChain::new();
            let mut payload = chain.augment(&event).unwrap();
            payload.push(b'\n');
            std::fs::write(dir.join("audit.jsonl"), payload).unwrap();
            std::fs::write(
                dir.join("audit.anchor"),
                chain.anchor().expect("chain has events").to_line(),
            )
            .unwrap();
        }
    }

    fn tamper_run_log(home: &std::path::Path, run_id: &str) {
        let mut event =
            lns_ocsf::volume_mount(&octx(run_id, "2026-06-29T13:00:00Z"), "data", "/data");
        event
            .as_object_mut()
            .unwrap()
            .insert("prev_hash".into(), "deadbeef".into());
        let line = format!("{event}\n");
        for data in ["Library/Application Support", ".local/share"] {
            let log = home
                .join(data)
                .join("lns")
                .join("runs")
                .join(run_id)
                .join("audit.jsonl");
            if log.exists() {
                std::fs::write(&log, &line).unwrap();
            }
        }
    }

    fn write_ledger(home: &std::path::Path) {
        let event = lns_ocsf::connection(
            &octx("5e6f7a8b0000000000000000000000bb", "2026-06-29T14:02:11Z"),
            "some-oauth",
            "oauth",
            Some("@hchen"),
            &["repo".to_string()],
            None,
        )
        .to_string();
        for data in ["Library/Application Support", ".local/share"] {
            let dir = home.join(data).join("lns");
            std::fs::create_dir_all(&dir).unwrap();
            let mut chain = lns_ipc::AuditChain::new();
            let mut line = chain.augment(&event).unwrap();
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
    fn kind_arg_labels_match_the_event_kind_names() {
        assert_eq!(KindArg::Launch.label(), "launch");
        assert_eq!(KindArg::Egress.label(), "egress");
        assert_eq!(KindArg::Env.label(), "env");
        assert_eq!(KindArg::Volume.label(), "volume");
        assert_eq!(KindArg::Bind.label(), "bind");
        assert_eq!(KindArg::Approval.label(), "approval");
        assert_eq!(KindArg::Connection.label(), "connection");
        assert_eq!(KindArg::Credential.label(), "credential");
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
        assert_eq!(v["unmapped"]["lns_kind"], "connection", "got: {text}");
        assert_eq!(v["class_uid"], 3002, "emitted as OCSF: {text}");
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
    async fn a_compromised_run_chain_is_still_listed_when_reading_the_timeline() {
        let home = tempfile::TempDir::new().unwrap();
        write_run_chain(home.path(), "424242");
        tamper_run_log(home.path(), "424242");
        let _env = home_env(home.path());
        let mut out = Vec::new();
        let code = dispatch_argv(&["lns", "audit"], &mut out).await.unwrap();
        assert_eq!(code, 0, "a compromised chain warns but still lists");
        assert!(String::from_utf8(out).unwrap().contains("WHEN"));
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
    async fn the_json_flag_labels_a_sandbox_run() {
        let home = tempfile::TempDir::new().unwrap();
        write_sandbox_run_chain(home.path(), "5a5a5a");
        let _env = home_env(home.path());
        let mut out = Vec::new();
        let code = dispatch_argv(&["lns", "audit", "--json"], &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("sandbox_run"), "got: {text}");
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
