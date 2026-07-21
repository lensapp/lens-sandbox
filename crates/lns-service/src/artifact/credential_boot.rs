use lns_artifact::spec::CredentialSlot;
use lns_policy::credentials::{CredentialStateFile, has_armed_entry};
use lns_policy::integrations::{AuthKind, Integration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub placeholder: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectPrompt {
    pub integration: String,
    pub env: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotPlan {
    Armed { env: String, placeholder: String },
    Connect(ConnectPrompt),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootGate {
    StartWorkload,
    AwaitConnect,
}

pub fn plan_slot(slot: &CredentialSlot, binding: Option<Binding>) -> SlotPlan {
    match binding {
        Some(binding) => SlotPlan::Armed {
            env: slot.env.clone(),
            placeholder: binding.placeholder,
        },
        None => SlotPlan::Connect(ConnectPrompt {
            integration: slot.name.clone(),
            env: slot.env.clone(),
            required: slot.required,
        }),
    }
}

/// Plan each launch-gated id (a definition's required credential slots): an `oauth` id with no armed machine grant blocks the boot on a required connect (the sign-in), while a credential id stays armed — its consent gate is the reactive per-machine value decision at first use. Ids the catalog lacks are the unknown-id refusal's job, not this gate's.
pub fn plan_declared_integrations(
    declared: &[String],
    catalog: &[Integration],
    state: &CredentialStateFile,
) -> Vec<SlotPlan> {
    declared
        .iter()
        .filter_map(|id| catalog.iter().find(|integ| &integ.id == id))
        .map(|integ| {
            let (env, placeholder) = match (&integ.oauth, &integ.credential) {
                (Some(o), _) => (o.env_var.clone(), o.placeholder.clone()),
                (None, Some(c)) => (c.env_var.clone(), c.placeholder.clone()),
                (None, None) => (String::new(), String::new()),
            };
            if integ.auth_kind == AuthKind::Oauth && !has_armed_entry(state, &integ.id) {
                SlotPlan::Connect(ConnectPrompt {
                    integration: integ.id.clone(),
                    env,
                    required: true,
                })
            } else {
                SlotPlan::Armed { env, placeholder }
            }
        })
        .collect()
}

pub fn boot_gate(plans: &[SlotPlan]) -> BootGate {
    if plans.iter().all(|p| matches!(p, SlotPlan::Armed { .. })) {
        BootGate::StartWorkload
    } else {
        BootGate::AwaitConnect
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

/// The ids the sign-in gate plans: only the definition's required credential slots, deduplicated in declaration order. A bare `spec.integrations` id is disclosure, not a launch contract — it is never force-armed and so never blocks the boot; its consent is the reactive connect offer on first use.
pub fn sign_in_gate_ids(slots: &[CredentialSlot]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    slots
        .iter()
        .filter(|s| s.required)
        .map(|s| s.name.clone())
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequiredSlotFailure {
    Unbound { integration: String, env: String },
    Denied { integration: String, env: String },
}

impl RequiredSlotFailure {
    pub fn as_message(&self) -> String {
        match self {
            RequiredSlotFailure::Unbound { integration, env } => format!(
                "this sandbox requires the \"{integration}\" credential, injected as {env}, \
                 and no value is bound on this machine; bind it with \
                 `lns integration connect {integration}`, then run again"
            ),
            RequiredSlotFailure::Denied { integration, env } => format!(
                "this sandbox requires the \"{integration}\" credential, injected as {env}, \
                 and you have denied it on this machine; change the decision with \
                 `lns integration connect {integration}`, then run again"
            ),
        }
    }
}

/// True when a value decision arms the slot for a launch: a stored or oauth value, or host-detect (the decision exists; the value arms at the boundary at request time).
fn binds_for_launch(state: &CredentialStateFile, id: &str) -> bool {
    matches!(
        state.get(id),
        Some(lns_policy::credentials::CredentialEntry::HostDetect)
    ) || has_armed_entry(state, id)
}

/// Fail a required credential-kind slot fast — before any microVM boots — when this machine has no armed value for it (or has denied it, a distinct refusal). Oauth-kind slots defer to the sign-in gate and ids the catalog lacks to the unknown-id refusal.
pub fn gate_required_slots(
    slots: &[CredentialSlot],
    catalog: &[Integration],
    state: &CredentialStateFile,
) -> Result<(), RequiredSlotFailure> {
    for slot in slots.iter().filter(|s| s.required) {
        let Some(integ) = catalog.iter().find(|i| i.id == slot.name) else {
            continue;
        };
        if integ.auth_kind == AuthKind::Oauth {
            continue;
        }
        if binds_for_launch(state, &slot.name) {
            continue;
        }
        let failure = match state.get(&slot.name) {
            Some(lns_policy::credentials::CredentialEntry::Deny) => RequiredSlotFailure::Denied {
                integration: slot.name.clone(),
                env: slot.env.clone(),
            },
            _ => RequiredSlotFailure::Unbound {
                integration: slot.name.clone(),
                env: slot.env.clone(),
            },
        };
        return Err(failure);
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

    fn oauth_integration(id: &str, env: &str) -> Integration {
        Integration {
            id: id.into(),
            name: None,
            auth_kind: AuthKind::Oauth,
            routes: Vec::new(),
            credential: None,
            oauth: Some(lns_policy::integrations::OauthAuth {
                flow: lns_policy::integrations::OauthFlow::Device,
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

    fn credential_integration(id: &str, env: &str) -> Integration {
        Integration {
            id: id.into(),
            name: None,
            auth_kind: AuthKind::Credential,
            routes: Vec::new(),
            credential: Some(lns_policy::integrations::CredentialAuth {
                env_var: env.into(),
                placeholder: format!("{id}-LNSPLACEHOLDER0000"),
                injections: Vec::new(),
            }),
            oauth: None,
            token_fallback: None,
        }
    }

    #[test]
    fn a_declared_oauth_integration_without_a_grant_blocks_on_a_required_connect() {
        let catalog = vec![oauth_integration("some-oauth", "SOME_OAUTH_TOKEN")];
        let plans = plan_declared_integrations(
            &["some-oauth".to_string()],
            &catalog,
            &CredentialStateFile::new(),
        );
        assert_eq!(
            plans,
            vec![SlotPlan::Connect(ConnectPrompt {
                integration: "some-oauth".into(),
                env: "SOME_OAUTH_TOKEN".into(),
                required: true,
            })]
        );
        assert_eq!(boot_gate(&plans), BootGate::AwaitConnect);
    }

    #[test]
    fn a_declared_oauth_integration_with_a_machine_grant_is_armed() {
        let catalog = vec![oauth_integration("some-oauth", "SOME_OAUTH_TOKEN")];
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
        let plans = plan_declared_integrations(&["some-oauth".to_string()], &catalog, &state);
        assert_eq!(boot_gate(&plans), BootGate::StartWorkload);
    }

    #[test]
    fn a_declared_credential_integration_never_blocks_the_boot() {
        let catalog = vec![credential_integration("some-provider", "SOME_TOKEN")];
        let plans = plan_declared_integrations(
            &["some-provider".to_string()],
            &catalog,
            &CredentialStateFile::new(),
        );
        assert_eq!(
            plans,
            vec![SlotPlan::Armed {
                env: "SOME_TOKEN".into(),
                placeholder: "some-provider-LNSPLACEHOLDER0000".into(),
            }],
            "a credential id's consent gate is the reactive value decision, not the boot"
        );
    }

    #[test]
    fn a_blockless_catalog_entry_arms_as_a_no_op_instead_of_blocking() {
        let catalog = vec![Integration {
            id: "some-blockless".into(),
            name: None,
            auth_kind: AuthKind::Credential,
            routes: Vec::new(),
            credential: None,
            oauth: None,
            token_fallback: None,
        }];
        let plans = plan_declared_integrations(
            &["some-blockless".to_string()],
            &catalog,
            &CredentialStateFile::new(),
        );
        assert_eq!(
            plans,
            vec![SlotPlan::Armed {
                env: String::new(),
                placeholder: String::new(),
            }],
            "an entry the catalog validator would refuse must never block a boot"
        );
        assert_eq!(boot_gate(&plans), BootGate::StartWorkload);
    }

    #[test]
    fn an_id_the_catalog_lacks_is_skipped_by_the_gate() {
        let plans = plan_declared_integrations(
            &["some-unknown".to_string()],
            &[],
            &CredentialStateFile::new(),
        );
        assert!(plans.is_empty(), "unknown ids are the refusal's job");
    }

    #[test]
    fn an_unbound_slot_forms_a_connect_prompt_that_discloses_its_target() {
        let plan = plan_slot(&slot(true), None);
        assert_eq!(
            plan,
            SlotPlan::Connect(ConnectPrompt {
                integration: "some-provider".into(),
                env: "SOME_TOKEN".into(),
                required: true,
            })
        );
    }

    #[test]
    fn connecting_an_unbound_slot_binds_it_and_starts_the_workload() {
        let prompt = ConnectPrompt {
            integration: "some-provider".into(),
            env: "SOME_TOKEN".into(),
            required: true,
        };
        let outcome = resolve_connect(&prompt, ConnectChoice::Connect);
        assert_eq!(outcome, SlotOutcome::Connected);
        assert!(outcome.starts_workload());
    }

    #[test]
    fn a_bound_slot_arms_under_its_env_without_a_prompt() {
        let plan = plan_slot(
            &slot(true),
            Some(Binding {
                placeholder: "some-provider-LNSPLACEHOLDER0000".into(),
            }),
        );
        assert_eq!(
            plan,
            SlotPlan::Armed {
                env: "SOME_TOKEN".into(),
                placeholder: "some-provider-LNSPLACEHOLDER0000".into(),
            }
        );
        assert_eq!(
            boot_gate(std::slice::from_ref(&plan)),
            BootGate::StartWorkload
        );
    }

    #[test]
    fn declining_a_required_slot_aborts_the_launch() {
        let prompt = ConnectPrompt {
            integration: "some-provider".into(),
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
            integration: "some-provider".into(),
            env: "SOME_TOKEN".into(),
            required: false,
        };
        let outcome = resolve_connect(&prompt, ConnectChoice::Decline);
        assert_eq!(outcome, SlotOutcome::LeftUnbound);
        assert!(outcome.starts_workload());
    }

    fn stored(value: &str) -> lns_policy::credentials::CredentialEntry {
        lns_policy::credentials::CredentialEntry::Stored {
            value: value.into(),
        }
    }

    #[test]
    fn sign_in_gate_ids_are_the_required_slots_only_once_each() {
        let slots = vec![
            CredentialSlot {
                name: "some-oauth".into(),
                env: "SOME_OAUTH_TOKEN".into(),
                required: true,
            },
            CredentialSlot {
                name: "some-oauth".into(),
                env: "SOME_OAUTH_TOKEN".into(),
                required: true,
            },
            CredentialSlot {
                name: "other-provider".into(),
                env: "OTHER_TOKEN".into(),
                required: true,
            },
            CredentialSlot {
                name: "optional-provider".into(),
                env: "OPTIONAL_TOKEN".into(),
                required: false,
            },
        ];
        assert_eq!(
            sign_in_gate_ids(&slots),
            vec!["some-oauth".to_string(), "other-provider".to_string()],
            "required slots join once; an optional slot never blocks a sign-in; a bare declared id never gates"
        );
        assert!(sign_in_gate_ids(&[]).is_empty());
    }

    #[test]
    fn a_required_slot_with_no_entry_fails_as_unbound_with_the_full_fix() {
        let catalog = vec![credential_integration("some-provider", "SOME_TOKEN")];
        let err =
            gate_required_slots(&[slot(true)], &catalog, &CredentialStateFile::new()).unwrap_err();
        assert_eq!(
            err,
            RequiredSlotFailure::Unbound {
                integration: "some-provider".into(),
                env: "SOME_TOKEN".into(),
            }
        );
        let msg = err.as_message();
        assert!(msg.contains("\"some-provider\""), "got: {msg}");
        assert!(msg.contains("injected as SOME_TOKEN"), "got: {msg}");
        assert!(
            msg.contains("`lns integration connect some-provider`"),
            "got: {msg}"
        );
        assert!(msg.contains("no value is bound"), "got: {msg}");
    }

    #[test]
    fn a_required_slot_with_a_deny_entry_fails_distinctly_from_never_bound() {
        let catalog = vec![credential_integration("some-provider", "SOME_TOKEN")];
        let mut state = CredentialStateFile::new();
        state.insert(
            "some-provider".into(),
            lns_policy::credentials::CredentialEntry::Deny,
        );
        let err = gate_required_slots(&[slot(true)], &catalog, &state).unwrap_err();
        assert_eq!(
            err,
            RequiredSlotFailure::Denied {
                integration: "some-provider".into(),
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
            msg.contains("`lns integration connect some-provider`"),
            "got: {msg}"
        );
    }

    #[test]
    fn a_required_slot_with_an_empty_stored_value_fails_as_unbound() {
        let catalog = vec![credential_integration("some-provider", "SOME_TOKEN")];
        let mut state = CredentialStateFile::new();
        state.insert("some-provider".into(), stored(""));
        let err = gate_required_slots(&[slot(true)], &catalog, &state).unwrap_err();
        assert!(matches!(err, RequiredSlotFailure::Unbound { .. }));
    }

    #[test]
    fn a_bound_or_host_detect_decision_passes_the_required_gate() {
        let catalog = vec![credential_integration("some-provider", "SOME_TOKEN")];
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
        let catalog = vec![credential_integration("some-provider", "SOME_TOKEN")];
        assert_eq!(
            gate_required_slots(&[slot(false)], &catalog, &CredentialStateFile::new()),
            Ok(()),
            "an unbound optional slot runs reactively"
        );
    }

    #[test]
    fn a_required_oauth_slot_defers_to_the_sign_in_gate() {
        let catalog = vec![oauth_integration("some-oauth", "SOME_OAUTH_TOKEN")];
        let slots = vec![CredentialSlot {
            name: "some-oauth".into(),
            env: "SOME_OAUTH_TOKEN".into(),
            required: true,
        }];
        assert_eq!(
            gate_required_slots(&slots, &catalog, &CredentialStateFile::new()),
            Ok(()),
            "an oauth slot blocks on the sign-in gate, not the value refusal"
        );
    }

    #[test]
    fn a_slot_the_catalog_lacks_is_the_unknown_id_refusals_job() {
        let slots = vec![CredentialSlot {
            name: "some-unknown".into(),
            env: "SOME_TOKEN".into(),
            required: true,
        }];
        assert_eq!(
            gate_required_slots(&slots, &[], &CredentialStateFile::new()),
            Ok(()),
            "unknown ids refuse via the unknown-integration path, not this gate"
        );
    }
}
