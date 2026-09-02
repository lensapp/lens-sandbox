//! What an install refuses, so no later launch has to decide an ambiguous offer (`docs/sandbox-spec.md` §7.1).
//!
//! Only the cross-connector checks live here. What one document decides on its own
//! — a block a method may not carry, a fileset over budget — the parse already
//! refuses, and an install gets that by parsing.

use std::collections::BTreeSet;

use lns_artifact::connector::ConnectorDefinition;
use lns_policy::matching::destinations_overlap;

/// Why a candidate cannot join the installed set, holding what the message needs to name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// Two connectors would answer for one destination, so an offer could not choose.
    Serves {
        installed: String,
        theirs: String,
        mine: String,
    },
    /// Two connectors would claim one variable, so an injection could not choose.
    Variable { installed: String, variable: String },
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serves {
                installed,
                theirs,
                mine,
            } => write!(
                f,
                "{installed} already serves {theirs}, which overlaps {mine}; uninstall it first or install a connector that does not cover the same destination"
            ),
            Self::Variable {
                installed,
                variable,
            } => write!(
                f,
                "{installed} already claims the variable {variable}; one variable holds one value, so uninstall it first"
            ),
        }
    }
}

/// Every variable a connector claims: an `envVar` a credential injects, and a plain `env` key. Two methods of one connector may claim one, because they are alternatives and only one is ever applied, so this is the union over its methods (§7.1).
pub fn variables_claimed(connector: &ConnectorDefinition) -> BTreeSet<String> {
    let mut claimed = BTreeSet::new();
    for method in &connector.spec.methods {
        claimed.extend(method.credentials.iter().filter_map(|c| c.env_var.clone()));
        claimed.extend(method.env.keys().cloned());
    }
    claimed
}

/// Refuse `new` where it would collide with something already installed. A connector of the same name is its own update, not its own conflict.
pub fn refuse_a_conflict(
    new: &ConnectorDefinition,
    installed: &[ConnectorDefinition],
) -> Result<(), Conflict> {
    let mine = variables_claimed(new);
    for other in installed.iter().filter(|c| c.name != new.name) {
        if let Some(conflict) = serves_conflict(new, other) {
            return Err(conflict);
        }
        if let Some(variable) = variables_claimed(other).intersection(&mine).next() {
            return Err(Conflict::Variable {
                installed: other.name.clone(),
                variable: variable.clone(),
            });
        }
    }
    Ok(())
}

