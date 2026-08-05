use std::collections::{HashMap, HashSet};

use lns_policy::connectors::{AuthKind, Connector, OauthAuth, OauthFlow};
use lns_policy::grants::{GrantRecord, GrantVerdict, WorkloadGrantFile, WorkloadIdentity};
use lns_policy::providers::ProviderDef;
use lns_policy::{Policy, RouteRule};

use crate::credential_flow::providers::{DefProvider, Provider};

/// The wire providers, routes, and (for oauth entries) device-flow / pkce configs a run's applied connectors contribute.
#[derive(Default)]
pub struct AppliedConnectors {
    pub providers: Vec<DefProvider>,
    pub routes: Vec<RouteRule>,
    pub oauth_configs: HashMap<String, OauthAuth>,
    pub pkce_configs: HashMap<String, OauthAuth>,
}

/// Catalog connectors that aren't yet connected, with their routes held ready to allow live on connect, and device-flow / pkce configs for a sign-in dance on connect; a definition-declared entry seeds its placeholder env so the workload attempts its first request, while the rest stay detect-only.
#[derive(Default)]
pub struct ConnectableConnectors {
    pub providers: Vec<DefProvider>,
    pub routes: HashMap<String, Vec<RouteRule>>,
    pub oauth_configs: HashMap<String, OauthAuth>,
    pub pkce_configs: HashMap<String, OauthAuth>,
}

/// The env/placeholder/injection wiring a connector seeds, taken from whichever block its authKind carries.
fn wire_provider_def(integ: &Connector) -> Option<ProviderDef> {
    let (env_var, placeholder, injections) = match integ.auth_kind {
        AuthKind::Credential => {
            let c = integ.credential.as_ref()?;
            (&c.env_var, &c.placeholder, &c.injections)
        }
        AuthKind::Oauth => {
            let o = integ.oauth.as_ref()?;
            (&o.env_var, &o.placeholder, &o.injections)
        }
    };
    Some(ProviderDef {
        id: integ.id.clone(),
        env_var: env_var.clone(),
        placeholder: placeholder.clone(),
        injections: injections.clone(),
    })
}

fn wire_provider(integ: &Connector) -> Option<DefProvider> {
    wire_provider_def(integ).map(DefProvider::new)
}

/// The oauth block usable for a device sign-in: the device flow with a client_id baked in (community builds ship none, so they fall back to the token paste).
fn signin_oauth(integ: &Connector) -> Option<&OauthAuth> {
    integ.oauth.as_ref().filter(|o| {
        o.flow == OauthFlow::Device && o.client_id.as_deref().is_some_and(|c| !c.is_empty())
    })
}

/// The oauth block usable for a pkce browser sign-in: the pkce flow with an authorization endpoint to redirect through.
fn signin_pkce(integ: &Connector) -> Option<&OauthAuth> {
    integ
        .oauth
        .as_ref()
        .filter(|o| o.flow == OauthFlow::Pkce && o.authorization_endpoint.is_some())
}

/// The env var and injection domains a connector discloses on its consent card, or `None` when its authKind carries no usable block.
pub fn disclosure_of(integ: &Connector) -> Option<(String, Vec<String>)> {
    wire_provider(integ).map(|p| p.disclosure_snapshot())
}

/// A connector this workload has already answered for, either way: a grant recorded against the same disclosure the card showed. A redefined connector is a different question, so neither a yes nor a no carries over to a new shape.
fn is_decided(
    integ: &Connector,
    project: &str,
    workload: &WorkloadIdentity,
    grants: &WorkloadGrantFile,
) -> bool {
    let Some(grant) = grants.lookup(project, workload, &integ.id) else {
        return false;
    };
    disclosure_of(integ)
        .is_some_and(|(env_var, domains)| grant.matches_disclosure(&env_var, &domains))
}

/// The policy a launch actually runs: the user's own rules minus the allows that would let a request past a connector nobody has decided yet, so the first request to a claimed domain is held and the connect offer gets its chance. Once the connector is connected or decided either way, the withheld rule applies as written. Only an `offerable` connector can withhold anything — a connector with no path to a card could never release the rule again.
pub fn withhold_undecided_connector_allows(
    policy: &Policy,
    catalog: &[Connector],
    offerable: &HashSet<String>,
    project: &str,
    workload: &WorkloadIdentity,
    grants: &WorkloadGrantFile,
) -> Policy {
    let undecided: Vec<&str> = catalog
        .iter()
        .filter(|integ| offerable.contains(&integ.id))
        // `offerable` is a launch-time snapshot, so a connector connected since — by a policy edit, which writes no grant — is still in it.
        .filter(|integ| !policy.connectors.iter().any(|id| id == &integ.id))
        .filter(|integ| !is_decided(integ, project, workload, grants))
        .flat_map(claimed_domains)
        .collect();
    let mut out = policy.clone();
    out.network.egress.http.retain(|rule| {
        rule.verdict != lns_policy::Verdict::Allow
            || !undecided
                .iter()
                .any(|claimed| domains_overlap(claimed, &rule.match_pattern))
    });
    out
}

/// Pairs each connectable connector's id with its catalog display name and route patterns, so a held request to one of those domains can offer to connect it instead of asking about the bare host.
pub fn offerable_connectors(
    connectable: &ConnectableConnectors,
    catalog: &[Connector],
) -> Vec<crate::approval_flow::session::OfferableConnector> {
    connectable
        .routes
        .iter()
        .map(|(id, routes)| {
            let entry = catalog.iter().find(|i| &i.id == id);
            crate::approval_flow::session::OfferableConnector {
                id: id.clone(),
                display_name: entry
                    .map(|i| i.display_name().to_string())
                    .unwrap_or_else(|| id.clone()),
                patterns: routes.iter().map(|r| r.match_pattern.clone()).collect(),
                token_fallback: entry.and_then(|i| i.token_fallback.clone()),
            }
        })
        .collect()
}

