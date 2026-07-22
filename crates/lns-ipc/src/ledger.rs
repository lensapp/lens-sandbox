use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A short, non-reversible identifier for a secret value — lets the ledger say "the same key was used across two runs" without ever recording the value.
pub fn fingerprint(value: &str) -> String {
    crate::hex_encode(&Sha256::digest(value.as_bytes()))[..12].to_string()
}

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
    Connector,
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
        connector: Option<String>,
    },
    Connection {
        connector: String,
        auth: AuthKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        scopes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires: Option<String>,
    },
    CredentialUse {
        connector: String,
        auth: AuthKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fp: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        dest: Vec<String>,
    },
}

impl LedgerEvent {
    pub fn connector(&self) -> Option<&str> {
        match self {
            LedgerEvent::Approval { connector, .. } => connector.as_deref(),
            LedgerEvent::Connection { connector, .. }
            | LedgerEvent::CredentialUse { connector, .. } => Some(connector),
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
            connector: None,
        });
        assert_eq!(back.event.name(), "approval");
        assert_eq!(back.event.connector(), None);
    }

    #[test]
    fn connection_event_round_trips_with_oauth_detail() {
        let back = round_trip(LedgerEvent::Connection {
            connector: "some-oauth".into(),
            auth: AuthKind::Oauth,
            account: Some("@hchen".into()),
            scopes: vec!["repo".into(), "read:org".into()],
            expires: Some("2026-07-29T00:00:00Z".into()),
        });
        assert_eq!(back.event.name(), "connection");
        assert_eq!(back.event.connector(), Some("some-oauth"));
    }

    #[test]
    fn credential_use_event_round_trips_with_fingerprint() {
        let back = round_trip(LedgerEvent::CredentialUse {
            connector: "some-provider".into(),
            auth: AuthKind::Apikey,
            fp: Some("9c2f1a3d".into()),
            dest: vec!["api.some-provider.example".into()],
        });
        assert_eq!(back.event.name(), "credential_use");
        assert_eq!(back.event.connector(), Some("some-provider"));
    }

    #[test]
    fn a_chain_augmented_line_still_deserializes_into_a_record() {
        let record = LedgerRecord {
            ts: "2026-06-29T14:05:30Z".into(),
            run: "5e6f7a8b0000000000000000000000bb".into(),
            microvm: "calm-finch".into(),
            event: LedgerEvent::CredentialUse {
                connector: "some-provider".into(),
                auth: AuthKind::Apikey,
                fp: Some("9c2f1a3d".into()),
                dest: vec!["api.some-provider.example".into()],
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
    fn fingerprint_is_deterministic_short_and_never_the_value() {
        let secret = "sk-super-secret-value";
        let fp = fingerprint(secret);
        assert_eq!(fp, fingerprint(secret), "same value → same fingerprint");
        assert_eq!(fp.len(), 12);
        assert!(
            !secret.contains(&fp),
            "the fingerprint must not leak the value"
        );
        assert_ne!(fingerprint("key-a"), fingerprint("key-b"));
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
