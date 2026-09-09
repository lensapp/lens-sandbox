use crate::approval_flow::{answering::Granting, session::ConnectionChoice};
use crate::shutdown::Shutdown;
use lns_ipc::{DashboardApproval, DashboardEvent, DashboardSandbox, Response};
use tokio::io::{AsyncWrite, AsyncWriteExt};

pub trait ApprovalHost {
    fn offered(&self, id: &str) -> Option<lns_ipc::ConnectorView>;
    fn grant(&self, id: &str, method: &str, connection: ConnectionChoice) -> Granting;
}

pub fn reply_to_action(host: &impl ApprovalHost, request: lns_ipc::Request) -> Response {
    use lns_ipc::Request;
    match request {
        Request::InspectApprovalOffer { id } => Response::ApprovalOffer {
            offer: host.offered(&id),
        },
        Request::GrantApproval {
            id,
            digest,
            method,
            connection,
        } => {
            let connection = host
                .offered(&id)
                .ok_or_else(|| crate::approval_flow::answering::NOT_OFFERED.to_string())
                .and_then(|offer| {
                    if offer.digest == digest {
                        Ok(offer)
                    } else {
                        Err("the connector offer changed; reopen it before granting".into())
                    }
                })
                .and_then(|offer| {
                    crate::approval_flow::inbox::approval_connection(&offer, &method, connection)
                });
            let connection = match connection {
                Ok(connection) => connection,
                Err(reason) => return Response::ApprovalNotWritten { id, reason },
            };
            match host.grant(&id, &method, connection) {
                Granting::Granted => Response::Acknowledged,
                Granting::UnknownId => Response::ApprovalUnknown { id },
                Granting::NotOffered => Response::ApprovalNotWritten {
                    id,
                    reason: crate::approval_flow::answering::NOT_OFFERED.into(),
                },
            }
        }
        _ => Response::Error {
            message: "not a dashboard approval action".into(),
        },
    }
}

#[derive(Debug, Default)]
pub struct Snapshot {
    pub sandboxes: Vec<DashboardSandbox>,
    pub approvals: Vec<DashboardApproval>,
    pub events: Vec<DashboardEvent>,
    pub warnings: Vec<String>,
}

pub fn assemble(
    active: Vec<DashboardSandbox>,
    timeline: lns_audit::Timeline,
    entries: Vec<crate::approval_flow::entries::Entry>,
) -> Snapshot {
    use crate::dashboard::{approvals, sandboxes};
    let sandboxes = sandboxes::merge_sandboxes(&active, &timeline.rows);
    let approvals = entries
        .iter()
        .map(|entry| DashboardApproval {
            entry: crate::approval_flow::answering::view(entry),
            raw: approvals::is_raw(entry),
            answers: approvals::offers(entry),
            grantable: approvals::is_grantable(entry),
        })
        .collect();
    Snapshot {
        sandboxes,
        approvals,
        events: event_rows(timeline.rows),
        warnings: timeline.warnings,
    }
}

fn event_rows(rows: Vec<lns_audit::TimelineRow>) -> Vec<DashboardEvent> {
    let mut occurrences = std::collections::HashMap::<String, usize>::new();
    let mut events: Vec<_> = rows
        .into_iter()
        .rev()
        .map(|row| {
            let raw = row.raw.to_string();
            let hash = lns_ipc::line_hash(format!("{}\0{raw}", row.run).as_bytes());
            let occurrence = occurrences.entry(hash.clone()).or_default();
            let id = format!("{hash}:{occurrence}");
            *occurrence += 1;
            DashboardEvent {
                id,
                ts: row.ts,
                when: row.when,
                run: row.run,
                kind: row.kind,
                detail: row.detail,
                raw,
            }
        })
        .collect();
    events.reverse();
    events
}

pub fn snapshot_frames(snapshot: Snapshot) -> impl Iterator<Item = Response> {
    std::iter::once(Response::DashboardBegin)
        .chain(
            snapshot
                .sandboxes
                .into_iter()
                .map(|sandbox| Response::DashboardSandbox { sandbox }),
        )
        .chain(
            snapshot
                .approvals
                .into_iter()
                .map(|approval| Response::DashboardApproval { approval }),
        )
        .chain(
            snapshot
                .events
                .into_iter()
                .map(|event| Response::DashboardEvent { event }),
        )
        .chain(
            snapshot
                .warnings
                .into_iter()
                .map(|message| Response::DashboardWarning { message }),
        )
        .chain(std::iter::once(Response::DashboardEnd))
}

