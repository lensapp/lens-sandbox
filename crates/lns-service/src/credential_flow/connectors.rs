use std::collections::{HashMap, HashSet};

use lns_policy::connectors::{Connector, OauthAuth, OauthFlow, SignInMethod};
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
    /// Which connector each provider id belongs to, so a connect persists the connector rather than the sign-in method the user picked.
    pub owner: HashMap<String, String>,
}

/// Catalog connectors that aren't yet connected, with their routes held ready to allow live on connect, and device-flow / pkce configs for a sign-in dance on connect; a definition-declared entry seeds its placeholder env so the workload attempts its first request, while the rest stay detect-only.
#[derive(Default)]
pub struct ConnectableConnectors {
    pub providers: Vec<DefProvider>,
    pub routes: HashMap<String, Vec<RouteRule>>,
    pub oauth_configs: HashMap<String, OauthAuth>,
    pub pkce_configs: HashMap<String, OauthAuth>,
    /// Which connector each provider id belongs to, so a connect persists the connector rather than the sign-in method the user picked.
    pub owner: HashMap<String, String>,
}

/// The env/placeholder/injection wiring one sign-in method seeds, keyed by the store id that method binds under.
fn method_provider_def(integ: &Connector, method: &SignInMethod) -> ProviderDef {
    ProviderDef {
        id: integ.provider_id_of(method),
        env_var: method.env_var().to_string(),
        placeholder: method.placeholder().to_string(),
        injections: method.injections().to_vec(),
    }
}

/// The wiring a connector seeds where no choice has been made — its only method. A connector reachable several ways has no single answer, so nothing seeds until the user picks one.
fn wire_provider_def(integ: &Connector) -> Option<ProviderDef> {
    integ
        .sole_method()
        .map(|method| method_provider_def(integ, method))
}

fn wire_provider(integ: &Connector) -> Option<DefProvider> {
    wire_provider_def(integ).map(DefProvider::new)
}

/// One wire provider per sign-in method. `seeds` reaches the workload env only for a connector with a single way in; a method the user has not chosen stays detect-only, so its placeholder is recognized on the wire but never seeded.
fn wire_providers(integ: &Connector, seeds: bool) -> Vec<DefProvider> {
    let sole = integ.sole_method().is_some();
    integ
        .methods
        .iter()
        .map(|method| {
            let provider = DefProvider::new(method_provider_def(integ, method));
            if seeds && sole {
                provider
            } else {
                provider.detect_only()
            }
        })
        .collect()
}

/// True when the machine (or this workload) holds a sign-in bound under this provider id — a stored value, an oauth token, or a host-detect choice, but never a refusal. A method picked on the offer card records only a grant, because nothing was seeded for a value card to answer.
pub fn machine_holds_sign_in<'a>(
    state: &'a lns_policy::credentials::CredentialStateFile,
    grants: &'a WorkloadGrantFile,
    project: &'a str,
    workload: &'a WorkloadIdentity,
) -> impl Fn(&str) -> bool + 'a {
    move |provider| {
        state
            .get(provider)
            .is_some_and(|entry| !matches!(entry, lns_policy::credentials::CredentialEntry::Deny))
            || grants
                .lookup(project, workload, provider)
                .is_some_and(|g| g.verdict == GrantVerdict::Allow)
    }
}

/// A sign-in this machine holds, paired with the key its value and grant actually sit under.
pub(crate) struct HeldSignIn<'a> {
    pub method: &'a SignInMethod,
    /// The bare connector id for a binding made before the connector grew a second way in, so the value and grant already stored keep resolving.
    pub provider_id: String,
}

/// The sign-in this machine actually holds: the only way in, the one whose provider id is chosen, or — for a binding made before the connector grew a second way in, which sits under the bare connector id — the one the catalog still lists first.
pub(crate) fn held_method<'a>(
    integ: &'a Connector,
    chosen: MethodChosen<'_>,
) -> Option<HeldSignIn<'a>> {
    if let Some(method) = integ.sole_method() {
        return Some(HeldSignIn {
            method,
            provider_id: integ.provider_id_of(method),
        });
    }
    if let Some(method) = integ
        .methods
        .iter()
        .find(|m| chosen(&integ.provider_id_of(m)))
    {
        return Some(HeldSignIn {
            method,
            provider_id: integ.provider_id_of(method),
        });
    }
    chosen(&integ.id)
        .then(|| integ.methods.first())
        .flatten()
        .map(|method| HeldSignIn {
            method,
            provider_id: integ.id.clone(),
        })
}