/// The allow-routes a set of connected connector ids contributes, re-derived from the catalog so boot and a watcher reload reconstruct the same live routes from an id-only policy.
pub fn applied_connector_routes(ids: &[String], catalog: &[Connector]) -> Vec<RouteRule> {
    let applied: HashSet<&str> = ids.iter().map(String::as_str).collect();
    catalog
        .iter()
        .filter(|integ| applied.contains(integ.id.as_str()))
        .flat_map(|integ| integ.routes.iter().map(|r| r.to_route_rule()))
        .collect()
}

/// Definition-declared ids the effective catalog cannot arm; each refuses the launch, unlike a stale `lns-policy.yaml` id which stays a tolerant skip.
pub fn unknown_connector_ids(declared: &[String], catalog: &[Connector]) -> Vec<String> {
    let known: HashSet<&str> = catalog.iter().map(|i| i.id.as_str()).collect();
    declared
        .iter()
        .filter(|id| !known.contains(id.as_str()))
        .cloned()
        .collect()
}

/// The launch-refusal message for definition-declared ids missing from the machine catalog, pointing at `lns connector add`.
pub fn unknown_connectors_refusal(unknown: &[String]) -> String {
    let ids = unknown
        .iter()
        .map(|id| format!("\"{id}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "the sandbox definition declares connector {ids} which this machine's catalog does not know; \
         declare it with `lns connector add`, or remove it from spec.connectors"
    )
}

/// Resolves the policy's applied connector ids against the effective catalog.
pub fn resolve_applied_connectors(policy: &Policy, catalog: &[Connector]) -> AppliedConnectors {
    let applied: HashSet<&str> = policy.connectors.iter().map(String::as_str).collect();

    let mut out = AppliedConnectors {
        routes: applied_connector_routes(&policy.connectors, catalog),
        ..AppliedConnectors::default()
    };
    for integ in catalog {
        if !applied.contains(integ.id.as_str()) {
            continue;
        }
        if let Some(p) = wire_provider(integ) {
            out.providers.push(p);
        }
        if let Some(o) = signin_oauth(integ) {
            out.oauth_configs.insert(integ.id.clone(), o.clone());
        }
        if let Some(o) = signin_pkce(integ) {
            out.pkce_configs.insert(integ.id.clone(), o.clone());
        }
    }
    out
}

/// A definition's credential slots resolve like declared connectors, with each slot's env name overriding the catalog default and winning over a same-id declared entry so the remap holds.
pub fn resolve_applied_with_slots(
    policy: &Policy,
    slots: &[lns_artifact::spec::CredentialSlot],
    catalog: &[Connector],
) -> AppliedConnectors {
    let slot_ids: HashSet<&str> = slots.iter().map(|s| s.name.as_str()).collect();
    let mut base = policy.clone();
    base.connectors.retain(|id| !slot_ids.contains(id.as_str()));
    let mut out = resolve_applied_connectors(&base, catalog);
    let ceiling_denies = crate::artifact::policy::is_closed(policy);
    for slot in slots {
        let Some(integ) = catalog.iter().find(|i| i.id == slot.name) else {
            continue;
        };
        if let Some(mut def) = wire_provider_def(integ) {
            def.env_var = slot.env.clone();
            out.providers.push(DefProvider::new(def));
        }
        // A slot is artifact-declared, so its route must not widen the user's lockdown.
        if !ceiling_denies {
            out.routes
                .extend(integ.routes.iter().map(|r| r.to_route_rule()));
        }
        if let Some(o) = signin_oauth(integ) {
            out.oauth_configs.insert(integ.id.clone(), o.clone());
        }
        if let Some(o) = signin_pkce(integ) {
            out.pkce_configs.insert(integ.id.clone(), o.clone());
        }
    }
    out
}

/// A slot's connector is already reachable through the definition, so it is never offered as a fresh connect.
pub fn resolve_connectable_with_slots(
    policy: &Policy,
    slots: &[lns_artifact::spec::CredentialSlot],
    declared: &[String],
    catalog: &[Connector],
) -> ConnectableConnectors {
    let mut owned = policy.clone();
    owned
        .connectors
        .extend(slots.iter().map(|s| s.name.clone()));
    resolve_connectable_with_declared(&owned, declared, catalog)
}

/// A run's wire provider set and the consent boundary derived from it, composed in exactly one place so the Layer 2 rig and production cannot drift apart on who may arm a machine-stored value.
pub struct RunProviders {
    pub providers: Vec<DefProvider>,
    /// The ids consented at boot — only the applied (overlay-connected + slot) providers; a connectable id, declared or not, joins live on connect.
    pub armed: HashSet<String>,
    pub connectable_ids: HashSet<String>,
}

pub fn run_providers(applied: Vec<DefProvider>, connectable: Vec<DefProvider>) -> RunProviders {
    let armed = applied.iter().map(|p| p.id().to_string()).collect();
    let connectable_ids = connectable.iter().map(|p| p.id().to_string()).collect();
    let mut providers = applied;
    providers.extend(connectable);
    RunProviders {
        providers,
        armed,
        connectable_ids,
    }
}

fn grant_arms(
    p: &DefProvider,
    project: &str,
    workload: &WorkloadIdentity,
    grants: &WorkloadGrantFile,
) -> bool {
    let Some(grant) = grants.lookup(project, workload, p.id()) else {
        return false;
    };
    if grant.verdict != GrantVerdict::Allow {
        return false;
    }
    let (env_var, domains) = p.disclosure_snapshot();
    grant.matches_disclosure(&env_var, &domains)
}

/// Arm only the applied ids this workload holds an allow grant for whose recorded (env var, injection domains) snapshot still matches *every* provider resolved under that id — arming is per id, so one unmatched provider would ride in on another's grant; an ungranted, denied, or stale id stays unarmed and falls through to a first-use offer, so a machine-global value never arms without this workload's consent.
pub fn gate_armed_by_grant(
    applied: &HashSet<String>,
    providers: &[DefProvider],
    project: &str,
    workload: &WorkloadIdentity,
    grants: &WorkloadGrantFile,
) -> HashSet<String> {
    let mut armed: HashSet<String> = HashSet::new();
    let mut unmatched: HashSet<&str> = HashSet::new();
    for p in providers.iter().filter(|p| applied.contains(p.id())) {
        if grant_arms(p, project, workload, grants) {
            armed.insert(p.id().to_string());
        } else {
            unmatched.insert(p.id());
        }
    }
    armed.retain(|id| !unmatched.contains(id.as_str()));
    armed
}

/// The allow grants a boot-gate sign-in earns, pinned to each resolved provider's own disclosure snapshot so they satisfy [`gate_armed_by_grant`] by construction; an id with no resolved provider yields none.
pub fn boot_sign_in_grants(
    signed_in: &[String],
    providers: &[DefProvider],
    project: &str,
    workload: &WorkloadIdentity,
) -> Vec<GrantRecord> {
    signed_in
        .iter()
        .filter_map(|id| providers.iter().find(|p| p.id() == id))
        .map(|p| {
            let (env_var, domains) = p.disclosure_snapshot();
            GrantRecord::allow(project, workload, p.id(), env_var, domains)
        })
        .collect()
}

/// Two route patterns collide if either matches the other as a host under the gate's own wildcard- and case-insensitive rule, so an applied domain suppresses a connectable that shares it even when the patterns aren't byte-identical.
fn domains_overlap(a: &str, b: &str) -> bool {
    use lns_policy::matching::domain_matches;
    domain_matches(a, b) || domain_matches(b, a)
}

/// Every domain a connector claims: its route patterns plus the domains its credential/oauth injections actually write a token onto (a custom catalog may inject on a domain it doesn't route).
fn claimed_domains(integ: &Connector) -> impl Iterator<Item = &str> {
    integ
        .routes
        .iter()
        .map(|r| r.match_pattern.as_str())
        .chain(
            integ
                .credential
                .iter()
                .flat_map(|c| c.injections.iter().map(|i| i.domain.as_str())),
        )
        .chain(
            integ
                .oauth
                .iter()
                .flat_map(|o| o.injections.iter().map(|i| i.domain.as_str())),
        )
}

/// The catalog connectors a run can offer to connect: every entry (credential or oauth) not already applied and not colliding with an applied connector's domain.
pub fn resolve_connectable_connectors(
    policy: &Policy,
    catalog: &[Connector],
) -> ConnectableConnectors {
    resolve_connectable_with_declared(policy, &[], catalog)
}

/// Connectables minus any colliding with a domain already spoken for — by an applied credential (`policy.connectors`) or by an artifact-declared, offered-not-armed connector (`declared`) — so a colliding entry's machine-global stored value can never inject over the credential that owns that domain (e.g. a leftover `anthropic` value clobbering a declared `claude-code-subscription` on api.anthropic.com, even when that domain is an injection target rather than a declared route).
fn resolve_connectable_with_declared(
    policy: &Policy,
    declared: &[String],
    catalog: &[Connector],
) -> ConnectableConnectors {
    let owned: HashSet<&str> = policy.connectors.iter().map(String::as_str).collect();
    let protected: HashSet<&str> = owned
        .iter()
        .copied()
        .chain(declared.iter().map(String::as_str))
        .collect();

    let mut out = ConnectableConnectors::default();
    for integ in catalog {
        if owned.contains(integ.id.as_str()) {
            continue;
        }
        let integ_domains: Vec<&str> = claimed_domains(integ).collect();
        let collides = catalog
            .iter()
            .filter(|other| other.id != integ.id && protected.contains(other.id.as_str()))
            .flat_map(claimed_domains)
            .any(|guarded| {
                integ_domains
                    .iter()
                    .copied()
                    .any(|d| domains_overlap(guarded, d))
            });
        if collides {
            continue;
        }
        if let Some(p) = wire_provider(integ) {
            let seeds = declared.iter().any(|id| id == &integ.id);
            out.providers.push(if seeds { p } else { p.detect_only() });
            out.routes.insert(
                integ.id.clone(),
                integ.routes.iter().map(|r| r.to_route_rule()).collect(),
            );
            if let Some(o) = signin_oauth(integ) {
                out.oauth_configs.insert(integ.id.clone(), o.clone());
            }
            if let Some(o) = signin_pkce(integ) {
                out.pkce_configs.insert(integ.id.clone(), o.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_flow::providers::Provider;
    use lns_policy::connectors::{ConnectorRoute, CredentialAuth, OauthAuth, OauthFlow};
    use lns_policy::grants::GrantRecord;
    use lns_policy::providers::{InjectionDef, InjectionKind};

    fn cred_connector(id: &str, env_var: &str, domain: &str) -> Connector {
        Connector {
            id: id.into(),
            name: None,
            auth_kind: AuthKind::Credential,
            routes: vec![ConnectorRoute {
                match_pattern: domain.into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            credential: Some(CredentialAuth {
                env_var: env_var.into(),
                placeholder: format!("lns-{id}-placeholder"),
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: domain.into(),
                    header: None,
                }],
            }),
            oauth: None,
            token_fallback: None,
        }
    }

    fn policy_applying(ids: &[&str]) -> Policy {
        Policy {
            connectors: ids.iter().map(|s| s.to_string()).collect(),
            ..Policy::default()
        }
    }

    #[test]
    fn a_connector_the_policy_itself_connects_keeps_its_routes_unwithheld() {
        let catalog = vec![cred_connector("acme", "ACME_API_KEY", "api.acme.corp")];
        // A `lns-policy.yaml` edit connects a connector without writing any grant, and the offerable set is a launch-time snapshot that still names it.
        let mut policy = policy_applying(&["acme"]);
        policy.add_rule(RouteRule::allow_host("api.acme.corp"));
        let offerable = HashSet::from(["acme".to_string()]);
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };

        let out = withhold_undecided_connector_allows(
            &policy,
            &catalog,
            &offerable,
            "proj",
            &workload,
            &WorkloadGrantFile::default(),
        );

        assert_eq!(
            out.network.egress.http.len(),
            1,
            "a connected connector's own routes must survive withholding, or connecting by hand leaves them dead until a relaunch"
        );
    }

    #[test]
    fn resolves_an_applied_credential_connector_into_a_provider_and_its_routes() {
        let catalog = vec![cred_connector("gitlab", "GITLAB_TOKEN", "gitlab.com")];
        let out = resolve_applied_connectors(&policy_applying(&["gitlab"]), &catalog);
        assert_eq!(out.providers.len(), 1);
        assert_eq!(out.providers[0].id(), "gitlab");
        assert_eq!(out.providers[0].env_var(), "GITLAB_TOKEN");
        assert_eq!(out.routes.len(), 1);
        assert_eq!(out.routes[0].match_pattern, "gitlab.com");
        assert_eq!(out.routes[0].verdict, lns_policy::Verdict::Allow);
    }

    fn provider_of(id: &str, env: &str, domain: &str) -> DefProvider {
        resolve_applied_connectors(&policy_applying(&[id]), &[cred_connector(id, env, domain)])
            .providers
            .pop()
            .expect("one provider")
    }

    fn allow_grants(
        workload: &WorkloadIdentity,
        connector: &str,
        env: &str,
        domain: &str,
    ) -> WorkloadGrantFile {
        let mut grants = WorkloadGrantFile::default();
        grants.upsert(GrantRecord::allow(
            "proj",
            workload,
            connector,
            env,
            vec![domain.to_string()],
        ));
        grants
    }

    #[test]
    fn gate_armed_by_grant_arms_an_allow_grant_with_a_matching_snapshot() {
        let providers = vec![provider_of(
            "some-provider",
            "SOME_TOKEN",
            "api.some-provider.example",
        )];
        let applied: HashSet<String> = ["some-provider".to_string()].into_iter().collect();
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let grants = allow_grants(
            &workload,
            "some-provider",
            "SOME_TOKEN",
            "api.some-provider.example",
        );
        let armed = gate_armed_by_grant(&applied, &providers, "proj", &workload, &grants);
        assert_eq!(
            armed, applied,
            "an allow grant whose snapshot matches the resolved provider must arm it"
        );
    }

    #[test]
    fn gate_armed_by_grant_holds_an_id_whose_second_provider_the_grant_does_not_cover() {
        let providers = vec![
            provider_of("some-provider", "SOME_TOKEN", "api.some-provider.example"),
            provider_of("some-provider", "OTHER_TOKEN", "api.some-provider.example"),
        ];
        let applied: HashSet<String> = ["some-provider".to_string()].into_iter().collect();
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let grants = allow_grants(
            &workload,
            "some-provider",
            "SOME_TOKEN",
            "api.some-provider.example",
        );
        let armed = gate_armed_by_grant(&applied, &providers, "proj", &workload, &grants);
        assert!(
            armed.is_empty(),
            "arming is per id, so admitting the id on one matching provider would arm the other one too — injecting the secret under an env var the card never disclosed"
        );
    }

    #[test]
    fn gate_armed_by_grant_leaves_an_ungranted_id_unarmed() {
        let providers = vec![provider_of(
            "some-provider",
            "SOME_TOKEN",
            "api.some-provider.example",
        )];
        let applied: HashSet<String> = ["some-provider".to_string()].into_iter().collect();
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let armed = gate_armed_by_grant(
            &applied,
            &providers,
            "proj",
            &workload,
            &WorkloadGrantFile::default(),
        );
        assert!(
            armed.is_empty(),
            "a connector this workload never consented to must stay unarmed"
        );
    }

    #[test]
    fn gate_armed_by_grant_leaves_a_denied_id_unarmed() {
        let providers = vec![provider_of(
            "some-provider",
            "SOME_TOKEN",
            "api.some-provider.example",
        )];
        let applied: HashSet<String> = ["some-provider".to_string()].into_iter().collect();
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let mut grants = WorkloadGrantFile::default();
        grants.upsert(GrantRecord::deny(
            "proj",
            &workload,
            "some-provider",
            "SOME_TOKEN",
            vec!["api.some-provider.example".into()],
        ));
        let armed = gate_armed_by_grant(&applied, &providers, "proj", &workload, &grants);
        assert!(armed.is_empty(), "a denied connector must never arm");
    }

    #[test]
    fn gate_armed_by_grant_rejects_a_grant_whose_env_var_drifted() {
        let providers = vec![provider_of(
            "some-provider",
            "SOME_TOKEN",
            "api.some-provider.example",
        )];
        let applied: HashSet<String> = ["some-provider".to_string()].into_iter().collect();
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let grants = allow_grants(
            &workload,
            "some-provider",
            "OLD_TOKEN",
            "api.some-provider.example",
        );
        let armed = gate_armed_by_grant(&applied, &providers, "proj", &workload, &grants);
        assert!(
            armed.is_empty(),
            "a grant pinned to a since-changed env var must be invalidated, not silently inherited"
        );
    }

    #[test]
    fn gate_armed_by_grant_rejects_a_grant_whose_injection_domains_drifted() {
        let providers = vec![provider_of(
            "some-provider",
            "SOME_TOKEN",
            "api.some-provider.example",
        )];
        let applied: HashSet<String> = ["some-provider".to_string()].into_iter().collect();
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let grants = allow_grants(&workload, "some-provider", "SOME_TOKEN", "api.old.example");
        let armed = gate_armed_by_grant(&applied, &providers, "proj", &workload, &grants);
        assert!(
            armed.is_empty(),
            "a grant pinned to since-changed injection domains must be invalidated"
        );
    }

    #[test]
    fn gate_armed_by_grant_ignores_a_provider_outside_the_applied_set() {
        let providers = vec![provider_of(
            "some-provider",
            "SOME_TOKEN",
            "api.some-provider.example",
        )];
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let grants = allow_grants(
            &workload,
            "some-provider",
            "SOME_TOKEN",
            "api.some-provider.example",
        );
        let armed = gate_armed_by_grant(&HashSet::new(), &providers, "proj", &workload, &grants);
        assert!(
            armed.is_empty(),
            "only ids in the applied set are candidates for arming, grant or no grant"
        );
    }

    #[test]
    fn boot_sign_in_grants_arm_through_the_same_gate_that_admits_later_runs() {
        let providers = vec![provider_of(
            "some-provider",
            "SOME_TOKEN",
            "api.some-provider.example",
        )];
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let records = boot_sign_in_grants(
            &["some-provider".to_string()],
            &providers,
            "proj",
            &workload,
        );
        let mut grants = WorkloadGrantFile::default();
        for r in records {
            grants.upsert(r);
        }
        let applied: HashSet<String> = ["some-provider".to_string()].into_iter().collect();
        let armed = gate_armed_by_grant(&applied, &providers, "proj", &workload, &grants);
        assert_eq!(
            armed, applied,
            "the grant a boot sign-in records must satisfy the very gate that admits the next run — a snapshot mismatch here means the fix silently fixes nothing"
        );
    }

    #[test]
    fn boot_sign_in_grant_records_the_slot_remapped_env_var_not_the_catalog_default() {
        let catalog = vec![oauth_connector(
            "some-oauth",
            "SOME_OAUTH_TOKEN",
            "api.some-oauth.example",
        )];
        let slots = vec![slot("some-oauth", "REMAPPED_TOKEN", true)];
        let providers = resolve_applied_with_slots(&Policy::default(), &slots, &catalog).providers;
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let records =
            boot_sign_in_grants(&["some-oauth".to_string()], &providers, "proj", &workload);
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].env_var, "REMAPPED_TOKEN",
            "the grant must pin the slot's effective env var, or a remapped slot's grant never matches at the next boot"
        );
        let mut grants = WorkloadGrantFile::default();
        grants.upsert(records[0].clone());
        let applied: HashSet<String> = ["some-oauth".to_string()].into_iter().collect();
        let armed = gate_armed_by_grant(&applied, &providers, "proj", &workload, &grants);
        assert_eq!(armed, applied);
    }

    #[test]
    fn boot_sign_in_grants_skip_an_id_that_resolved_no_provider() {
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let records = boot_sign_in_grants(&["some-oauth".to_string()], &[], "proj", &workload);
        assert!(
            records.is_empty(),
            "with no resolved provider there is no disclosure to pin a grant to"
        );
    }

    #[test]
    fn unknown_connector_ids_reports_only_ids_the_catalog_lacks_in_order() {
        let catalog = vec![cred_connector(
            "some-provider",
            "SOME_TOKEN",
            "api.example.test",
        )];
        let declared = vec![
            "some-unknown".to_string(),
            "some-provider".to_string(),
            "other-unknown".to_string(),
        ];
        assert_eq!(
            unknown_connector_ids(&declared, &catalog),
            vec!["some-unknown".to_string(), "other-unknown".to_string()]
        );
        assert!(unknown_connector_ids(&["some-provider".to_string()], &catalog).is_empty());
    }

    #[test]
    fn unknown_connectors_refusal_names_each_id_and_lns_connector_add() {
        let msg = unknown_connectors_refusal(&["some-unknown".to_string(), "other".to_string()]);
        assert!(msg.contains("\"some-unknown\""), "got: {msg}");
        assert!(msg.contains("\"other\""), "got: {msg}");
        assert!(msg.contains("`lns connector add`"), "got: {msg}");
    }

    #[test]
    fn skips_a_catalog_connector_that_is_not_applied() {
        let catalog = vec![cred_connector("gitlab", "GITLAB_TOKEN", "gitlab.com")];
        let out = resolve_applied_connectors(&policy_applying(&[]), &catalog);
        assert!(out.providers.is_empty());
        assert!(out.routes.is_empty());
    }

    #[test]
    fn applied_connector_routes_maps_connected_ids_to_their_catalog_routes() {
        let catalog = vec![cred_connector("gitlab", "GITLAB_TOKEN", "gitlab.com")];
        let routes = applied_connector_routes(&["gitlab".to_string()], &catalog);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].match_pattern, "gitlab.com");
        assert_eq!(routes[0].verdict, lns_policy::Verdict::Allow);
    }

    #[test]
    fn applied_connector_routes_ignores_ids_absent_from_the_catalog() {
        let catalog = vec![cred_connector("gitlab", "GITLAB_TOKEN", "gitlab.com")];
        assert!(applied_connector_routes(&["nope".to_string()], &catalog).is_empty());
    }

    fn oauth_connector(id: &str, env_var: &str, domain: &str) -> Connector {
        Connector {
            id: id.into(),
            name: None,
            auth_kind: AuthKind::Oauth,
            routes: vec![ConnectorRoute {
                match_pattern: domain.into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            credential: None,
            oauth: Some(OauthAuth {
                userinfo_endpoint: None,
                account_field: None,
                flow: OauthFlow::Device,
                client_id: Some(format!("Iv1.{id}")),
                client_secret: None,
                scopes: vec!["repo".into()],
                device_authorization_endpoint: Some(format!("https://{domain}/login/device/code")),
                authorization_endpoint: None,
                token_endpoint: format!("https://{domain}/login/oauth/access_token"),
                env_var: env_var.into(),
                placeholder: format!("lns-{id}-placeholder"),
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: domain.into(),
                    header: None,
                }],
            }),
            token_fallback: None,
        }
    }

    fn pkce_connector(id: &str, env_var: &str, domain: &str) -> Connector {
        Connector {
            id: id.into(),
            name: None,
            auth_kind: AuthKind::Oauth,
            routes: vec![ConnectorRoute {
                match_pattern: domain.into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            credential: None,
            oauth: Some(OauthAuth {
                userinfo_endpoint: None,
                account_field: None,
                flow: OauthFlow::Pkce,
                client_id: None,
                client_secret: None,
                scopes: Vec::new(),
                device_authorization_endpoint: None,
                authorization_endpoint: Some(format!("https://{domain}/auth")),
                token_endpoint: format!("https://{domain}/api/v1/auth/keys"),
                env_var: env_var.into(),
                placeholder: format!("lns-{id}-placeholder"),
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: domain.into(),
                    header: None,
                }],
            }),
            token_fallback: None,
        }
    }

    #[test]
    fn an_applied_oauth_connector_contributes_a_provider_routes_and_its_oauth_config() {
        let catalog = vec![oauth_connector(
            "somesaas",
            "SOMESAAS_TOKEN",
            "api.somesaas.com",
        )];
        let out = resolve_applied_connectors(&policy_applying(&["somesaas"]), &catalog);
        let ids: Vec<&str> = out.providers.iter().map(|p| p.id()).collect();
        assert_eq!(
            ids,
            ["somesaas"],
            "an oauth connector seeds its placeholder like any provider"
        );
        assert_eq!(
            out.routes.len(),
            1,
            "a connector's routes apply regardless of auth kind"
        );
        assert!(
            out.oauth_configs.contains_key("somesaas"),
            "the device-flow config must be surfaced for the sign-in dance"
        );
    }

    #[test]
    fn resolves_only_the_applied_subset_of_a_multi_entry_catalog() {
        let catalog = vec![
            cred_connector("gitlab", "GITLAB_TOKEN", "gitlab.com"),
            cred_connector("huggingface", "HF_TOKEN", "huggingface.co"),
        ];
        let out = resolve_applied_connectors(&policy_applying(&["huggingface"]), &catalog);
        let ids: Vec<&str> = out.providers.iter().map(|p| p.id()).collect();
        assert_eq!(ids, ["huggingface"]);
    }

    #[test]
    fn connectable_includes_an_unconnected_catalog_credential_connector() {
        let catalog = vec![cred_connector("gitlab", "GITLAB_TOKEN", "gitlab.com")];
        let c = resolve_connectable_connectors(&policy_applying(&[]), &catalog);
        assert_eq!(c.providers.len(), 1);
        assert_eq!(c.providers[0].id(), "gitlab");
        assert_eq!(c.routes.get("gitlab").map(|r| r.len()), Some(1));
    }

    #[test]
    fn run_providers_arms_only_the_applied_ids() {
        let applied = resolve_applied_connectors(
            &policy_applying(&["some-provider"]),
            &[cred_connector(
                "some-provider",
                "SOME_TOKEN",
                "api.example.test",
            )],
        );
        let connectable = resolve_connectable_connectors(
            &policy_applying(&[]),
            &[cred_connector(
                "other-provider",
                "OTHER_TOKEN",
                "api.other.test",
            )],
        );
        let run = run_providers(applied.providers, connectable.providers);
        assert_eq!(
            run.armed,
            std::collections::HashSet::from(["some-provider".to_string()]),
            "only an applied id may arm a machine-stored value; widening this at boot reopens the declared-connector exploit"
        );
        assert_eq!(
            run.connectable_ids,
            std::collections::HashSet::from(["other-provider".to_string()])
        );
        let ids: Vec<&str> = run.providers.iter().map(|p| p.id()).collect();
        assert_eq!(
            ids,
            ["some-provider", "other-provider"],
            "the wire set is applied ∪ connectable, applied first"
        );
    }

    #[test]
    fn an_undeclared_connectable_is_detect_only_so_it_seeds_no_phantom_placeholder() {
        let catalog = vec![cred_connector(
            "some-provider",
            "SOME_TOKEN",
            "api.example.test",
        )];
        let c = resolve_connectable_connectors(&policy_applying(&[]), &catalog);
        assert!(
            !c.providers[0].seeds_env(),
            "an undeclared connectable must not pollute the workload env"
        );
    }

    #[test]
    fn a_declared_connectable_seeds_its_placeholder_env_while_staying_offerable() {
        let catalog = vec![cred_connector(
            "some-provider",
            "SOME_TOKEN",
            "api.example.test",
        )];
        let c = resolve_connectable_with_slots(
            &policy_applying(&[]),
            &[],
            &["some-provider".to_string()],
            &catalog,
        );
        assert_eq!(c.providers.len(), 1, "a declared id stays offerable");
        assert!(
            c.providers[0].seeds_env(),
            "a declared id seeds its placeholder so the workload attempts the request that triggers the connect offer"
        );
        assert_eq!(
            c.routes.get("some-provider").map(|r| r.len()),
            Some(1),
            "its routes stay held for the connect, never pre-armed"
        );
    }

    #[test]
    fn connectable_excludes_an_already_applied_connector() {
        let catalog = vec![cred_connector("gitlab", "GITLAB_TOKEN", "gitlab.com")];
        let c = resolve_connectable_connectors(&policy_applying(&["gitlab"]), &catalog);
        assert!(
            c.providers.is_empty(),
            "an applied connector is not connectable"
        );
        assert!(c.routes.is_empty());
    }

    #[test]
    fn connectable_excludes_a_catalog_entry_that_collides_with_an_applied_domain() {
        let catalog = vec![
            cred_connector("some-primary", "PRIMARY_TOKEN", "api.example.test"),
            cred_connector("some-other", "OTHER_TOKEN", "api.example.test"),
        ];
        let c = resolve_connectable_connectors(&policy_applying(&["some-primary"]), &catalog);
        assert!(
            c.providers.is_empty(),
            "a connectable that shares the applied connector's domain must be suppressed so its machine-global stored value can't inject over the declared credential"
        );
        assert!(c.routes.is_empty());
    }

    #[test]
    fn connectable_excludes_an_entry_whose_wildcard_covers_an_applied_domain() {
        let catalog = vec![
            cred_connector("some-primary", "PRIMARY_TOKEN", "api.example.test"),
            cred_connector("some-wild", "WILD_TOKEN", "*.example.test"),
        ];
        let c = resolve_connectable_connectors(&policy_applying(&["some-primary"]), &catalog);
        assert!(
            c.providers.is_empty(),
            "a connectable whose wildcard covers the applied connector's host must be suppressed, not just a byte-identical pattern"
        );
    }

    #[test]
    fn connectable_excludes_an_entry_colliding_with_an_applied_injection_domain() {
        let mut applied = cred_connector(
            "some-primary",
            "PRIMARY_TOKEN",
            "login.some-primary.example",
        );
        applied.credential.as_mut().unwrap().injections[0].domain =
            "api.some-provider.example".into();
        let catalog = vec![
            applied,
            cred_connector("some-other", "OTHER_TOKEN", "api.some-provider.example"),
        ];
        let c = resolve_connectable_connectors(&policy_applying(&["some-primary"]), &catalog);
        assert!(
            c.providers.is_empty(),
            "a connectable sharing the applied connector's injection domain must be suppressed even when that domain is not among its declared routes"
        );
    }

    #[test]
    fn connectable_excludes_a_case_variant_of_an_applied_domain() {
        let catalog = vec![
            cred_connector("some-primary", "PRIMARY_TOKEN", "api.example.test"),
            cred_connector("some-upper", "UPPER_TOKEN", "API.Example.Test"),
        ];
        let c = resolve_connectable_connectors(&policy_applying(&["some-primary"]), &catalog);
        assert!(
            c.providers.is_empty(),
            "host matching is case-insensitive, so a case-variant of the applied domain must be suppressed"
        );
    }

    #[test]
    fn connectable_on_a_distinct_wildcard_domain_survives_an_applied_connector() {
        let catalog = vec![
            cred_connector("some-primary", "PRIMARY_TOKEN", "api.example.test"),
            cred_connector("some-wild", "WILD_TOKEN", "*.other.test"),
        ];
        let c = resolve_connectable_connectors(&policy_applying(&["some-primary"]), &catalog);
        assert_eq!(
            c.providers.len(),
            1,
            "a wildcard on an unrelated domain must not be over-suppressed"
        );
        assert_eq!(c.providers[0].id(), "some-wild");
    }

    #[test]
    fn connectable_on_a_distinct_domain_survives_an_applied_connector() {
        let catalog = vec![
            cred_connector("some-primary", "PRIMARY_TOKEN", "api.example.test"),
            cred_connector("some-other", "OTHER_TOKEN", "api.other.test"),
        ];
        let c = resolve_connectable_connectors(&policy_applying(&["some-primary"]), &catalog);
        assert_eq!(
            c.providers.len(),
            1,
            "a connectable on its own domain is unaffected by an applied connector elsewhere"
        );
        assert_eq!(c.providers[0].id(), "some-other");
    }

    #[test]
    fn connectable_includes_an_unconnected_oauth_connector_with_its_config() {
        let catalog = vec![oauth_connector(
            "somesaas",
            "SOMESAAS_TOKEN",
            "api.somesaas.com",
        )];
        let c = resolve_connectable_connectors(&policy_applying(&[]), &catalog);
        assert_eq!(
            c.providers.len(),
            1,
            "an unconnected oauth connector is offerable"
        );
        assert_eq!(c.providers[0].id(), "somesaas");
        assert_eq!(c.routes.get("somesaas").map(|r| r.len()), Some(1));
        assert!(
            c.oauth_configs.contains_key("somesaas"),
            "its device-flow config must be held ready for connect"
        );
    }

    #[test]
    fn an_applied_pkce_connector_contributes_a_provider_routes_and_its_pkce_config() {
        let catalog = vec![pkce_connector(
            "somepkce",
            "SOMEPKCE_TOKEN",
            "api.somepkce.com",
        )];
        let out = resolve_applied_connectors(&policy_applying(&["somepkce"]), &catalog);
        let ids: Vec<&str> = out.providers.iter().map(|p| p.id()).collect();
        assert_eq!(ids, ["somepkce"], "a pkce connector seeds its placeholder");
        assert_eq!(out.routes.len(), 1, "its routes apply");
        assert!(
            out.pkce_configs.contains_key("somepkce"),
            "the pkce config must be surfaced for the browser sign-in"
        );
        assert!(
            out.oauth_configs.is_empty(),
            "a pkce entry must not be wired as a device flow"
        );
    }

    #[test]
    fn an_applied_device_oauth_connector_is_not_wired_as_pkce() {
        let catalog = vec![oauth_connector(
            "somesaas",
            "SOMESAAS_TOKEN",
            "api.somesaas.com",
        )];
        let out = resolve_applied_connectors(&policy_applying(&["somesaas"]), &catalog);
        assert!(out.oauth_configs.contains_key("somesaas"));
        assert!(
            out.pkce_configs.is_empty(),
            "a device entry must not be wired as pkce"
        );
    }

    #[test]
    fn connectable_includes_an_unconnected_pkce_connector_with_its_config() {
        let catalog = vec![pkce_connector(
            "somepkce",
            "SOMEPKCE_TOKEN",
            "api.somepkce.com",
        )];
        let c = resolve_connectable_connectors(&policy_applying(&[]), &catalog);
        assert_eq!(c.providers.len(), 1);
        assert_eq!(c.providers[0].id(), "somepkce");
        assert!(
            c.pkce_configs.contains_key("somepkce"),
            "its pkce config must be held ready for connect"
        );
        assert!(c.oauth_configs.is_empty());
    }

    fn oauth_connector_without_client_id(id: &str, env_var: &str, domain: &str) -> Connector {
        let mut i = oauth_connector(id, env_var, domain);
        i.oauth.as_mut().unwrap().client_id = None;
        i
    }

    #[test]
    fn an_applied_oauth_connector_with_no_client_id_seeds_a_provider_but_withholds_the_device_flow()
    {
        let catalog = vec![oauth_connector_without_client_id(
            "somesaas",
            "SOMESAAS_TOKEN",
            "api.somesaas.com",
        )];
        let out = resolve_applied_connectors(&policy_applying(&["somesaas"]), &catalog);
        let ids: Vec<&str> = out.providers.iter().map(|p| p.id()).collect();
        assert_eq!(
            ids,
            ["somesaas"],
            "the placeholder is still seeded so a pasted token can arm it"
        );
        assert_eq!(out.routes.len(), 1, "its routes still apply");
        assert!(
            out.oauth_configs.is_empty(),
            "an empty client_id can't drive a device flow, so no oauth config is surfaced"
        );
    }

    fn slot(name: &str, env: &str, required: bool) -> lns_artifact::spec::CredentialSlot {
        lns_artifact::spec::CredentialSlot {
            name: name.into(),
            env: env.into(),
            required,
        }
    }

    #[test]
    fn a_slot_seeds_its_provider_under_the_slot_env_name_with_the_catalog_placeholder() {
        let catalog = vec![cred_connector(
            "some-provider",
            "SOME_TOKEN",
            "api.example.test",
        )];
        let out = resolve_applied_with_slots(
            &policy_applying(&[]),
            &[slot("some-provider", "PROVIDER_KEY", false)],
            &catalog,
        );
        assert_eq!(out.providers.len(), 1);
        assert_eq!(out.providers[0].id(), "some-provider");
        assert_eq!(
            out.providers[0].env_var(),
            "PROVIDER_KEY",
            "the slot's env remap must win over the catalog default"
        );
        assert_eq!(
            out.providers[0].placeholder(),
            "lns-some-provider-placeholder",
            "the placeholder stays the catalog's so the boundary still detects it"
        );
        assert_eq!(
            out.routes.len(),
            1,
            "a slot allows its routes like a declared id"
        );
        assert_eq!(out.routes[0].match_pattern, "api.example.test");
    }

    #[test]
    fn a_slot_wins_over_a_same_id_declared_connector_so_the_remap_holds() {
        let catalog = vec![cred_connector(
            "some-provider",
            "SOME_TOKEN",
            "api.example.test",
        )];
        let out = resolve_applied_with_slots(
            &policy_applying(&["some-provider"]),
            &[slot("some-provider", "PROVIDER_KEY", true)],
            &catalog,
        );
        assert_eq!(
            out.providers.len(),
            1,
            "the slot and the declared id must not double-seed"
        );
        assert_eq!(out.providers[0].env_var(), "PROVIDER_KEY");
    }

    #[test]
    fn a_slot_alongside_a_different_declared_connector_unions_without_loss() {
        let catalog = vec![
            cred_connector("some-provider", "SOME_TOKEN", "api.example.test"),
            cred_connector("other-provider", "OTHER_TOKEN", "api.other.example"),
        ];
        let out = resolve_applied_with_slots(
            &policy_applying(&["other-provider"]),
            &[slot("some-provider", "SOME_TOKEN", false)],
            &catalog,
        );
        let mut ids: Vec<&str> = out.providers.iter().map(|p| p.id()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["other-provider", "some-provider"]);
    }

    #[test]
    fn a_slot_naming_an_unknown_id_contributes_nothing_here() {
        let out = resolve_applied_with_slots(
            &policy_applying(&[]),
            &[slot("some-unknown", "SOME_TOKEN", true)],
            &[],
        );
        assert!(
            out.providers.is_empty(),
            "unknown ids are the refusal's job"
        );
        assert!(out.routes.is_empty());
    }

    #[test]
    fn an_oauth_slot_surfaces_its_sign_in_config_under_the_slot_env() {
        let catalog = vec![oauth_connector(
            "some-oauth",
            "SOME_OAUTH_TOKEN",
            "api.some-oauth.example",
        )];
        let out = resolve_applied_with_slots(
            &policy_applying(&[]),
            &[slot("some-oauth", "OAUTH_KEY", true)],
            &catalog,
        );
        assert_eq!(out.providers[0].env_var(), "OAUTH_KEY");
        assert!(
            out.oauth_configs.contains_key("some-oauth"),
            "the sign-in config must surface for the launch gate"
        );
    }

    #[test]
    fn a_pkce_oauth_slot_surfaces_its_pkce_config() {
        let catalog = vec![pkce_connector(
            "somepkce",
            "SOMEPKCE_TOKEN",
            "api.somepkce.com",
        )];
        let out = resolve_applied_with_slots(
            &policy_applying(&[]),
            &[slot("somepkce", "SOMEPKCE_TOKEN", false)],
            &catalog,
        );
        assert!(out.pkce_configs.contains_key("somepkce"));
        assert!(out.oauth_configs.is_empty());
    }

    #[test]
    fn a_slot_named_connector_is_not_offered_as_a_fresh_connect() {
        let catalog = vec![cred_connector(
            "some-provider",
            "SOME_TOKEN",
            "api.example.test",
        )];
        let c = resolve_connectable_with_slots(
            &policy_applying(&[]),
            &[slot("some-provider", "SOME_TOKEN", false)],
            &[],
            &catalog,
        );
        assert!(
            c.providers.is_empty(),
            "a slot's connector is already reachable, never a fresh offer"
        );
    }

    #[test]
    fn a_connectable_oauth_connector_with_no_client_id_is_offerable_without_a_device_flow() {
        let catalog = vec![oauth_connector_without_client_id(
            "somesaas",
            "SOMESAAS_TOKEN",
            "api.somesaas.com",
        )];
        let c = resolve_connectable_connectors(&policy_applying(&[]), &catalog);
        assert_eq!(
            c.providers.len(),
            1,
            "still offerable via its token fallback when no client_id is baked in"
        );
        assert_eq!(c.providers[0].id(), "somesaas");
        assert_eq!(c.routes.get("somesaas").map(|r| r.len()), Some(1));
        assert!(
            c.oauth_configs.is_empty(),
            "no client_id means there is no device flow to hold ready"
        );
    }
}
