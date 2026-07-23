use lns_artifact::spec::CredentialSlot;
use lns_policy::connectors::{AuthKind, Connector, TokenFallback};
use lns_policy::credentials::{CredentialStateFile, has_armed_entry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectPrompt {
    pub connector: String,
    pub env: String,
    pub required: bool,
}

/// The pre-boot value card for a required credential-kind slot with no bound value on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValuePrompt {
    pub connector: String,
    pub env: String,
    pub token_fallback: Option<TokenFallback>,
    pub injection_domains: Vec<String>,
}

/// What the launch gate does for one required slot: boot silently, block on a card, or refuse outright.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotGatePlan {
    Armed { connector: String, env: String },
    NeedsValue(ValuePrompt),
    NeedsSignIn(ConnectPrompt),
    Refused(RequiredSlotFailure),
}

/// Plan each required credential slot against this machine's store: a bound slot arms, an unbound oauth-kind slot blocks on the sign-in, an unbound credential-kind slot blocks on the value card, and a machine-denied or bindless one refuses. The slot's `env` (a remap of the catalog default) is carried into every prompt; ids the catalog lacks are the unknown-id refusal's job, and duplicate `(name, env)` slots plan once.
pub fn plan_required_slots(
    slots: &[CredentialSlot],
    catalog: &[Connector],
    state: &CredentialStateFile,
) -> Vec<SlotGatePlan> {
    let mut seen = std::collections::HashSet::new();
    slots
        .iter()
        .filter(|s| s.required)
        .filter(|s| seen.insert((s.name.clone(), s.env.clone())))
        .filter_map(|s| {
            catalog
                .iter()
                .find(|integ| integ.id == s.name)
                .map(|integ| plan_one(s, integ, state))
        })
        .collect()
}