/// Repeats the held sign-in's configs under the key its value actually sits under, so a binding made before the connector grew a second way in can still drive its sign-in and token refresh.
fn alias_legacy_signin_configs(
    integ: &Connector,
    held: &HeldSignIn<'_>,
    out: &mut AppliedConnectors,
) {
    if held.provider_id == integ.provider_id_of(held.method) {
        return;
    }
    let per_method = integ.provider_id_of(held.method);
    if let Some(cfg) = out.oauth_configs.get(&per_method).cloned() {
        out.oauth_configs.insert(held.provider_id.clone(), cfg);
    }
    if let Some(cfg) = out.pkce_configs.get(&per_method).cloned() {
        out.pkce_configs.insert(held.provider_id.clone(), cfg);
    }
    out.owner.insert(held.provider_id.clone(), integ.id.clone());
}

/// The providers a connected connector puts on the wire: the method it holds a sign-in for, or — while the choice is still open — every method detect-only, so the domain stays gated and host detection still works without seeding anything.
fn applied_providers(integ: &Connector, chosen: MethodChosen<'_>) -> Vec<DefProvider> {
    match held_method(integ, chosen) {
        Some(held) => vec![DefProvider::new(ProviderDef {
            id: held.provider_id,
            ..method_provider_def(integ, held.method)
        })],
        None => wire_providers(integ, false),
    }
}

/// The oauth block usable for a device sign-in: the device flow with a client_id baked in (community builds ship none, so they fall back to the token paste).
fn signin_oauth(method: &SignInMethod) -> Option<&OauthAuth> {
    method.oauth.as_ref().filter(|o| {
        o.flow == OauthFlow::Device && o.client_id.as_deref().is_some_and(|c| !c.is_empty())
    })
}

/// The oauth block usable for a pkce browser sign-in: the pkce flow with an authorization endpoint to redirect through.
fn signin_pkce(method: &SignInMethod) -> Option<&OauthAuth> {
    method
        .oauth
        .as_ref()
        .filter(|o| o.flow == OauthFlow::Pkce && o.authorization_endpoint.is_some())
}

/// Records every sign-in config a connector's methods offer, each under the store id its method binds to, so a non-first oauth method is reachable too.
fn collect_signin_configs(
    integ: &Connector,
    owner: &mut HashMap<String, String>,
    oauth_configs: &mut HashMap<String, OauthAuth>,
    pkce_configs: &mut HashMap<String, OauthAuth>,
) {
    for method in &integ.methods {
        let provider_id = integ.provider_id_of(method);
        owner.insert(provider_id.clone(), integ.id.clone());
        if let Some(o) = signin_oauth(method) {
            oauth_configs.insert(provider_id.clone(), o.clone());
        }
        if let Some(o) = signin_pkce(method) {
            pkce_configs.insert(provider_id, o.clone());
        }
    }
}

/// True when this provider id is the sign-in the machine (or this workload) actually holds, so a connected connector reachable several ways seeds only the method the user picked.
pub type MethodChosen<'a> = &'a dyn Fn(&str) -> bool;

/// The env var and injection domains a connector discloses on its consent card. A connector reachable several ways discloses none — no one env var describes it — so a decision about it is recorded against the connector alone.
pub fn disclosure_of(integ: &Connector) -> Option<(String, Vec<String>)> {
    wire_provider(integ).map(|p| p.disclosure_snapshot())
}

/// A connector this workload has already answered for, either way: a grant recorded against the same disclosure the card showed. A redefined connector is a different question, so neither a yes nor a no carries over to a new shape — but a grant that disclosed nothing has no shape to drift, and answers the connector as a whole.
fn is_decided(
    integ: &Connector,
    project: &str,
    workload: &WorkloadIdentity,
    grants: &WorkloadGrantFile,
) -> bool {
    let Some(grant) = grants.lookup(project, workload, &integ.id) else {
        return false;
    };
    grant.has_no_disclosure()
        || disclosure_of(integ)
            .is_some_and(|(env_var, domains)| grant.matches_disclosure(&env_var, &domains))
}

/// A connector the policy connects and this machine holds a sign-in for keeps its routes: its value card still carries the decision, and a hand edit writes no grant to prove one with. A connector no sign-in is held for can never authenticate, so its routes wait for the card instead. `held_method` is unconditionally `Some` for a single-method connector, so one can never be withheld here whatever the store says.
fn is_connected_with_a_sign_in(
    integ: &Connector,
    policy: &Policy,
    chosen: MethodChosen<'_>,
) -> bool {
    policy.connectors.iter().any(|id| id == &integ.id) && held_method(integ, chosen).is_some()
}

