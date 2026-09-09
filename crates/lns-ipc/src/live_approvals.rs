use serde::{Deserialize, Serialize};

use crate::{ConnectorView, SecretValues};

#[cfg(test)]
mod tests {
    #[test]
    fn swift_fixture_is_the_service_snapshot_wire_format() {
        let snapshot = crate::Response::LiveApprovals(super::LiveApprovalSnapshot {
            approvals: vec![super::LiveApproval {
                id: "approval-1".into(),
                token: "presentation-1".into(),
                host: "api.example.com".into(),
                action: "CONNECT api.example.com:443".into(),
                run: Some("quiet_river".into()),
                raw: false,
                waiting: true,
                submitting: false,
                offer: None,
            }],
            notices: vec!["A decision could not be saved.".into()],
        });
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../clients/macos/Tests/LNSClientTests/Fixtures/live-approvals.json"
        ))
        .unwrap();
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), fixture);
        assert_eq!(
            serde_json::from_value::<crate::Response>(fixture).unwrap(),
            snapshot
        );
    }
    use crate::{Request, decode_frame, encode_frame};
    use serde_json::json;

    #[test]
    fn any_client_can_subscribe_to_live_approvals() {
        let value = json!({"type": "WatchApprovals"});
        let decoded = serde_json::from_value::<Request>(value.clone());
        assert!(
            decoded.is_ok(),
            "live approvals must be available over IPC: {decoded:?}"
        );
        let frame = encode_frame(&decoded.unwrap()).unwrap();
        let decoded: Request = decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    }

    #[test]
    fn a_client_answers_the_exact_live_presentation_it_observed() {
        let value = json!({
            "type": "RespondToApproval",
            "token": "presentation-1",
            "action": {"kind": "allow_once"}
        });
        let decoded = serde_json::from_value::<Request>(value.clone());
        assert!(
            decoded.is_ok(),
            "live decisions must be available over IPC: {decoded:?}"
        );
        assert_eq!(serde_json::to_value(decoded.unwrap()).unwrap(), value);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveApprovalSnapshot {
    pub approvals: Vec<LiveApproval>,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveApproval {
    /// Stable while this presentation exists, including while a connector hold expires.
    pub id: String,
    /// Identifies the exact state the user answered; changed and consumed tokens are stale.
    pub token: String,
    pub host: String,
    pub action: String,
    pub run: Option<String>,
    pub raw: bool,
    pub waiting: bool,
    pub submitting: bool,
    pub offer: Option<ConnectorView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveApprovalAction {
    AllowOnce,
    AllowAlways,
    DenyOnce,
    DenyAlways,
    Dismiss,
    Decline,
    Grant {
        method: String,
        connection: ApprovalConnection,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApprovalConnection {
    None,
    Held { label: String },
    New { label: String, values: SecretValues },
}
