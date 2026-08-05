use std::sync::Arc;
use std::time::{Duration, Instant};

use cucumber::{given, then, when};
use lns_policy::connectors::{Connector, ConnectorRoute, CredentialAuth, SignInMethod};
use lns_policy::grants::{GrantRecord, WorkloadIdentity};
use lns_policy::matching::domain_matches;
use lns_policy::providers::{InjectionDef, InjectionKind};
use lns_policy::{Policy, RouteRule, Verdict};
use lns_service::approval_flow::protocol::{RequestPending, Treatment};
use lns_service::approval_flow::session::ApprovalSession;
use lns_service::credential_flow::connectors::{
    applied_connector_routes, offerable_connectors, resolve_connectable_connectors,
    withhold_undecided_connector_allows,
};

use crate::approval_rig::TestNotifier;
use crate::first_use_rig::FirstUseRig;
use crate::world::BehaviourWorld;

const PROJECT: &str = "rig-project";

fn this_workload() -> WorkloadIdentity {
    WorkloadIdentity::Definition {
        dir: "/rig/project".into(),
    }
}

fn other_workload() -> WorkloadIdentity {
    WorkloadIdentity::Definition {
        dir: "/rig/other".into(),
    }
}

fn rig(w: &mut BehaviourWorld) -> &mut FirstUseRig {
    w.first_use.get_or_insert_with(Default::default)
}

#[given(regex = r#"^the catalog claims "([^"]+)" for connector "([^"]+)" managing "([^"]+)"$"#)]
fn catalog_claims(w: &mut BehaviourWorld, host: String, id: String, env: String) {
    rig(w).catalog.push(Connector {
        id: id.clone(),
        name: None,
        routes: vec![ConnectorRoute {
            match_pattern: host.clone(),
            transport: None,
            scheme: None,
            tls_terminate: false,
            rules: Vec::new(),
        }],
        methods: vec![SignInMethod::credential(
            "token",
            CredentialAuth {
                env_var: env,
                placeholder: format!("{id}-LNSPLACEHOLDER0000"),
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: host,
                    header: None,
                }],
            },
        )],
    });
}

#[given("the directory policy lists no connectors")]
fn policy_lists_no_connectors(w: &mut BehaviourWorld) {
    assert!(
        rig(w).overlay.connectors.is_empty(),
        "the rig policy unexpectedly connects a connector"
    );
}

#[given(regex = r#"^the directory policy allows "([^"]+)"$"#)]
fn policy_allows(w: &mut BehaviourWorld, host: String) {
    rig(w).overlay.add_rule(RouteRule::allow_host(&host));
}

#[given(regex = r#"^the directory policy connects "([^"]+)"$"#)]
fn policy_connects(w: &mut BehaviourWorld, id: String) {
    rig(w).overlay.connectors.push(id);
}

fn record_decline(w: &mut BehaviourWorld, id: &str, workload: &WorkloadIdentity) {
    let rig = rig(w);
    let connector = rig
        .catalog
        .iter()
        .find(|c| c.id == id)
        .expect("a Given step must put the connector in the catalog");
    let (env_var, domains) = lns_service::credential_flow::connectors::disclosure_of(connector)
        .expect("a credential connector discloses an env var and domains");
    rig.grants
        .upsert(GrantRecord::deny(PROJECT, workload, id, env_var, domains));
}

#[given(regex = r#"^this workload declined the "([^"]+)" connect offer$"#)]
fn this_workload_declined(w: &mut BehaviourWorld, id: String) {
    record_decline(w, &id, &this_workload());
}

#[given(regex = r#"^this workload granted the "([^"]+)" connect offer$"#)]
fn this_workload_granted(w: &mut BehaviourWorld, id: String) {
    let workload = this_workload();
    let rig = rig(w);
    let connector = rig
        .catalog
        .iter()
        .find(|c| c.id == id)
        .expect("a Given step must put the connector in the catalog");
    let (env_var, domains) = lns_service::credential_flow::connectors::disclosure_of(connector)
        .expect("a credential connector discloses an env var and domains");
    rig.grants.upsert(GrantRecord::allow(
        PROJECT, &workload, &id, env_var, domains,
    ));
}

#[given(regex = r#"^another workload declined the "([^"]+)" connect offer$"#)]
fn other_workload_declined(w: &mut BehaviourWorld, id: String) {
    record_decline(w, &id, &other_workload());
}

fn first_match_allows(policy: &Policy, host: &str) -> bool {
    // The guest gate is first-match-wins; only a leading allow lets the request past without a card.
    policy
        .network
        .egress
        .http
        .iter()
        .find(|r| domain_matches(&r.match_pattern, host))
        .is_some_and(|r| r.verdict == Verdict::Allow)
}

