use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use super::table::render_table;
use super::{AuditArgs, events, friendly_when, show, store};

#[derive(Debug)]
struct Row {
    ts: String,
    when: String,
    run: String,
    kind: String,
    detail: String,
    raw: Value,
    integration: Option<String>,
}

pub(super) fn run(args: &AuditArgs, out: &mut dyn Write) -> Result<i32> {
    let cache_root = lns_ipc::cache_root().context("locating the cache root")?;
    let runs_root = cache_root.join("runs");
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

    let mut rows = Vec::new();
    collect_ledger_rows(&ledger_path, scope.as_deref(), &mut rows)?;
    collect_run_rows(&runs_root, scope.as_deref(), &mut rows)?;

    rows.retain(|row| matches_filter(row, args));
    sort_newest_first(&mut rows);

    if args.json {
        emit_json(&rows, out)?;
    } else {
        render(&rows, args.sandbox.as_deref(), out)?;
    }
    Ok(0)
}

fn sort_newest_first(rows: &mut [Row]) {
    rows.sort_by(|a, b| {
        b.ts.cmp(&a.ts)
            .then_with(|| a.run.cmp(&b.run))
            .then_with(|| a.kind.cmp(&b.kind))
    });
}

fn matches_filter(row: &Row, args: &AuditArgs) -> bool {
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

fn render(rows: &[Row], sandbox: Option<&str>, out: &mut dyn Write) -> Result<()> {
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

fn emit_json(rows: &[Row], out: &mut dyn Write) -> Result<()> {
    for row in rows {
        let line = serde_json::to_string(&row.raw).context("serializing audit event")?;
        writeln!(out, "{line}")?;
    }
    Ok(())
}

fn collect_ledger_rows(ledger_path: &Path, scope: Option<&str>, rows: &mut Vec<Row>) -> Result<()> {
    super::warn_if_compromised(
        ledger_path,
        &lns_ipc::connection_ledger_anchor().context("locating the connection ledger anchor")?,
    );
    for record in store::stream_ledger(ledger_path)? {
        let record = record?;
        if scope.is_some_and(|run_id| record.run != run_id) {
            continue;
        }
        let raw = serde_json::to_value(&record).context("serializing ledger record")?;
        let kind = match record.event.name() {
            "credential_use" => "credential",
            other => other,
        };
        rows.push(Row {
            when: friendly_when(&record.ts),
            ts: record.ts.clone(),
            run: record.run.clone(),
            kind: kind.to_string(),
            detail: events::detail(&record.event),
            integration: record.event.integration().map(str::to_string),
            raw,
        });
    }
    Ok(())
}

fn collect_run_rows(runs_root: &Path, scope: Option<&str>, rows: &mut Vec<Row>) -> Result<()> {
    for run_id in run_ids_in(runs_root)? {
        if scope.is_some_and(|wanted| run_id != wanted) {
            continue;
        }
        collect_one_run(runs_root, &run_id, rows)?;
    }
    Ok(())
}

fn collect_one_run(runs_root: &Path, run_id: &str, rows: &mut Vec<Row>) -> Result<()> {
    let log_path = runs_root.join(run_id).join("audit.jsonl");
    let text = match std::fs::read_to_string(&log_path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(e).with_context(|| format!("reading audit log {}", log_path.display()));
        }
    };
    super::warn_if_compromised(&log_path, &log_path.with_file_name("audit.anchor"));
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("parsing audit line {} of {}", idx + 1, log_path.display()))?;
        let Value::Object(mut obj) = value else {
            anyhow::bail!(
                "audit line {} of {} is not a JSON object",
                idx + 1,
                log_path.display()
            );
        };
        obj.insert("run".to_string(), Value::String(run_id.to_string()));
        let (kind, detail) = show::describe(&obj);
        rows.push(Row {
            ts: obj
                .get("ts")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            when: show::when(&obj),
            run: run_id.to_string(),
            kind,
            detail,
            integration: None,
            raw: Value::Object(obj),
        });
    }
    Ok(())
}

fn run_ids_in(runs_root: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(runs_root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(e).with_context(|| format!("reading runs dir {}", runs_root.display()));
        }
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("reading runs dir {}", runs_root.display()))?;
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
            && let Some(name) = entry.file_name().to_str()
        {
            ids.push(name.to_string());
        }
    }
    Ok(ids)
}

