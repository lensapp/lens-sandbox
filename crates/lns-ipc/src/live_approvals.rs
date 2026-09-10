use serde::{Deserialize, Serialize};

use crate::{ConnectorView, SecretValues};

pub fn live_approval_frames(
    snapshot: LiveApprovalSnapshot,
) -> Result<Vec<crate::Response>, serde_json::Error> {
    let json = serde_json::to_string(&crate::Response::LiveApprovals(snapshot))?;
    let chunk_bytes = crate::MAX_FRAME_SIZE as usize / 6 - 128;
    let mut offset = 0;
    let mut frames = Vec::new();
    while offset < json.len() {
        let end = json.floor_char_boundary((offset + chunk_bytes).min(json.len()));
        frames.push(crate::Response::LiveApprovalsChunk {
            offset,
            data: json[offset..end].to_string(),
            complete: end == json.len(),
        });
        offset = end;
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    #[test]
    fn large_live_snapshots_cross_bounded_frames_without_losing_notices() {
        #[derive(serde::Deserialize)]
        struct Chunk {
            #[serde(rename = "type")]
            kind: String,
            offset: usize,
            data: String,
            complete: bool,
        }
        let snapshot = super::LiveApprovalSnapshot {
            approvals: vec![],
            notices: (0..1024)
                .map(|i| format!("{i}: {}", "é🦀\n\"\\".repeat(256)))
                .collect(),
        };
        let expected = crate::Response::LiveApprovals(snapshot.clone());
        let frames = super::live_approval_frames(snapshot).unwrap();
        let mut joined = String::new();
        let mut completed = false;
        for frame in frames {
            let wire = crate::encode_frame(&frame)
                .expect("each live approval frame must fit the IPC limit");
            let decoded: Chunk = crate::decode_frame(&mut &wire[..]).unwrap();
            assert_eq!(decoded.kind, "LiveApprovalsChunk");
            assert!(!completed);
            assert_eq!(decoded.offset, joined.len());
            joined.push_str(&decoded.data);
            completed = decoded.complete;
        }
        assert!(completed);
        assert_eq!(
            serde_json::from_str::<crate::Response>(&joined).unwrap(),
            expected
        );
    }
    #[test]
    fn native_dashboard_requests_are_language_neutral() {
        for value in [
            serde_json::json!({"type": "ReadDashboard"}),
            serde_json::json!({"type": "WatchDashboard"}),
            serde_json::json!({"type": "DismissApprovalNotices", "notices": ["old warning"]}),
            serde_json::json!({"type": "InspectApprovalOffer", "id": "entry-1"}),
            serde_json::json!({"type": "GrantApproval", "id": "entry-1", "digest": "sha256:test", "method": "token", "connection": {"kind": "held", "label": "work"}}),
        ] {
            let decoded = serde_json::from_value::<crate::Request>(value.clone());
            assert!(
                decoded.is_ok(),
                "dashboard requests must cross IPC: {decoded:?}"
            );
            assert_eq!(serde_json::to_value(decoded.unwrap()).unwrap(), value);
        }
    }

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
