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
            name: lns_ipc::short_run_id(&row.run).to_string(),
            status: String::new(),
        });
    }
    out
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
    fn a_run_seen_only_in_the_timeline_is_added_with_a_short_id_name() {
        let merged = merge_sandboxes(&[], &[row("1a2b3c4d0000000000000000000000bb")]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "1a2b3c4d0000000000000000000000bb");
        assert_eq!(merged[0].name, "1a2b3c4d0000");
        assert_eq!(merged[0].status, "");
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
