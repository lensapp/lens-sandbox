//! The launch-triggered origin of the value card: unlike a request-triggered `CredentialPending`, there is no held request to release — the decision gates the boot itself.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::approval_flow::window::WindowState;
use crate::artifact::credential_boot::ValuePrompt;
use crate::credential_flow::session::{CredentialDecisionRequest, CredentialPendingPrompt};
use lns_policy::credentials::{CredentialEntry, CredentialStore};

pub fn launch_prompt(prompt: &ValuePrompt) -> CredentialPendingPrompt {
    CredentialPendingPrompt {
        id: format!("launch-{}-{}", prompt.connector, prompt.env),
        credential_id: prompt.connector.clone(),
        action: format!(
            "provide the \"{}\" credential this sandbox requires",
            prompt.connector
        ),
        oauth_display_name: None,
        token_fallback: prompt.token_fallback.clone(),
        env_var: Some(prompt.env.clone()),
        injection_domains: prompt.injection_domains.clone(),
        is_project_defined: true,
    }
}

/// A saved value persists to the per-machine store and the boot proceeds; a decline or timeout aborts the launch and persists nothing — a launch-origin deny is never a machine-wide `Deny`.
#[derive(Debug, PartialEq, Eq)]
pub enum LaunchResolution {
    Persist(CredentialEntry),
    Abort(String),
}

pub fn resolve_launch_decision(
    connector: &str,
    request: CredentialDecisionRequest,
) -> LaunchResolution {
    match request {
        CredentialDecisionRequest::Allow(CredentialEntry::Deny)
        | CredentialDecisionRequest::Deny => LaunchResolution::Abort(format!(
            "the \"{connector}\" credential this sandbox requires was declined; launch aborted"
        )),
        CredentialDecisionRequest::Allow(entry) => LaunchResolution::Persist(entry),
        CredentialDecisionRequest::Timeout => LaunchResolution::Abort(format!(
            "the \"{connector}\" value decision timed out; launch aborted"
        )),
    }
}

pub struct ValueCardDeps<'a> {
    pub window: &'a WindowState,
    pub store: &'a dyn CredentialStore,
    pub wait: Duration,
    pub host_value_available: &'a (dyn Fn(&ValuePrompt) -> bool + Sync),
    pub announce: &'a (dyn Fn(&str) + Sync),
    pub repaint: &'a (dyn Fn() + Sync),
}

/// Block the boot on each unbound required slot's value card in declaration order; the first decline, timeout, duplicate card, or store failure aborts the launch.
pub async fn gate_value_prompts(
    prompts: &[ValuePrompt],
    deps: &ValueCardDeps<'_>,
) -> Result<(), String> {
    for prompt in prompts {
        gate_one(prompt, deps).await?;
    }
    Ok(())
}

async fn gate_one(prompt: &ValuePrompt, deps: &ValueCardDeps<'_>) -> Result<(), String> {
    let card = launch_prompt(prompt);
    let card_id = card.id.clone();
    let (decision_tx, mut decision_rx) = mpsc::unbounded_channel();
    let inserted = deps.window.try_insert_credential_pending(
        card,
        (deps.host_value_available)(prompt),
        decision_tx,
    );
    if !inserted {
        return Err(format!(
            "a value decision for \"{}\" is already pending in the approval window",
            prompt.connector
        ));
    }
    (deps.repaint)();
    (deps.announce)(&format!(
        "the \"{}\" credential is required before the workload starts — waiting for the value decision in the Lens Sandbox window",
        prompt.connector
    ));
    let request = match tokio::time::timeout(deps.wait, decision_rx.recv()).await {
        Ok(Some(delivery)) => delivery.request,
        Ok(None) | Err(_) => {
            deps.window.remove_credential_pending(&card_id);
            CredentialDecisionRequest::Timeout
        }
    };
    match resolve_launch_decision(&prompt.connector, request) {
        LaunchResolution::Persist(entry) => persist(deps.store, &prompt.connector, entry),
        LaunchResolution::Abort(reason) => Err(reason),
    }
}

