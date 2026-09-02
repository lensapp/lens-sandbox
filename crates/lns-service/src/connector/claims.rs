//! Which declared credential an installed connector can answer, and with which marker (`docs/sandbox-spec.md` §3.1.7).

use std::collections::BTreeMap;

use lns_artifact::connector::ConnectorDefinition;

use crate::workload_env::{Filled, claimable};

/// The variable each declaration is left to a connector for, holding that connector's placeholder.
///
/// `undecided` carries only the connectors this run has neither granted nor declined: a decline is a standing no, and a grant fills the variable itself. `run_policy` decides the third silence — a document that denies everything a connector serves.
pub fn left_to_a_connector(
    declared: &[lns_spec::Credential],
    undecided: &[ConnectorDefinition],
    filled_by_a_grant: &BTreeMap<String, Filled>,
    run_policy: Option<&lns_policy::NetworkPolicy>,
) -> BTreeMap<String, Filled> {
    let mut left = BTreeMap::new();
    for variable in declared.iter().filter_map(|c| c.env_var.as_deref()) {
        if filled_by_a_grant.contains_key(variable) || !claimable(variable) {
            continue;
        }
        if let Some((connector, placeholder)) = answering(undecided, variable, run_policy) {
            left.insert(
                variable.to_string(),
                Filled {
                    connector,
                    placeholder,
                },
            );
        }
    }
    left
}

/// The one connector whose credentials claim this variable. A plain `env` key claim answers nothing: it carries no placeholder to lend.
fn answering(
    undecided: &[ConnectorDefinition],
    variable: &str,
    run_policy: Option<&lns_policy::NetworkPolicy>,
) -> Option<(String, String)> {
    undecided
        .iter()
        .filter(|c| can_be_asked_about(c, run_policy))
        .find_map(|connector| {
            connector
                .spec
                .methods
                .iter()
                .flat_map(|method| &method.credentials)
                .find(|credential| credential.env_var.as_deref() == Some(variable))
                .map(|credential| (connector.name.clone(), credential.placeholder.clone()))
        })
}