fn plan_one(slot: &CredentialSlot, integ: &Connector, state: &CredentialStateFile) -> SlotGatePlan {
    let armed = || SlotGatePlan::Armed {
        connector: slot.name.clone(),
        env: slot.env.clone(),
    };
    match integ.auth_kind {
        AuthKind::Oauth if has_armed_entry(state, &integ.id) => armed(),
        AuthKind::Oauth => SlotGatePlan::NeedsSignIn(ConnectPrompt {
            connector: slot.name.clone(),
            env: slot.env.clone(),
            required: true,
        }),
        AuthKind::Credential if denied_on_machine(state, &integ.id) => {
            SlotGatePlan::Refused(RequiredSlotFailure::Denied {
                connector: slot.name.clone(),
                env: slot.env.clone(),
            })
        }
        AuthKind::Credential if binds_for_launch(state, &integ.id) => armed(),
        AuthKind::Credential => match &integ.credential {
            Some(cred) => SlotGatePlan::NeedsValue(ValuePrompt {
                connector: slot.name.clone(),
                env: slot.env.clone(),
                token_fallback: integ.token_fallback.clone(),
                injection_domains: cred.injections.iter().map(|i| i.domain.clone()).collect(),
            }),
            // A bindless entry has no value to collect, so a card could never unblock it.
            None => SlotGatePlan::Refused(RequiredSlotFailure::Unbound {
                connector: slot.name.clone(),
                env: slot.env.clone(),
            }),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectChoice {
    Connect,
    Decline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotOutcome {
    Connected,
    LeftUnbound,
    AbortLaunch,
}

impl SlotOutcome {
    pub fn starts_workload(self) -> bool {
        !matches!(self, SlotOutcome::AbortLaunch)
    }
}

pub fn resolve_connect(prompt: &ConnectPrompt, choice: ConnectChoice) -> SlotOutcome {
    match choice {
        ConnectChoice::Connect => SlotOutcome::Connected,
        ConnectChoice::Decline if prompt.required => SlotOutcome::AbortLaunch,
        ConnectChoice::Decline => SlotOutcome::LeftUnbound,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequiredSlotFailure {
    Unbound { connector: String, env: String },
    Denied { connector: String, env: String },
}

impl RequiredSlotFailure {
    pub fn as_message(&self) -> String {
        match self {
            RequiredSlotFailure::Unbound { connector, env } => format!(
                "this sandbox requires the \"{connector}\" credential, injected as {env}, \
                 and no value is bound on this machine; bind it with \
                 `lns connector connect {connector}`, then run again"
            ),
            RequiredSlotFailure::Denied { connector, env } => format!(
                "this sandbox requires the \"{connector}\" credential, injected as {env}, \
                 and you have denied it on this machine; change the decision with \
                 `lns connector connect {connector}`, then run again"
            ),
        }
    }
}

fn denied_on_machine(state: &CredentialStateFile, id: &str) -> bool {
    matches!(
        state.get(id),
        Some(lns_policy::credentials::CredentialEntry::Deny)
    )
}

/// True when a value decision arms the slot for a launch: a stored or oauth value, or host-detect (the decision exists; the value arms at the boundary at request time).
fn binds_for_launch(state: &CredentialStateFile, id: &str) -> bool {
    matches!(
        state.get(id),
        Some(lns_policy::credentials::CredentialEntry::HostDetect)
    ) || has_armed_entry(state, id)
}

/// The headless view of [`plan_required_slots`]: with no window to card, an unbound required credential-kind slot fails fast — before any microVM boots — exactly as a machine-denied or bindless one does.
pub fn gate_required_slots(
    slots: &[CredentialSlot],
    catalog: &[Connector],
    state: &CredentialStateFile,
) -> Result<(), RequiredSlotFailure> {
    for plan in plan_required_slots(slots, catalog, state) {
        match plan {
            SlotGatePlan::NeedsValue(prompt) => {
                return Err(RequiredSlotFailure::Unbound {
                    connector: prompt.connector,
                    env: prompt.env,
                });
            }
            SlotGatePlan::Refused(failure) => return Err(failure),
            SlotGatePlan::Armed { .. } | SlotGatePlan::NeedsSignIn(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(required: bool) -> CredentialSlot {
        CredentialSlot {
            name: "some-provider".into(),
            env: "SOME_TOKEN".into(),
            required,
        }
    }

    fn oauth_connector(id: &str, env: &str) -> Connector {
        Connector {
            id: id.into(),
            name: None,
            auth_kind: AuthKind::Oauth,
            routes: Vec::new(),
            credential: None,
            oauth: Some(lns_policy::connectors::OauthAuth {
                flow: lns_policy::connectors::OauthFlow::Device,
                client_id: Some("some-client".into()),
                client_secret: None,
                scopes: Vec::new(),
                device_authorization_endpoint: Some("https://api.some-oauth.example/device".into()),
                authorization_endpoint: None,
                token_endpoint: "https://api.some-oauth.example/token".into(),
                userinfo_endpoint: None,
                account_field: None,
                env_var: env.into(),
                placeholder: format!("{id}-LNSPLACEHOLDER0000"),
                injections: Vec::new(),
            }),
            token_fallback: None,
        }
    }

    fn credential_connector(id: &str, env: &str) -> Connector {
        Connector {
            id: id.into(),
            name: None,
            auth_kind: AuthKind::Credential,
            routes: Vec::new(),
            credential: Some(lns_policy::connectors::CredentialAuth {
                env_var: env.into(),
                placeholder: format!("{id}-LNSPLACEHOLDER0000"),
                injections: vec![lns_policy::providers::InjectionDef {
                    kind: lns_policy::providers::InjectionKind::BearerHeader,
                    domain: "api.some-provider.example".into(),
                    header: None,
                }],
            }),
            oauth: None,
            token_fallback: Some(TokenFallback {
                help: Some("https://docs.example.test/token".into()),
                command: Some("some-cli setup-token".into()),
            }),
        }
    }

    fn required_slot(name: &str, env: &str) -> CredentialSlot {
        CredentialSlot {
            name: name.into(),
            env: env.into(),
            required: true,
        }
    }

    fn stored(value: &str) -> lns_policy::credentials::CredentialEntry {
        lns_policy::credentials::CredentialEntry::Stored {
            value: value.into(),
        }
    }

    fn value_env(plan: &SlotGatePlan) -> Option<&str> {
        match plan {
            SlotGatePlan::NeedsValue(prompt) => Some(&prompt.env),
            _ => None,
        }
    }

    #[test]
    fn an_unbound_required_credential_slot_plans_the_value_card() {
        let catalog = vec![credential_connector("some-provider", "SOME_TOKEN")];
        let plans = plan_required_slots(&[slot(true)], &catalog, &CredentialStateFile::new());
        assert_eq!(
            plans,
            vec![SlotGatePlan::NeedsValue(ValuePrompt {
                connector: "some-provider".into(),
                env: "SOME_TOKEN".into(),
                token_fallback: Some(TokenFallback {
                    help: Some("https://docs.example.test/token".into()),
                    command: Some("some-cli setup-token".into()),
                }),
                injection_domains: vec!["api.some-provider.example".into()],
            })],
            "the card carries the mint instructions and the domains the value reaches"
        );
    }

    #[test]
    fn the_value_card_carries_the_slots_env_remap_not_the_catalog_default() {
        let catalog = vec![credential_connector("some-provider", "SOME_TOKEN")];
        let plans = plan_required_slots(
            &[required_slot("some-provider", "PROVIDER_KEY")],
            &catalog,
            &CredentialStateFile::new(),
        );
        let envs: Vec<&str> = plans.iter().filter_map(value_env).collect();
        assert_eq!(envs, vec!["PROVIDER_KEY"], "got: {plans:?}");
    }

    #[test]
    fn the_sign_in_prompt_carries_the_slots_env_remap_not_the_catalog_default() {
        let catalog = vec![oauth_connector("some-oauth", "SOME_OAUTH_TOKEN")];
        let plans = plan_required_slots(
            &[required_slot("some-oauth", "OAUTH_KEY")],
            &catalog,
            &CredentialStateFile::new(),
        );
        assert_eq!(
            plans,
            vec![SlotGatePlan::NeedsSignIn(ConnectPrompt {
                connector: "some-oauth".into(),
                env: "OAUTH_KEY".into(),
                required: true,
            })]
        );
    }

    #[test]
    fn an_oauth_slot_with_a_machine_grant_arms_without_a_prompt() {
        let catalog = vec![oauth_connector("some-oauth", "SOME_OAUTH_TOKEN")];
        let mut state = CredentialStateFile::new();
        state.insert(
            "some-oauth".into(),
            lns_policy::credentials::CredentialEntry::Oauth {
                access_token: "some-access".into(),
                refresh_token: "some-refresh".into(),
                expires_at: 9999,
                scopes: vec![],
                account: None,
            },
        );
        let plans = plan_required_slots(
            &[required_slot("some-oauth", "SOME_OAUTH_TOKEN")],
            &catalog,
            &state,
        );
        assert_eq!(
            plans,
            vec![SlotGatePlan::Armed {
                connector: "some-oauth".into(),
                env: "SOME_OAUTH_TOKEN".into(),
            }]
        );
    }

    #[test]
    fn a_bound_or_host_detect_credential_slot_arms_without_a_prompt() {
        let catalog = vec![credential_connector("some-provider", "SOME_TOKEN")];
        let mut state = CredentialStateFile::new();
        state.insert("some-provider".into(), stored("some-secret"));
        let armed = vec![SlotGatePlan::Armed {
            connector: "some-provider".into(),
            env: "SOME_TOKEN".into(),
        }];
        assert_eq!(plan_required_slots(&[slot(true)], &catalog, &state), armed);
        state.insert(
            "some-provider".into(),
            lns_policy::credentials::CredentialEntry::HostDetect,
        );
        let plans = plan_required_slots(&[slot(true)], &catalog, &state);
        assert_eq!(
            plans, armed,
            "a host-detect decision exists; it arms at the boundary at request time"
        );
        assert_eq!(
            plans.iter().filter_map(value_env).count(),
            0,
            "an armed plan raises no value card"
        );
    }

    #[test]
    fn an_empty_stored_value_still_plans_the_value_card() {
        let catalog = vec![credential_connector("some-provider", "SOME_TOKEN")];
        let mut state = CredentialStateFile::new();
        state.insert("some-provider".into(), stored(""));
        let plans = plan_required_slots(&[slot(true)], &catalog, &state);
        assert!(
            matches!(plans.as_slice(), [SlotGatePlan::NeedsValue(_)]),
            "an empty value cannot arm the boundary: {plans:?}"
        );
    }

    #[test]
    fn a_machine_denied_credential_refuses_instead_of_carding() {
        let catalog = vec![credential_connector("some-provider", "SOME_TOKEN")];
        let mut state = CredentialStateFile::new();
        state.insert(
            "some-provider".into(),
            lns_policy::credentials::CredentialEntry::Deny,
        );
        let plans = plan_required_slots(&[slot(true)], &catalog, &state);
        assert_eq!(
            plans,
            vec![SlotGatePlan::Refused(RequiredSlotFailure::Denied {
                connector: "some-provider".into(),
                env: "SOME_TOKEN".into(),
            })],
            "a machine-wide deny is an explicit standing decision, not a prompt to re-ask on every launch"
        );
    }

    #[test]
    fn a_bindless_catalog_entry_refuses_because_no_card_could_unblock_it() {
        let catalog = vec![Connector {
            id: "some-blockless".into(),
            name: None,
            auth_kind: AuthKind::Credential,
            routes: Vec::new(),
            credential: None,
            oauth: None,
            token_fallback: None,
        }];
        let plans = plan_required_slots(
            &[required_slot("some-blockless", "SOME_TOKEN")],
            &catalog,
            &CredentialStateFile::new(),
        );
        assert_eq!(
            plans,
            vec![SlotGatePlan::Refused(RequiredSlotFailure::Unbound {
                connector: "some-blockless".into(),
                env: "SOME_TOKEN".into(),
            })]
        );
    }

    #[test]
    fn an_optional_slot_never_plans_a_gate() {
        let catalog = vec![credential_connector("some-provider", "SOME_TOKEN")];
        assert!(
            plan_required_slots(&[slot(false)], &catalog, &CredentialStateFile::new()).is_empty(),
            "an unbound optional slot runs reactively"
        );
    }

    #[test]
    fn a_slot_the_catalog_lacks_is_skipped_as_the_unknown_id_refusals_job() {
        let plans = plan_required_slots(&[slot(true)], &[], &CredentialStateFile::new());
        assert!(plans.is_empty());
    }

    #[test]
    fn duplicate_slots_plan_once_but_distinct_env_remaps_plan_separately() {
        let catalog = vec![credential_connector("some-provider", "SOME_TOKEN")];
        let plans = plan_required_slots(
            &[
                required_slot("some-provider", "SOME_TOKEN"),
                required_slot("some-provider", "SOME_TOKEN"),
                required_slot("some-provider", "PROVIDER_KEY"),
            ],
            &catalog,
            &CredentialStateFile::new(),
        );
        assert_eq!(plans.len(), 2, "got: {plans:?}");
        let envs: Vec<&str> = plans.iter().filter_map(value_env).collect();
        assert_eq!(
            envs,
            vec!["SOME_TOKEN", "PROVIDER_KEY"],
            "identical (name, env) slots coalesce; a distinct remap is its own capability"
        );
    }

    #[test]
    fn connecting_an_unbound_slot_binds_it_and_starts_the_workload() {
        let prompt = ConnectPrompt {
            connector: "some-provider".into(),
            env: "SOME_TOKEN".into(),
            required: true,
        };
        let outcome = resolve_connect(&prompt, ConnectChoice::Connect);
        assert_eq!(outcome, SlotOutcome::Connected);
        assert!(outcome.starts_workload());
    }

    #[test]
    fn declining_a_required_slot_aborts_the_launch() {
        let prompt = ConnectPrompt {
            connector: "some-provider".into(),
            env: "SOME_TOKEN".into(),
            required: true,
        };
        let outcome = resolve_connect(&prompt, ConnectChoice::Decline);
        assert_eq!(outcome, SlotOutcome::AbortLaunch);
        assert!(!outcome.starts_workload());
    }

    #[test]
    fn declining_an_optional_slot_proceeds_with_the_slot_unbound() {
        let prompt = ConnectPrompt {
            connector: "some-provider".into(),
            env: "SOME_TOKEN".into(),
            required: false,
        };
        let outcome = resolve_connect(&prompt, ConnectChoice::Decline);
        assert_eq!(outcome, SlotOutcome::LeftUnbound);
        assert!(outcome.starts_workload());
    }

    #[test]
    fn a_required_slot_with_no_entry_fails_as_unbound_with_the_full_fix() {
        let catalog = vec![credential_connector("some-provider", "SOME_TOKEN")];
        let err =
            gate_required_slots(&[slot(true)], &catalog, &CredentialStateFile::new()).unwrap_err();
        assert_eq!(
            err,
            RequiredSlotFailure::Unbound {
                connector: "some-provider".into(),
                env: "SOME_TOKEN".into(),
            }
        );
        let msg = err.as_message();
        assert!(msg.contains("\"some-provider\""), "got: {msg}");
        assert!(msg.contains("injected as SOME_TOKEN"), "got: {msg}");
        assert!(
            msg.contains("`lns connector connect some-provider`"),
            "got: {msg}"
        );
        assert!(msg.contains("no value is bound"), "got: {msg}");
    }

    #[test]
    fn a_required_slot_with_a_deny_entry_fails_distinctly_from_never_bound() {
        let catalog = vec![credential_connector("some-provider", "SOME_TOKEN")];
        let mut state = CredentialStateFile::new();
        state.insert(
            "some-provider".into(),
            lns_policy::credentials::CredentialEntry::Deny,
        );
        let err = gate_required_slots(&[slot(true)], &catalog, &state).unwrap_err();
        assert_eq!(
            err,
            RequiredSlotFailure::Denied {
                connector: "some-provider".into(),
                env: "SOME_TOKEN".into(),
            }
        );
        let msg = err.as_message();
        assert!(msg.contains("you have denied it"), "got: {msg}");
        assert!(
            !msg.contains("no value is bound"),
            "a deny must not read as never-bound: {msg}"
        );
        assert!(
            msg.contains("`lns connector connect some-provider`"),
            "got: {msg}"
        );
    }

    #[test]
    fn a_required_slot_with_an_empty_stored_value_fails_as_unbound() {
        let catalog = vec![credential_connector("some-provider", "SOME_TOKEN")];
        let mut state = CredentialStateFile::new();
        state.insert("some-provider".into(), stored(""));
        let err = gate_required_slots(&[slot(true)], &catalog, &state).unwrap_err();
        assert!(matches!(err, RequiredSlotFailure::Unbound { .. }));
    }

    #[test]
    fn a_bound_or_host_detect_decision_passes_the_required_gate() {
        let catalog = vec![credential_connector("some-provider", "SOME_TOKEN")];
        let mut state = CredentialStateFile::new();
        state.insert("some-provider".into(), stored("some-secret"));
        assert_eq!(gate_required_slots(&[slot(true)], &catalog, &state), Ok(()));
        state.insert(
            "some-provider".into(),
            lns_policy::credentials::CredentialEntry::HostDetect,
        );
        assert_eq!(
            gate_required_slots(&[slot(true)], &catalog, &state),
            Ok(()),
            "a host-detect decision exists; it arms at the boundary at request time"
        );
    }

    #[test]
    fn an_optional_slot_never_fails_the_gate() {
        let catalog = vec![credential_connector("some-provider", "SOME_TOKEN")];
        assert_eq!(
            gate_required_slots(&[slot(false)], &catalog, &CredentialStateFile::new()),
            Ok(()),
            "an unbound optional slot runs reactively"
        );
    }

    #[test]
    fn a_required_oauth_slot_defers_to_the_sign_in_gate() {
        let catalog = vec![oauth_connector("some-oauth", "SOME_OAUTH_TOKEN")];
        let slots = vec![required_slot("some-oauth", "SOME_OAUTH_TOKEN")];
        assert_eq!(
            gate_required_slots(&slots, &catalog, &CredentialStateFile::new()),
            Ok(()),
            "an oauth slot blocks on the sign-in gate, not the value refusal"
        );
    }

    #[test]
    fn a_slot_the_catalog_lacks_is_the_unknown_id_refusals_job() {
        let slots = vec![required_slot("some-unknown", "SOME_TOKEN")];
        assert_eq!(
            gate_required_slots(&slots, &[], &CredentialStateFile::new()),
            Ok(()),
            "unknown ids refuse via the unknown-connector path, not this gate"
        );
    }
}
