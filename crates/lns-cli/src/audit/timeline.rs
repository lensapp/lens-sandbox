use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use lns_audit::{TimelineRow, collect_timeline};

use super::AuditArgs;
use super::table::render_table;
use crate::log;

pub(super) fn run(args: &AuditArgs, out: &mut dyn Write) -> Result<i32> {
    let runs_root = lns_ipc::audit_runs_root().context("locating the audit runs root")?;
    let ledger_path = lns_ipc::connection_ledger().context("locating the connection ledger")?;

    let scope = match &args.sandbox {
        Some(sandbox) => match resolve_scope(sandbox, &runs_root, &ledger_path)? {
            Some(run_id) => Some(run_id),
            None => {
                writeln!(out, "No audit events for sandbox {sandbox}.")?;
                return Ok(0);
            }
        },
        None => None,
    };

    let timeline = collect_timeline(&runs_root, &ledger_path, scope.as_deref())?;
    for warning in &timeline.warnings {
        log::warn!("{warning}");
    }
    let mut rows = timeline.rows;
    rows.retain(|row| matches_filter(row, args));

    if args.json {
        emit_json(&rows, out)?;
    } else {
        render(&rows, args.sandbox.as_deref(), out)?;
    }
    Ok(0)
}

fn matches_filter(row: &TimelineRow, args: &AuditArgs) -> bool {
    if let Some(integration) = &args.integration
        && row.integration.as_deref() != Some(integration.as_str())
    {
        return false;
    }
    if let Some(kind) = args.kind
        && row.kind != kind.label()
    {
        return false;
    }
    true
}

fn render(rows: &[TimelineRow], sandbox: Option<&str>, out: &mut dyn Write) -> Result<()> {
    if rows.is_empty() {
        match sandbox {
            Some(sandbox) => writeln!(out, "No audit events for sandbox {sandbox}.")?,
            None => writeln!(out, "No audit events.")?,
        }
        return Ok(());
    }
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.when.clone(),
                lns_ipc::short_run_id(&row.run).to_string(),
                row.kind.clone(),
                row.detail.clone(),
            ]
        })
        .collect();
    render_table(out, &["WHEN", "RUN", "KIND", "DETAIL"], &table)?;
    Ok(())
}

fn emit_json(rows: &[TimelineRow], out: &mut dyn Write) -> Result<()> {
    for row in rows {
        let line = serde_json::to_string(&row.raw).context("serializing audit event")?;
        writeln!(out, "{line}")?;
    }
    Ok(())
}