/// Whether a card could still fire for this connector. A run whose own document denies every destination it serves silences the card (§3.1.7), so a marker there could never be armed.
fn can_be_asked_about(
    connector: &ConnectorDefinition,
    run_policy: Option<&lns_policy::NetworkPolicy>,
) -> bool {
    let Some(policy) = run_policy else {
        return true;
    };
    connector
        .spec
        .serves
        .iter()
        .any(|pattern| !policy.denies_every_destination_of(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARKER: &str = "some-provider-LNSPLACEHOLDER000";

    fn declaring(variable: &str) -> Vec<lns_spec::Credential> {
        vec![lns_spec::Credential {
            env_var: Some(variable.to_string()),
            placeholder: "the_declarations_own_LNSPLACEHOLDER".to_string(),
            field: None,
            injections: Vec::new(),
        }]
    }

    fn connector(name: &str, methods: serde_json::Value) -> ConnectorDefinition {
        let document = serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": name,
            "spec": { "serves": [format!("api.{name}.example")], "methods": methods },
        });
        lns_artifact::connector::parse(document.to_string().as_bytes()).expect("a valid connector")
    }

    fn claiming(name: &str, variable: &str) -> ConnectorDefinition {
        connector(
            name,
            serde_json::json!([{
                "name": "token",
                "auth": { "kind": "token" },
                "credentials": [{ "envVar": variable, "placeholder": MARKER }],
            }]),
        )
    }

    #[test]
    fn a_declaration_takes_the_marker_of_the_connector_that_claims_its_variable() {
        // §3.1.7: the marker is the connector's placeholder, not the declaration's — it is the one the boundary substitutes for once the method is granted.
        assert_eq!(
            left_to_a_connector(
                &declaring("SOME_TOKEN"),
                &[claiming("some-provider", "SOME_TOKEN")],
                &Default::default(),
                None,
            ),
            [(
                "SOME_TOKEN".to_string(),
                Filled {
                    connector: "some-provider".to_string(),
                    placeholder: MARKER.to_string(),
                }
            )]
            .into()
        );
    }

    fn denying(pattern: &str) -> lns_policy::NetworkPolicy {
        lns_policy::NetworkPolicy {
            egress: lns_policy::Egress {
                http: vec![lns_policy::RouteRule::deny_host(pattern)],
                tcp: Vec::new(),
            },
        }
    }

    #[test]
    fn a_connector_whose_every_destination_the_run_denies_is_left_nothing() {
        // §3.1.7: a deny silences the card, so no card would ever fire — and an unarmable marker tells the workload it is signed in when nothing can ever sign it in.
        assert!(
            left_to_a_connector(
                &declaring("SOME_TOKEN"),
                &[claiming("some-provider", "SOME_TOKEN")],
                &Default::default(),
                Some(&denying("api.some-provider.example")),
            )
            .is_empty(),
            "no card could ever fire here, so the marker is left to nobody"
        );
    }

    #[test]
    fn a_deny_that_shouts_the_host_silences_the_card_just_as_loudly() {
        // Host comparison folds case everywhere else, so a document that shouts a destination must not read as denying only part of it.
        assert!(
            left_to_a_connector(
                &declaring("SOME_TOKEN"),
                &[claiming("some-provider", "SOME_TOKEN")],
                &Default::default(),
                Some(&denying("API.SOME-PROVIDER.EXAMPLE")),
            )
            .is_empty()
        );
    }

    #[test]
    fn a_run_that_denies_only_part_of_what_a_connector_serves_still_leaves_it_the_declaration() {
        // §3.1.7 says every destination; a card can still fire on the part that is left, so the marker is what gets a client there.
        assert!(
            !left_to_a_connector(
                &declaring("SOME_TOKEN"),
                &[claiming("some-provider", "SOME_TOKEN")],
                &Default::default(),
                Some(&denying("blocked.some-provider.example")),
            )
            .is_empty()
        );
    }

    #[test]
    fn a_declaration_no_installed_connector_claims_is_left_to_nobody() {
        assert!(
            left_to_a_connector(
                &declaring("SOME_TOKEN"),
                &[claiming("other-provider", "OTHER_TOKEN")],
                &Default::default(),
                None,
            )
            .is_empty(),
            "§3.1.7 leaves that one to nobody"
        );
    }

    #[test]
    fn a_variable_a_grant_already_fills_is_left_to_nobody() {
        // The granted method fills it with the placeholder the boundary is armed for, so a second writer could only disagree.
        let filled = [(
            "SOME_TOKEN".to_string(),
            Filled {
                connector: "some-provider".to_string(),
                placeholder: MARKER.to_string(),
            },
        )]
        .into();
        assert!(
            left_to_a_connector(
                &declaring("SOME_TOKEN"),
                &[claiming("some-provider", "SOME_TOKEN")],
                &filled,
                None,
            )
            .is_empty()
        );
    }

    #[test]
    fn a_method_claiming_the_variable_through_its_env_block_answers_nothing() {
        // A plain `env` key carries no placeholder, so there is no marker to lend and §3.1.7 leaves the declaration to nobody.
        let by_env = connector(
            "some-provider",
            serde_json::json!([{
                "name": "token",
                "auth": { "kind": "token" },
                "env": { "SOME_TOKEN": "not-a-marker" },
            }]),
        );
        assert!(
            left_to_a_connector(
                &declaring("SOME_TOKEN"),
                &[by_env],
                &Default::default(),
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn a_declaration_naming_a_variable_the_guest_composes_is_left_to_nobody() {
        for variable in ["PATH", "HOME", "USER", "LENS_SANDBOX_WORKLOAD_HOME"] {
            assert!(
                left_to_a_connector(
                    &declaring(variable),
                    &[claiming("some-provider", variable)],
                    &Default::default(),
                    None,
                )
                .is_empty(),
                "{variable} is the guest's to compose, whoever asks for it"
            );
        }
    }

    #[test]
    fn a_later_method_claiming_the_variable_answers_as_readily_as_the_first() {
        // Methods are alternatives and any of them may be the one that names the variable, so a search that stopped at the first would leave a declaration unanswered.
        let two_methods = connector(
            "some-provider",
            serde_json::json!([
                {
                    "name": "token",
                    "auth": { "kind": "token" },
                    "credentials": [{ "envVar": "OTHER_TOKEN", "placeholder": "other_LNSPLACEHOLDER00000" }],
                },
                {
                    "name": "sso",
                    "auth": { "kind": "token" },
                    "credentials": [{ "envVar": "SOME_TOKEN", "placeholder": MARKER }],
                },
            ]),
        );

        let left = left_to_a_connector(
            &declaring("SOME_TOKEN"),
            &[two_methods],
            &Default::default(),
            None,
        );

        assert_eq!(left["SOME_TOKEN"].placeholder, MARKER);
    }

    #[test]
    fn a_declaration_with_no_variable_to_fill_is_left_to_nobody() {
        // §4.1 lets a credential exist to be injected on the wire alone; it names no variable, so there is nothing to mark.
        let wire_only = vec![lns_spec::Credential {
            env_var: None,
            placeholder: "wire_only_LNSPLACEHOLDER0000".to_string(),
            field: None,
            injections: Vec::new(),
        }];
        assert!(
            left_to_a_connector(
                &wire_only,
                &[claiming("some-provider", "SOME_TOKEN")],
                &Default::default(),
                None,
            )
            .is_empty()
        );
    }
}
