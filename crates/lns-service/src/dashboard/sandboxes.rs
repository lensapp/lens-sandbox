use std::collections::HashSet;

use lns_audit::TimelineRow;

use super::Sandbox;

pub fn merge_sandboxes(active: &[Sandbox], rows: &[TimelineRow]) -> Vec<Sandbox> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<Sandbox> = Vec::new();
    for sb in active {
        if seen.insert(sb.id.as_str()) {
            out.push(sb.clone());
        }
    }
    for row in rows {
        if row.run.is_empty() || !seen.insert(row.run.as_str()) {
            continue;
        }
        out.push(Sandbox {
            id: row.run.clone(),
            name: historical_name(row),
            status: String::new(),
        });
    }
    out
}

/// A finished run keeps its auto-name in every OCSF event's `unmapped.lns_microvm`; fall back to a short run id only when the row carries no name.
fn historical_name(row: &TimelineRow) -> String {
    row.raw
        .as_object()
        .map(lns_audit::microvm)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| lns_ipc::short_run_id(&row.run).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn row(run: &str) -> TimelineRow {
        TimelineRow {
            ts: "2026-06-29T14:00:00Z".into(),
            when: "2026-06-29 14:00:00".into(),
            run: run.into(),
            kind: "egress".into(),
            detail: String::new(),
            raw: Value::Null,
            integration: None,
        }
    }

    fn row_named(run: &str, microvm: &str) -> TimelineRow {
        TimelineRow {
            raw: serde_json::json!({
                "unmapped": {"lns_run": run, "lns_microvm": microvm, "lns_kind": "egress"}
            }),
            ..row(run)
        }
    }

    fn active(id: &str, name: &str, status: &str) -> Sandbox {
        Sandbox {
            id: id.into(),
            name: name.into(),
            status: status.into(),
        }
    }

    #[test]
    fn active_sandboxes_lead_and_keep_their_name_and_status() {
        let merged = merge_sandboxes(
            &[active(
                "9e8d7c6b0000000000000000000000aa",
                "calm-finch",
                "running",
            )],
            &[row("9e8d7c6b0000000000000000000000aa")],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "calm-finch");
        assert_eq!(merged[0].status, "running");
    }

    #[test]
    fn a_finished_run_keeps_its_auto_name_from_the_audit_trail() {
        let merged = merge_sandboxes(
            &[],
            &[row_named("1a2b3c4d0000000000000000000000bb", "calm-finch")],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "1a2b3c4d0000000000000000000000bb");
        assert_eq!(
            merged[0].name, "calm-finch",
            "a run no longer in the registry still reads by its auto-name, not a truncated id"
        );
        assert_eq!(merged[0].status, "");
    }

    #[test]
    fn a_timeline_row_without_a_name_falls_back_to_a_short_id() {
        let no_microvm = TimelineRow {
            raw: serde_json::json!({"unmapped": {"lns_run": "x"}}),
            ..row("1a2b3c4d0000000000000000000000bb")
        };
        let merged = merge_sandboxes(&[], &[row("5e6f7a8b0000000000000000000000cc"), no_microvm]);
        assert_eq!(merged[0].name, "5e6f7a8b0000", "no raw object → short id");
        assert_eq!(
            merged[1].name, "1a2b3c4d0000",
            "raw present but no lns_microvm → short id"
        );
    }

    #[test]
    fn a_run_repeated_across_rows_appears_once_and_after_the_active_set() {
        let merged = merge_sandboxes(
            &[active("aaaa1111", "bold-otter", "exited")],
            &[
                row("bbbb2222"),
                row("bbbb2222"),
                row("aaaa1111"),
                row("cccc3333"),
            ],
        );
        let ids: Vec<&str> = merged.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["aaaa1111", "bbbb2222", "cccc3333"]);
    }

    #[test]
    fn a_blank_run_id_never_becomes_a_sandbox() {
        assert!(merge_sandboxes(&[], &[row("")]).is_empty());
    }
}
