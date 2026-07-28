use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use lns_audit::{TimelineRow, collect_timeline};

use super::{AuditArgs, AuditFormat};
use crate::log;
use crate::output::render_table;

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

    match args.format() {
        AuditFormat::Jsonl => emit_json(&rows, out)?,
        AuditFormat::Table => render(&rows, args.sandbox.as_deref(), out)?,
    }
    Ok(0)
}

fn matches_filter(row: &TimelineRow, args: &AuditArgs) -> bool {
    if let Some(connector) = &args.connector
        && row.connector.as_deref() != Some(connector.as_str())
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
    let mut names: HashMap<String, (String, String)> = HashMap::new();
    let mut ids: Vec<String> = Vec::new();
    for run_id in lns_audit::run_ids_in(runs_root)? {
        if let Some((name, ts)) = lns_audit::run_microvm_identity(runs_root, &run_id) {
            index_newest(&mut names, name, run_id.clone(), ts);
        }
        ids.push(run_id);
    }
    for event in lns_audit::stream_ledger(ledger_path)? {
        let Ok(event) = event else { continue };
        let Ok(row) = lns_audit::read(&event) else {
            continue;
        };
        index_newest(
            &mut names,
            lns_audit::microvm(&event),
            row.run.clone(),
            row.ts,
        );
        ids.push(row.run);
    }
    ids.sort();
    ids.dedup();

    if ids.iter().any(|id| id == sandbox) {
        return Ok(Some(sandbox.to_string()));
    }
    if let Some((run, _ts)) = names.get(sandbox) {
        return Ok(Some(run.clone()));
    }
    let mut prefix_matches = ids.iter().filter(|id| id.starts_with(sandbox));
    match (prefix_matches.next(), prefix_matches.next()) {
        (Some(only), None) => Ok(Some(only.clone())),
        _ => Ok(None),
    }
}

