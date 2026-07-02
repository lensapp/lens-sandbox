use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::{integrity_advisory, ocsf, store, verify};

#[derive(Debug, Clone)]
pub struct TimelineRow {
    pub ts: String,
    pub when: String,
    pub run: String,
    pub kind: String,
    pub detail: String,
    pub raw: Value,
    pub integration: Option<String>,
}

#[derive(Debug, Default)]
pub struct Timeline {
    pub rows: Vec<TimelineRow>,
    pub warnings: Vec<String>,
}

pub fn collect_timeline(
    runs_root: &Path,
    ledger_path: &Path,
    scope: Option<&str>,
) -> Result<Timeline> {
    let mut timeline = Timeline::default();
    collect_ledger_rows(ledger_path, scope, &mut timeline)?;
    collect_run_rows(runs_root, scope, &mut timeline)?;
    sort_newest_first(&mut timeline.rows);
    Ok(timeline)
}

fn integrity_warning(log: &Path, anchor: &Path) -> Option<String> {
    verify::verify_chain_with_anchor(log, anchor)
        .ok()
        .and_then(|outcome| integrity_advisory(&outcome))
}

fn collect_ledger_rows(
    ledger_path: &Path,
    scope: Option<&str>,
    timeline: &mut Timeline,
) -> Result<()> {
    if let Some(warning) =
        integrity_warning(ledger_path, &ledger_path.with_file_name("ledger.anchor"))
    {
        timeline.warnings.push(warning);
    }
    for event in store::stream_ledger(ledger_path)? {
        match read_ledger_row(event) {
            Ok(row) => {
                if scope.is_some_and(|run_id| row.run != run_id) {
                    continue;
                }
                timeline.rows.push(row);
            }
            Err(reason) => timeline.warnings.push(unreadable_ledger_warning(&reason)),
        }
    }
    Ok(())
}

fn read_ledger_row(
    event: Result<serde_json::Map<String, Value>>,
) -> std::result::Result<TimelineRow, String> {
    let event = event.map_err(|e| format!("{e:#}"))?;
    let row = ocsf::read(&event).map_err(|e| format!("{e:#}"))?;
    let when = crate::friendly_when(&row.ts);
    Ok(TimelineRow {
        ts: row.ts,
        when,
        run: row.run,
        kind: row.kind,
        detail: row.detail,
        integration: row.integration,
        raw: Value::Object(event),
    })
}

fn unreadable_ledger_warning(reason: &str) -> String {
    format!("audit integrity: unreadable ledger entry ({reason}) — that entry is not shown")
}

fn collect_run_rows(runs_root: &Path, scope: Option<&str>, timeline: &mut Timeline) -> Result<()> {
    for run_id in run_ids_in(runs_root)? {
        if scope.is_some_and(|wanted| run_id != wanted) {
            continue;
        }
        collect_one_run(runs_root, &run_id, timeline)?;
    }
    Ok(())
}

fn collect_one_run(runs_root: &Path, run_id: &str, timeline: &mut Timeline) -> Result<()> {
    let log_path = runs_root.join(run_id).join("audit.jsonl");
    let text = match std::fs::read_to_string(&log_path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(e).with_context(|| format!("reading audit log {}", log_path.display()));
        }
    };
    if let Some(warning) = integrity_warning(&log_path, &log_path.with_file_name("audit.anchor")) {
        timeline.warnings.push(warning);
    }
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match read_run_row(line, run_id) {
            Ok(row) => timeline.rows.push(row),
            Err(reason) => {
                timeline
                    .warnings
                    .push(unreadable_line_warning(&log_path, idx + 1, &reason))
            }
        }
    }
    Ok(())
}

