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

/// What a run decided about a connector. Connecting is not one: a connection is the machine's, held by no run, so no run's timeline could account for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorVerb {
    Granted,
    Declined,
    Forgotten,
}

impl ConnectorVerb {
    pub fn word(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Declined => "declined",
            Self::Forgotten => "forgot",
        }
    }
}

/// Who answered a connector question: the card the tray holds, a terminal prompt, or `--yes` on the grant command. Only the wire carries it, because the service cannot tell one caller from another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerSource {
    Card,
    Terminal,
    Flag,
}

impl AnswerSource {
    pub fn word(self) -> &'static str {
        match self {
            Self::Card => "card",
            Self::Terminal => "terminal",
            Self::Flag => "flag",
        }
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
    Connector {
        connector: String,
        verb: ConnectorVerb,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        method: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connection: Option<String>,
        /// The bytes the grant bound to; a decline answers for every version and a forget clears whatever was there, so neither carries one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        digest: Option<String>,
        /// How the grant was answered. A forget answers no question, and a line written before this field read without one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answered_by: Option<AnswerSource>,
    },
}

impl LedgerEvent {
    pub fn name(&self) -> &'static str {
        match self {
            LedgerEvent::Approval { .. } => "approval",
            LedgerEvent::Connector { .. } => "connector",
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
    fn a_connector_decision_round_trips_through_json() {
        let back = round_trip(LedgerEvent::Connector {
            connector: "some-provider".into(),
            verb: ConnectorVerb::Granted,
            method: Some("token".into()),
            connection: Some("work".into()),
            digest: Some("sha256:abc".into()),
            answered_by: Some(AnswerSource::Flag),
        });
        assert_eq!(back.event.name(), "connector");
    }

    #[test]
    fn a_line_written_before_the_answer_source_reads_back_without_one() {
        let line = r#"{"ts":"2026-06-29T14:02:11Z","run":"5e6f7a8b0000000000000000000000bb","microvm":"calm-finch","event":"connector","connector":"some-provider","verb":"granted","method":"token"}"#;
        let back: LedgerRecord = serde_json::from_str(line).expect("deserialize");
        assert_eq!(
            back.event,
            LedgerEvent::Connector {
                connector: "some-provider".into(),
                verb: ConnectorVerb::Granted,
                method: Some("token".into()),
                connection: None,
                digest: None,
                answered_by: None,
            }
        );
    }

    #[test]
    fn every_answer_source_has_a_word_the_timeline_reads() {
        for (source, word) in [
            (AnswerSource::Card, "card"),
            (AnswerSource::Terminal, "terminal"),
            (AnswerSource::Flag, "flag"),
        ] {
            assert_eq!(source.word(), word);
            assert_eq!(
                serde_json::to_value(source).expect("serialize"),
                serde_json::Value::String(word.to_string()),
                "the wire spells it the way the timeline reads it"
            );
        }
    }

    #[test]
    fn a_decision_that_names_no_method_or_connection_writes_neither() {
        let record = LedgerRecord {
            ts: "2026-06-29T14:02:11Z".into(),
            run: "5e6f7a8b0000000000000000000000bb".into(),
            microvm: "calm-finch".into(),
            event: LedgerEvent::Connector {
                connector: "some-provider".into(),
                verb: ConnectorVerb::Forgotten,
                method: None,
                connection: None,
                digest: None,
                answered_by: None,
            },
        };
        let line = serde_json::to_string(&record).expect("serialize");
        assert!(!line.contains("method"), "{line}");
        assert!(!line.contains("digest"), "{line}");
        assert!(!line.contains("answered_by"), "{line}");
        assert_eq!(
            serde_json::from_str::<LedgerRecord>(&line).expect("deserialize"),
            record
        );
    }

    #[test]
    fn every_verb_has_a_word_the_timeline_reads() {
        for (verb, word) in [
            (ConnectorVerb::Granted, "granted"),
            (ConnectorVerb::Declined, "declined"),
            (ConnectorVerb::Forgotten, "forgot"),
        ] {
            assert_eq!(verb.word(), word);
        }
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
