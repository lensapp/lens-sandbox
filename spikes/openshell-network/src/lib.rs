pub mod dns;
mod dns_policy;
pub mod dns_wire;
pub mod launch;
mod translate;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use openshell_supervisor_network::opa::{NetworkAction, NetworkInput, OpaEngine};
use tokio::sync::{mpsc, oneshot};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

pub struct Pending {
    pub host: String,
    pub port: u16,
    pub answer: oneshot::Sender<bool>,
}

pub struct Gate {
    pub engine: Arc<OpaEngine>,
    pending: mpsc::UnboundedSender<Pending>,
    loaded: Arc<Mutex<Option<LoadedPolicy>>>,
}

struct LoadedPolicy {
    data: String,
    generation: u64,
    policy: lns_policy::Policy,
}

impl Gate {
    pub fn dns_eligible(&self, host: &str) -> Result<bool, Error> {
        let loaded = self
            .loaded
            .lock()
            .map_err(|_| "policy cache lock poisoned")?;
        let Some(loaded) = loaded.as_ref() else {
            return Ok(false);
        };
        let Ok(guard) = self.engine.generation_guard(loaded.generation) else {
            return Ok(false);
        };
        let eligible = dns_policy::eligible(&loaded.policy, host);
        Ok(eligible && guard.ensure_current().is_ok())
    }