#[when(regex = r#"^the workload requests "([^"]+)"$"#)]
fn workload_requests(w: &mut BehaviourWorld, host: String) {
    let (running, offers) = {
        let rig = rig(w);
        // The launch splices a connected connector's own routes in before anything is withheld.
        let mut composed = rig.overlay.clone();
        composed
            .network
            .egress
            .http
            .extend(applied_connector_routes(
                &rig.overlay.connectors,
                &rig.catalog,
            ));
        let connectable = resolve_connectable_connectors(&rig.overlay, &rig.catalog);
        let offers = offerable_connectors(&connectable, &rig.catalog);
        let offerable_ids = offers.iter().map(|o| o.id.clone()).collect();
        let running = withhold_undecided_connector_allows(
            &composed,
            &rig.catalog,
            &offerable_ids,
            PROJECT,
            &this_workload(),
            &rig.grants,
        );
        (running, offers)
    };
    if first_match_allows(&running, &host) {
        let rig = rig(w);
        rig.running_policy = Some(running);
        rig.proceeded = true;
        return;
    }
    let notifier = Arc::new(TestNotifier::default());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let session = Arc::new(
        ApprovalSession::new(
            running.clone(),
            notifier.clone(),
            Arc::new(crate::approval_rig::FlakyStore::new(
                std::env::temp_dir().join("lns-first-use-rig-unused.yaml"),
            )),
            tx,
            Duration::from_secs(30),
        )
        .with_offers(offers),
    );
    session.submit_pending(
        RequestPending {
            id: "req-1".into(),
            host: host.clone(),
            action: format!("CONNECT {host}:443"),
            reason: "policy-ambiguous".into(),
            treatment: Treatment::Inspected,
        },
        Instant::now(),
    );
    let presented = notifier.presented.lock().unwrap().clone();
    let rig = rig(w);
    session.set_connector(Arc::new(DeclineRecorder {
        declined: rig.declined.clone(),
    }));
    rig.running_policy = Some(running);
    rig.held = !presented.is_empty();
    rig.offered = presented.iter().filter_map(|p| p.offer.clone()).collect();
    rig.card = presented.first().map(|p| p.id.clone());
    rig.session = Some(session);
}

/// A connect port that never connects, so answering the card exercises the decline path alone.
struct DeclineRecorder {
    declined: Arc<std::sync::Mutex<Vec<String>>>,
}

impl lns_service::approval_flow::session::ConnectPort for DeclineRecorder {
    fn connect<'a>(&'a self, _id: &'a str) -> futures_util::future::BoxFuture<'a, bool> {
        Box::pin(async { false })
    }
    fn connect_with_token<'a>(
        &'a self,
        _id: &'a str,
        _value: String,
    ) -> futures_util::future::BoxFuture<'a, bool> {
        Box::pin(async { false })
    }
    fn decline(&self, id: &str) {
        self.declined.lock().unwrap().push(id.to_string());
    }
}

#[when("the developer answers the card without connecting")]
fn developer_answers_without_connecting(w: &mut BehaviourWorld) {
    let rig = rig(w);
    let card = rig
        .card
        .clone()
        .expect("the request must have raised a card");
    let session = rig.session.clone().expect("the request built a session");
    session.record_decision(
        &card,
        lns_service::approval_flow::protocol::Decision::AllowOnce,
    );
}

#[then(regex = r#"^the "([^"]+)" decline is remembered for this workload$"#)]
fn decline_remembered(w: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let rig = w.first_use.as_ref().ok_or("no request happened")?;
    let declined = rig.declined.lock().unwrap().clone();
    if declined.contains(&id) {
        Ok(())
    } else {
        Err(format!(
            "answering the offer card without connecting left no standing no for {id}; declined: {declined:?}"
        ))
    }
}

#[then(regex = r#"^the approval window offers to connect "([^"]+)"$"#)]
fn window_offers_connect(w: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let rig = w.first_use.as_ref().ok_or("no request happened")?;
    let connector = rig
        .catalog
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| format!("{id} is not in the rig catalog"))?;
    let expected = connector.display_name();
    if rig.offered.iter().any(|o| o == expected) {
        Ok(())
    } else {
        Err(format!(
            "expected an offer to connect {expected}; offers: {:?}, held: {}, proceeded: {}",
            rig.offered, rig.held, rig.proceeded
        ))
    }
}

#[then("the request is held until the offer is answered")]
fn request_is_held(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.first_use.as_ref().ok_or("no request happened")?;
    if rig.proceeded {
        return Err("the request went out under an allow rule instead of being held".to_string());
    }
    if rig.held {
        Ok(())
    } else {
        Err("no card was presented, so nothing held the request".to_string())
    }
}

#[then("no offer is presented")]
fn no_offer_presented(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.first_use.as_ref().ok_or("no request happened")?;
    if rig.offered.is_empty() {
        Ok(())
    } else {
        Err(format!("an offer was presented: {:?}", rig.offered))
    }
}

#[then("the request proceeds under the network policy alone")]
fn request_proceeds(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.first_use.as_ref().ok_or("no request happened")?;
    if rig.proceeded {
        Ok(())
    } else {
        Err(format!(
            "the request did not go out under the policy; held: {}, policy: {:?}",
            rig.held,
            rig.running_policy
                .as_ref()
                .map(|p| p.network.egress.http.clone())
        ))
    }
}
