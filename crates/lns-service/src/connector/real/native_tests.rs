use super::*;
use crate::approval_flow::session::{ConnectRound, ConnectRoundPort, ConnectorPort};
use crate::connector::connect::tests::Rig;
use crate::connector::connect::{Connecting, Turn};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    canceled: std::sync::atomic::AtomicBool,
    fails: bool,
}
impl Fake {
    fn record(&self, action: &str, session: &str) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{action}:{session}"));
        if self.fails {
            anyhow::bail!("injected unavailable connector store");
        }
        Ok(())
    }
}
impl NativeAccess for Fake {
    fn status(&self, session: &str) -> Result<Turn> {
        self.record("status", session)?;
        Ok(Turn {
            connector: "provider".into(),
            connecting: Connecting::Pending {
                session: session.into(),
                progress: if self.canceled.load(std::sync::atomic::Ordering::Relaxed) {
                    lns_ipc::OAuthProgress::Canceled
                } else {
                    lns_ipc::OAuthProgress::Starting {
                        destinations: vec!["https://auth.example/token".into()],
                        scopes: vec!["read".into()],
                    }
                },
            },
        })
    }
    fn open_browser(&self, session: &str) -> Result<()> {
        self.record("open", session)
    }
    fn cancel(&self, session: &str) -> Result<()> {
        self.record("cancel", session)?;
        self.canceled
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    fn offers(&self, _: &GrantHolder) -> Result<Vec<lns_ipc::ConnectorView>> {
        self.record("offers", "")?;
        Ok(vec![])
    }
    fn supply(&self, _: &GrantHolder) -> Result<BTreeMap<String, GrantedPayload>> {
        self.record("supply", "")?;
        Ok(BTreeMap::from([(
            "provider".into(),
            GrantedPayload::default(),
        )]))
    }
}
#[tokio::test]
async fn native_ipc_status_cancel_and_browser_actions_share_the_existing_connector_dispatch() {
    let rig = Rig::holding(
        r#"{"apiVersion":"lns.run/v1","kind":"connector","name":"provider","spec":{"serves":["api.example"],"methods":[{"name":"public"}]}}"#,
        None,
    );
    let fake = Fake::default();
    for call in [
        Call::Status("operation".into()),
        Call::OpenBrowser("operation".into()),
        Call::Cancel("operation".into()),
    ] {
        let response = answer_in(rig.store(), call, &fake).await.unwrap();
        assert!(matches!(response,Response::ConnectorPending{session,..} if session=="operation"));
    }
    assert_eq!(
        *fake.calls.lock().unwrap(),
        [
            "status:operation",
            "open:operation",
            "status:operation",
            "cancel:operation",
            "status:operation"
        ]
    );
}
#[test]
fn native_approval_adapter_shares_progress_and_disarms_unreadable_supply() {
    for fails in [false, true] {
        let fake = Arc::new(Fake {
            fails,
            ..Default::default()
        });
        let port = RealConnectorPort {
            holder: GrantHolder::Run("run".into()),
            microvm: "vm".into(),
            native: fake.clone(),
        };
        let result = port.poll_connect("operation");
        if fails {
            assert!(result.unwrap_err().contains("unavailable"));
        } else {
            assert!(
                matches!(result.unwrap(),ConnectRound::Pending{session,..} if session=="operation")
            );
        }
        assert_eq!(port.open_connect_browser("operation").is_err(), fails);
        assert_eq!(port.current_offers(), Some(vec![]));
        assert_eq!(port.current_supply().unwrap().len(), usize::from(!fails));
        port.abandon_connect("operation");
        assert!(
            fake.calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c == "cancel:operation")
        );
    }
}