    pub fn new(pending: mpsc::UnboundedSender<Pending>) -> Result<Self, Error> {
        let engine = OpaEngine::from_strings(
            include_str!("policy.rego"),
            &translate::policy_data(&lns_policy::Policy::default())?,
        )
        .map_err(|error| error.to_string())?;
        let engine = Arc::new(engine);
        let weak = Arc::downgrade(&engine);
        let approvals = pending.clone();
        let loaded = Arc::new(Mutex::new(None));
        let callback_loaded = loaded.clone();
        engine
            .set_connection_approver(Arc::new(move |input, action| {
                if !matches!(action, NetworkAction::Deny { reason } if reason == "lns:ask") {
                    return false;
                }
                let Some(engine) = weak.upgrade() else {
                    return false;
                };
                let gate = Self {
                    engine,
                    pending: approvals.clone(),
                    loaded: callback_loaded.clone(),
                };
                tokio::runtime::Handle::current()
                    .block_on(gate.authorize(input))
                    .unwrap_or(false)
            }))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            engine,
            pending,
            loaded,
        })
    }

    pub async fn authorize(&self, input: &NetworkInput) -> Result<bool, Error> {
        let authorization = self
            .engine
            .authorize_egress(input)
            .map_err(|error| error.to_string())?;
        match authorization.action {
            NetworkAction::Allow { .. } => Ok(true),
            NetworkAction::Deny { reason } if reason == "lns:ask" => {
                let (answer, receiver) = oneshot::channel();
                if self
                    .pending
                    .send(Pending {
                        host: input.host.clone(),
                        port: input.port,
                        answer,
                    })
                    .is_err()
                {
                    return Ok(false);
                }
                let approved = matches!(
                    tokio::time::timeout(
                        Duration::from_secs(60),
                        self.wait_for_decision(input, receiver)
                    )
                    .await,
                    Ok(Ok(true))
                );
                if !approved {
                    return Ok(false);
                }
                let current = self
                    .engine
                    .authorize_egress(input)
                    .map_err(|error| error.to_string())?;
                Ok(matches!(current.action, NetworkAction::Allow { .. })
                    || matches!(current.action, NetworkAction::Deny { reason } if reason == "lns:ask"))
            }
            NetworkAction::Deny { .. } => Ok(false),
        }
    }

    async fn wait_for_decision(
        &self,
        input: &NetworkInput,
        mut receiver: oneshot::Receiver<bool>,
    ) -> Result<bool, Error> {
        loop {
            let current = self
                .engine
                .authorize_egress(input)
                .map_err(|error| error.to_string())?;
            match current.action {
                NetworkAction::Allow { .. } => return Ok(true),
                NetworkAction::Deny { reason } if reason != "lns:ask" => return Ok(false),
                _ => {}
            }
            let Ok(guard) = self.engine.generation_guard(current.generation) else {
                continue;
            };
            tokio::select! {
                answer = &mut receiver => return Ok(matches!(answer, Ok(true))),
                () = guard.wait_until_stale() => {}
            }
        }
    }

    pub fn reload(&self, policy: &lns_policy::Policy) -> Result<(), Error> {
        let result = translate::policy_data(policy).and_then(|data| {
            let mut loaded = self
                .loaded
                .lock()
                .map_err(|_| "policy cache lock poisoned")?;
            if loaded.as_ref().is_some_and(|old| {
                old.data == data && old.generation == self.engine.current_generation()
            }) {
                return Ok(());
            }
            self.engine
                .reload(include_str!("policy.rego"), &data)
                .map_err(|error| error.to_string())?;
            *loaded = Some(LoadedPolicy {
                data,
                generation: self.engine.current_generation(),
                policy: policy.clone(),
            });
            Ok(())
        });
        if let Err(error) = &result {
            self.engine
                .enter_fail_closed(error.to_string())
                .map_err(|error| error.to_string())?;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_is_closed_before_policy_and_after_quarantine_or_withdrawal() {
        let (tx, mut pending) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        assert!(!gate.dns_eligible("api.example").unwrap());
        let allow = serde_json::from_value(serde_json::json!({"network":{"egress":{"http":[{"match":"api.example", "verdict":"allow"}]}}})).unwrap();
        gate.reload(&allow).unwrap();
        assert!(gate.dns_eligible("api.example").unwrap());
        gate.engine.enter_fail_closed("relay disconnected").unwrap();
        assert!(!gate.dns_eligible("api.example").unwrap());
        gate.reload(&allow).unwrap();
        assert!(gate.dns_eligible("api.example").unwrap());
        gate.reload(&lns_policy::Policy::default()).unwrap();
        assert!(!gate.dns_eligible("api.example").unwrap());
        assert!(pending.try_recv().is_err());
    }

    #[test]
    fn dns_eligibility_is_sandbox_wide_but_connections_stay_binary_scoped() {
        let (tx, mut pending) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        gate.reload(
            &serde_json::from_value(serde_json::json!({"network":{"egress":{"http":[
                {"match":"*.example.com", "verdict":"allow", "binaries":["/usr/bin/curl"]}
            ]}}}))
            .unwrap(),
        )
        .unwrap();
        for host in ["example.com", "api.example.com", "a.b.example.com"] {
            assert!(
                gate.dns_eligible(host).unwrap(),
                "allowed name must be DNS eligible: {host}"
            );
            let mut request = input(host, 443);
            request.binary_path = "/usr/bin/other".into();
            assert!(matches!(
                gate.engine.authorize_egress(&request).unwrap().action,
                NetworkAction::Deny { .. }
            ));
        }
        assert!(!gate.dns_eligible("unknown.test").unwrap());
        assert!(
            pending.try_recv().is_err(),
            "DNS must not create an approval"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_shot_connection_approval_keeps_the_raw_tls_route() {
        let (tx, mut pending) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        let engine = gate.engine.clone();
        let connection = tokio::task::spawn_blocking(move || {
            engine.authorize_connection(&input("example.com", 443))
        });
        pending.recv().await.unwrap().answer.send(true).unwrap();
        let decision = connection.await.unwrap().unwrap();
        assert!(
            matches!(
                decision.action,
                NetworkAction::Allow {
                    matched_policy: Some(_)
                }
            ),
            "OpenShell ignores TLS route metadata without a matched policy"
        );
        assert_eq!(
            openshell_supervisor_network::l7::parse_tls_mode(&decision.endpoint_configs[0]),
            openshell_supervisor_network::l7::TlsMode::Skip
        );
    }

    #[test]
    fn raw_https_allow_does_not_enable_implicit_tls_termination() {
        let (tx, _) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        let policy = serde_json::from_value(serde_json::json!({
            "network": {"egress": {"http": [{"match": "registry.npmjs.org", "verdict": "allow"}]}}
        }))
        .unwrap();
        gate.reload(&policy).unwrap();
        let decision = gate
            .engine
            .authorize_egress(&input("registry.npmjs.org", 443))
            .unwrap();
        assert!(matches!(
            decision.action,
            NetworkAction::Allow {
                matched_policy: Some(_)
            }
        ));
        let mode = decision
            .endpoint_configs
            .first()
            .map(openshell_supervisor_network::l7::parse_tls_mode)
            .unwrap_or_default();
        assert_eq!(mode, openshell_supervisor_network::l7::TlsMode::Skip);
    }

    #[test]
    fn raw_connect_policy_supplies_the_credential_guard_query() {
        let (tx, _) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        let guards = gate
            .engine
            .query_endpoint_credential_guards(&input("registry.npmjs.org", 443))
            .expect("raw CONNECT must define an empty credential guard list");
        assert!(guards.is_empty());
    }

    #[test]
    fn raw_proxy_policy_supplies_the_upstream_middleware_query() {
        let (tx, _) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        let (chain, _) = gate
            .engine
            .query_middleware_chain_with_generation(&input("example.com", 80))
            .expect("raw proxy policy must define an empty middleware chain");
        assert!(chain.is_empty());
    }

    #[test]
    fn repeated_identical_host_policies_do_not_revoke_the_connection_generation() {
        let (tx, _) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        let policy = lns_policy::Policy::default();
        gate.reload(&policy).unwrap();
        let generation = gate.engine.current_generation();
        gate.reload(&policy).unwrap();
        assert_eq!(
            gate.engine.current_generation(),
            generation,
            "a duplicate policy must not invalidate an admitted connection"
        );
        gate.engine.enter_fail_closed("disconnected").unwrap();
        gate.reload(&policy).unwrap();
        assert!(
            matches!(action(&gate, "example.com", 80), NetworkAction::Deny { reason } if reason == "lns:ask")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_policy_update_resumes_an_already_held_request_without_a_decision_frame() {
        let (tx, mut pending) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        let destination = input("example.com", 80);
        let mut result = Box::pin(gate.authorize(&destination));
        let _held = tokio::select! {
            held = pending.recv() => held.unwrap(),
            result = &mut result => panic!("completed before policy update: {result:?}"),
        };
        let policy = serde_json::from_value(serde_json::json!({"network":{"egress":{"http":[],"tcp":[{"match":"example.com:80","verdict":"allow"}]}}})).unwrap();
        gate.reload(&policy).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), result)
                .await
                .expect("policy update must wake the held request")
                .unwrap()
        );
    }

    fn input(host: &str, port: u16) -> NetworkInput {
        NetworkInput {
            host: host.to_string(),
            port,
            binary_path: "/usr/bin/curl".into(),
            binary_sha256: String::new(),
            ancestors: Vec::new(),
            cmdline_paths: Vec::new(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn unknown_destination_waits_for_the_same_requests_approval() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        let request = input("api.example.com", 443);
        let mut authorization = Box::pin(gate.authorize(&request));
        let pending = tokio::select! {
            result = &mut authorization => panic!("unknown request completed before approval: {result:?}"),
            pending = rx.recv() => pending.expect("approval request"),
        };
        assert_eq!(
            (pending.host.as_str(), pending.port),
            ("api.example.com", 443)
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), &mut authorization)
                .await
                .is_err()
        );
        pending.answer.send(true).unwrap();
        assert!(authorization.await.unwrap());
    }

    fn configured(value: serde_json::Value) -> Gate {
        let (tx, _) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        gate.reload(&serde_json::from_value(value).unwrap())
            .unwrap();
        gate
    }

    fn action(gate: &Gate, host: &str, port: u16) -> NetworkAction {
        gate.engine
            .authorize_egress(&input(host, port))
            .unwrap()
            .action
    }

    #[test]
    fn first_matching_rule_decides_even_when_a_later_rule_allows() {
        let gate = configured(serde_json::json!({"network":{"egress":{"http":[
            {"match":"private.example.com","verdict":"deny"},
            {"match":"*.example.com","verdict":"allow"}
        ]}}}));
        assert_eq!(
            action(&gate, "private.example.com", 443),
            NetworkAction::Deny {
                reason: "lns:deny".into()
            }
        );
        assert!(matches!(
            action(&gate, "public.example.com", 443),
            NetworkAction::Allow { .. }
        ));
    }

    #[test]
    fn wildcard_matching_agrees_with_lns_for_apex_and_nested_subdomains() {
        for pattern in ["*.example.com", "*", "api.*.example.com", "api.example.com"] {
            let gate = configured(serde_json::json!({"network":{"egress":{"http":[
                {"match":pattern,"verdict":"allow"}
            ]}}}));
            for host in [
                "example.com",
                "api.example.com",
                "a.b.example.com",
                "api.a.b.example.com",
                "API.EXAMPLE.COM",
                "notexample.com",
                "api.example.com.evil.test",
            ] {
                assert_eq!(
                    matches!(action(&gate, host, 443), NetworkAction::Allow { .. }),
                    lns_policy::matching::domain_matches(pattern, host),
                    "{pattern} -> {host}"
                );
            }
        }
    }

    #[test]
    fn tcp_rules_precede_http_and_keep_their_port_scope() {
        let gate = configured(serde_json::json!({"network":{"egress":{
            "tcp":[{"match":"db.example.com:5432","verdict":"deny"}],
            "http":[{"match":"db.example.com","verdict":"allow"}]
        }}}));
        assert_eq!(
            action(&gate, "db.example.com", 5432),
            NetworkAction::Deny {
                reason: "lns:deny".into()
            }
        );
        assert!(matches!(
            action(&gate, "db.example.com", 443),
            NetworkAction::Allow { .. }
        ));
    }

    #[test]
    fn binary_mismatch_denies_instead_of_falling_through() {
        let gate = configured(serde_json::json!({"network":{"egress":{"http":[
            {"match":"api.example.com","verdict":"allow","binaries":["/usr/bin/git"]},
            {"match":"*","verdict":"allow"}
        ]}}}));
        assert_eq!(
            action(&gate, "api.example.com", 443),
            NetworkAction::Deny {
                reason: "lns:deny".into()
            }
        );
        let mut git = input("api.example.com", 443);
        git.binary_path = "/usr/bin/git".into();
        assert!(matches!(
            gate.engine.authorize_egress(&git).unwrap().action,
            NetworkAction::Allow { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn denial_installed_while_waiting_overrides_an_old_approval() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        let request = input("api.example.com", 443);
        let mut authorization = Box::pin(gate.authorize(&request));
        let pending = tokio::select! {
            result = &mut authorization => panic!("request was not held: {result:?}"),
            pending = rx.recv() => pending.unwrap(),
        };
        gate.engine.enter_fail_closed("policy withdrawn").unwrap();
        pending.answer.send(true).unwrap();
        assert!(
            !authorization.await.unwrap(),
            "stale approval overrode fail-closed policy"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn unanswered_request_times_out_closed() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        let request = input("api.example.com", 443);
        let mut authorization = Box::pin(gate.authorize(&request));
        let pending = tokio::select! {
            result = &mut authorization => panic!("request was not held: {result:?}"),
            pending = rx.recv() => pending.unwrap(),
        };
        let result = tokio::time::timeout(std::time::Duration::from_secs(61), authorization).await;
        assert!(
            matches!(result, Ok(Ok(false))),
            "an unanswered approval must time out closed: {result:?}"
        );
        assert!(pending.answer.is_closed());
    }

    #[test]
    fn unsupported_inspection_and_cidr_rules_are_rejected() {
        for rule in [
            serde_json::json!({"match":"api.example.com","verdict":"allow","rules":[{"method":"GET","path":"/v1"}]}),
            serde_json::json!({"match":"api.example.com","verdict":"allow","tlsTerminate":true}),
            serde_json::json!({"match":"api.example.com","verdict":"allow","scheme":"https"}),
            serde_json::json!({"match":"10.0.0.0/8","verdict":"allow"}),
        ] {
            let (tx, _) = mpsc::unbounded_channel();
            let gate = Gate::new(tx).unwrap();
            let policy =
                serde_json::from_value(serde_json::json!({"network":{"egress":{"http":[rule]}}}))
                    .unwrap();
            assert!(
                gate.reload(&policy).is_err(),
                "unsupported policy must not silently lose constraints"
            );
        }
    }

    #[test]
    fn unsupported_update_quarantines_a_previously_allowed_destination() {
        let gate = configured(serde_json::json!({"network":{"egress":{"http":[
            {"match":"api.example.com","verdict":"allow"}
        ]}}}));
        let policy = serde_json::from_value(serde_json::json!({"network":{"egress":{"http":[
            {"match":"api.example.com","verdict":"allow","rules":[{"method":"GET"}]}
        ]}}}))
        .unwrap();
        assert!(gate.reload(&policy).is_err());
        assert!(
            matches!(
                action(&gate, "api.example.com", 443),
                NetworkAction::Deny { .. }
            ),
            "failed update left the old broad allow active"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn allow_once_does_not_authorize_the_next_connection() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        let request = input("api.example.com", 443);
        for answer in [true, false] {
            let mut authorization = Box::pin(gate.authorize(&request));
            let pending = tokio::select! {
                result = &mut authorization => panic!("request bypassed approval: {result:?}"),
                pending = rx.recv() => pending.unwrap(),
            };
            pending.answer.send(answer).unwrap();
            assert_eq!(authorization.await.unwrap(), answer);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn lost_approval_channel_denies_and_cancelled_request_cannot_resume() {
        let (tx, rx) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        drop(rx);
        assert!(
            !gate
                .authorize(&input("api.example.com", 443))
                .await
                .unwrap()
        );

        let (tx, mut rx) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        let request = input("api.example.com", 443);
        let mut authorization = Box::pin(gate.authorize(&request));
        let pending = tokio::select! {
            result = &mut authorization => panic!("request bypassed approval: {result:?}"),
            pending = rx.recv() => pending.unwrap(),
        };
        drop(authorization);
        assert!(pending.answer.send(true).is_err());
    }
}
