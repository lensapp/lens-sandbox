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

/// Plan each definition-declared integration for the launch gate: an `oauth` id with no armed machine grant blocks the boot on a required connect (the sign-in), while a credential id stays armed — its consent gate is the reactive per-machine value decision at first use. Ids the catalog lacks are the unknown-id refusal's job, not this gate's.
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
}
