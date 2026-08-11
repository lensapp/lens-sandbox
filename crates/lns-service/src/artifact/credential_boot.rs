use lns_policy::connectors::{AuthKind, Connector};
use lns_policy::credentials::{CredentialStateFile, has_armed_entry};
use lns_spec::Credential;

use crate::credential_flow::connectors::declared_suppliers;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectPrompt {
    pub connector: String,
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

/// Plan each launch-gated connector (the suppliers of a definition's declared credentials): an `oauth` supplier with no armed machine grant blocks the boot on its sign-in, while a credential supplier stays armed — its consent gate is the reactive per-machine value decision at first use. Ids the catalog lacks are the unknown-id refusal's job, not this gate's.
pub fn plan_declared_connectors(
    declared: &[String],
    catalog: &[Connector],
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
                    connector: integ.id.clone(),
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

/// The connectors the sign-in gate plans: the supplier of each declared credential, in declaration order. A declaration names no connector, so the machine's catalog decides which one can obtain its value — and a credential nothing supplies asks for a pasted value at first use instead.
pub fn sign_in_gate_ids(credentials: &[Credential], catalog: &[Connector]) -> Vec<String> {
    declared_suppliers(credentials, catalog)
        .into_iter()
        .filter_map(|(_, supplier)| supplier)
        .map(|integ| integ.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_spec::{InjectionDef, InjectionKind};

    fn credential(env_var: &str, domain: &str) -> Credential {
        Credential {
            env_var: env_var.into(),
            placeholder: format!("lns-placeholder-{env_var}"),
            injections: vec![InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: domain.into(),
                header: None,
            }],
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
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: "api.some-oauth.example".into(),
                    header: None,
                }],
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
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: "api.some-provider.example".into(),
                    header: None,
                }],
            }),
            oauth: None,
            token_fallback: None,
        }
    }

    #[test]
    fn a_declared_oauth_connector_without_a_grant_blocks_on_a_connect() {
        let catalog = vec![oauth_connector("some-oauth", "SOME_OAUTH_TOKEN")];
        let plans = plan_declared_connectors(
            &["some-oauth".to_string()],
            &catalog,
            &CredentialStateFile::new(),
        );
        assert_eq!(
            plans,
            vec![SlotPlan::Connect(ConnectPrompt {
                connector: "some-oauth".into(),
            })]
        );
        assert_eq!(boot_gate(&plans), BootGate::AwaitConnect);
    }

    #[test]
    fn a_declared_oauth_connector_with_a_machine_grant_is_armed() {
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
        let plans = plan_declared_connectors(&["some-oauth".to_string()], &catalog, &state);
        assert_eq!(boot_gate(&plans), BootGate::StartWorkload);
    }

    #[test]
    fn a_declared_credential_connector_never_blocks_the_boot() {
        let catalog = vec![credential_connector("some-provider", "SOME_TOKEN")];
        let plans = plan_declared_connectors(
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
            "a credential supplier's consent gate is the reactive value decision, not the boot"
        );
    }

    #[test]
    fn a_blockless_catalog_entry_arms_as_a_no_op_instead_of_blocking() {
        let catalog = vec![Connector {
            id: "some-blockless".into(),
            name: None,
            auth_kind: AuthKind::Credential,
            routes: Vec::new(),
            credential: None,
            oauth: None,
            token_fallback: None,
        }];
        let plans = plan_declared_connectors(
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
        let plans = plan_declared_connectors(
            &["some-unknown".to_string()],
            &[],
            &CredentialStateFile::new(),
        );
        assert!(plans.is_empty(), "unknown ids are the refusal's job");
    }

    #[test]
    fn the_sign_in_gate_plans_the_supplier_of_each_declared_credential() {
        let catalog = vec![
            oauth_connector("some-oauth", "SOME_OAUTH_TOKEN"),
            credential_connector("some-provider", "SOME_TOKEN"),
        ];
        let credentials = vec![
            credential("FIRST_TOKEN", "api.some-oauth.example"),
            credential("SECOND_TOKEN", "api.some-oauth.example"),
            credential("THIRD_TOKEN", "api.some-provider.example"),
        ];
        assert_eq!(
            sign_in_gate_ids(&credentials, &catalog),
            vec!["some-oauth".to_string(), "some-provider".to_string()],
            "the oauth connector supplies the first declaration only, so its sign-in is planned once; the second falls back to a pasted value and gates nothing"
        );
    }

    #[test]
    fn a_credential_no_connector_supplies_never_gates_a_boot() {
        let catalog = vec![oauth_connector("some-oauth", "SOME_OAUTH_TOKEN")];
        assert!(
            sign_in_gate_ids(
                &[credential("SOME_TOKEN", "api.nobody-claims.example")],
                &catalog
            )
            .is_empty(),
            "with nothing to sign in to, the value is asked for at first use rather than blocking the boot"
        );
        assert!(sign_in_gate_ids(&[], &catalog).is_empty());
    }
}
