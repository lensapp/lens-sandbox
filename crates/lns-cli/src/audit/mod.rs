pub mod verify;

use clap::FromArgMatches;

use crate::command::{CommandSpec, RunCtx, RunFuture, subcommand};
use crate::log;

#[derive(clap::Args)]
pub struct AuditArgs {
    #[arg(help = "Run identifier surfaced by `lns run` as `✓ started run #<id>`.")]
    pub run_id: String,
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<AuditArgs>("audit").about("Verify the audit chain of a completed run."),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "audit",
    augment,
    run,
    announces_update_check: true,
};

pub fn run<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = AuditArgs::from_arg_matches(matches)?;
        run_verify(args)
    })
}

pub fn run_verify(args: AuditArgs) -> anyhow::Result<i32> {
    let path = lns_ipc::audit_log_for_run(&args.run_id)?;
    log::info!("Verifying", "audit chain at {}", path.display());
    Ok(report_outcome(verify::verify_chain(&path)?))
}

fn report_outcome(outcome: verify::VerifyOutcome) -> i32 {
    match outcome {
        verify::VerifyOutcome::Ok { line_count } => {
            println!("Verified {line_count} audit events");
            0
        }
        verify::VerifyOutcome::Broken { at_line, reason } => {
            log::error!("audit chain TAMPERED at line {at_line}: {reason}");
            1
        }
        verify::VerifyOutcome::Truncated { reason } => {
            log::error!("audit chain TRUNCATED: {reason}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expect_ok(outcome: verify::VerifyOutcome) -> usize {
        let verify::VerifyOutcome::Ok { line_count } = outcome else {
            panic!("expected Ok, got {outcome:?}")
        };
        line_count
    }

    fn expect_broken(outcome: verify::VerifyOutcome) -> (usize, String) {
        let verify::VerifyOutcome::Broken { at_line, reason } = outcome else {
            panic!("expected Broken, got {outcome:?}")
        };
        (at_line, reason)
    }

    #[test]
    #[should_panic(expected = "expected Ok")]
    fn expect_ok_panics_on_broken() {
        let _ = expect_ok(verify::VerifyOutcome::Broken {
            at_line: 1,
            reason: "x".into(),
        });
    }

    #[test]
    #[should_panic(expected = "expected Broken")]
    fn expect_broken_panics_on_ok() {
        let _ = expect_broken(verify::VerifyOutcome::Ok { line_count: 0 });
    }

    #[test]
    fn round_trip_chain_then_verify_succeeds() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("audit.jsonl");
        let mut chain = lns_ipc::AuditChain::new();
        let lines = vec![
            r#"{"type":"audit_event","seq":1,"summary":"egress"}"#,
            r#"{"type":"audit_event","seq":2,"summary":"file_read"}"#,
            r#"{"type":"audit_event","seq":3,"summary":"file_write"}"#,
        ];
        let mut payload = String::new();
        for line in &lines {
            let augmented = chain.augment(line).unwrap();
            payload.push_str(std::str::from_utf8(&augmented).unwrap());
            payload.push('\n');
        }
        std::fs::write(&path, payload).unwrap();

        assert_eq!(expect_ok(verify::verify_chain(&path).unwrap()), 3);
    }

    #[test]
    fn tampered_middle_line_is_detected() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("audit.jsonl");
        let mut chain = lns_ipc::AuditChain::new();
        let mut payload = String::new();
        for seq in 1..=3 {
            let line = format!(r#"{{"type":"audit_event","seq":{seq}}}"#);
            let augmented = chain.augment(&line).unwrap();
            payload.push_str(std::str::from_utf8(&augmented).unwrap());
            payload.push('\n');
        }
        let tampered = payload.replacen("\"seq\":2", "\"seq\":99", 1);
        assert_ne!(tampered, payload);
        std::fs::write(&path, tampered).unwrap();

        let (at_line, _) = expect_broken(verify::verify_chain(&path).unwrap());
        assert_eq!(at_line, 3);
    }

    #[test]
    fn empty_audit_log_verifies_ok() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("audit.jsonl");
        std::fs::write(&path, "").unwrap();
        assert_eq!(expect_ok(verify::verify_chain(&path).unwrap()), 0);
    }

    #[test]
    fn missing_file_is_distinguishable_from_empty() {
        let err = verify::verify_chain(std::path::Path::new("/nope/never")).unwrap_err();
        assert!(format!("{err:#}").contains("opening"));
    }

    #[test]
    fn report_outcome_clean_chain_returns_zero() {
        let code = report_outcome(verify::VerifyOutcome::Ok { line_count: 3 });
        assert_eq!(code, 0);
    }

    #[test]
    fn report_outcome_tampered_chain_returns_one() {
        let code = report_outcome(verify::VerifyOutcome::Broken {
            at_line: 7,
            reason: "prev_hash mismatch".into(),
        });
        assert_eq!(code, 1);
    }

    #[test]
    fn report_outcome_truncated_chain_returns_one() {
        let code = report_outcome(verify::VerifyOutcome::Truncated {
            reason: "tail truncated".into(),
        });
        assert_eq!(code, 1);
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn run_dispatches_clap_matches_through_to_chain_verification() {
        let cache_root = tempfile::TempDir::new().unwrap();
        let run_id = "424242";
        let macos_cache = cache_root.path().join("Library").join("Caches");
        let linux_cache = cache_root.path().join(".cache");
        for base in [&macos_cache, &linux_cache] {
            let runs_dir = base.join("lns").join("runs").join(run_id);
            std::fs::create_dir_all(&runs_dir).unwrap();
            let mut chain = lns_ipc::AuditChain::new();
            let mut payload = String::new();
            for line in [
                r#"{"type":"audit_event","seq":1}"#,
                r#"{"type":"audit_event","seq":2}"#,
            ] {
                let aug = chain.augment(line).unwrap();
                payload.push_str(std::str::from_utf8(&aug).unwrap());
                payload.push('\n');
            }
            std::fs::write(runs_dir.join("audit.jsonl"), payload).unwrap();
        }

        let _home = crate::test_env::EnvScope::set("HOME", cache_root.path());
        let _xdg = crate::test_env::EnvScope::set("XDG_CACHE_HOME", &linux_cache);
        let matches = crate::command::build_cli()
            .try_get_matches_from(["lns", "audit", run_id])
            .unwrap();
        let (_, sub) = matches.subcommand().unwrap();
        let mut input: &[u8] = b"";
        let mut out: Vec<u8> = Vec::new();
        let ctx = crate::command::RunCtx {
            debug: false,
            cwd: std::path::PathBuf::from("/"),
            input: &mut input,
            out: &mut out,
        };
        let code = run(sub, ctx).await.expect("audit run on valid chain");
        assert_eq!(code, 0);
    }

    #[test]
    fn first_line_genesis_prev_hash_is_zero() {
        let mut chain = lns_ipc::AuditChain::new();
        let augmented = chain.augment(r#"{"type":"audit_event","seq":1}"#).unwrap();
        let s = std::str::from_utf8(&augmented).unwrap();
        assert!(s.contains(&format!("\"prev_hash\":\"{}\"", "0".repeat(64))));
    }
}