fn read_run_row(line: &str, run_id: &str) -> std::result::Result<TimelineRow, String> {
    let value: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let Value::Object(mut obj) = value else {
        return Err("line is not a JSON object".to_string());
    };
    obj.insert("run".to_string(), Value::String(run_id.to_string()));
    let row = ocsf::read(&obj).map_err(|e| format!("{e:#}"))?;
    let when = crate::friendly_when(&row.ts);
    Ok(TimelineRow {
        ts: row.ts,
        when,
        run: run_id.to_string(),
        kind: row.kind,
        detail: row.detail,
        integration: row.integration,
        raw: Value::Object(obj),
    })
}

fn unreadable_line_warning(log: &Path, line_no: usize, reason: &str) -> String {
    format!(
        "audit integrity: unreadable entry at line {line_no} of {} ({reason}) — that entry is not shown",
        log.display()
    )
}

pub fn run_ids_in(runs_root: &Path) -> Result<Vec<String>> {
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

/// The microVM name and timestamp of a run's first readable audit event, so a run that only ever produced run-log events (no ledger row) is still resolvable by its auto-name.
pub fn run_microvm_identity(runs_root: &Path, run_id: &str) -> Option<(String, String)> {
    let log_path = runs_root.join(run_id).join("audit.jsonl");
    let text = std::fs::read_to_string(log_path).ok()?;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Ok(row) = ocsf::read(&obj) else {
            continue;
        };
        return Some((ocsf::microvm(&obj), row.ts));
    }
    None
}