fn resolve_scope(sandbox: &str, runs_root: &Path, ledger_path: &Path) -> Result<Option<String>> {
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut ids: Vec<String> = Vec::new();
    for run_id in lns_audit::run_ids_in(runs_root)? {
        ids.push(run_id);
    }
    for entry in lns_audit::stream_ledger(ledger_path)? {
        let record = entry?.record;
        ids.push(record.run.clone());
        names.entry(record.microvm.clone()).or_insert(record.run);
    }
    ids.sort();
    ids.dedup();

    if ids.iter().any(|id| id == sandbox) {
        return Ok(Some(sandbox.to_string()));
    }
    if let Some(run) = names.get(sandbox) {
        return Ok(Some(run.clone()));
    }
    let mut prefix_matches = ids.iter().filter(|id| id.starts_with(sandbox));
    match (prefix_matches.next(), prefix_matches.next()) {
        (Some(only), None) => Ok(Some(only.clone())),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::KindArg;
    use lns_ipc::{AuthKind, LedgerEvent, LedgerRecord};
    use serde_json::Value;

    struct Fixture {
        _home: tempfile::TempDir,
        runs_root: std::path::PathBuf,
        ledger_path: std::path::PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let home = tempfile::TempDir::new().unwrap();
            let runs_root = home.path().join("cache").join("runs");
            let ledger_path = home.path().join("data").join("ledger.jsonl");
            std::fs::create_dir_all(&runs_root).unwrap();
            std::fs::create_dir_all(ledger_path.parent().unwrap()).unwrap();
            Self {
                _home: home,
                runs_root,
                ledger_path,
            }
        }

        fn write_run(&self, run_id: &str, lines: &[&str]) {
            let dir = self.runs_root.join(run_id);
            std::fs::create_dir_all(&dir).unwrap();
            let mut chain = lns_ipc::AuditChain::new();
            let mut payload = String::new();
            for line in lines {
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

        fn write_ledger(&self, records: &[LedgerRecord]) {
            let mut chain = lns_ipc::AuditChain::new();
            let mut payload = String::new();
            for record in records {
                let aug = chain
                    .augment(&serde_json::to_string(record).unwrap())
                    .unwrap();
                payload.push_str(std::str::from_utf8(&aug).unwrap());
                payload.push('\n');
            }
            std::fs::write(&self.ledger_path, payload).unwrap();
            std::fs::write(
                self.ledger_path.with_file_name("ledger.anchor"),
                chain.anchor().expect("chain has events").to_line(),
            )
            .unwrap();
        }

        fn collect(&self, args: &AuditArgs) -> Result<Vec<TimelineRow>> {
            let scope = match &args.sandbox {
                Some(sandbox) => {
                    resolve_scope(sandbox, &self.runs_root, &self.ledger_path)?.map(Some)
                }
                None => Some(None),
            };
            let Some(scope) = scope else {
                return Ok(Vec::new());
            };
            let mut rows =
                collect_timeline(&self.runs_root, &self.ledger_path, scope.as_deref())?.rows;
            rows.retain(|row| matches_filter(row, args));
            Ok(rows)
        }

        fn render(&self, args: &AuditArgs) -> String {
            let rows = self.collect(args).unwrap();
            let mut buf = Vec::new();
            if args.json {
                emit_json(&rows, &mut buf).unwrap();
            } else {
                render(&rows, args.sandbox.as_deref(), &mut buf).unwrap();
            }
            String::from_utf8(buf).unwrap()
        }
    }

    fn args() -> AuditArgs {
        AuditArgs {
            sandbox: None,
            integration: None,
            kind: None,
            json: false,
        }
    }

    fn connection(run: &str, ts: &str) -> LedgerRecord {
        LedgerRecord {
            ts: ts.into(),
            run: run.into(),
            microvm: "calm-finch".into(),
            event: LedgerEvent::Connection {
                integration: "some-oauth".into(),
                auth: AuthKind::Oauth,
                account: Some("@hchen".into()),
                scopes: vec!["repo".into()],
                expires: None,
            },
        }
    }

    fn credential_use(run: &str, ts: &str) -> LedgerRecord {
        LedgerRecord {
            ts: ts.into(),
            run: run.into(),
            microvm: "calm-finch".into(),
            event: LedgerEvent::CredentialUse {
                integration: "some-provider".into(),
                auth: AuthKind::Apikey,
                fp: Some("9c2f1a3d".into()),
                dest: vec!["api.some-provider.example".into()],
            },
        }
    }

    #[test]
    fn the_bare_timeline_renders_every_kind_in_plain_language() {
        let fix = Fixture::new();
        fix.write_run(
            "1a2b3c4d0000000000000000000000aa",
            &[
                r#"{"ts":"2026-06-29T13:00:00Z","event":"run_env","env":{"FOO":"bar"}}"#,
                r#"{"ts":"2026-06-29T13:30:00Z","action":"GET http://api.example.test:443/","metadata":{"reason":"user-allowed-once"}}"#,
            ],
        );
        fix.write_ledger(&[
            connection("1a2b3c4d0000000000000000000000aa", "2026-06-29T14:00:00Z"),
            credential_use("1a2b3c4d0000000000000000000000aa", "2026-06-29T15:00:00Z"),
        ]);
        let text = fix.render(&args());
        assert!(text.contains("WHEN") && text.contains("DETAIL"), "{text}");
        assert!(text.contains("injected: FOO"), "{text}");
        assert!(
            text.contains("GET api.example.test:443 — allowed once"),
            "{text}"
        );
        assert!(
            text.contains("connect some-oauth (oauth) @hchen [repo]"),
            "{text}"
        );
        assert!(
            text.contains("use some-provider fp 9c2f1a3d → api.some-provider.example"),
            "{text}"
        );
    }

    #[test]
    fn an_empty_world_reports_no_events() {
        let fix = Fixture::new();
        assert_eq!(fix.render(&args()).trim(), "No audit events.");
    }

    #[test]
    fn an_exact_run_id_scopes_to_that_run() {
        let fix = Fixture::new();
        fix.write_run(
            "1a2b3c4d0000000000000000000000aa",
            &[r#"{"ts":"2026-06-29T13:00:00Z","type":"volume_attached","name":"data","target":"/data"}"#],
        );
        fix.write_ledger(&[connection(
            "5e6f7a8b0000000000000000000000bb",
            "2026-06-29T14:00:00Z",
        )]);
        let scoped = AuditArgs {
            sandbox: Some("1a2b3c4d0000000000000000000000aa".into()),
            ..args()
        };
        let text = fix.render(&scoped);
        assert!(text.contains("data → /data"), "{text}");
        assert!(!text.contains("some-oauth"), "{text}");
    }

    #[test]
    fn a_run_name_scopes_to_the_matching_run() {
        let fix = Fixture::new();
        let mut rec = connection("5e6f7a8b0000000000000000000000bb", "2026-06-29T14:00:00Z");
        rec.microvm = "calm-finch".into();
        fix.write_ledger(&[rec]);
        let scoped = AuditArgs {
            sandbox: Some("calm-finch".into()),
            ..args()
        };
        assert!(fix.render(&scoped).contains("some-oauth"));
    }

    #[test]
    fn a_unique_run_id_prefix_scopes_to_the_matching_run() {
        let fix = Fixture::new();
        fix.write_run(
            "1a2b3c4d0000000000000000000000aa",
            &[r#"{"ts":"2026-06-29T13:00:00Z","type":"volume_attached","name":"data","target":"/data"}"#],
        );
        let scoped = AuditArgs {
            sandbox: Some("1a2b".into()),
            ..args()
        };
        assert!(fix.render(&scoped).contains("data → /data"));
    }

    #[test]
    fn an_ambiguous_prefix_does_not_resolve() {
        let fix = Fixture::new();
        fix.write_run(
            "ab11",
            &[r#"{"ts":"2026-06-29T13:00:00Z","type":"volume_attached","name":"a","target":"/a"}"#],
        );
        fix.write_run(
            "ab22",
            &[r#"{"ts":"2026-06-29T13:00:00Z","type":"volume_attached","name":"b","target":"/b"}"#],
        );
        assert!(
            resolve_scope("ab", &fix.runs_root, &fix.ledger_path)
                .unwrap()
                .is_none()
        );
        let scoped = AuditArgs {
            sandbox: Some("ab".into()),
            ..args()
        };
        assert_eq!(
            fix.render(&scoped).trim(),
            "No audit events for sandbox ab."
        );
    }

    #[test]
    fn an_unknown_sandbox_reports_no_events_scoped() {
        let fix = Fixture::new();
        fix.write_run(
            "1a2b",
            &[r#"{"ts":"2026-06-29T13:00:00Z","type":"volume_attached","name":"a","target":"/a"}"#],
        );
        let scoped = AuditArgs {
            sandbox: Some("nope".into()),
            ..args()
        };
        assert_eq!(
            fix.render(&scoped).trim(),
            "No audit events for sandbox nope."
        );
    }

    #[test]
    fn the_integration_filter_keeps_only_matching_ledger_events() {
        let fix = Fixture::new();
        fix.write_run(
            "1a2b3c4d0000000000000000000000aa",
            &[r#"{"ts":"2026-06-29T13:00:00Z","action":"GET http://api.example.test:443/"}"#],
        );
        fix.write_ledger(&[
            connection("1a2b3c4d0000000000000000000000aa", "2026-06-29T14:00:00Z"),
            credential_use("1a2b3c4d0000000000000000000000aa", "2026-06-29T15:00:00Z"),
        ]);
        let filtered = AuditArgs {
            integration: Some("some-oauth".into()),
            ..args()
        };
        let rows = fix.collect(&filtered).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "connection");
    }

    #[test]
    fn the_kind_filter_keeps_only_the_named_kind() {
        let fix = Fixture::new();
        fix.write_run(
            "1a2b3c4d0000000000000000000000aa",
            &[
                r#"{"ts":"2026-06-29T13:00:00Z","event":"run_env","env":{"FOO":"bar"}}"#,
                r#"{"ts":"2026-06-29T13:30:00Z","action":"GET http://api.example.test:443/"}"#,
            ],
        );
        let filtered = AuditArgs {
            kind: Some(KindArg::Egress),
            ..args()
        };
        let rows = fix.collect(&filtered).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "egress");
    }

    #[test]
    fn the_kind_filter_accepts_credential_for_credential_use_events() {
        let fix = Fixture::new();
        fix.write_ledger(&[credential_use(
            "1a2b3c4d0000000000000000000000aa",
            "2026-06-29T15:00:00Z",
        )]);
        let filtered = AuditArgs {
            kind: Some(KindArg::Credential),
            ..args()
        };
        let rows = fix.collect(&filtered).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "credential");
    }

    #[test]
    fn json_emits_one_self_describing_object_per_event_in_sorted_order() {
        let fix = Fixture::new();
        fix.write_run(
            "1a2b3c4d0000000000000000000000aa",
            &[r#"{"ts":"2026-06-29T13:00:00Z","event":"run_env","env":{"FOO":"bar"}}"#],
        );
        fix.write_ledger(&[connection(
            "1a2b3c4d0000000000000000000000aa",
            "2026-06-29T14:00:00Z",
        )]);
        let json = AuditArgs {
            json: true,
            ..args()
        };
        let text = fix.render(&json);
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 2);
        let newest: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(newest["event"], "connection", "newest event first");
        let oldest: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(oldest["run"], "1a2b3c4d0000000000000000000000aa");
    }

    #[test]
    fn a_corrupt_run_line_propagates_the_read_error() {
        let fix = Fixture::new();
        let dir = fix.runs_root.join("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("audit.jsonl"), "not json\n").unwrap();
        assert!(fix.collect(&args()).is_err());
    }
}