/// The policy a launch actually runs: the user's own rules minus the allows that would let a request past a connector nobody has decided yet, so the first request to a claimed domain is held and the connect offer gets its chance. Once the connector is connected or decided either way, the withheld rule applies as written. Only an `offerable` connector can withhold anything — a connector with no path to a card could never release the rule again.
pub fn withhold_undecided_connector_allows(
    policy: &Policy,
    catalog: &[Connector],
    offerable: &HashSet<String>,
    project: &str,
    workload: &WorkloadIdentity,
    grants: &WorkloadGrantFile,
    chosen: MethodChosen<'_>,
) -> Policy {
    let undecided: Vec<&str> = catalog
        .iter()
        .filter(|integ| offerable.contains(&integ.id))
        .filter(|integ| !is_connected_with_a_sign_in(integ, policy, chosen))
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

/// Pairs each connectable connector's id with its catalog display name, route patterns, and the sign-ins it offers, so a held request to one of those domains can offer to connect it instead of asking about the bare host.
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
                methods: entry.map(offer_methods).unwrap_or_default(),
            }
        })
        .collect()
}

/// Every sign-in a connector offers, each paired with the per-machine store id it binds under.
fn offer_methods(integ: &Connector) -> Vec<crate::approval_flow::session::OfferMethod> {
    integ
        .methods
        .iter()
        .map(|method| crate::approval_flow::session::OfferMethod {
            method_id: method.id.clone(),
            provider_id: integ.provider_id_of(method),
            display_name: method.display_name().to_string(),
            token_fallback: method.token_fallback.clone(),
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

/// Connected connectors reachable several ways that this machine holds no sign-in for. Each wires no provider, so nothing seeds a placeholder and — being connected, its routes already allowed — no card can ever be raised: every request would leave unauthenticated in silence.
pub fn unchosen_connected_connectors(
    policy: &Policy,
    catalog: &[Connector],
    chosen: MethodChosen<'_>,
) -> Vec<String> {
    catalog
        .iter()
        .filter(|integ| policy.connectors.iter().any(|id| id == &integ.id))
        .filter(|integ| integ.sole_method().is_none())
        .filter(|integ| held_method(integ, chosen).is_none())
        .map(|integ| integ.id.clone())
        .collect()
}

/// The launch-refusal message for a connected connector no sign-in is held for, pointing at the one recovery that works: disconnecting makes it connectable again, and its first use then asks which sign-in.
pub fn unchosen_connectors_refusal(ids: &[String]) -> String {
    let list = ids
        .iter()
        .map(|id| format!("\"{id}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "connector {list} is connected but this machine holds no sign-in for it, so nothing would authenticate its requests; \
         run `lns connector disconnect {}` and the next request to its domain will ask which sign-in to use",
        ids.first().map(String::as_str).unwrap_or_default()
    )
}

/// Resolves the policy's applied connector ids against the effective catalog.
pub fn resolve_applied_connectors(
    policy: &Policy,
    catalog: &[Connector],
    chosen: MethodChosen<'_>,
) -> AppliedConnectors {
    let applied: HashSet<&str> = policy.connectors.iter().map(String::as_str).collect();

    let mut out = AppliedConnectors {
        routes: applied_connector_routes(&policy.connectors, catalog),
        ..AppliedConnectors::default()
    };
    for integ in catalog {
        if !applied.contains(integ.id.as_str()) {
            continue;
        }
        out.providers.extend(applied_providers(integ, chosen));
        collect_signin_configs(
            integ,
            &mut out.owner,
            &mut out.oauth_configs,
            &mut out.pkce_configs,
        );
        if let Some(held) = held_method(integ, chosen) {
            alias_legacy_signin_configs(integ, &held, &mut out);
        }
    }
    out
}

/// A definition's credential slots resolve like declared connectors, with each slot's env name overriding the catalog default and winning over a same-id declared entry so the remap holds.
pub fn resolve_applied_with_slots(
    policy: &Policy,
    slots: &[lns_artifact::spec::CredentialSlot],
    catalog: &[Connector],
    chosen: MethodChosen<'_>,
) -> AppliedConnectors {
    let slot_ids: HashSet<&str> = slots.iter().map(|s| s.name.as_str()).collect();
    let mut base = policy.clone();
    base.connectors.retain(|id| !slot_ids.contains(id.as_str()));
    let mut out = resolve_applied_connectors(&base, catalog, chosen);
    let ceiling_denies = crate::artifact::policy::is_closed(policy);
    for slot in slots {
        let Some(integ) = catalog.iter().find(|i| i.id == slot.name) else {
            continue;
        };
        // One slot carries one env name, so only the method this machine holds a sign-in for can fill it.
        let Some(held) = held_method(integ, chosen) else {
            continue;
        };
        let mut def = method_provider_def(integ, held.method);
        def.id = held.provider_id;
        def.env_var = slot.env.clone();
        out.providers.push(DefProvider::new(def));
        // A slot is artifact-declared, so its route must not widen the user's lockdown.
        if !ceiling_denies {
            out.routes
                .extend(integ.routes.iter().map(|r| r.to_route_rule()));
        }
        collect_signin_configs(
            integ,
            &mut out.owner,
            &mut out.oauth_configs,
            &mut out.pkce_configs,
        );
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

/// Every domain a connector claims: its route patterns plus the domains any of its sign-in methods actually write a token onto (a custom catalog may inject on a domain it doesn't route).
fn claimed_domains(integ: &Connector) -> impl Iterator<Item = &str> {
    integ.routes.iter().map(|r| r.match_pattern.as_str()).chain(
        integ
            .methods
            .iter()
            .flat_map(|m| m.injections().iter().map(|i| i.domain.as_str())),
    )
}

/// The catalog connectors a run can offer to connect: every entry (credential or oauth) not already applied and not colliding with an applied connector's domain.
pub fn resolve_connectable_connectors(
    policy: &Policy,
    catalog: &[Connector],
) -> ConnectableConnectors {
    resolve_connectable_with_declared(policy, &[], catalog)
}

/// Connectables minus any colliding with a domain already spoken for — by an applied credential (`policy.connectors`) or by an artifact-declared, offered-not-armed connector (`declared`) — so a colliding entry's machine-global stored value can never inject over the credential that owns that domain, even where that domain is an injection target rather than a declared route. Two *bundled* entries can no longer collide (one connector owns a domain, its alternatives are its sign-in methods); this still guards a user catalog entry that claims a bundled connector's domain.
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
        let seeds = declared.iter().any(|id| id == &integ.id);
        let providers = wire_providers(integ, seeds);
        if providers.is_empty() {
            continue;
        }
        out.providers.extend(providers);
        out.routes.insert(
            integ.id.clone(),
            integ.routes.iter().map(|r| r.to_route_rule()).collect(),
        );
        collect_signin_configs(
            integ,
            &mut out.owner,
            &mut out.oauth_configs,
            &mut out.pkce_configs,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_flow::providers::Provider;
    use lns_policy::connectors::{
        ConnectorRoute, CredentialAuth, OauthAuth, OauthFlow, SignInMethod,
    };
    use lns_policy::grants::GrantRecord;
    use lns_policy::providers::{InjectionDef, InjectionKind};

    fn cred_connector(id: &str, env_var: &str, domain: &str) -> Connector {
        Connector {
            id: id.into(),
            name: None,
            routes: vec![ConnectorRoute {
                match_pattern: domain.into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            methods: vec![SignInMethod::credential(
                "token",
                CredentialAuth {
                    env_var: env_var.into(),
                    placeholder: format!("lns-{id}-placeholder"),
                    injections: vec![InjectionDef {
                        kind: InjectionKind::BearerHeader,
                        domain: domain.into(),
                        header: None,
                    }],
                },
            )],
        }
    }

    /// A machine holding no sign-in at all; a connector with one way in ignores this, so it is the honest default for a single-method fixture.
    fn nothing_chosen(_provider: &str) -> bool {
        false
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
            &nothing_chosen,
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
        let out =
            resolve_applied_connectors(&policy_applying(&["gitlab"]), &catalog, &nothing_chosen);
        assert_eq!(out.providers.len(), 1);
        assert_eq!(out.providers[0].id(), "gitlab");
        assert_eq!(out.providers[0].env_var(), "GITLAB_TOKEN");
        assert_eq!(out.routes.len(), 1);
        assert_eq!(out.routes[0].match_pattern, "gitlab.com");
        assert_eq!(out.routes[0].verdict, lns_policy::Verdict::Allow);
    }

    fn provider_of(id: &str, env: &str, domain: &str) -> DefProvider {
        resolve_applied_connectors(
            &policy_applying(&[id]),
            &[cred_connector(id, env, domain)],
            &nothing_chosen,
        )
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
        let providers =
            resolve_applied_with_slots(&Policy::default(), &slots, &catalog, &nothing_chosen)
                .providers;
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
        let out = resolve_applied_connectors(&policy_applying(&[]), &catalog, &nothing_chosen);
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
            routes: vec![ConnectorRoute {
                match_pattern: domain.into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            methods: vec![SignInMethod::oauth(
                "device",
                OauthAuth {
                    userinfo_endpoint: None,
                    account_field: None,
                    flow: OauthFlow::Device,
                    client_id: Some(format!("Iv1.{id}")),
                    client_secret: None,
                    scopes: vec!["repo".into()],
                    device_authorization_endpoint: Some(format!(
                        "https://{domain}/login/device/code"
                    )),
                    authorization_endpoint: None,
                    token_endpoint: format!("https://{domain}/login/oauth/access_token"),
                    env_var: env_var.into(),
                    placeholder: format!("lns-{id}-placeholder"),
                    injections: vec![InjectionDef {
                        kind: InjectionKind::BearerHeader,
                        domain: domain.into(),
                        header: None,
                    }],
                },
            )],
        }
    }

    /// A connector reachable two ways, each with its own env var — the shape a user must choose between.
    fn two_method_connector(id: &str, domain: &str) -> Connector {
        let mut connector = cred_connector(id, "SOME_TOKEN", domain);
        connector.methods[0].id = "api-key".into();
        connector.methods.push(SignInMethod::credential(
            "subscription",
            CredentialAuth {
                env_var: "SOME_SUBSCRIPTION_TOKEN".into(),
                placeholder: format!("lns-{id}-subscription-placeholder"),
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: domain.into(),
                    header: None,
                }],
            },
        ));
        connector
    }

    #[test]
    fn declining_a_connector_reachable_two_ways_is_remembered_although_no_env_var_describes_it() {
        let catalog = vec![two_method_connector(
            "some-provider",
            "api.some-provider.example",
        )];
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let mut grants = WorkloadGrantFile::default();
        grants.upsert(GrantRecord::deny(
            "/proj",
            &workload,
            "some-provider",
            "",
            Vec::new(),
        ));
        let mut policy = Policy::default();
        policy.add_rule(RouteRule::allow_host("api.some-provider.example"));

        let out = withhold_undecided_connector_allows(
            &policy,
            &catalog,
            &HashSet::from(["some-provider".to_string()]),
            "/proj",
            &workload,
            &grants,
            &nothing_chosen,
        );
        assert_eq!(
            out.network.egress.http.len(),
            1,
            "a connector reachable several ways discloses no single env var, so its refusal records none; reading that as undecided would hold the domain against the user's own rule for ever"
        );
    }

    #[test]
    fn a_connected_connector_reachable_two_ways_seeds_nothing_until_a_method_is_chosen() {
        let catalog = vec![two_method_connector(
            "some-provider",
            "api.some-provider.example",
        )];
        let out = resolve_applied_connectors(
            &policy_applying(&["some-provider"]),
            &catalog,
            &nothing_chosen,
        );

        let seeded: Vec<&str> = out
            .providers
            .iter()
            .filter(|p| p.seeds_env())
            .map(|p| p.env_var())
            .collect();
        assert!(
            seeded.is_empty(),
            "connecting the connector does not answer which way in the user holds, so no env var may be seeded yet: {seeded:?}"
        );
    }

    #[test]
    fn a_sign_in_counts_as_chosen_when_the_machine_holds_a_value_or_this_workload_granted_it() {
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let mut state = lns_policy::credentials::CredentialStateFile::new();
        state.insert(
            "some-provider:api-key".into(),
            lns_policy::credentials::CredentialEntry::HostDetect,
        );
        state.insert(
            "some-provider:refused".into(),
            lns_policy::credentials::CredentialEntry::Deny,
        );
        let mut grants = WorkloadGrantFile::default();
        grants.upsert(GrantRecord::allow(
            "/proj",
            &workload,
            "some-provider:subscription",
            "SOME_SUBSCRIPTION_TOKEN",
            vec![],
        ));

        let chosen = machine_holds_sign_in(&state, &grants, "/proj", &workload);

        assert!(
            chosen("some-provider:api-key"),
            "choosing to use the host's own value is still a choice, so that sign-in seeds on the next launch"
        );
        assert!(
            chosen("some-provider:subscription"),
            "picking a sign-in on the offer card records only a grant — nothing was seeded for a value card to answer — so the grant alone must count, or the choice would evaporate"
        );
        assert!(
            !chosen("some-provider:refused"),
            "a refusal is not a sign-in the machine holds"
        );
        assert!(!chosen("some-provider:never-touched"));
    }

    #[test]
    fn a_connected_connector_no_sign_in_is_held_for_still_withholds_its_route() {
        let catalog = vec![two_method_connector(
            "some-provider",
            "api.some-provider.example",
        )];
        // A policy write mid-run connects it; nothing seeds, so no value card can ever fire for it.
        let mut policy = policy_applying(&["some-provider"]);
        policy.add_rule(RouteRule::allow_host("api.some-provider.example"));
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };

        let out = withhold_undecided_connector_allows(
            &policy,
            &catalog,
            &HashSet::from(["some-provider".to_string()]),
            "/proj",
            &workload,
            &WorkloadGrantFile::default(),
            &nothing_chosen,
        );

        assert!(
            out.network.egress.http.is_empty(),
            "the connector can never authenticate, so its route must wait for the card rather than letting requests leave unauthenticated"
        );
    }

    #[test]
    fn a_connected_connector_whose_sign_in_is_held_keeps_its_routes() {
        let catalog = vec![two_method_connector(
            "some-provider",
            "api.some-provider.example",
        )];
        let mut policy = policy_applying(&["some-provider"]);
        policy.add_rule(RouteRule::allow_host("api.some-provider.example"));
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };

        let out = withhold_undecided_connector_allows(
            &policy,
            &catalog,
            &HashSet::from(["some-provider".to_string()]),
            "/proj",
            &workload,
            &WorkloadGrantFile::default(),
            &|provider| provider == "some-provider:api-key",
        );

        assert_eq!(
            out.network.egress.http.len(),
            1,
            "the sign-in is held, so its value card carries the decision and withholding the route would only strand a working connector"
        );
    }

    #[test]
    fn a_connected_connector_no_sign_in_is_held_for_names_itself_for_refusal() {
        let catalog = vec![
            two_method_connector("some-provider", "api.some-provider.example"),
            cred_connector(
                "other-provider",
                "OTHER_TOKEN",
                "api.other-provider.example",
            ),
        ];
        let policy = policy_applying(&["some-provider", "other-provider"]);

        assert_eq!(
            unchosen_connected_connectors(&policy, &catalog, &nothing_chosen),
            vec!["some-provider".to_string()],
            "a connector with one way in is never in this state, and one whose sign-in is held is fine — only the silent case may refuse a launch"
        );
        assert!(
            unchosen_connected_connectors(&policy, &catalog, &|provider| provider
                == "some-provider:api-key")
            .is_empty(),
            "once a sign-in is held there is a value to inject, so the launch must proceed"
        );
    }

    #[test]
    fn the_refusal_points_at_disconnecting_so_first_use_can_ask_which_sign_in() {
        let message = unchosen_connectors_refusal(&["some-provider".to_string()]);
        assert!(
            message.contains("lns connector disconnect some-provider"),
            "the fix must name the one recovery that works — `lns connector connect` refuses a connector reachable several ways, so pointing there would be a dead end: {message}"
        );
    }

    #[test]
    fn a_slot_naming_a_connector_reachable_two_ways_remaps_the_held_methods_env() {
        let catalog = vec![two_method_connector(
            "some-provider",
            "api.some-provider.example",
        )];
        let out = resolve_applied_with_slots(
            &policy_applying(&[]),
            &[slot("some-provider", "REMAPPED_TOKEN", true)],
            &catalog,
            &|provider| provider == "some-provider:subscription",
        );
        let wired: Vec<(&str, &str)> = out
            .providers
            .iter()
            .map(|p| (p.id(), p.env_var()))
            .collect();
        assert_eq!(
            wired,
            vec![("some-provider:subscription", "REMAPPED_TOKEN")],
            "the slot renames the env var but the value still comes from the sign-in the machine holds"
        );
    }

    #[test]
    fn a_slot_wires_nothing_when_the_machine_holds_no_sign_in_for_the_connector() {
        let catalog = vec![two_method_connector(
            "some-provider",
            "api.some-provider.example",
        )];
        let out = resolve_applied_with_slots(
            &policy_applying(&[]),
            &[slot("some-provider", "REMAPPED_TOKEN", true)],
            &catalog,
            &nothing_chosen,
        );
        assert!(
            out.providers.is_empty(),
            "there is no value to inject, so wiring a provider would advertise a credential the workload never gets"
        );
    }

    #[test]
    fn a_value_bound_before_the_connector_grew_a_second_method_still_chooses_that_method() {
        let catalog = vec![two_method_connector(
            "some-provider",
            "api.some-provider.example",
        )];
        // What every release before sign-in methods wrote: one value under the bare connector id.
        let out = resolve_applied_connectors(
            &policy_applying(&["some-provider"]),
            &catalog,
            &|provider| provider == "some-provider",
        );

        let seeded: Vec<(&str, &str)> = out
            .providers
            .iter()
            .filter(|p| p.seeds_env())
            .map(|p| (p.id(), p.env_var()))
            .collect();
        assert_eq!(
            seeded,
            vec![("some-provider", "SOME_TOKEN")],
            "the legacy binding sits under the bare id, so the provider must keep that key — keying it per-method would seed the placeholder while the stored value resolved to nothing"
        );
    }

    #[test]
    fn a_legacy_binding_on_an_oauth_first_method_still_finds_its_sign_in_config() {
        let mut connector =
            oauth_connector("some-oauth", "SOME_OAUTH_TOKEN", "api.some-oauth.example");
        connector.methods.push(SignInMethod::credential(
            "token",
            CredentialAuth {
                env_var: "SOME_PASTED_TOKEN".into(),
                placeholder: "some-oauth-token-LNSPLACEHOLDER".into(),
                injections: Vec::new(),
            },
        ));
        let catalog = vec![connector];

        let out =
            resolve_applied_connectors(&policy_applying(&["some-oauth"]), &catalog, &|provider| {
                provider == "some-oauth"
            });

        let provider_id = out.providers[0].id().to_string();
        assert_eq!(
            provider_id, "some-oauth",
            "the legacy binding keys under the bare id"
        );
        let keys: Vec<&String> = out.oauth_configs.keys().collect();
        assert!(
            out.oauth_configs.contains_key(&provider_id),
            "the sign-in config must be reachable under the same key the provider resolved to, or the device flow and its token refresh are silently lost: {keys:?}"
        );
    }

    #[test]
    fn a_legacy_binding_on_a_pkce_first_method_still_finds_its_browser_sign_in_config() {
        let mut connector = pkce_connector("some-pkce", "SOME_PKCE_TOKEN", "api.some-pkce.example");
        connector.methods.push(SignInMethod::credential(
            "token",
            CredentialAuth {
                env_var: "SOME_PASTED_TOKEN".into(),
                placeholder: "some-pkce-token-LNSPLACEHOLDER".into(),
                injections: Vec::new(),
            },
        ));
        let catalog = vec![connector];

        let out =
            resolve_applied_connectors(&policy_applying(&["some-pkce"]), &catalog, &|provider| {
                provider == "some-pkce"
            });

        let keys: Vec<&String> = out.pkce_configs.keys().collect();
        assert!(
            out.pkce_configs.contains_key("some-pkce"),
            "the browser redirect is configured per method, so without the alias a legacy binding could never complete its sign-in: {keys:?}"
        );
    }

    #[test]
    fn a_value_bound_before_the_connector_grew_a_second_method_still_arms_and_injects() {
        use crate::credential_flow::registry::expand_credentials_for_wire_with_custom;
        use lns_policy::credentials::{CredentialEntry, CredentialStateFile};

        let catalog = vec![two_method_connector(
            "some-provider",
            "api.some-provider.example",
        )];
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        // Exactly what a release before sign-in methods wrote: value and grant both under the bare connector id.
        let mut state = CredentialStateFile::new();
        state.insert(
            "some-provider".into(),
            CredentialEntry::Stored {
                value: "some-secret".into(),
            },
        );
        let mut grants = WorkloadGrantFile::default();
        grants.upsert(GrantRecord::allow(
            "/proj",
            &workload,
            "some-provider",
            "SOME_TOKEN",
            vec!["api.some-provider.example".to_string()],
        ));
        let chosen = |provider: &str| {
            state
                .get(provider)
                .is_some_and(|e| !matches!(e, CredentialEntry::Deny))
        };

        let applied =
            resolve_applied_connectors(&policy_applying(&["some-provider"]), &catalog, &chosen);
        let run = run_providers(applied.providers, Vec::new());
        let armed = gate_armed_by_grant(&run.armed, &run.providers, "/proj", &workload, &grants);
        assert_eq!(
            armed,
            HashSet::from(["some-provider".to_string()]),
            "the grant the user already gave must still arm the connector, or the upgrade silently revokes their consent"
        );

        let wire = expand_credentials_for_wire_with_custom(&state, &run.providers, &armed);
        assert!(
            wire.iter().any(|c| !c.injections.is_empty()),
            "the stored value must still reach the wire, or every request leaves unauthenticated while the tool believes it is signed in"
        );
    }

    #[test]
    fn an_explicit_method_choice_beats_the_value_bound_under_the_bare_id() {
        let catalog = vec![two_method_connector(
            "some-provider",
            "api.some-provider.example",
        )];
        let out = resolve_applied_connectors(
            &policy_applying(&["some-provider"]),
            &catalog,
            &|provider| provider == "some-provider" || provider == "some-provider:subscription",
        );

        let seeded: Vec<&str> = out
            .providers
            .iter()
            .filter(|p| p.seeds_env())
            .map(|p| p.id())
            .collect();
        assert_eq!(
            seeded,
            vec!["some-provider:subscription"],
            "a sign-in the user picked outright must win over the one inferred from a legacy binding"
        );
    }

    #[test]
    fn a_connected_connector_seeds_only_the_sign_in_method_the_machine_holds() {
        let catalog = vec![two_method_connector(
            "some-provider",
            "api.some-provider.example",
        )];
        let out = resolve_applied_connectors(
            &policy_applying(&["some-provider"]),
            &catalog,
            &|provider| provider == "some-provider:subscription",
        );

        let seeded: Vec<(&str, &str)> = out
            .providers
            .iter()
            .filter(|p| p.seeds_env())
            .map(|p| (p.id(), p.env_var()))
            .collect();
        assert_eq!(
            seeded,
            vec![("some-provider:subscription", "SOME_SUBSCRIPTION_TOKEN")],
            "the chosen method's env var is what the tool branches on to see it is signed in, and the method not chosen must not appear at all"
        );
        assert_eq!(
            out.providers.len(),
            1,
            "a method the user did not choose has no business on the wire once another is bound"
        );
    }

    #[test]
    fn a_connected_connector_with_one_way_in_ignores_the_choice_predicate() {
        let catalog = vec![cred_connector(
            "some-provider",
            "SOME_TOKEN",
            "api.some-provider.example",
        )];
        let out = resolve_applied_connectors(
            &policy_applying(&["some-provider"]),
            &catalog,
            &nothing_chosen,
        );
        let seeded: Vec<&str> = out
            .providers
            .iter()
            .filter(|p| p.seeds_env())
            .map(|p| p.id())
            .collect();
        assert_eq!(
            seeded,
            vec!["some-provider"],
            "there is no choice to make, so a connector with one way in must keep seeding exactly as before this existed"
        );
    }

    #[test]
    fn a_catalog_entry_with_no_way_in_is_skipped_rather_than_offered_unconnectably() {
        let mut methodless =
            cred_connector("some-provider", "SOME_TOKEN", "api.some-provider.example");
        methodless.methods.clear();
        let out = resolve_connectable_connectors(&Policy::default(), &[methodless]);
        assert!(
            out.routes.is_empty() && out.providers.is_empty(),
            "nothing could ever connect it, so offering it would hold the domain against a card that can never be answered"
        );
    }

    #[test]
    fn a_connector_reachable_two_ways_seeds_neither_env_var_until_the_user_picks_one() {
        let catalog = vec![two_method_connector(
            "some-provider",
            "api.some-provider.example",
        )];
        let out = resolve_connectable_with_declared(
            &Policy::default(),
            &["some-provider".to_string()],
            &catalog,
        );

        let seeded: Vec<&str> = out
            .providers
            .iter()
            .filter(|p| p.seeds_env())
            .map(|p| p.env_var())
            .collect();
        assert!(
            seeded.is_empty(),
            "seeding one env var picks the sign-in for the user and seeding both fakes a login state no real sign-in produces, so neither may reach the workload: {seeded:?}"
        );
    }

    #[test]
    fn a_connector_reachable_two_ways_still_wires_each_method_so_it_can_be_offered() {
        let catalog = vec![two_method_connector(
            "some-provider",
            "api.some-provider.example",
        )];
        let out = resolve_connectable_connectors(&Policy::default(), &catalog);

        let mut ids: Vec<&str> = out.providers.iter().map(|p| p.id()).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec!["some-provider:api-key", "some-provider:subscription"],
            "each method needs its own wire provider under its own store key, or the MITM could not recognize its placeholder and the choice could not be bound"
        );
        assert!(
            out.routes.contains_key("some-provider"),
            "without its routes the connector is not offerable, so the first request to its domain could never raise the card"
        );
    }

    #[test]
    fn a_connector_with_one_way_in_still_seeds_its_placeholder_when_the_definition_declares_it() {
        let catalog = vec![cred_connector(
            "some-provider",
            "SOME_TOKEN",
            "api.some-provider.example",
        )];
        let out = resolve_connectable_with_declared(
            &Policy::default(),
            &["some-provider".to_string()],
            &catalog,
        );
        let seeded: Vec<&str> = out
            .providers
            .iter()
            .filter(|p| p.seeds_env())
            .map(|p| p.id())
            .collect();
        assert_eq!(
            seeded,
            vec!["some-provider"],
            "a connector with one way in has no choice to make, so declaring it must keep seeding under the bare id every stored value is already bound to"
        );
    }

    fn pkce_connector(id: &str, env_var: &str, domain: &str) -> Connector {
        Connector {
            id: id.into(),
            name: None,
            routes: vec![ConnectorRoute {
                match_pattern: domain.into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            methods: vec![SignInMethod::oauth(
                "device",
                OauthAuth {
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
                },
            )],
        }
    }

    #[test]
    fn an_applied_oauth_connector_contributes_a_provider_routes_and_its_oauth_config() {
        let catalog = vec![oauth_connector(
            "somesaas",
            "SOMESAAS_TOKEN",
            "api.somesaas.com",
        )];
        let out =
            resolve_applied_connectors(&policy_applying(&["somesaas"]), &catalog, &nothing_chosen);
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
        let out = resolve_applied_connectors(
            &policy_applying(&["huggingface"]),
            &catalog,
            &nothing_chosen,
        );
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
            &nothing_chosen,
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
        applied.methods[0].credential.as_mut().unwrap().injections[0].domain =
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
        let out =
            resolve_applied_connectors(&policy_applying(&["somepkce"]), &catalog, &nothing_chosen);
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
        let out =
            resolve_applied_connectors(&policy_applying(&["somesaas"]), &catalog, &nothing_chosen);
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
        i.methods[0].oauth.as_mut().unwrap().client_id = None;
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
        let out =
            resolve_applied_connectors(&policy_applying(&["somesaas"]), &catalog, &nothing_chosen);
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
            &nothing_chosen,
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
            &nothing_chosen,
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
            &nothing_chosen,
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
            &nothing_chosen,
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
            &nothing_chosen,
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
            &nothing_chosen,
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
