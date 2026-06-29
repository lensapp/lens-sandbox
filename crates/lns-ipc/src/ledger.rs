use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    Oauth,
    Apikey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    Network,
    Credential,
    Integration,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        integration: Option<String>,
    },
    Connection {
        integration: String,
        auth: AuthKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        scopes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires: Option<String>,
    },
    CredentialUse {
        integration: String,
        auth: AuthKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fp: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        dest: Vec<String>,
    },
}

impl LedgerEvent {
    pub fn integration(&self) -> Option<&str> {
        match self {
            LedgerEvent::Approval { integration, .. } => integration.as_deref(),
            LedgerEvent::Connection { integration, .. }
            | LedgerEvent::CredentialUse { integration, .. } => Some(integration),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            LedgerEvent::Approval { .. } => "approval",
            LedgerEvent::Connection { .. } => "connection",
            LedgerEvent::CredentialUse { .. } => "credential_use",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerRecord {
    pub ts: String,
    pub run: u32,
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
            run: 49,
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
            integration: None,
        });
        assert_eq!(back.event.name(), "approval");
        assert_eq!(back.event.integration(), None);
    }

    #[test]
    fn connection_event_round_trips_with_oauth_detail() {
        let back = round_trip(LedgerEvent::Connection {
            integration: "github".into(),
            auth: AuthKind::Oauth,
            account: Some("@hchen".into()),
            scopes: vec!["repo".into(), "read:org".into()],
            expires: Some("2026-07-29T00:00:00Z".into()),
        });
        assert_eq!(back.event.name(), "connection");
        assert_eq!(back.event.integration(), Some("github"));
    }

    #[test]
    fn credential_use_event_round_trips_with_fingerprint() {
        let back = round_trip(LedgerEvent::CredentialUse {
            integration: "open-router".into(),
            auth: AuthKind::Apikey,
            fp: Some("9c2f1a3d".into()),
            dest: vec!["api.openrouter.ai".into()],
        });
        assert_eq!(back.event.name(), "credential_use");
        assert_eq!(back.event.integration(), Some("open-router"));
    }

    #[test]
    fn a_chain_augmented_line_still_deserializes_into_a_record() {
        let record = LedgerRecord {
            ts: "2026-06-29T14:05:30Z".into(),
            run: 49,
            microvm: "calm-finch".into(),
            event: LedgerEvent::CredentialUse {
                integration: "open-router".into(),
                auth: AuthKind::Apikey,
                fp: Some("9c2f1a3d".into()),
                dest: vec!["api.openrouter.ai".into()],
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

    #[test]
    fn auth_and_approval_kinds_serialize_snake_case() {
        assert_eq!(
            serde_json::to_string(&AuthKind::Apikey).unwrap(),
            "\"apikey\""
        );
        assert_eq!(
            serde_json::to_string(&ApprovalKind::Credential).unwrap(),
            "\"credential\""
        );
    }
}