fn sort_newest_first(rows: &mut [TimelineRow]) {
    rows.sort_by(|a, b| {
        b.ts.cmp(&a.ts)
            .then_with(|| a.run.cmp(&b.run))
            .then_with(|| a.kind.cmp(&b.kind))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture {
        _home: tempfile::TempDir,
        runs_root: PathBuf,
        ledger_path: PathBuf,
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
            Some("@some-user"),
            &["repo".to_string()],
            None,
        )
        .to_string()
    }

    fn credential(run: &str, ts: &str) -> String {
        lns_ocsf::credential_use(
            &octx(run, ts),
            "some-provider",
            "apikey",
            Some("9c2f1a3d"),
            &["api.some-provider.example".to_string()],
        )
        .to_string()
    }

    fn egress(run: &str, ts: &str, url: &str) -> String {
        lns_ocsf::egress(&octx(run, ts), "GET", url, None, None, None, true).to_string()
    }

    fn chained(lines: &[String]) -> (String, lns_ipc::Anchor) {
        let mut chain = lns_ipc::AuditChain::new();
        let mut payload = String::new();
        for line in lines {
            let augmented = chain.augment(line).unwrap();
            payload.push_str(std::str::from_utf8(&augmented).unwrap());
            payload.push('\n');
        }
        (payload, chain.anchor().expect("chain has events"))
    }

    impl Fixture {
        fn new() -> Self {
            let home = tempfile::TempDir::new().unwrap();
            let runs_root = home.path().join("runs");
            let ledger_path = home.path().join("ledger.jsonl");
            std::fs::create_dir_all(&runs_root).unwrap();
            Self {
                _home: home,
                runs_root,
                ledger_path,
            }
        }

        fn write_run(&self, run_id: &str, events: &[String]) {
            let dir = self.runs_root.join(run_id);
            std::fs::create_dir_all(&dir).unwrap();
            let (payload, anchor) = chained(events);
            std::fs::write(dir.join("audit.jsonl"), payload).unwrap();
            std::fs::write(dir.join("audit.anchor"), anchor.to_line()).unwrap();
        }

        fn write_ledger(&self, events: &[String]) {
            let (payload, anchor) = chained(events);
            std::fs::write(&self.ledger_path, payload).unwrap();
            std::fs::write(
                self.ledger_path.with_file_name("ledger.anchor"),
                anchor.to_line(),
            )
            .unwrap();
        }

        fn collect(&self, scope: Option<&str>) -> Timeline {
            collect_timeline(&self.runs_root, &self.ledger_path, scope).unwrap()
        }
    }

    #[test]
    fn a_run_log_and_the_ledger_merge_newest_first() {
        let fix = Fixture::new();
        fix.write_run(
            "aa01",
            &[egress(
                "aa01",
                "2026-06-29T14:02:00Z",
                "http://api.example.test:443/",
            )],
        );
        fix.write_ledger(&[connection("aa01", "2026-06-29T14:02:11Z")]);

        let timeline = fix.collect(None);

        assert_eq!(timeline.rows.len(), 2);
        assert_eq!(timeline.rows[0].kind, "connection", "14:02:11 is newest");
        assert_eq!(timeline.rows[1].kind, "egress");
        assert_eq!(timeline.rows[0].integration.as_deref(), Some("some-oauth"));
        assert_eq!(timeline.rows[1].detail, "GET api.example.test:443");
        assert!(timeline.warnings.is_empty());
    }

    #[test]
    fn a_scope_keeps_only_the_named_run_across_both_sources() {
        let fix = Fixture::new();
        fix.write_run(
            "aa01",
            &[egress("aa01", "2026-06-29T14:02:00Z", "http://x/")],
        );
        fix.write_run(
            "bb02",
            &[egress("bb02", "2026-06-29T14:03:00Z", "http://y/")],
        );
        fix.write_ledger(&[
            connection("aa01", "2026-06-29T14:02:11Z"),
            credential("bb02", "2026-06-29T14:03:30Z"),
        ]);

        let timeline = fix.collect(Some("aa01"));

        assert!(timeline.rows.iter().all(|r| r.run == "aa01"));
        assert_eq!(timeline.rows.len(), 2);
    }

    #[test]
    fn a_credential_use_event_renders_as_the_credential_kind() {
        let fix = Fixture::new();
        fix.write_ledger(&[credential("aa01", "2026-06-29T14:03:30Z")]);
        let timeline = fix.collect(None);
        assert_eq!(timeline.rows[0].kind, "credential");
    }

    #[test]
    fn ties_break_by_run_then_kind() {
        let mut rows = vec![
            TimelineRow {
                ts: "t".into(),
                when: "t".into(),
                run: "bb".into(),
                kind: "env".into(),
                detail: String::new(),
                raw: Value::Null,
                integration: None,
            },
            TimelineRow {
                ts: "t".into(),
                when: "t".into(),
                run: "aa".into(),
                kind: "volume".into(),
                detail: String::new(),
                raw: Value::Null,
                integration: None,
            },
            TimelineRow {
                ts: "t".into(),
                when: "t".into(),
                run: "aa".into(),
                kind: "env".into(),
                detail: String::new(),
                raw: Value::Null,
                integration: None,
            },
        ];
        sort_newest_first(&mut rows);
        let order: Vec<_> = rows
            .iter()
            .map(|r| (r.run.as_str(), r.kind.as_str()))
            .collect();
        assert_eq!(order, [("aa", "env"), ("aa", "volume"), ("bb", "env")]);
    }

    #[test]
    fn a_missing_run_log_and_an_absent_runs_root_yield_no_rows() {
        let fix = Fixture::new();
        std::fs::create_dir_all(fix.runs_root.join("empty")).unwrap();
        let timeline = fix.collect(None);
        assert!(timeline.rows.is_empty());

        let gone = fix.runs_root.join("absent");
        assert!(run_ids_in(&gone).unwrap().is_empty());
    }

    #[test]
    fn blank_lines_are_skipped() {
        let fix = Fixture::new();
        let dir = fix.runs_root.join("aa01");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("audit.jsonl"), "\n\n").unwrap();
        let timeline = fix.collect(None);
        assert!(timeline.rows.is_empty());
    }

    #[test]
    fn a_non_object_line_is_skipped_with_a_warning_not_an_error() {
        let fix = Fixture::new();
        let dir = fix.runs_root.join("aa01");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("audit.jsonl"), "42\n").unwrap();
        let timeline = fix.collect(None);
        assert!(timeline.rows.is_empty(), "a non-object line yields no row");
        assert!(
            timeline
                .warnings
                .iter()
                .any(|w| w.contains("not a JSON object") && w.contains("not shown")),
            "{:?}",
            timeline.warnings
        );
    }

    #[test]
    fn a_malformed_line_is_skipped_with_a_line_numbered_warning() {
        let fix = Fixture::new();
        let dir = fix.runs_root.join("aa01");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("audit.jsonl"), "not json\n").unwrap();
        let timeline = fix.collect(None);
        assert!(timeline.rows.is_empty());
        assert!(
            timeline
                .warnings
                .iter()
                .any(|w| w.contains("line 1") && w.contains("not shown")),
            "{:?}",
            timeline.warnings
        );
    }

    #[test]
    fn a_bad_line_is_skipped_while_the_good_lines_in_the_same_run_still_list() {
        let fix = Fixture::new();
        let dir = fix.runs_root.join("aa01");
        std::fs::create_dir_all(&dir).unwrap();
        let (mut payload, anchor) = chained(&[egress(
            "aa01",
            "2026-06-29T14:02:00Z",
            "http://api.example.test:443/",
        )]);
        payload.push_str("this is not json\n");
        std::fs::write(dir.join("audit.jsonl"), payload).unwrap();
        std::fs::write(dir.join("audit.anchor"), anchor.to_line()).unwrap();

        let timeline = fix.collect(None);

        assert_eq!(timeline.rows.len(), 1, "the good egress line still lists");
        assert_eq!(timeline.rows[0].kind, "egress");
        assert!(
            timeline
                .warnings
                .iter()
                .any(|w| w.contains("line 2") && w.contains("not shown")),
            "the unreadable second line is flagged: {:?}",
            timeline.warnings
        );
    }

    #[test]
    fn a_run_log_that_is_a_directory_surfaces_a_read_error() {
        let fix = Fixture::new();
        std::fs::create_dir_all(fix.runs_root.join("weird").join("audit.jsonl")).unwrap();
        let err = collect_timeline(&fix.runs_root, &fix.ledger_path, None).unwrap_err();
        assert!(format!("{err:#}").contains("reading audit log"), "{err:#}");
    }

    #[test]
    fn a_runs_root_that_is_a_file_surfaces_a_read_error() {
        let fix = Fixture::new();
        let not_a_dir = fix._home.path().join("afile");
        std::fs::write(&not_a_dir, "x").unwrap();
        let err = run_ids_in(&not_a_dir).unwrap_err();
        assert!(format!("{err:#}").contains("reading runs dir"), "{err:#}");
    }

    #[test]
    fn a_tampered_ledger_still_lists_events_but_warns() {
        let fix = Fixture::new();
        fix.write_ledger(&[connection("aa01", "2026-06-29T14:02:11Z")]);
        let good = std::fs::read_to_string(&fix.ledger_path).unwrap();
        std::fs::write(&fix.ledger_path, good.replacen("some-oauth", "tampered", 1)).unwrap();

        let timeline = fix.collect(None);

        assert_eq!(timeline.rows.len(), 1, "the event is still listed");
        assert!(
            timeline.warnings.iter().any(|w| w.contains("integrity")),
            "a tampered ledger warns: {:?}",
            timeline.warnings
        );
    }

    #[test]
    fn a_run_log_without_an_anchor_warns_about_the_missing_anchor() {
        let fix = Fixture::new();
        let dir = fix.runs_root.join("aa01");
        std::fs::create_dir_all(&dir).unwrap();
        let (payload, _anchor) = chained(&[egress("aa01", "2026-06-29T14:02:00Z", "http://x/")]);
        std::fs::write(dir.join("audit.jsonl"), payload).unwrap();

        let timeline = fix.collect(None);

        assert_eq!(timeline.rows.len(), 1);
        assert!(
            timeline.warnings.iter().any(|w| w.contains("no anchor")),
            "a non-empty log with no anchor warns: {:?}",
            timeline.warnings
        );
    }

    fn unreadable_event() -> String {
        r#"{"class_uid":1,"unmapped":{"lns_run":"aa01"}}"#.to_string()
    }

    #[test]
    fn an_unreadable_run_event_is_skipped_with_a_line_numbered_warning() {
        let fix = Fixture::new();
        let dir = fix.runs_root.join("aa01");
        std::fs::create_dir_all(&dir).unwrap();
        let (payload, anchor) = chained(&[unreadable_event()]);
        std::fs::write(dir.join("audit.jsonl"), payload).unwrap();
        std::fs::write(dir.join("audit.anchor"), anchor.to_line()).unwrap();

        let timeline = fix.collect(None);

        assert!(
            timeline.rows.is_empty(),
            "the unreadable event yields no row"
        );
        assert!(
            timeline
                .warnings
                .iter()
                .any(|w| w.contains("line 1") && w.contains("lns_kind")),
            "the missing-kind event is flagged by line: {:?}",
            timeline.warnings
        );
    }

    #[test]
    fn an_unreadable_ledger_event_is_skipped_with_a_warning_not_an_error() {
        let fix = Fixture::new();
        fix.write_ledger(&[unreadable_event()]);
        let timeline = fix.collect(None);
        assert!(
            timeline.rows.is_empty(),
            "the unreadable ledger event yields no row"
        );
        assert!(
            timeline
                .warnings
                .iter()
                .any(|w| w.contains("ledger") && w.contains("not shown")),
            "{:?}",
            timeline.warnings
        );
    }

    #[test]
    fn a_torn_ledger_line_is_skipped_while_the_good_ledger_events_still_list() {
        let fix = Fixture::new();
        let (mut payload, anchor) = chained(&[connection("aa01", "2026-06-29T14:02:11Z")]);
        payload.push_str("torn-partial-append\n");
        std::fs::write(&fix.ledger_path, payload).unwrap();
        std::fs::write(
            fix.ledger_path.with_file_name("ledger.anchor"),
            anchor.to_line(),
        )
        .unwrap();

        let timeline = fix.collect(None);

        assert_eq!(timeline.rows.len(), 1, "the good ledger event still lists");
        assert_eq!(timeline.rows[0].kind, "connection");
        assert!(
            timeline
                .warnings
                .iter()
                .any(|w| w.contains("ledger") && w.contains("not shown")),
            "a torn ledger line is flagged, not fatal for every sandbox: {:?}",
            timeline.warnings
        );
    }

    #[test]
    fn run_microvm_identity_reads_the_name_and_ts_of_the_first_readable_event() {
        let fix = Fixture::new();
        fix.write_run(
            "aa01",
            &[egress("aa01", "2026-06-29T14:02:00Z", "http://x/")],
        );
        let (name, ts) = run_microvm_identity(&fix.runs_root, "aa01").expect("identity");
        assert_eq!(name, "calm-finch");
        assert_eq!(ts, "2026-06-29T14:02:00Z");
    }

    #[test]
    fn run_microvm_identity_skips_unreadable_leading_lines() {
        let fix = Fixture::new();
        let dir = fix.runs_root.join("aa01");
        std::fs::create_dir_all(&dir).unwrap();
        let mut body = String::from("garbage-not-json\n\n");
        body.push_str(&unreadable_event());
        body.push('\n');
        let (good, _anchor) = chained(&[egress("aa01", "2026-06-29T14:02:00Z", "http://x/")]);
        body.push_str(&good);
        std::fs::write(dir.join("audit.jsonl"), body).unwrap();

        let (name, ts) = run_microvm_identity(&fix.runs_root, "aa01").expect("identity");
        assert_eq!(name, "calm-finch");
        assert_eq!(ts, "2026-06-29T14:02:00Z");
    }

    #[test]
    fn run_microvm_identity_is_none_for_a_missing_or_wholly_unreadable_log() {
        let fix = Fixture::new();
        assert!(run_microvm_identity(&fix.runs_root, "absent").is_none());
        let dir = fix.runs_root.join("aa01");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("audit.jsonl"), "not json\n\n").unwrap();
        assert!(run_microvm_identity(&fix.runs_root, "aa01").is_none());
    }
}
