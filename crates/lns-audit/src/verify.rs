use anyhow::{Context, Result};
use lns_ipc::{Anchor, GENESIS_PREV_HASH, hex_encode};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Debug)]
pub enum VerifyOutcome {
    Ok { line_count: usize },
    NoAnchor { line_count: usize },
    AnchorUnreadable { line_count: usize, reason: String },
    Broken { at_line: usize, reason: String },
    Truncated { reason: String },
}

enum AnchorState {
    Present(Anchor),
    Absent,
    Unreadable(String),
}

fn anchor_path_for(path: &Path) -> std::path::PathBuf {
    path.with_file_name("audit.anchor")
}

fn read_anchor_state(path: &Path) -> AnchorState {
    match std::fs::read_to_string(path) {
        Ok(text) => match Anchor::parse(&text) {
            Ok(anchor) => AnchorState::Present(anchor),
            Err(e) => AnchorState::Unreadable(format!("corrupt anchor: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => AnchorState::Absent,
        Err(e) => AnchorState::Unreadable(e.to_string()),
    }
}

struct ChainWalk {
    head: String,
    line_count: u64,
}

fn walk_chain(path: &Path) -> Result<std::result::Result<ChainWalk, VerifyOutcome>> {
    let f = File::open(path).with_context(|| format!("opening audit log {}", path.display()))?;
    let reader = BufReader::new(f);

    let mut head = GENESIS_PREV_HASH.to_string();
    let mut idx = 0usize;
    let mut content_lines = 0u64;
    for line_result in reader.lines() {
        idx += 1;
        let line =
            line_result.with_context(|| format!("reading line {idx} of {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        content_lines += 1;
        if let Some(broken) = check_line(&line, idx, &head) {
            return Ok(Err(broken));
        }
        head = hex_encode(&Sha256::digest(line.as_bytes()));
    }
    Ok(Ok(ChainWalk {
        head,
        line_count: content_lines,
    }))
}

fn check_line(line: &str, idx: usize, expected_prev: &str) -> Option<VerifyOutcome> {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(VerifyOutcome::Broken {
                at_line: idx,
                reason: format!("invalid JSON: {e}"),
            });
        }
    };
    let obj = match value.as_object() {
        Some(o) => o,
        None => {
            return Some(VerifyOutcome::Broken {
                at_line: idx,
                reason: "line is not a JSON object".into(),
            });
        }
    };
    let prev_hash = match obj.get("prev_hash").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => {
            return Some(VerifyOutcome::Broken {
                at_line: idx,
                reason: "missing `prev_hash` field".into(),
            });
        }
    };
    if prev_hash != expected_prev {
        return Some(VerifyOutcome::Broken {
            at_line: idx,
            reason: format!(
                "prev_hash mismatch — line claims {prev_hash}, chain expected {expected_prev}"
            ),
        });
    }
    None
}

pub fn verify_chain(path: &Path) -> Result<VerifyOutcome> {
    verify_chain_with_anchor(path, &anchor_path_for(path))
}

pub fn verify_chain_with_anchor(path: &Path, anchor_path: &Path) -> Result<VerifyOutcome> {
    verify_chain_against_anchor(path, read_anchor_state(anchor_path))
}

fn verify_chain_against_anchor(path: &Path, anchor: AnchorState) -> Result<VerifyOutcome> {
    let walk = match walk_chain(path)? {
        Ok(w) => w,
        Err(broken) => return Ok(broken),
    };
    let line_count = walk.line_count as usize;
    Ok(match anchor {
        AnchorState::Present(anchor) => {
            check_anchor(&walk, &anchor).unwrap_or(VerifyOutcome::Ok { line_count })
        }
        AnchorState::Absent => VerifyOutcome::NoAnchor { line_count },
        AnchorState::Unreadable(reason) => VerifyOutcome::AnchorUnreadable { line_count, reason },
    })
}

fn check_anchor(walk: &ChainWalk, anchor: &Anchor) -> Option<VerifyOutcome> {
    if walk.line_count < anchor.line_count {
        return Some(VerifyOutcome::Truncated {
            reason: format!(
                "log holds {} events but the anchor recorded {} — the tail was truncated or rolled back",
                walk.line_count, anchor.line_count
            ),
        });
    }
    if walk.line_count != anchor.line_count || walk.head != anchor.head_hash {
        return Some(VerifyOutcome::Truncated {
            reason: format!(
                "log head {} over {} events does not match the anchored head {} over {} events",
                walk.head, walk.line_count, anchor.head_hash, anchor.line_count
            ),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_ipc::AuditChain;

    fn write_chain_to(path: &Path, events: &[&str]) -> Anchor {
        let mut c = AuditChain::new();
        let mut payload = String::new();
        for e in events {
            let bytes = c.augment(e).unwrap();
            payload.push_str(std::str::from_utf8(&bytes).unwrap());
            payload.push('\n');
        }
        std::fs::write(path, payload).unwrap();
        c.anchor().expect("chain has at least one event")
    }

    fn write_anchor_beside(path: &Path, anchor: &Anchor) {
        std::fs::write(anchor_path_for(path), anchor.to_line()).unwrap();
    }

    fn expect_ok(outcome: VerifyOutcome) -> usize {
        let VerifyOutcome::Ok { line_count } = outcome else {
            panic!("expected Ok, got {outcome:?}")
        };
        line_count
    }

    fn expect_broken(outcome: VerifyOutcome) -> (usize, String) {
        let VerifyOutcome::Broken { at_line, reason } = outcome else {
            panic!("expected Broken, got {outcome:?}")
        };
        (at_line, reason)
    }

    fn expect_truncated(outcome: VerifyOutcome) -> String {
        let VerifyOutcome::Truncated { reason } = outcome else {
            panic!("expected Truncated, got {outcome:?}")
        };
        reason
    }

    #[test]
    #[should_panic(expected = "expected Ok")]
    fn expect_ok_panics_on_broken() {
        let _ = expect_ok(VerifyOutcome::Broken {
            at_line: 1,
            reason: "x".into(),
        });
    }

    #[test]
    #[should_panic(expected = "expected Broken")]
    fn expect_broken_panics_on_ok() {
        let _ = expect_broken(VerifyOutcome::Ok { line_count: 0 });
    }

    #[test]
    #[should_panic(expected = "expected Truncated")]
    fn expect_truncated_panics_on_ok() {
        let _ = expect_truncated(VerifyOutcome::Ok { line_count: 0 });
    }

    #[test]
    fn good_chain_reports_ok_with_count() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        let anchor = write_chain_to(
            &path,
            &[
                r#"{"type":"audit_event","seq":1}"#,
                r#"{"type":"audit_event","seq":2}"#,
            ],
        );
        write_anchor_beside(&path, &anchor);
        assert_eq!(expect_ok(verify_chain(&path).unwrap()), 2);
    }

    #[test]
    fn missing_prev_hash_is_reported_per_line() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        std::fs::write(&path, r#"{"type":"audit_event"}"#).unwrap();
        let (at_line, reason) = expect_broken(verify_chain(&path).unwrap());
        assert_eq!(at_line, 1);
        assert!(reason.contains("prev_hash"));
    }

    #[test]
    fn invalid_json_is_reported() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        std::fs::write(&path, "not json\n").unwrap();
        let (at_line, reason) = expect_broken(verify_chain(&path).unwrap());
        assert_eq!(at_line, 1);
        assert!(reason.to_lowercase().contains("invalid json"));
    }

    #[test]
    fn non_object_json_is_reported() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        std::fs::write(&path, "42\n").unwrap();
        let (at_line, reason) = expect_broken(verify_chain(&path).unwrap());
        assert_eq!(at_line, 1);
        assert!(reason.contains("not a JSON object"));
    }

    #[test]
    fn an_empty_log_without_an_anchor_reports_no_anchor() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        std::fs::write(&path, "").unwrap();
        let outcome = verify_chain(&path).unwrap();
        assert!(
            matches!(outcome, VerifyOutcome::NoAnchor { line_count: 0 }),
            "an empty log with no anchor must report NoAnchor: {outcome:?}"
        );
    }

    #[test]
    fn truncation_below_the_anchored_line_count_is_detected() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        let anchor = write_chain_to(
            &path,
            &[
                r#"{"type":"audit_event","seq":1}"#,
                r#"{"type":"audit_event","seq":2}"#,
                r#"{"type":"audit_event","seq":3}"#,
            ],
        );
        write_anchor_beside(&path, &anchor);

        let full = std::fs::read_to_string(&path).unwrap();
        let mut kept: Vec<&str> = full.lines().take(2).collect();
        kept.push("");
        std::fs::write(&path, kept.join("\n")).unwrap();

        let reason = expect_truncated(verify_chain(&path).unwrap());
        assert!(
            reason.contains("truncated") || reason.contains("rolled back"),
            "truncating below the anchored count must be reported as truncation: {reason}"
        );
    }

    #[test]
    fn whole_log_rollback_to_empty_is_detected_against_the_anchor() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        let anchor = write_chain_to(
            &path,
            &[
                r#"{"type":"audit_event","seq":1}"#,
                r#"{"type":"audit_event","seq":2}"#,
            ],
        );
        write_anchor_beside(&path, &anchor);
        std::fs::write(&path, "").unwrap();

        let reason = expect_truncated(verify_chain(&path).unwrap());
        assert!(reason.contains("truncated") || reason.contains("rolled back"));
    }

    #[test]
    fn anchor_head_mismatch_at_the_same_count_is_detected_against_the_anchor() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        let anchor = write_chain_to(
            &path,
            &[
                r#"{"type":"audit_event","seq":1}"#,
                r#"{"type":"audit_event","seq":2}"#,
            ],
        );
        let forged = Anchor {
            head_hash: "f".repeat(64),
            line_count: anchor.line_count,
        };
        write_anchor_beside(&path, &forged);

        let reason = expect_truncated(verify_chain(&path).unwrap());
        assert!(
            reason.contains("does not match the anchored head"),
            "a head divergence at the same length must surface as a mismatch: {reason}"
        );
    }

    #[test]
    fn intact_chain_matching_its_anchor_verifies_ok() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        let anchor = write_chain_to(
            &path,
            &[
                r#"{"type":"audit_event","seq":1}"#,
                r#"{"type":"audit_event","seq":2}"#,
            ],
        );
        write_anchor_beside(&path, &anchor);
        assert_eq!(expect_ok(verify_chain(&path).unwrap()), 2);
    }

    #[test]
    fn a_missing_anchor_is_surfaced_as_no_anchor_not_clean_ok() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        write_chain_to(&path, &[r#"{"type":"audit_event","seq":1}"#]);
        let outcome = verify_chain(&path).unwrap();
        assert!(
            matches!(outcome, VerifyOutcome::NoAnchor { line_count: 1 }),
            "a chain with no anchor beside it must report NoAnchor: {outcome:?}"
        );
    }

    #[test]
    fn a_corrupt_anchor_file_is_surfaced_as_anchor_unreadable() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        write_chain_to(&path, &[r#"{"type":"audit_event","seq":1}"#]);
        std::fs::write(anchor_path_for(&path), "not-an-anchor").unwrap();
        let outcome = verify_chain(&path).unwrap();
        assert!(
            matches!(&outcome, VerifyOutcome::AnchorUnreadable { line_count: 1, reason } if reason.contains("corrupt")),
            "a garbage anchor must report AnchorUnreadable, never clean Ok: {outcome:?}"
        );
    }

    #[test]
    fn an_unreadable_anchor_io_error_is_surfaced_as_anchor_unreadable() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        write_chain_to(&path, &[r#"{"type":"audit_event","seq":1}"#]);
        std::fs::create_dir(anchor_path_for(&path)).unwrap();
        let outcome = verify_chain(&path).unwrap();
        let unreadable = matches!(
            &outcome,
            VerifyOutcome::AnchorUnreadable { line_count: 1, .. }
        );
        assert!(
            unreadable,
            "an anchor path that cannot be read must report AnchorUnreadable: {outcome:?}"
        );
    }

    #[test]
    fn anchor_match_function_accepts_an_exact_match() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        let anchor = write_chain_to(&path, &[r#"{"type":"audit_event","seq":1}"#]);
        let walk = walk_chain(&path).unwrap().unwrap();
        assert!(check_anchor(&walk, &anchor).is_none());
    }

    #[test]
    fn a_tampered_middle_line_breaks_the_chain_at_the_next_line() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        let mut c = AuditChain::new();
        let mut payload = String::new();
        for seq in 1..=3 {
            let line = format!(r#"{{"type":"audit_event","seq":{seq}}}"#);
            let aug = c.augment(&line).unwrap();
            payload.push_str(std::str::from_utf8(&aug).unwrap());
            payload.push('\n');
        }
        let tampered = payload.replacen("\"seq\":2", "\"seq\":99", 1);
        assert_ne!(tampered, payload);
        std::fs::write(&path, tampered).unwrap();
        let (at_line, reason) = expect_broken(verify_chain(&path).unwrap());
        assert_eq!(at_line, 3);
        assert!(reason.contains("prev_hash mismatch"));
    }

    #[test]
    fn an_explicit_anchor_path_lets_the_ledger_be_verified_against_its_own_anchor() {
        let d = tempfile::TempDir::new().unwrap();
        let ledger = d.path().join("ledger.jsonl");
        let anchor = write_chain_to(&ledger, &[r#"{"type":"audit_event","seq":1}"#]);
        let anchor_path = d.path().join("ledger.anchor");
        std::fs::write(&anchor_path, anchor.to_line()).unwrap();
        assert_eq!(
            expect_ok(verify_chain_with_anchor(&ledger, &anchor_path).unwrap()),
            1
        );
    }

    #[test]
    fn blank_lines_skipped_and_only_content_lines_counted() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        let mut c = AuditChain::new();
        let line1 = c.augment(r#"{"a":1}"#).unwrap();
        let line2 = c.augment(r#"{"a":2}"#).unwrap();
        let mut payload = String::new();
        payload.push_str(std::str::from_utf8(&line1).unwrap());
        payload.push('\n');
        payload.push('\n');
        payload.push_str(std::str::from_utf8(&line2).unwrap());
        payload.push('\n');
        std::fs::write(&path, payload).unwrap();
        write_anchor_beside(&path, &c.anchor().expect("chain has events"));
        assert_eq!(expect_ok(verify_chain(&path).unwrap()), 2);
    }
}