pub async fn write_snapshot<W: AsyncWrite + Unpin>(
    writer: &mut W,
    snapshot: Snapshot,
    shutdown: &Shutdown,
) -> anyhow::Result<()> {
    for response in snapshot_frames(snapshot) {
        let bytes = lns_ipc::encode_frame(&response)?;
        tokio::select! {
            result = writer.write_all(&bytes) => result?,
            _ = shutdown.wait_async() => return Ok(()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Offers {
        offer: Option<lns_ipc::ConnectorView>,
        result: Granting,
        grants: std::cell::Cell<usize>,
    }

    impl ApprovalHost for Offers {
        fn offered(&self, _: &str) -> Option<lns_ipc::ConnectorView> {
            self.offer.clone()
        }
        fn grant(&self, _: &str, _: &str, _: ConnectionChoice) -> Granting {
            self.grants.set(self.grants.get() + 1);
            self.result
        }
    }

    #[test]
    fn history_grants_revalidate_the_current_offer_and_report_refusals() {
        use lns_ipc::{ApprovalConnection, ConnectorMethodView, ConnectorView, Request};
        let mut host = Offers {
            offer: Some(ConnectorView {
                name: "linear".into(),
                digest: "sha256:test".into(),
                serves: vec![],
                connections: vec![],
                methods: vec![ConnectorMethodView {
                    name: "public".into(),
                    label: "Public".into(),
                    auth_label: None,
                    offerable: true,
                    opens: vec![],
                    writes: vec![],
                    env: vec![],
                    credentials: vec![],
                    asks: vec![],
                    help: None,
                    overrides: Some(vec![]),
                }],
            }),
            result: Granting::Granted,
            grants: std::cell::Cell::new(0),
        };
        assert_eq!(
            reply_to_action(&host, Request::InspectApprovalOffer { id: "entry".into() }),
            Response::ApprovalOffer {
                offer: host.offer.clone()
            }
        );
        let request = |method: &str| Request::GrantApproval {
            id: "entry".into(),
            digest: "sha256:test".into(),
            method: method.into(),
            connection: ApprovalConnection::None,
        };
        let changed = Request::GrantApproval {
            id: "entry".into(),
            digest: "sha256:older-offer".into(),
            method: "public".into(),
            connection: ApprovalConnection::None,
        };
        assert!(
            matches!(reply_to_action(&host, changed), Response::ApprovalNotWritten { reason, .. } if reason.contains("changed")),
            "a changed connector must be disclosed again before granting"
        );
        assert!(
            matches!(reply_to_action(&host, request("missing")), Response::ApprovalNotWritten { reason, .. } if reason.contains("unavailable"))
        );
        assert_eq!(
            host.grants.get(),
            0,
            "invalid methods must never reach the grant port"
        );
        assert_eq!(
            reply_to_action(&host, request("public")),
            Response::Acknowledged
        );
        assert_eq!(host.grants.get(), 1);
        host.result = Granting::UnknownId;
        assert!(matches!(
            reply_to_action(&host, request("public")),
            Response::ApprovalUnknown { .. }
        ));
        host.result = Granting::NotOffered;
        assert!(matches!(
            reply_to_action(&host, request("public")),
            Response::ApprovalNotWritten { .. }
        ));
        host.offer = None;
        assert!(matches!(
            reply_to_action(&host, request("public")),
            Response::ApprovalNotWritten { .. }
        ));
        assert!(matches!(
            reply_to_action(&host, Request::Ping),
            Response::Error { .. }
        ));
    }

    fn audit_row(run: &str, message: &str) -> lns_audit::TimelineRow {
        lns_audit::TimelineRow {
            ts: "2026-09-09T12:00:00Z".into(),
            when: "2026-09-09 12:00:00".into(),
            run: run.into(),
            kind: "launch".into(),
            detail: message.into(),
            connector: None,
            raw: serde_json::json!({"message": message, "unmapped": {"lns_microvm": "quiet_river", "lns_image": "alpine:3.20"}}),
        }
    }

    #[test]
    fn the_native_dashboard_keeps_historical_sandboxes_and_service_owned_answers() {
        use crate::approval_flow::entries::{Entry, EntryKind, EntryState};
        let snapshot = assemble(
            vec![],
            lns_audit::Timeline {
                rows: vec![audit_row("run-1", "launched")],
                warnings: vec!["integrity warning".into()],
            },
            vec![
                Entry::new(
                    Some("quiet_river".into()),
                    EntryKind::Destination {
                        destination: "example.com".into(),
                        action: "CONNECT example.com:443".into(),
                        raw: true,
                    },
                    EntryState::AlwaysAllowed,
                ),
                Entry::new(
                    Some("quiet_river".into()),
                    EntryKind::Connector {
                        name: "linear".into(),
                    },
                    EntryState::Undecided,
                ),
            ],
        );
        assert_eq!(
            snapshot.sandboxes.len(),
            1,
            "historical runs must remain selectable"
        );
        assert_eq!(snapshot.sandboxes[0].name, "quiet_river");
        assert_eq!(snapshot.sandboxes[0].image, "alpine:3.20");
        assert!(snapshot.approvals[0].raw);
        assert_eq!(
            snapshot.approvals[0].answers,
            [
                lns_ipc::ApprovalAnswer::AlwaysDeny,
                lns_ipc::ApprovalAnswer::AskAgain
            ]
        );
        assert!(snapshot.approvals[1].grantable);
        assert_eq!(snapshot.warnings, ["integrity warning"]);
        assert!(snapshot_frames(snapshot).any(|frame| matches!(frame, Response::DashboardSandbox { sandbox } if sandbox.name == "quiet_river")));
    }

    #[test]
    fn new_events_do_not_change_old_row_identity_and_identical_rows_stay_distinct() {
        let before = assemble(
            vec![],
            lns_audit::Timeline {
                rows: vec![audit_row("run-1", "same"), audit_row("run-1", "same")],
                warnings: vec![],
            },
            vec![],
        );
        let after = assemble(
            vec![],
            lns_audit::Timeline {
                rows: vec![
                    audit_row("run-1", "new"),
                    audit_row("run-1", "same"),
                    audit_row("run-1", "same"),
                ],
                warnings: vec![],
            },
            vec![],
        );
        assert_eq!(before.events.len(), 2, "each event must survive assembly");
        assert_ne!(before.events[0].id, before.events[1].id);
        assert_eq!(before.events[0].id, after.events[1].id);
        assert_eq!(before.events[1].id, after.events[2].id);
    }

    #[tokio::test]
    async fn an_empty_dashboard_is_explicitly_framed_not_a_disconnect() {
        let mut output = Vec::new();
        write_snapshot(&mut output, Snapshot::default(), &Shutdown::new())
            .await
            .unwrap();
        assert!(
            !output.is_empty(),
            "an empty dashboard still needs begin and completion frames"
        );
        let mut bytes = output.as_slice();
        assert_eq!(
            lns_ipc::decode_frame::<Response, _>(&mut bytes).unwrap(),
            Response::DashboardBegin
        );
        assert_eq!(
            lns_ipc::decode_frame::<Response, _>(&mut bytes).unwrap(),
            Response::DashboardEnd
        );
        assert!(bytes.is_empty());
    }

    #[tokio::test]
    async fn shutdown_releases_a_dashboard_client_that_stopped_reading() {
        let (mut writer, _client) = tokio::io::duplex(1);
        let shutdown = Shutdown::new();
        shutdown.signal();
        write_snapshot(&mut writer, Snapshot::default(), &shutdown)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_closed_dashboard_connection_and_an_oversized_event_are_errors_not_completion() {
        let (mut writer, client) = tokio::io::duplex(1);
        drop(client);
        assert!(
            write_snapshot(&mut writer, Snapshot::default(), &Shutdown::new())
                .await
                .is_err()
        );
        let mut writer = Vec::new();
        let huge = Snapshot {
            warnings: vec!["x".repeat(lns_ipc::MAX_FRAME_SIZE as usize)],
            ..Default::default()
        };
        assert!(
            write_snapshot(&mut writer, huge, &Shutdown::new())
                .await
                .is_err()
        );
        assert_eq!(
            writer,
            lns_ipc::encode_frame(&Response::DashboardBegin).unwrap()
        );
    }
}