fn persist(store: &dyn CredentialStore, id: &str, entry: CredentialEntry) -> Result<(), String> {
    let mut state = store
        .load()
        .map_err(|e| format!("reading the credential store failed: {e}"))?;
    state.insert(id.to_string(), entry);
    store
        .save(&state)
        .map_err(|e| format!("storing the value decision failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::connectors::TokenFallback;
    use lns_policy::credentials::CredentialStateFile;
    use std::sync::Mutex;

    fn value_prompt(connector: &str, env: &str) -> ValuePrompt {
        ValuePrompt {
            connector: connector.into(),
            env: env.into(),
            token_fallback: Some(TokenFallback {
                help: Some("https://docs.example.test/token".into()),
                command: Some("some-cli setup-token".into()),
            }),
            injection_domains: vec!["api.some-provider.example".into()],
        }
    }

    #[derive(Default)]
    struct MemStore {
        state: Mutex<CredentialStateFile>,
        fail_load: bool,
        fail_save: bool,
    }

    impl CredentialStore for MemStore {
        fn load(&self) -> std::io::Result<CredentialStateFile> {
            if self.fail_load {
                return Err(std::io::Error::other("store unreadable"));
            }
            Ok(self.state.lock().unwrap().clone())
        }
        fn save(&self, state: &CredentialStateFile) -> std::io::Result<()> {
            if self.fail_save {
                return Err(std::io::Error::other("disk full"));
            }
            *self.state.lock().unwrap() = state.clone();
            Ok(())
        }
    }

    struct Rig {
        window: std::sync::Arc<WindowState>,
        store: MemStore,
        announced: Mutex<Vec<String>>,
        repaints: Mutex<usize>,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                window: WindowState::new(),
                store: MemStore::default(),
                announced: Mutex::new(Vec::new()),
                repaints: Mutex::new(0),
            }
        }

        async fn run(
            &self,
            prompts: &[ValuePrompt],
            decide: impl AsyncFn(&WindowState),
        ) -> Result<(), String> {
            let deps = ValueCardDeps {
                window: &self.window,
                store: &self.store,
                wait: Duration::from_secs(5),
                host_value_available: &|_| false,
                announce: &|msg| self.announced.lock().unwrap().push(msg.to_string()),
                repaint: &|| *self.repaints.lock().unwrap() += 1,
            };
            let gate = gate_value_prompts(prompts, &deps);
            let (outcome, ()) = tokio::join!(gate, decide(&self.window));
            outcome
        }
    }

    async fn visible_card(
        window: &WindowState,
    ) -> crate::approval_flow::window::CredentialCardPrompt {
        let mut card = None;
        for _ in 0..1000 {
            card = window.snapshot().pending_credentials.first().cloned();
            if card.is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
        card.expect("no value card became visible")
    }

    fn abort_reason(resolution: LaunchResolution) -> Option<String> {
        match resolution {
            LaunchResolution::Abort(reason) => Some(reason),
            LaunchResolution::Persist(_) => None,
        }
    }

    #[test]
    fn the_launch_prompt_is_its_own_origin_not_a_synthetic_wire_event() {
        let card = launch_prompt(&value_prompt("some-provider", "PROVIDER_KEY"));
        assert_eq!(card.id, "launch-some-provider-PROVIDER_KEY");
        assert_eq!(card.credential_id, "some-provider");
        assert_eq!(
            card.env_var.as_deref(),
            Some("PROVIDER_KEY"),
            "the card discloses the slot's env remap, not the catalog default"
        );
        assert!(
            card.action.contains("this sandbox requires"),
            "{}",
            card.action
        );
        assert_eq!(card.oauth_display_name, None);
        assert!(
            card.is_project_defined,
            "the requirement comes from the sandbox definition"
        );
        assert!(
            card.token_fallback.is_some(),
            "the card shows how to mint the value"
        );
        assert_eq!(
            card.injection_domains,
            vec!["api.some-provider.example".to_string()]
        );
    }

    #[test]
    fn a_saved_value_resolves_to_persist() {
        let entry = CredentialEntry::Stored {
            value: "some-secret".into(),
        };
        let resolution = resolve_launch_decision(
            "some-provider",
            CredentialDecisionRequest::Allow(entry.clone()),
        );
        assert_eq!(resolution, LaunchResolution::Persist(entry));
        assert_eq!(
            abort_reason(resolution),
            None,
            "a saved value must not abort the launch"
        );
    }

    #[test]
    fn a_host_detect_choice_resolves_to_persist() {
        assert_eq!(
            resolve_launch_decision(
                "some-provider",
                CredentialDecisionRequest::Allow(CredentialEntry::HostDetect)
            ),
            LaunchResolution::Persist(CredentialEntry::HostDetect)
        );
    }

    #[test]
    fn a_decline_aborts_the_launch_naming_the_connector() {
        let reason = abort_reason(resolve_launch_decision(
            "some-provider",
            CredentialDecisionRequest::Deny,
        ))
        .expect("a decline must abort");
        assert!(reason.contains("\"some-provider\""), "{reason}");
        assert!(reason.contains("launch aborted"), "{reason}");
    }

    #[test]
    fn an_allow_carrying_a_deny_entry_still_aborts_instead_of_persisting_it() {
        let resolution = resolve_launch_decision(
            "some-provider",
            CredentialDecisionRequest::Allow(CredentialEntry::Deny),
        );
        let aborts = matches!(resolution, LaunchResolution::Abort(_));
        assert!(
            aborts,
            "a launch-origin decision must never write a machine-wide deny"
        );
    }

    #[test]
    fn a_timeout_aborts_the_launch_distinctly() {
        let reason = abort_reason(resolve_launch_decision(
            "some-provider",
            CredentialDecisionRequest::Timeout,
        ))
        .expect("a timeout must abort");
        assert!(reason.contains("timed out"), "{reason}");
    }

    #[tokio::test]
    async fn saving_a_value_on_the_card_persists_it_and_lets_the_boot_proceed() {
        let rig = Rig::new();
        let outcome = rig
            .run(&[value_prompt("some-provider", "SOME_TOKEN")], async |w| {
                let card = visible_card(w).await;
                assert!(!card.host_value_available);
                w.decide_credential(
                    &card.id,
                    CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                        value: "some-secret".into(),
                    }),
                );
            })
            .await;
        assert_eq!(outcome, Ok(()));
        assert_eq!(
            rig.store.state.lock().unwrap().get("some-provider"),
            Some(&CredentialEntry::Stored {
                value: "some-secret".into()
            })
        );
        assert!(
            rig.window.snapshot().pending_credentials.is_empty(),
            "the decided card leaves the window"
        );
        assert_eq!(
            *rig.repaints.lock().unwrap(),
            1,
            "the inserted card repaints the window"
        );
        let announced = rig.announced.lock().unwrap();
        assert_eq!(announced.len(), 1);
        let message = announced[0].clone();
        assert!(message.contains("\"some-provider\""), "{message}");
        assert!(message.contains("before the workload starts"), "{message}");
    }

    #[tokio::test]
    async fn declining_the_card_aborts_the_launch_without_a_machine_wide_deny() {
        let rig = Rig::new();
        let outcome = rig
            .run(&[value_prompt("some-provider", "SOME_TOKEN")], async |w| {
                let card = visible_card(w).await;
                w.decide_credential(&card.id, CredentialDecisionRequest::Deny);
            })
            .await;
        let reason = outcome.expect_err("a decline must abort the launch");
        assert!(reason.contains("launch aborted"), "{reason}");
        assert!(
            !rig.store
                .state
                .lock()
                .unwrap()
                .contains_key("some-provider"),
            "a launch-origin decline must not persist a machine-wide deny"
        );
    }

    #[tokio::test]
    async fn an_undecided_card_times_out_aborting_the_launch_and_leaving_the_window_clean() {
        let rig = Rig::new();
        let deps = ValueCardDeps {
            window: &rig.window,
            store: &rig.store,
            wait: Duration::from_millis(5),
            host_value_available: &|_| false,
            announce: &|_| {},
            repaint: &|| {},
        };
        let reason = gate_value_prompts(&[value_prompt("some-provider", "SOME_TOKEN")], &deps)
            .await
            .expect_err("an undecided card must abort the launch");
        assert!(reason.contains("timed out"), "{reason}");
        assert!(
            rig.window.snapshot().pending_credentials.is_empty(),
            "the timed-out card is torn down"
        );
        assert!(rig.store.state.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_detected_host_value_is_offered_on_the_card() {
        let rig = Rig::new();
        let deps = ValueCardDeps {
            window: &rig.window,
            store: &rig.store,
            wait: Duration::from_secs(5),
            host_value_available: &|p| p.env == "SOME_TOKEN",
            announce: &|_| {},
            repaint: &|| {},
        };
        let prompts = [value_prompt("some-provider", "SOME_TOKEN")];
        let gate = gate_value_prompts(&prompts, &deps);
        let decide = async {
            let card = visible_card(&rig.window).await;
            assert!(
                card.host_value_available,
                "the card offers the detected host value"
            );
            rig.window.decide_credential(
                &card.id,
                CredentialDecisionRequest::Allow(CredentialEntry::HostDetect),
            );
        };
        let (outcome, ()) = tokio::join!(gate, decide);
        assert_eq!(outcome, Ok(()));
        assert_eq!(
            rig.store.state.lock().unwrap().get("some-provider"),
            Some(&CredentialEntry::HostDetect)
        );
    }

    #[tokio::test]
    async fn each_prompt_cards_in_declaration_order_until_all_are_saved() {
        let rig = Rig::new();
        let prompts = vec![
            value_prompt("some-provider", "SOME_TOKEN"),
            value_prompt("other-provider", "OTHER_TOKEN"),
        ];
        let outcome = rig
            .run(&prompts, async |w| {
                for expected in ["some-provider", "other-provider"] {
                    let card = visible_card(w).await;
                    assert_eq!(card.credential_id, expected);
                    w.decide_credential(
                        &card.id,
                        CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                            value: format!("{expected}-secret"),
                        }),
                    );
                }
            })
            .await;
        assert_eq!(outcome, Ok(()));
        let state = rig.store.state.lock().unwrap();
        assert!(state.contains_key("some-provider"));
        assert!(state.contains_key("other-provider"));
    }

    #[tokio::test]
    async fn a_duplicate_pending_card_refuses_instead_of_coalescing() {
        let rig = Rig::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        assert!(rig.window.try_insert_credential_pending(
            launch_prompt(&value_prompt("some-provider", "SOME_TOKEN")),
            false,
            tx
        ));
        let outcome = rig
            .run(&[value_prompt("some-provider", "SOME_TOKEN")], async |_| {})
            .await;
        let reason = outcome.expect_err("a duplicate card must refuse");
        assert!(reason.contains("already pending"), "{reason}");
    }

    #[tokio::test]
    async fn an_unreadable_store_aborts_the_launch_after_a_save() {
        let rig = Rig::new();
        let deps = ValueCardDeps {
            window: &rig.window,
            store: &MemStore {
                fail_load: true,
                ..MemStore::default()
            },
            wait: Duration::from_secs(5),
            host_value_available: &|_| false,
            announce: &|_| {},
            repaint: &|| {},
        };
        let prompts = [value_prompt("some-provider", "SOME_TOKEN")];
        let gate = gate_value_prompts(&prompts, &deps);
        let decide = async {
            let card = visible_card(&rig.window).await;
            rig.window.decide_credential(
                &card.id,
                CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                    value: "some-secret".into(),
                }),
            );
        };
        let (outcome, ()) = tokio::join!(gate, decide);
        let reason = outcome.expect_err("an unreadable store must abort");
        assert!(
            reason.contains("reading the credential store failed"),
            "{reason}"
        );
    }

    #[tokio::test]
    async fn a_store_that_cannot_persist_aborts_the_launch_after_a_save() {
        let rig = Rig::new();
        let deps = ValueCardDeps {
            window: &rig.window,
            store: &MemStore {
                fail_save: true,
                ..MemStore::default()
            },
            wait: Duration::from_secs(5),
            host_value_available: &|_| false,
            announce: &|_| {},
            repaint: &|| {},
        };
        let prompts = [value_prompt("some-provider", "SOME_TOKEN")];
        let gate = gate_value_prompts(&prompts, &deps);
        let decide = async {
            let card = visible_card(&rig.window).await;
            rig.window.decide_credential(
                &card.id,
                CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                    value: "some-secret".into(),
                }),
            );
        };
        let (outcome, ()) = tokio::join!(gate, decide);
        let reason = outcome.expect_err("an unpersistable store must abort");
        assert!(
            reason.contains("storing the value decision failed"),
            "{reason}"
        );
    }
}