fn resolve_scope(sandbox: &str, runs_root: &Path, ledger_path: &Path) -> Result<Option<String>> {
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut ids: Vec<String> = Vec::new();
    for run_id in run_ids_in(runs_root)? {
        ids.push(run_id);
    }
    for record in store::stream_ledger(ledger_path)? {
        let record = record?;
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

        fn collect(&self, args: &AuditArgs) -> Result<Vec<Row>> {
            let scope = match &args.sandbox {
                Some(sandbox) => {
                    resolve_scope(sandbox, &self.runs_root, &self.ledger_path)?.map(Some)
                }
                None => Some(None),
            };
            let Some(scope) = scope else {
                return Ok(Vec::new());
            };
            let mut rows = Vec::new();
            collect_ledger_rows(&self.ledger_path, scope.as_deref(), &mut rows)?;
            collect_run_rows(&self.runs_root, scope.as_deref(), &mut rows)?;
            rows.retain(|row| matches_filter(row, args));
            sort_newest_first(&mut rows);
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
    fn the_bare_timeline_merges_ledger_and_run_events_in_timestamp_order() {
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
        let rows = fix.collect(&args()).unwrap();
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
        assert_eq!(
            kinds,
            ["credential", "connection", "egress", "env"],
            "newest event first"
        );
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
        let text = fix.render(&scoped);
        assert!(text.contains("some-oauth"), "{text}");
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
        let text = fix.render(&scoped);
        assert!(text.contains("data → /data"), "{text}");
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
        let scoped = AuditArgs {
            sandbox: Some("ab".into()),
            ..args()
        };
        assert!(
            resolve_scope("ab", &fix.runs_root, &fix.ledger_path)
                .unwrap()
                .is_none()
        );
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
    fn the_integration_filter_excludes_run_events_and_keeps_matching_ledger_events() {
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
        fix.write_ledger(&[
            connection("1a2b3c4d0000000000000000000000aa", "2026-06-29T14:00:00Z"),
            credential_use("1a2b3c4d0000000000000000000000aa", "2026-06-29T15:00:00Z"),
        ]);
        let filtered = AuditArgs {
            kind: Some(KindArg::Credential),
            ..args()
        };
        let rows = fix.collect(&filtered).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].kind, "credential",
            "the user-facing kind is `credential`, not the wire name `credential_use`"
        );
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
        assert_eq!(oldest["event"], "run_env");
        assert_eq!(
            oldest["run"], "1a2b3c4d0000000000000000000000aa",
            "the per-run object must carry its injected run id"
        );
    }

    #[test]
    fn a_malformed_run_line_surfaces_its_line_number() {
        let fix = Fixture::new();
        let dir = fix.runs_root.join("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("audit.jsonl"), "not json\n").unwrap();
        let err = fix.collect(&args()).unwrap_err();
        assert!(format!("{err:#}").contains("audit line 1"), "{err:#}");
    }

    #[test]
    fn a_non_object_run_line_is_reported_as_corruption() {
        let fix = Fixture::new();
        let dir = fix.runs_root.join("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("audit.jsonl"), "42\n").unwrap();
        let err = fix.collect(&args()).unwrap_err();
        assert!(format!("{err:#}").contains("not a JSON object"), "{err:#}");
    }

    #[test]
    fn a_run_dir_without_an_audit_log_contributes_no_rows() {
        let fix = Fixture::new();
        std::fs::create_dir_all(fix.runs_root.join("empty")).unwrap();
        assert_eq!(fix.render(&args()).trim(), "No audit events.");
    }

    #[test]
    fn ties_break_by_run_then_kind() {
        let fix = Fixture::new();
        fix.write_ledger(&[
            credential_use("bbbb", "2026-06-29T14:00:00Z"),
            connection("aaaa", "2026-06-29T14:00:00Z"),
        ]);
        let rows = fix.collect(&args()).unwrap();
        assert_eq!(rows[0].run, "aaaa");
        assert_eq!(rows[1].run, "bbbb");
    }

    #[test]
    fn scoping_skips_run_dirs_other_than_the_target() {
        let fix = Fixture::new();
        fix.write_run(
            "aaaa",
            &[r#"{"ts":"2026-06-29T13:00:00Z","type":"volume_attached","name":"keep","target":"/keep"}"#],
        );
        fix.write_run(
            "bbbb",
            &[r#"{"ts":"2026-06-29T13:00:00Z","type":"volume_attached","name":"skip","target":"/skip"}"#],
        );
        let scoped = AuditArgs {
            sandbox: Some("aaaa".into()),
            ..args()
        };
        let text = fix.render(&scoped);
        assert!(text.contains("keep → /keep"), "{text}");
        assert!(
            !text.contains("skip → /skip"),
            "a run dir other than the scoped one must be skipped: {text}"
        );
    }

    #[test]
    fn a_run_whose_audit_log_is_unreadable_surfaces_a_reading_error() {
        let fix = Fixture::new();
        std::fs::create_dir_all(fix.runs_root.join("weird").join("audit.jsonl")).unwrap();
        let err = fix.collect(&args()).unwrap_err();
        assert!(format!("{err:#}").contains("reading audit log"), "{err:#}");
    }

    #[test]
    fn blank_lines_in_a_run_log_are_skipped() {
        let fix = Fixture::new();
        let dir = fix.runs_root.join("blanks");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("audit.jsonl"),
            "\n   \n{\"ts\":\"2026-06-29T13:00:00Z\",\"type\":\"volume_attached\",\"name\":\"d\",\"target\":\"/d\"}\n",
        )
        .unwrap();
        let rows = fix.collect(&args()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "volume");
    }

    #[test]
    fn a_runs_root_that_is_a_file_surfaces_a_reading_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let not_a_dir = dir.path().join("runs");
        std::fs::write(&not_a_dir, "x").unwrap();
        let err = run_ids_in(&not_a_dir).unwrap_err();
        assert!(format!("{err:#}").contains("reading runs dir"), "{err:#}");
    }
}
