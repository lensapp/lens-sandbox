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
    Broken { at_line: usize, reason: String },
    Truncated { reason: String },
}

fn anchor_path_for(path: &Path) -> std::path::PathBuf {
    path.with_file_name("audit.anchor")
}

fn read_anchor(path: &Path) -> Option<Anchor> {
    Anchor::parse(&std::fs::read_to_string(path).ok()?).ok()
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
    verify_chain_against_anchor(path, read_anchor(&anchor_path_for(path)))
}

fn verify_chain_against_anchor(path: &Path, anchor: Option<Anchor>) -> Result<VerifyOutcome> {
    let walk = match walk_chain(path)? {
        Ok(w) => w,
        Err(broken) => return Ok(broken),
    };
    if let Some(anchor) = anchor
        && let Some(truncated) = check_anchor(&walk, &anchor)
    {
        return Ok(truncated);
    }
    Ok(VerifyOutcome::Ok {
        line_count: walk.line_count as usize,
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
        write_chain_to(
            &path,
            &[
                r#"{"type":"audit_event","seq":1}"#,
                r#"{"type":"audit_event","seq":2}"#,
            ],
        );
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
    fn empty_file_is_ok() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        std::fs::write(&path, "").unwrap();
        assert_eq!(expect_ok(verify_chain(&path).unwrap()), 0);
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
    fn a_garbage_anchor_file_is_ignored_and_the_chain_still_verifies() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("audit.jsonl");
        write_chain_to(&path, &[r#"{"type":"audit_event","seq":1}"#]);
        std::fs::write(anchor_path_for(&path), "not-an-anchor").unwrap();
        assert_eq!(expect_ok(verify_chain(&path).unwrap()), 1);
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
        assert_eq!(expect_ok(verify_chain(&path).unwrap()), 2);
    }
}
