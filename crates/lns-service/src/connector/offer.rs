//! Which connector a destination offers, per `docs/sandbox-spec.md` §3.2.1.

use lns_artifact::connector::ConnectorDefinition;
use lns_policy::matching::destination_covers;

/// What a held destination offers. `Ambiguous` is reachable: install refuses an overlap it can see, and two mid-segment wildcards sharing hosts are one it cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer<'a> {
    None,
    One(&'a ConnectorDefinition),
    Ambiguous(Vec<&'a str>),
}

/// The connector a destination is offered for, among those this project has neither granted nor declined. A `serves` entry with no port matches any port; one with a port narrows to it (§3.2.1).
pub fn offers_for<'a>(
    destination: &str,
    installed: &'a [ConnectorDefinition],
    decided: &[String],
) -> Offer<'a> {
    let mut serving: Vec<&'a ConnectorDefinition> = installed
        .iter()
        .filter(|connector| !decided.contains(&connector.name))
        .filter(|connector| serves(connector, destination))
        .collect();
    match serving.len() {
        0 => Offer::None,
        1 => Offer::One(serving.remove(0)),
        _ => Offer::Ambiguous(serving.into_iter().map(|c| c.name.as_str()).collect()),
    }
}

fn serves(connector: &ConnectorDefinition, destination: &str) -> bool {
    connector
        .spec
        .serves
        .iter()
        .any(|pattern| destination_covers(pattern, destination))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connector(name: &str, serves: &[&str]) -> ConnectorDefinition {
        let json = serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": name,
            "spec": {
                "serves": serves,
                "methods": [{ "name": "token", "auth": { "kind": "token" } }],
            },
        });
        lns_artifact::connector::parse(json.to_string().as_bytes()).expect("a valid connector")
    }

    fn name_of<'a>(offer: &Offer<'a>) -> Option<&'a str> {
        match offer {
            Offer::One(connector) => Some(connector.name.as_str()),
            _ => None,
        }
    }

    #[test]
    fn a_served_host_offers_its_connector() {
        let installed = [connector("some-provider", &["api.some-provider.example"])];
        assert_eq!(
            name_of(&offers_for("api.some-provider.example", &installed, &[])),
            Some("some-provider")
        );
    }

    #[test]
    fn a_bare_host_serves_every_port() {
        // §3.2.1: the opposite default from egress.tcp, so a raw-stream connector is not installed and never offered.
        let installed = [connector("some-provider", &["db.some-provider.example"])];
        for destination in [
            "db.some-provider.example",
            "db.some-provider.example:443",
            "db.some-provider.example:5432",
        ] {
            assert_eq!(
                name_of(&offers_for(destination, &installed, &[])),
                Some("some-provider"),
                "{destination}"
            );
        }
    }

    #[test]
    fn a_ported_entry_narrows_to_that_port() {
        let installed = [connector(
            "some-provider",
            &["db.some-provider.example:5432"],
        )];
        assert_eq!(
            name_of(&offers_for(
                "db.some-provider.example:5432",
                &installed,
                &[]
            )),
            Some("some-provider")
        );
        assert_eq!(
            offers_for("db.some-provider.example:6432", &installed, &[]),
            Offer::None,
            "another port is not the service this connector serves"
        );
    }

    #[test]
    fn a_wildcard_entry_offers_for_every_host_it_covers() {
        let installed = [connector("some-provider", &["*.some-provider.example"])];
        for host in ["api.some-provider.example", "eu.api.some-provider.example"] {
            assert_eq!(
                name_of(&offers_for(host, &installed, &[])),
                Some("some-provider"),
                "{host}"
            );
        }
        assert_eq!(
            offers_for("some-provider.example.test", &installed, &[]),
            Offer::None
        );
    }

    #[test]
    fn a_destination_nothing_serves_offers_nothing() {
        let installed = [connector("some-provider", &["api.some-provider.example"])];
        let offer = offers_for("api.other-provider.example", &installed, &[]);
        assert_eq!(offer, Offer::None);
        assert_eq!(
            name_of(&offer),
            None,
            "nothing is offered, so nothing is named"
        );
    }

    #[test]
    fn a_machine_with_nothing_installed_offers_nothing() {
        assert_eq!(
            offers_for("api.some-provider.example", &[], &[]),
            Offer::None
        );
    }

    #[test]
    fn a_connector_this_project_already_decided_is_not_offered_again() {
        // §3.2.1: a destination asks only while the project has neither granted nor declined it.
        let installed = [connector("some-provider", &["api.some-provider.example"])];
        assert_eq!(
            offers_for(
                "api.some-provider.example",
                &installed,
                &["some-provider".to_string()]
            ),
            Offer::None
        );
    }

    #[test]
    fn one_project_s_decision_does_not_silence_another_connector() {
        let installed = [
            connector("some-provider", &["api.some-provider.example"]),
            connector("other-provider", &["api.other-provider.example"]),
        ];
        assert_eq!(
            name_of(&offers_for(
                "api.other-provider.example",
                &installed,
                &["some-provider".to_string()]
            )),
            Some("other-provider")
        );
    }

    #[test]
    fn two_connectors_serving_one_destination_are_ambiguous_rather_than_arbitrary() {
        // Install refuses an overlap it can see, but `destination_covers` cannot detect two mid-segment wildcards that share hosts, so this state is reachable and must not be resolved by picking one.
        let installed = [
            connector("some-provider", &["api.*.example"]),
            connector("other-provider", &["*.eu.example"]),
        ];
        let mut offer = offers_for("api.eu.example", &installed, &[]);
        if let Offer::Ambiguous(names) = &mut offer {
            names.sort();
        }
        assert_eq!(
            offer,
            Offer::Ambiguous(vec!["other-provider", "some-provider"]),
            "two connectors serve this destination, so it must not resolve to one"
        );
    }

    #[test]
    fn a_decided_connector_cannot_make_the_rest_ambiguous() {
        let installed = [
            connector("some-provider", &["api.*.example"]),
            connector("other-provider", &["*.eu.example"]),
        ];
        assert_eq!(
            name_of(&offers_for(
                "api.eu.example",
                &installed,
                &["some-provider".to_string()]
            )),
            Some("other-provider"),
            "one of an overlapping pair having been decided leaves the other unambiguous"
        );
    }

    #[test]
    fn a_connector_serving_several_destinations_is_offered_for_each() {
        let installed = [connector(
            "some-provider",
            &["api.some-provider.example", "auth.some-provider.example"],
        )];
        for host in ["api.some-provider.example", "auth.some-provider.example"] {
            assert_eq!(
                name_of(&offers_for(host, &installed, &[])),
                Some("some-provider"),
                "{host}"
            );
        }
    }
}
