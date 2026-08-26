use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    Network,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    AllowOnce,
    AllowAlways,
    DenyOnce,
    DenyAlways,
    Allow,
    Deny,
}

impl Decision {
    pub fn is_allow(self) -> bool {
        matches!(
            self,
            Decision::AllowOnce | Decision::AllowAlways | Decision::Allow
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum LedgerEvent {
    Approval {
        kind: ApprovalKind,
        target: String,
        decision: Decision,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl LedgerEvent {
    pub fn name(&self) -> &'static str {
        match self {
            LedgerEvent::Approval { .. } => "approval",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerRecord {
    pub ts: String,
    pub run: String,
    pub microvm: String,
    #[serde(flatten)]
    pub event: LedgerEvent,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AuditChain;

    fn round_trip(event: LedgerEvent) -> LedgerRecord {
        let record = LedgerRecord {
            ts: "2026-06-29T14:02:11Z".into(),
            run: "5e6f7a8b0000000000000000000000bb".into(),
            microvm: "calm-finch".into(),
            event,
        };
        let line = serde_json::to_string(&record).expect("serialize");
        let back: LedgerRecord = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(back, record);
        back
    }

    #[test]
    fn approval_event_round_trips_through_json() {
        let back = round_trip(LedgerEvent::Approval {
            kind: ApprovalKind::Network,
            target: "api.foo.com:443".into(),
            decision: Decision::AllowAlways,
            reason: Some("policy-ambiguous".into()),
        });
        assert_eq!(back.event.name(), "approval");
    }

    #[test]
    fn a_chain_augmented_line_still_deserializes_into_a_record() {
        let record = LedgerRecord {
            ts: "2026-06-29T14:05:30Z".into(),
            run: "5e6f7a8b0000000000000000000000bb".into(),
            microvm: "calm-finch".into(),
            event: LedgerEvent::Approval {
                kind: ApprovalKind::Network,
                target: "api.example.test:443".into(),
                decision: Decision::AllowAlways,
                reason: None,
            },
        };
        let mut chain = AuditChain::new();
        let augmented = chain
            .augment(&serde_json::to_string(&record).unwrap())
            .expect("a record is a JSON object");
        let line = std::str::from_utf8(&augmented).unwrap();
        assert!(line.contains("\"prev_hash\""));
        let back: LedgerRecord = serde_json::from_str(line).expect("prev_hash is ignored on read");
        assert_eq!(back, record);
    }

    #[test]
    fn decision_classifies_allow_variants() {
        for d in [Decision::AllowOnce, Decision::AllowAlways, Decision::Allow] {
            assert!(d.is_allow(), "{d:?} should be an allow");
        }
        for d in [Decision::DenyOnce, Decision::DenyAlways, Decision::Deny] {
            assert!(!d.is_allow(), "{d:?} should not be an allow");
        }
    }
}