fn serves_conflict(new: &ConnectorDefinition, other: &ConnectorDefinition) -> Option<Conflict> {
    new.spec.serves.iter().find_map(|mine| {
        other
            .spec
            .serves
            .iter()
            .find(|theirs| destinations_overlap(mine, theirs))
            .map(|theirs| Conflict::Serves {
                installed: other.name.clone(),
                theirs: theirs.clone(),
                mine: mine.clone(),
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connector(name: &str, yaml_spec: &str) -> ConnectorDefinition {
        let json = serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": name,
            "spec": serde_yaml::from_str::<serde_json::Value>(yaml_spec).unwrap(),
        });
        lns_artifact::connector::parse(json.to_string().as_bytes()).unwrap()
    }

    /// A connector must declare a method, so the smallest one that parses carries a payload-free method.
    fn serving(name: &str, host: &str) -> ConnectorDefinition {
        connector(
            name,
            &format!(
                r#"
serves: ["{host}"]
methods:
  - name: token
    auth: {{ kind: token }}
"#
            ),
        )
    }

    fn claiming(name: &str, host: &str, var: &str) -> ConnectorDefinition {
        connector(
            name,
            &format!(
                r#"
serves: ["{host}"]
methods:
  - name: token
    auth: {{ kind: token }}
    credentials:
      - envVar: {var}
        placeholder: some_LNSPLACEHOLDER0000000000
"#
            ),
        )
    }

    #[test]
    fn a_connector_serving_a_destination_an_installed_wildcard_covers_is_refused() {
        let installed = [serving("some-provider", "*.some-provider.example")];
        let err = refuse_a_conflict(
            &serving("other-provider", "api.some-provider.example"),
            &installed,
        )
        .expect_err("an ambiguous offer must be refused at install");
        assert_eq!(
            err,
            Conflict::Serves {
                installed: "some-provider".to_string(),
                theirs: "*.some-provider.example".to_string(),
                mine: "api.some-provider.example".to_string(),
            }
        );
    }

    #[test]
    fn the_refusal_names_the_installed_connector_and_both_patterns() {
        let installed = [serving("some-provider", "*.some-provider.example")];
        let message = refuse_a_conflict(
            &serving("other-provider", "api.some-provider.example"),
            &installed,
        )
        .expect_err("must conflict")
        .to_string();
        for expected in [
            "some-provider",
            "*.some-provider.example",
            "api.some-provider.example",
        ] {
            assert!(
                message.contains(expected),
                "{expected:?} missing: {message}"
            );
        }
    }

    #[test]
    fn two_connectors_on_different_ports_of_one_host_coexist() {
        let installed = [serving("some-provider", "db.shared.example:5432")];
        assert_eq!(
            refuse_a_conflict(
                &serving("other-provider", "db.shared.example:6432"),
                &installed
            ),
            Ok(())
        );
    }

    #[test]
    fn reinstalling_a_connector_does_not_conflict_with_the_copy_it_replaces() {
        // Without the self-filter no connector is ever updatable: its own installed `serves` overlaps itself.
        let installed = [claiming(
            "some-provider",
            "api.some-provider.example",
            "SOME_TOKEN",
        )];
        assert_eq!(
            refuse_a_conflict(
                &claiming("some-provider", "api.some-provider.example", "SOME_TOKEN"),
                &installed
            ),
            Ok(())
        );
    }

    #[test]
    fn a_variable_an_installed_connector_claims_is_refused_though_the_destinations_differ() {
        let installed = [claiming(
            "some-provider",
            "api.some-provider.example",
            "SHARED",
        )];
        let err = refuse_a_conflict(
            &claiming("other-provider", "api.other-provider.example", "SHARED"),
            &installed,
        )
        .expect_err("one variable holds one value");
        assert_eq!(
            err,
            Conflict::Variable {
                installed: "some-provider".to_string(),
                variable: "SHARED".to_string(),
            }
        );
        assert!(err.to_string().contains("SHARED"));
    }

    #[test]
    fn a_plain_env_key_claims_a_variable_just_as_an_env_var_does() {
        let installed = [claiming(
            "some-provider",
            "api.some-provider.example",
            "SOME_REGION",
        )];
        let candidate = connector(
            "other-provider",
            r#"
serves: [api.other-provider.example]
methods:
  - name: token
    auth: { kind: token }
    env:
      SOME_REGION: eu
"#,
        );
        assert!(refuse_a_conflict(&candidate, &installed).is_err());
    }

    #[test]
    fn two_methods_of_one_connector_may_claim_one_variable() {
        let candidate = connector(
            "some-provider",
            r#"
serves: [api.some-provider.example]
methods:
  - name: token
    auth: { kind: token }
    credentials:
      - envVar: SOME_TOKEN
        placeholder: some_LNSPLACEHOLDER0000000000
  - name: session
    auth: { kind: token }
    credentials:
      - envVar: SOME_TOKEN
        placeholder: some_LNSPLACEHOLDER0000000000
"#,
        );
        assert_eq!(variables_claimed(&candidate).len(), 1);
        assert_eq!(refuse_a_conflict(&candidate, &[]), Ok(()));
    }

    #[test]
    fn a_credential_injecting_by_placeholder_alone_claims_no_variable() {
        let candidate = connector(
            "some-provider",
            r#"
serves: [api.some-provider.example]
methods:
  - name: token
    auth: { kind: token }
    credentials:
      - placeholder: some_LNSPLACEHOLDER0000000000
        injections:
          - kind: bearer_header
            domain: api.some-provider.example
"#,
        );
        assert!(variables_claimed(&candidate).is_empty());
    }

    #[test]
    fn an_empty_installed_set_refuses_nothing() {
        assert_eq!(
            refuse_a_conflict(&serving("some-provider", "api.some-provider.example"), &[]),
            Ok(())
        );
    }
}