fn index_newest(
    names: &mut HashMap<String, (String, String)>,
    name: String,
    run: String,
    ts: String,
) {
    match names.get(&name) {
        Some((_, prev_ts)) if *prev_ts >= ts => {}
        _ => {
            names.insert(name, (run, ts));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::KindArg;
    use serde_json::Value;

    struct Fixture {
        _home: tempfile::TempDir,
        runs_root: std::path::PathBuf,
        ledger_path: std::path::PathBuf,
    }

    fn octx<'a>(run: &'a str, ts: &'a str) -> lns_ocsf::Context<'a> {
        lns_ocsf::Context {
            time_unix_secs: 1_780_000_000,
            ts_rfc3339: ts,
            run,
            microvm: "calm-finch",
        }
    }

    fn connection(run: &str, ts: &str) -> String {
        lns_ocsf::connection(
            &octx(run, ts),
            "some-oauth",
            "oauth",
            Some("@hchen"),
            &["repo".to_string()],
            None,
        )
        .to_string()
    }

    fn credential_use(run: &str, ts: &str) -> String {
        lns_ocsf::credential_use(
            &octx(run, ts),
            "some-provider",
            "apikey",
            Some("9c2f1a3d"),
            &["api.some-provider.example".to_string()],
        )
        .to_string()
    }

    fn egress(run: &str, ts: &str, url: &str, reason: Option<&str>) -> String {
        lns_ocsf::egress(&octx(run, ts), "GET", url, None, None, reason, true).to_string()
    }

    fn run_env(run: &str, ts: &str, keys: &[&str]) -> String {
        let mut env = serde_json::Map::new();
        for key in keys {
            env.insert(key.to_string(), Value::String("…".into()));
        }
        lns_ocsf::run_env(&octx(run, ts), &env).to_string()
    }

    fn volume(run: &str, ts: &str, name: &str, target: &str) -> String {
        lns_ocsf::volume_mount(&octx(run, ts), name, target).to_string()
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

        fn chain(lines: &[String]) -> (String, lns_ipc::Anchor) {
            let mut chain = lns_ipc::AuditChain::new();
            let mut payload = String::new();
            for line in lines {
                let aug = chain.augment(line).unwrap();
                payload.push_str(std::str::from_utf8(&aug).unwrap());
                payload.push('\n');
            }
            (payload, chain.anchor().expect("chain has events"))
        }

        fn write_run(&self, run_id: &str, lines: &[String]) {
            let dir = self.runs_root.join(run_id);
            std::fs::create_dir_all(&dir).unwrap();
            let (payload, anchor) = Self::chain(lines);
            std::fs::write(dir.join("audit.jsonl"), payload).unwrap();
            std::fs::write(dir.join("audit.anchor"), anchor.to_line()).unwrap();
        }

        fn write_ledger(&self, events: &[String]) {
            let (payload, anchor) = Self::chain(events);
            std::fs::write(&self.ledger_path, payload).unwrap();
            std::fs::write(
                self.ledger_path.with_file_name("ledger.anchor"),
                anchor.to_line(),
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
            match args.format() {
                AuditFormat::Jsonl => emit_json(&rows, &mut buf).unwrap(),
                AuditFormat::Table => render(&rows, args.sandbox.as_deref(), &mut buf).unwrap(),
            }
            String::from_utf8(buf).unwrap()
        }
    }

    fn args() -> AuditArgs {
        AuditArgs {
            sandbox: None,
            connector: None,
            kind: None,
            format: None,
            json: false,
        }
    }

    const RUN: &str = "1a2b3c4d0000000000000000000000aa";

    #[test]
    fn the_bare_timeline_renders_every_kind_in_plain_language() {
        let fix = Fixture::new();
        fix.write_run(
            RUN,
            &[
                run_env(RUN, "2026-06-29T13:00:00Z", &["FOO"]),
                egress(
                    RUN,
                    "2026-06-29T13:30:00Z",
                    "http://api.example.test:443/",
                    Some("user-allowed-once"),
                ),
            ],
        );
        fix.write_ledger(&[
            connection(RUN, "2026-06-29T14:00:00Z"),
            credential_use(RUN, "2026-06-29T15:00:00Z"),
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
        fix.write_run(RUN, &[volume(RUN, "2026-06-29T13:00:00Z", "data", "/data")]);
        fix.write_ledger(&[connection(
            "5e6f7a8b0000000000000000000000bb",
            "2026-06-29T14:00:00Z",
        )]);
        let scoped = AuditArgs {
            sandbox: Some(RUN.into()),
            ..args()
        };
        let text = fix.render(&scoped);
        assert!(text.contains("data → /data"), "{text}");
        assert!(!text.contains("some-oauth"), "{text}");
    }

    #[test]
    fn a_run_name_scopes_to_the_matching_run() {
        let fix = Fixture::new();
        fix.write_ledger(&[connection(
            "5e6f7a8b0000000000000000000000bb",
            "2026-06-29T14:00:00Z",
        )]);
        let scoped = AuditArgs {
            sandbox: Some("calm-finch".into()),
            ..args()
        };
        assert!(fix.render(&scoped).contains("some-oauth"));
    }

    #[test]
    fn a_reused_run_name_resolves_to_the_most_recent_run_regardless_of_ledger_order() {
        let fix = Fixture::new();
        let older = "1111111100000000000000000000aaaa";
        let newer = "2222222200000000000000000000bbbb";
        let middle = "3333333300000000000000000000cccc";
        fix.write_ledger(&[
            connection(older, "2026-06-29T10:00:00Z"),
            connection(newer, "2026-06-29T20:00:00Z"),
            connection(middle, "2026-06-29T15:00:00Z"),
        ]);
        let resolved = resolve_scope("calm-finch", &fix.runs_root, &fix.ledger_path).unwrap();
        assert_eq!(
            resolved.as_deref(),
            Some(newer),
            "a recurring auto-name must resolve to its latest holder by time, not by ledger position"
        );
    }

    #[test]
    fn an_auto_name_resolves_a_run_that_only_produced_run_log_events() {
        let fix = Fixture::new();
        fix.write_run(RUN, &[volume(RUN, "2026-06-29T13:00:00Z", "data", "/data")]);
        let resolved = resolve_scope("calm-finch", &fix.runs_root, &fix.ledger_path).unwrap();
        assert_eq!(
            resolved.as_deref(),
            Some(RUN),
            "a sandbox that never wrote a ledger row is still resolvable by its auto-name via its run-log device.name"
        );
    }

    #[test]
    fn resolve_scope_skips_corrupt_ledger_lines_instead_of_failing_every_lookup() {
        let fix = Fixture::new();
        let good = connection(RUN, "2026-06-29T14:00:00Z");
        let unreadable = r#"{"class_uid":1,"unmapped":{"lns_run":"x"}}"#;
        std::fs::write(
            &fix.ledger_path,
            format!("{good}\n{unreadable}\ntorn-partial-line\n"),
        )
        .unwrap();
        let resolved = resolve_scope("calm-finch", &fix.runs_root, &fix.ledger_path).unwrap();
        assert_eq!(
            resolved.as_deref(),
            Some(RUN),
            "a torn or unreadable ledger line must not blank name resolution for every sandbox"
        );
    }

    #[test]
    fn a_unique_run_id_prefix_scopes_to_the_matching_run() {
        let fix = Fixture::new();
        fix.write_run(RUN, &[volume(RUN, "2026-06-29T13:00:00Z", "data", "/data")]);
        let scoped = AuditArgs {
            sandbox: Some("1a2b".into()),
            ..args()
        };
        assert!(fix.render(&scoped).contains("data → /data"));
    }

    #[test]
    fn an_ambiguous_prefix_does_not_resolve() {
        let fix = Fixture::new();
        fix.write_run("ab11", &[volume("ab11", "2026-06-29T13:00:00Z", "a", "/a")]);
        fix.write_run("ab22", &[volume("ab22", "2026-06-29T13:00:00Z", "b", "/b")]);
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
        fix.write_run("1a2b", &[volume("1a2b", "2026-06-29T13:00:00Z", "a", "/a")]);
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
    fn the_connector_filter_keeps_only_matching_ledger_events() {
        let fix = Fixture::new();
        fix.write_run(
            RUN,
            &[egress(
                RUN,
                "2026-06-29T13:00:00Z",
                "http://api.example.test:443/",
                None,
            )],
        );
        fix.write_ledger(&[
            connection(RUN, "2026-06-29T14:00:00Z"),
            credential_use(RUN, "2026-06-29T15:00:00Z"),
        ]);
        let filtered = AuditArgs {
            connector: Some("some-oauth".into()),
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
            RUN,
            &[
                run_env(RUN, "2026-06-29T13:00:00Z", &["FOO"]),
                egress(
                    RUN,
                    "2026-06-29T13:30:00Z",
                    "http://api.example.test:443/",
                    None,
                ),
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
        fix.write_ledger(&[credential_use(RUN, "2026-06-29T15:00:00Z")]);
        let filtered = AuditArgs {
            kind: Some(KindArg::Credential),
            ..args()
        };
        let rows = fix.collect(&filtered).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "credential");
    }

    #[test]
    fn json_emits_one_ocsf_object_per_event_in_sorted_order() {
        let fix = Fixture::new();
        fix.write_run(RUN, &[run_env(RUN, "2026-06-29T13:00:00Z", &["FOO"])]);
        fix.write_ledger(&[connection(RUN, "2026-06-29T14:00:00Z")]);
        let json = AuditArgs {
            json: true,
            ..args()
        };
        let text = fix.render(&json);
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 2);
        let newest: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(newest["unmapped"]["lns_kind"], "connection", "newest first");
        assert_eq!(newest["class_uid"], 3002, "emitted as OCSF");
        let oldest: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(oldest["unmapped"]["lns_kind"], "env");
        assert_eq!(oldest["run"], RUN, "the per-run event carries its run id");
    }

    #[test]
    fn a_corrupt_run_line_is_skipped_with_a_warning_not_a_hard_error() {
        let fix = Fixture::new();
        let dir = fix.runs_root.join("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("audit.jsonl"), "not json\n").unwrap();
        let timeline = collect_timeline(&fix.runs_root, &fix.ledger_path, None).unwrap();
        assert!(timeline.rows.is_empty(), "the corrupt line yields no row");
        assert!(
            timeline.warnings.iter().any(|w| w.contains("not shown")),
            "a corrupt run line is flagged, not fatal: {:?}",
            timeline.warnings
        );
    }
}
