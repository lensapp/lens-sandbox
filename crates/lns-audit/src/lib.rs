mod collect;
mod ocsf;
mod store;
pub mod verify;

pub use collect::{Timeline, TimelineRow, collect_timeline, run_ids_in, run_microvm_identity};
pub use ocsf::{Row, microvm, read};
pub use store::{LedgerStream, stream_ledger};
pub use verify::{VerifyOutcome, verify_chain, verify_chain_with_anchor};

pub fn friendly_when(ts: &str) -> String {
    ts.trim_end_matches('Z').replacen('T', " ", 1)
}

pub fn integrity_advisory(outcome: &VerifyOutcome) -> Option<String> {
    match outcome {
        VerifyOutcome::Ok { .. } => None,
        VerifyOutcome::Broken { at_line, reason } => Some(format!(
            "audit integrity: chain broken at line {at_line} ({reason}) — entries shown may have been altered"
        )),
        VerifyOutcome::Truncated { reason } => Some(format!(
            "audit integrity: log truncated or rolled back ({reason}) — entries may be missing"
        )),
        VerifyOutcome::AnchorUnreadable { reason, .. } => Some(format!(
            "audit integrity: anchor unreadable ({reason}) — the log cannot be confirmed intact"
        )),
        VerifyOutcome::NoAnchor { line_count: 0 } => None,
        VerifyOutcome::NoAnchor { .. } => Some(
            "audit integrity: no anchor beside the log — truncation or rollback cannot be detected"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_when_strips_the_zulu_marker_and_spaces_the_date() {
        assert_eq!(friendly_when("2026-06-29T14:02:11Z"), "2026-06-29 14:02:11");
        assert_eq!(friendly_when(""), "");
    }

    #[test]
    fn integrity_advisory_is_silent_only_for_an_intact_chain() {
        assert_eq!(
            integrity_advisory(&VerifyOutcome::Ok { line_count: 3 }),
            None
        );
        let broken = integrity_advisory(&VerifyOutcome::Broken {
            at_line: 7,
            reason: "prev_hash mismatch".into(),
        })
        .expect("a broken chain advises");
        assert!(
            broken.contains("line 7") && broken.contains("altered"),
            "{broken}"
        );
        assert!(
            integrity_advisory(&VerifyOutcome::Truncated {
                reason: "tail".into()
            })
            .expect("truncation advises")
            .contains("truncated")
        );
        assert!(
            integrity_advisory(&VerifyOutcome::AnchorUnreadable {
                line_count: 2,
                reason: "corrupt".into(),
            })
            .expect("an unreadable anchor advises")
            .contains("anchor unreadable")
        );
        assert!(
            integrity_advisory(&VerifyOutcome::NoAnchor { line_count: 2 })
                .expect("a missing anchor advises")
                .contains("no anchor")
        );
    }

    #[test]
    fn integrity_advisory_ignores_a_missing_anchor_only_when_the_log_is_empty() {
        assert_eq!(
            integrity_advisory(&VerifyOutcome::NoAnchor { line_count: 0 }),
            None,
            "a zero-event log has nothing to protect and nothing truncatable; a wiped non-empty log is caught by Truncated against its surviving anchor"
        );
        assert!(
            integrity_advisory(&VerifyOutcome::NoAnchor { line_count: 1 })
                .expect("a non-empty log with no anchor still advises")
                .contains("no anchor"),
            "content with no checkpoint is still a real integrity gap"
        );
    }
}
