use std::collections::HashMap;

use lns_audit::TimelineRow;

use super::Sandbox;

pub fn merge_sandboxes(active: &[Sandbox], rows: &[TimelineRow]) -> Vec<Sandbox> {
    let mut out: Vec<Sandbox> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for sb in active {
        index.entry(sb.id.as_str()).or_insert_with(|| {
            out.push(sb.clone());
            out.len() - 1
        });
    }
    for row in rows {
        if row.run.is_empty() {
            continue;
        }
        match index.get(row.run.as_str()) {
            Some(&i) => backfill_image(&mut out[i], row),
            None => {
                index.insert(row.run.as_str(), out.len());
                out.push(Sandbox {
                    id: row.run.clone(),
                    name: historical_name(row),
                    image: row_image(row),
                    status: String::new(),
                });
            }
        }
    }
    out
}

fn backfill_image(sandbox: &mut Sandbox, row: &TimelineRow) {
    if sandbox.image.is_empty() {
        sandbox.image = row_image(row);
    }
}

/// A finished run keeps its auto-name in every OCSF event's `unmapped.lns_microvm`; fall back to a short run id only when the row carries no name.
fn historical_name(row: &TimelineRow) -> String {
    row.raw
        .as_object()
        .map(lns_audit::microvm)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| lns_ipc::short_run_id(&row.run).to_string())
}

/// Only the per-run launch event carries the image; every other row yields an empty string, so the first non-empty one wins.
fn row_image(row: &TimelineRow) -> String {
    row.raw
        .as_object()
        .map(lns_audit::image)
        .unwrap_or_default()
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
            connector: None,
            raw: Value::Null,
        }
    }

    fn row_named(run: &str, microvm: &str) -> TimelineRow {
        TimelineRow {
            connector: None,
            raw: serde_json::json!({
                "unmapped": {"lns_run": run, "lns_microvm": microvm, "lns_kind": "egress"}
            }),
            ..row(run)
        }
    }

    fn row_launch(run: &str, microvm: &str, image: &str) -> TimelineRow {
        TimelineRow {
            connector: None,
            raw: serde_json::json!({
                "unmapped": {"lns_run": run, "lns_microvm": microvm, "lns_kind": "launch", "lns_image": image}
            }),
            ..row(run)
        }
    }

    fn active(id: &str, name: &str, image: &str, status: &str) -> Sandbox {
        Sandbox {
            id: id.into(),
            name: name.into(),
            image: image.into(),
            status: status.into(),
        }
    }

    #[test]
    fn active_sandboxes_lead_and_keep_their_name_status_and_image() {
        let merged = merge_sandboxes(
            &[active(
                "9e8d7c6b0000000000000000000000aa",
                "calm-finch",
                "alpine:latest",
                "running",
            )],
            &[row("9e8d7c6b0000000000000000000000aa")],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "calm-finch");
        assert_eq!(merged[0].image, "alpine:latest");
        assert_eq!(merged[0].status, "running");
    }

    #[test]
    fn a_finished_runs_image_is_recovered_from_its_launch_row() {
        let run = "1a2b3c4d0000000000000000000000bb";
        let merged = merge_sandboxes(
            &[],
            &[
                row_named(run, "calm-finch"),
                row_launch(run, "calm-finch", "ubuntu:24.04"),
            ],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].image, "ubuntu:24.04",
            "the image is backfilled from the launch row even though newer rows precede it"
        );
    }

    #[test]
    fn an_active_runs_registry_image_is_not_clobbered_by_a_launch_row() {
        let run = "9e8d7c6b0000000000000000000000aa";
        let merged = merge_sandboxes(
            &[active(run, "calm-finch", "alpine:3.20", "running")],
            &[row_launch(run, "calm-finch", "ubuntu:24.04")],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].image, "alpine:3.20",
            "an active run keeps the image the registry reported"
        );
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
            &[active("aaaa1111", "bold-otter", "", "exited")],
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
