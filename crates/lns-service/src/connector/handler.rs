//! The three machine verbs of `docs/cli-spec.md` §3.3, over a store and a source.

use anyhow::Result;
use lns_ipc::{ConnectorMethodView, ConnectorProfileView, ConnectorView};

use super::source::{ConnectorSource, Source};
use super::store::{ConnectorStore, Installed, Profile};
use std::collections::BTreeMap;

/// Resolve `<REF|PATH>` and install what it names. The digest comes from the resolver, never from a caller, because a grant binds to it.
pub async fn install<S: ConnectorSource + ?Sized>(
    store: &ConnectorStore<'_>,
    source: &S,
    operand: &str,
) -> Result<ConnectorView> {
    let fetched = source.fetch(&Source::of(operand)?).await?;
    let definition = store.install(&fetched.digest, &fetched.document)?;
    Ok(view_of(
        &definition,
        &fetched.digest,
        &store.profiles_of(&definition.name)?,
    ))
}

pub fn uninstall(store: &ConnectorStore<'_>, name: &str) -> Result<Option<usize>> {
    let held = store.profiles_of(name)?.len();
    Ok(store.uninstall(name)?.then_some(held))
}

pub fn list(store: &ConnectorStore<'_>) -> Result<Vec<ConnectorView>> {
    store
        .installed()?
        .iter()
        .map(|entry| one_installed(store, entry))
        .collect()
}

/// A stored document that will not parse is still listed, by name and digest alone, so `list` can show what `install` refuses to work beside.
fn one_installed(store: &ConnectorStore<'_>, entry: &Installed) -> Result<ConnectorView> {
    let profiles = store.profiles_of(&entry.name)?;
    Ok(match lns_artifact::connector::parse(&entry.document) {
        Ok(definition) => view_of(&definition, &entry.digest, &profiles),
        Err(_) => ConnectorView {
            name: entry.name.clone(),
            digest: entry.digest.clone(),
            serves: Vec::new(),
            methods: Vec::new(),
            profiles: profile_views(&profiles),
        },
    })
}

fn view_of(
    definition: &lns_artifact::connector::ConnectorDefinition,
    digest: &str,
    profiles: &BTreeMap<String, Profile>,
) -> ConnectorView {
    ConnectorView {
        name: definition.name.clone(),
        digest: digest.to_string(),
        serves: definition.spec.serves.clone(),
        methods: definition
            .spec
            .methods
            .iter()
            .map(|method| ConnectorMethodView {
                name: method.name.clone(),
                label: method.label().to_string(),
                needs_connect: method.auth.is_some(),
                offerable: method.is_offerable(),
                opens: opened_by(method),
                writes: method
                    .filesets
                    .iter()
                    .map(|fileset| fileset.guest_path.clone())
                    .collect(),
                env: method.env.keys().cloned().collect(),
                credentials: method
                    .credentials
                    .iter()
                    .map(|credential| credential.owner().to_string())
                    .collect(),
            })
            .collect(),
        profiles: profile_views(profiles),
    }
}

/// Every destination a method's egress opens, http and raw alike, because `serves` does not bound what a method opens. A `deny` rule closes rather than opens, so it is not disclosed as one.
fn opened_by(method: &lns_artifact::connector::Method) -> Vec<String> {
    let opens = |verdict: lns_policy::Verdict| verdict != lns_policy::Verdict::Deny;
    method
        .egress
        .http
        .iter()
        .filter(|rule| opens(rule.verdict))
        .map(|rule| rule.match_pattern.clone())
        .chain(
            method
                .egress
                .tcp
                .iter()
                .filter(|rule| opens(rule.verdict))
                .map(|rule| rule.match_pattern.clone()),
        )
        .collect()
}

fn profile_views(profiles: &BTreeMap<String, Profile>) -> Vec<ConnectorProfileView> {
    profiles
        .iter()
        .map(|(label, profile)| ConnectorProfileView {
            label: label.clone(),
            method: profile.method.clone(),
            authority: profile.authority.0.iter().cloned().collect(),
        })
        .collect()
}

/// What one connect produced: the profile it stored, and the project directories whose grant its authority no longer matches (§3.2.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connected {
    pub profile: String,
    pub invalidated: Vec<String>,
}

/// What one grant recorded, and the method it displaced — a project holds one grant per connector (§3.2.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Granted {
    pub method: String,
    pub profile: Option<String>,
    pub displaced: Option<String>,
    /// True when the project already held exactly this grant, which cli-spec §3.3 makes an exit-1 answer rather than a change.
    pub unchanged: bool,
}

/// Store what an authentication returned as a profile. A method with no `auth` has nothing to connect and is granted instead (cli-spec §3.3).
pub fn connect(
    store: &ConnectorStore<'_>,
    name: &str,
    method: &str,
    label: &str,
    values: std::collections::BTreeMap<String, String>,
) -> Result<Connected> {
    let definition = definition_of(store, name)?;
    let method = offerable_method(&definition, method)?;
    if method.auth.is_none() {
        anyhow::bail!(
            "method {} of {name} has no authentication, so there is nothing to connect; grant it instead",
            method.name
        );
    }
    let invalidated = store.record_authentication(
        name,
        label,
        super::store::Profile {
            method: method.name.clone(),
            // A `kind: token` exchange reports no authority (§3.2.4).
            authority: super::store::Authority::default(),
            values,
        },
    )?;
    Ok(Connected {
        profile: label.to_string(),
        invalidated,
    })
}

/// Drop one profile, or every profile of a connector. The connector stays installed and grants naming a dropped profile stay (cli-spec §3.3).
pub fn disconnect(store: &ConnectorStore<'_>, name: &str, profile: Option<&str>) -> Result<usize> {
    installed_entry(store, name)?;
    Ok(store.drop_profiles(name, profile)?)
}

/// Record this project's grant of one method, replacing whatever it decided before.
pub fn grant(
    store: &ConnectorStore<'_>,
    name: &str,
    project_dir: &str,
    method: &str,
    profile: Option<&str>,
) -> Result<Granted> {
    let entry = installed_entry(store, name)?;
    let definition = lns_artifact::connector::parse(&entry.document)?;
    let method = offerable_method(&definition, method)?;
    let profile = behind_the_method(store, name, method, profile)?;
    let authority = match &profile {
        Some(label) => store
            .profiles_of(name)?
            .get(label)
            .map(|held| held.authority.clone())
            .unwrap_or_default(),
        None => super::store::Authority::default(),
    };
    let decision = super::store::ProjectDecision::Granted {
        digest: entry.digest.clone(),
        method: method.name.clone(),
        profile: profile.clone(),
        authority,
    };
    let held = store.decision(project_dir, name)?;
    if held.as_ref() == Some(&decision) {
        return Ok(Granted {
            method: method.name.clone(),
            profile,
            displaced: None,
            unchanged: true,
        });
    }
    let displaced = store.decide(project_dir, name, decision)?;
    Ok(Granted {
        method: method.name.clone(),
        profile,
        displaced: displaced.and_then(displaced_method),
        unchanged: false,
    })
}

pub fn forget(store: &ConnectorStore<'_>, name: &str, project_dir: &str) -> Result<bool> {
    Ok(store.forget(project_dir, name)?)
}

/// Every destination an installed connector serves that this project has not decided, so a run holds them for an offer (§3.2.1).
pub fn held_patterns(store: &ConnectorStore<'_>, project_dir: &str) -> Result<Vec<String>> {
    let mut patterns = Vec::new();
    let mut unreadable = Vec::new();
    for entry in store.installed()? {
        if decided_here(store, project_dir, &entry) {
            continue;
        }
        match lns_artifact::connector::parse(&entry.document) {
            Ok(definition) => patterns.extend(definition.spec.serves),
            Err(_) => unreadable.push(entry.name),
        }
    }
    if !unreadable.is_empty() {
        let names = unreadable.join(", ");
        crate::log::warn!(
            "cannot read the installed connector(s) {names}, so this run is not offered what they serve"
        );
    }
    Ok(patterns)
}

/// A decision this run cannot read is not one: holding asks, which a grant then answers, where letting the destination through cannot be taken back (§3.2.1).
fn decided_here(store: &ConnectorStore<'_>, project_dir: &str, entry: &Installed) -> bool {
    match store.decision(project_dir, &entry.name) {
        Ok(decision) => decision.is_some_and(|decision| decision.decides(&entry.digest)),
        Err(e) => {
            crate::log::warn!(
                "cannot read what this project decided about {}, so its destinations are held: {e}",
                entry.name
            );
            false
        }
    }
}

/// Which profile stands behind a method: none for one that does not authenticate, the named one, or the only one held.
fn behind_the_method(
    store: &ConnectorStore<'_>,
    name: &str,
    method: &lns_artifact::connector::Method,
    named: Option<&str>,
) -> Result<Option<String>> {
    if method.auth.is_none() {
        if let Some(named) = named {
            anyhow::bail!(
                "method {} of {name} does not authenticate, so it takes no profile; drop --profile {named}",
                method.name
            );
        }
        return Ok(None);
    }
    // A profile records the method that produced it, so one made for another method holds values for another method's credentials.
    let held: BTreeMap<String, Profile> = store
        .profiles_of(name)?
        .into_iter()
        .filter(|(_, profile)| profile.method == method.name)
        .collect();
    match named {
        Some(named) if held.contains_key(named) => Ok(Some(named.to_string())),
        Some(named) => anyhow::bail!(
            "{name} holds no profile named {named} for method {}",
            method.name
        ),
        None if held.len() == 1 => Ok(held.into_keys().next()),
        None if held.is_empty() => anyhow::bail!(
            "{name} authenticates and this machine holds no profile for method {}; run `lns connector connect {name} --method {}` first",
            method.name,
            method.name
        ),
        None => anyhow::bail!(
            "{name} holds {} profiles for method {}, so name one with --profile",
            held.len(),
            method.name
        ),
    }
}

fn displaced_method(decision: super::store::ProjectDecision) -> Option<String> {
    match decision {
        super::store::ProjectDecision::Granted { method, .. } => Some(method),
        super::store::ProjectDecision::Declined => None,
    }
}

fn installed_entry(store: &ConnectorStore<'_>, name: &str) -> Result<Installed> {
    store
        .installed()?
        .into_iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| anyhow::anyhow!("no connector named {name} is installed on this machine"))
}

fn definition_of(
    store: &ConnectorStore<'_>,
    name: &str,
) -> Result<lns_artifact::connector::ConnectorDefinition> {
    let entry = installed_entry(store, name)?;
    lns_artifact::connector::parse(&entry.document)
}

/// The named method, refused when this version cannot offer it — the card could not either (§3.2.2).
fn offerable_method<'a>(
    definition: &'a lns_artifact::connector::ConnectorDefinition,
    method: &str,
) -> Result<&'a lns_artifact::connector::Method> {
    let found = definition
        .spec
        .methods
        .iter()
        .find(|m| m.name == method)
        .ok_or_else(|| anyhow::anyhow!("{} declares no method named {method}", definition.name))?;
    if !found.is_offerable() {
        anyhow::bail!("method {method} of {} needs a newer lns", definition.name);
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connector::source::FetchedConnector;
    use crate::connector::store::{Authority, InstalledSet, ProjectDecision};
    use lns_policy::decision_store::{DecisionFile, DecisionStore};
    use std::sync::Mutex;

    struct FakeMap<T> {
        state: Mutex<DecisionFile<T>>,
        /// Set when the test needs the file to be there but unreadable, as a truncated or hand-edited json file is.
        unreadable: std::sync::atomic::AtomicBool,
    }

    impl<T> Default for FakeMap<T> {
        fn default() -> Self {
            Self {
                state: Mutex::new(DecisionFile::new()),
                unreadable: std::sync::atomic::AtomicBool::new(false),
            }
        }
    }

    impl<T> FakeMap<T> {
        fn make_unreadable(&self) {
            self.unreadable
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    impl<T: Clone + Send + Sync> DecisionStore<T> for FakeMap<T> {
        fn load(&self) -> std::io::Result<DecisionFile<T>> {
            if self.unreadable.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "trailing characters",
                ));
            }
            Ok(self.state.lock().unwrap().clone())
        }
        fn save(&self, state: &DecisionFile<T>) -> std::io::Result<()> {
            *self.state.lock().unwrap() = state.clone();
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeSet {
        entries: Mutex<Vec<Installed>>,
    }

    impl InstalledSet for FakeSet {
        fn list(&self) -> std::io::Result<Vec<Installed>> {
            Ok(self.entries.lock().unwrap().clone())
        }
        fn put(&self, name: &str, digest: &str, document: &[u8]) -> std::io::Result<()> {
            let mut held = self.entries.lock().unwrap();
            held.retain(|e| e.name != name);
            held.push(Installed {
                name: name.to_string(),
                digest: digest.to_string(),
                document: document.to_vec(),
            });
            Ok(())
        }
        fn remove(&self, name: &str) -> std::io::Result<bool> {
            let mut held = self.entries.lock().unwrap();
            let before = held.len();
            held.retain(|e| e.name != name);
            Ok(held.len() != before)
        }
    }

    /// Answers with one prepared connector, and records what it was asked for.
    struct FakeSource {
        digest: String,
        document: Vec<u8>,
        asked: Mutex<Vec<Source>>,
    }

    impl ConnectorSource for FakeSource {
        async fn fetch(&self, source: &Source) -> Result<FetchedConnector> {
            self.asked.lock().unwrap().push(source.clone());
            Ok(FetchedConnector {
                digest: self.digest.clone(),
                document: self.document.clone(),
            })
        }
    }

    fn document(name: &str, host: &str) -> Vec<u8> {
        serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": name,
            "spec": {
                "serves": [host],
                "methods": [
                    { "name": "token", "label": "API token", "auth": { "kind": "token" } },
                    { "name": "open", "env": { "SOME_REGION": "eu" } },
                ],
            },
        })
        .to_string()
        .into_bytes()
    }

    struct Rig {
        set: FakeSet,
        values: FakeMap<Profile>,
        grants: FakeMap<super::super::store::ProjectDecision>,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                set: FakeSet::default(),
                values: FakeMap::default(),
                grants: FakeMap::default(),
            }
        }
        fn store(&self) -> ConnectorStore<'_> {
            ConnectorStore::new(&self.set, &self.values, &self.grants)
        }
    }

    fn source(name: &str, host: &str) -> FakeSource {
        FakeSource {
            digest: "sha256:abc".to_string(),
            document: document(name, host),
            asked: Mutex::new(Vec::new()),
        }
    }

    #[tokio::test]
    async fn installing_reports_what_it_serves_and_which_methods_need_a_connect() {
        let rig = Rig::new();
        let src = source("some-provider", "api.some-provider.example");
        let view = install(&rig.store(), &src, "ghcr.io/acme/some-provider:1")
            .await
            .unwrap();

        assert_eq!(view.name, "some-provider");
        assert_eq!(view.digest, "sha256:abc");
        assert_eq!(view.serves, ["api.some-provider.example"]);
        assert_eq!(
            view.methods
                .iter()
                .map(|m| (m.name.as_str(), m.label.as_str(), m.needs_connect))
                .collect::<Vec<_>>(),
            [("token", "API token", true), ("open", "open", false)],
            "a method with no auth has nothing to connect, and one with no label falls back to its name"
        );
        assert!(view.profiles.is_empty(), "installing connects nothing");
    }

    #[tokio::test]
    async fn the_digest_stored_is_the_one_the_resolver_computed() {
        // No caller names a digest: a grant binds to it, so a mismatched one would apply a grant to bytes nobody consented to.
        let rig = Rig::new();
        let src = source("some-provider", "api.some-provider.example");
        install(&rig.store(), &src, "/work/some-provider")
            .await
            .unwrap();
        let stored = rig.store().installed().unwrap();
        assert_eq!(stored[0].digest, "sha256:abc");
        assert_eq!(
            stored[0].document,
            document("some-provider", "api.some-provider.example")
        );
    }

    #[tokio::test]
    async fn a_reference_and_an_absolute_path_reach_the_resolver_as_the_form_they_are() {
        let rig = Rig::new();
        let src = source("some-provider", "api.some-provider.example");
        install(&rig.store(), &src, "ghcr.io/acme/some-provider:1")
            .await
            .unwrap();
        install(&rig.store(), &src, "/work/some-provider")
            .await
            .unwrap();
        assert_eq!(
            *src.asked.lock().unwrap(),
            [
                Source::Reference("ghcr.io/acme/some-provider:1".to_string()),
                Source::Local("/work/some-provider".into()),
            ]
        );
    }

    #[tokio::test]
    async fn a_relative_path_is_refused_before_anything_is_fetched() {
        let rig = Rig::new();
        let src = source("some-provider", "api.some-provider.example");
        let err = install(&rig.store(), &src, "./some-provider")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("relative"), "{err}");
        assert!(src.asked.lock().unwrap().is_empty(), "nothing was fetched");
    }

    #[tokio::test]
    async fn an_install_the_store_refuses_reports_the_refusal() {
        let rig = Rig::new();
        install(
            &rig.store(),
            &source("some-provider", "*.shared.example"),
            "ghcr.io/acme/some-provider:1",
        )
        .await
        .unwrap();
        let err = install(
            &rig.store(),
            &source("other-provider", "api.shared.example"),
            "ghcr.io/acme/other-provider:1",
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("some-provider"), "{err}");
    }

    #[tokio::test]
    async fn uninstalling_reports_how_many_profiles_it_dropped() {
        let rig = Rig::new();
        install(
            &rig.store(),
            &source("some-provider", "api.some-provider.example"),
            "ghcr.io/acme/some-provider:1",
        )
        .await
        .unwrap();
        for label in ["work", "personal"] {
            rig.store()
                .record_authentication(
                    "some-provider",
                    label,
                    Profile {
                        method: "token".to_string(),
                        authority: Authority::default(),
                        values: Default::default(),
                    },
                )
                .unwrap();
        }
        assert_eq!(uninstall(&rig.store(), "some-provider").unwrap(), Some(2));
    }

    #[test]
    fn uninstalling_something_never_installed_answers_none() {
        let rig = Rig::new();
        assert_eq!(uninstall(&rig.store(), "absent").unwrap(), None);
    }

    #[tokio::test]
    async fn the_list_names_each_connector_with_the_profiles_this_machine_holds() {
        let rig = Rig::new();
        install(
            &rig.store(),
            &source("some-provider", "api.some-provider.example"),
            "ghcr.io/acme/some-provider:1",
        )
        .await
        .unwrap();
        rig.store()
            .record_authentication(
                "some-provider",
                "work",
                Profile {
                    method: "token".to_string(),
                    authority: Authority::of(["repo:read"]),
                    values: Default::default(),
                },
            )
            .unwrap();

        let listed = list(&rig.store()).unwrap();

        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].profiles,
            [ConnectorProfileView {
                label: "work".to_string(),
                method: "token".to_string(),
                authority: vec!["repo:read".to_string()],
            }]
        );
    }

    #[test]
    fn a_machine_with_nothing_installed_lists_nothing() {
        assert!(list(&Rig::new().store()).unwrap().is_empty());
    }

    #[test]
    fn an_unreadable_document_is_still_listed_by_name_and_digest() {
        // `install` refuses while one of these is present, so `list` is how the user finds out which to uninstall.
        let rig = Rig::new();
        rig.set
            .put("mystery", "sha256:xyz", b"not a document")
            .unwrap();
        let listed = list(&rig.store()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "mystery");
        assert_eq!(listed[0].digest, "sha256:xyz");
        assert!(listed[0].serves.is_empty());
        assert!(listed[0].methods.is_empty());
    }
    async fn installed(rig: &Rig) {
        install(
            &rig.store(),
            &source("some-provider", "api.some-provider.example"),
            "ghcr.io/acme/some-provider:1",
        )
        .await
        .expect("install");
    }

    fn values() -> std::collections::BTreeMap<String, String> {
        [("SOME_TOKEN".to_string(), "real-secret".to_string())].into()
    }

    #[tokio::test]
    async fn connecting_stores_a_profile_the_machine_then_holds() {
        let rig = Rig::new();
        installed(&rig).await;
        let connected = connect(&rig.store(), "some-provider", "token", "work", values())
            .expect("token is an offerable method that authenticates");
        assert_eq!(connected.profile, "work");
        assert!(connected.invalidated.is_empty());
        assert_eq!(
            rig.store().profiles_of("some-provider").unwrap()["work"].method,
            "token"
        );
    }

    #[tokio::test]
    async fn connecting_a_method_that_does_not_authenticate_is_refused_and_names_granting() {
        // cli-spec §3.3: a method with no `auth` has nothing to connect, so it is granted instead.
        let rig = Rig::new();
        installed(&rig).await;
        let err = connect(&rig.store(), "some-provider", "open", "work", values())
            .unwrap_err()
            .to_string();
        assert!(err.contains("nothing to connect"), "{err}");
        assert!(err.contains("grant it instead"), "{err}");
    }

    #[tokio::test]
    async fn connecting_names_the_connector_that_is_not_installed() {
        let rig = Rig::new();
        let err = connect(&rig.store(), "absent", "token", "work", values())
            .unwrap_err()
            .to_string();
        assert!(err.contains("absent"), "{err}");
    }

    #[tokio::test]
    async fn connecting_a_method_the_connector_does_not_declare_is_refused() {
        let rig = Rig::new();
        installed(&rig).await;
        let err = connect(&rig.store(), "some-provider", "mystery", "work", values())
            .unwrap_err()
            .to_string();
        assert!(err.contains("no method named mystery"), "{err}");
    }

    #[tokio::test]
    async fn disconnecting_drops_the_profiles_and_leaves_the_connector_installed() {
        let rig = Rig::new();
        installed(&rig).await;
        connect(&rig.store(), "some-provider", "token", "work", values()).unwrap();
        connect(&rig.store(), "some-provider", "token", "personal", values()).unwrap();

        assert_eq!(
            disconnect(&rig.store(), "some-provider", Some("work")).unwrap(),
            1
        );
        assert_eq!(disconnect(&rig.store(), "some-provider", None).unwrap(), 1);
        assert_eq!(
            rig.store().installed().unwrap().len(),
            1,
            "the connector stays installed"
        );
    }

    #[tokio::test]
    async fn disconnecting_a_connector_holding_none_drops_nothing() {
        // The caller exits 1 on this, so it must be an answer rather than an error.
        let rig = Rig::new();
        installed(&rig).await;
        assert_eq!(disconnect(&rig.store(), "some-provider", None).unwrap(), 0);
    }

    #[tokio::test]
    async fn granting_a_method_that_authenticates_takes_the_only_profile_held() {
        let rig = Rig::new();
        installed(&rig).await;
        connect(&rig.store(), "some-provider", "token", "work", values()).unwrap();
        let granted = grant(&rig.store(), "some-provider", "/work", "token", None).expect("grant");
        assert_eq!(granted.method, "token");
        assert_eq!(granted.profile.as_deref(), Some("work"));
        assert_eq!(granted.displaced, None);
    }

    #[tokio::test]
    async fn granting_binds_to_the_digest_installed_now() {
        // §7.1: a grant bound to other bytes is not one, so the grant has to record the digest it consented to.
        let rig = Rig::new();
        installed(&rig).await;
        grant(&rig.store(), "some-provider", "/work", "open", None).expect("grant");
        assert!(
            rig.store()
                .grant_for("/work", "some-provider", "sha256:abc")
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn granting_a_method_that_authenticates_with_no_profile_held_says_to_connect_first() {
        let rig = Rig::new();
        installed(&rig).await;
        let err = grant(&rig.store(), "some-provider", "/work", "token", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("connector connect"), "{err}");
    }

    #[tokio::test]
    async fn granting_with_several_profiles_held_and_none_named_asks_for_one() {
        let rig = Rig::new();
        installed(&rig).await;
        for label in ["work", "personal"] {
            connect(&rig.store(), "some-provider", "token", label, values()).unwrap();
        }
        let err = grant(&rig.store(), "some-provider", "/work", "token", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--profile"), "{err}");
    }

    #[tokio::test]
    async fn granting_a_profile_the_machine_does_not_hold_is_refused() {
        let rig = Rig::new();
        installed(&rig).await;
        connect(&rig.store(), "some-provider", "token", "work", values()).unwrap();
        let err = grant(
            &rig.store(),
            "some-provider",
            "/work",
            "token",
            Some("other"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("no profile named other"), "{err}");
    }

    #[tokio::test]
    async fn granting_a_method_that_does_not_authenticate_takes_no_profile() {
        let rig = Rig::new();
        installed(&rig).await;
        let granted = grant(&rig.store(), "some-provider", "/work", "open", None).expect("grant");
        assert_eq!(granted.profile, None);
        let err = grant(&rig.store(), "some-provider", "/work", "open", Some("work"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("takes no profile"), "{err}");
    }

    #[tokio::test]
    async fn granting_again_names_the_method_it_displaces() {
        // §3.2.4: a project holds one grant per connector, so what it prints names what it replaced.
        let rig = Rig::new();
        installed(&rig).await;
        connect(&rig.store(), "some-provider", "token", "work", values()).unwrap();
        grant(&rig.store(), "some-provider", "/work", "token", None).unwrap();
        let granted = grant(&rig.store(), "some-provider", "/work", "open", None).expect("grant");
        assert_eq!(granted.displaced.as_deref(), Some("token"));
    }

    #[tokio::test]
    async fn granting_over_a_decline_displaces_no_method() {
        let rig = Rig::new();
        installed(&rig).await;
        rig.store()
            .decide(
                "/work",
                "some-provider",
                super::super::store::ProjectDecision::Declined,
            )
            .unwrap();
        let granted = grant(&rig.store(), "some-provider", "/work", "open", None).expect("grant");
        assert_eq!(granted.displaced, None, "a decline is not a method");
    }

    #[tokio::test]
    async fn granting_an_unofferable_method_is_refused_because_the_card_could_not_offer_it() {
        let rig = Rig::new();
        let doc = serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": "some-provider",
            "spec": {
                "serves": ["api.some-provider.example"],
                "methods": [{ "name": "future", "auth": { "kind": "oauth_device" } }],
            },
        })
        .to_string()
        .into_bytes();
        rig.set.put("some-provider", "sha256:abc", &doc).unwrap();
        let err = grant(&rig.store(), "some-provider", "/work", "future", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("needs a newer lns"), "{err}");
    }

    #[tokio::test]
    async fn forgetting_clears_what_the_project_decided() {
        let rig = Rig::new();
        installed(&rig).await;
        grant(&rig.store(), "some-provider", "/work", "open", None).unwrap();
        assert!(forget(&rig.store(), "some-provider", "/work").unwrap());
        assert!(!forget(&rig.store(), "some-provider", "/work").unwrap());
        assert_eq!(
            rig.store().decision("/work", "some-provider").unwrap(),
            None
        );
    }
    fn two_token_methods(name: &str) -> Vec<u8> {
        serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": name,
            "spec": {
                "serves": ["api.some-provider.example"],
                "methods": [
                    { "name": "token", "auth": { "kind": "token" } },
                    { "name": "token-org", "auth": { "kind": "token" } },
                ],
            },
        })
        .to_string()
        .into_bytes()
    }

    #[tokio::test]
    async fn a_grant_will_not_take_a_profile_another_method_produced() {
        // A profile records the method that made it, so one made elsewhere holds values for another method's credentials.
        let rig = Rig::new();
        rig.set
            .put(
                "some-provider",
                "sha256:abc",
                &two_token_methods("some-provider"),
            )
            .unwrap();
        connect(&rig.store(), "some-provider", "token", "personal", values()).unwrap();

        let err = grant(&rig.store(), "some-provider", "/work", "token-org", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("connector connect"), "{err}");
        assert_eq!(
            rig.store().decision("/work", "some-provider").unwrap(),
            None,
            "nothing may be recorded when the profile does not belong to the method"
        );
    }

    #[tokio::test]
    async fn naming_a_profile_of_another_method_is_refused() {
        let rig = Rig::new();
        rig.set
            .put(
                "some-provider",
                "sha256:abc",
                &two_token_methods("some-provider"),
            )
            .unwrap();
        connect(&rig.store(), "some-provider", "token", "personal", values()).unwrap();
        let err = grant(
            &rig.store(),
            "some-provider",
            "/work",
            "token-org",
            Some("personal"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("no profile named personal"), "{err}");
    }

    #[tokio::test]
    async fn a_grant_the_project_already_holds_is_unchanged_rather_than_a_replacement() {
        // cli-spec §3.3: exits 1 when the project already granted that method and profile — and "replaced" would be false.
        let rig = Rig::new();
        installed(&rig).await;
        connect(&rig.store(), "some-provider", "token", "work", values()).unwrap();
        let first = grant(&rig.store(), "some-provider", "/work", "token", None).unwrap();
        assert!(!first.unchanged);

        let again = grant(&rig.store(), "some-provider", "/work", "token", None).unwrap();
        assert!(again.unchanged, "the same grant twice is not a change");
        assert_eq!(again.displaced, None, "it replaced nothing");
    }

    #[tokio::test]
    async fn a_method_view_discloses_the_whole_payload_a_grant_applies() {
        // §1.5: consent is given to what the card showed, so every part of the payload has to reach it.
        let rig = Rig::new();
        let doc = serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": "some-provider",
            "spec": {
                "serves": ["api.some-provider.example"],
                "methods": [{
                    "name": "token",
                    "auth": { "kind": "token" },
                    "egress": {
                        "http": [{ "match": "other.example", "verdict": "allow" }],
                        "tcp": [{ "match": "db.example:5432", "verdict": "allow" }],
                    },
                    "env": { "SOME_REGION": "eu" },
                    "credentials": [{
                        "envVar": "SOME_TOKEN",
                        "placeholder": "some_LNSPLACEHOLDER0000000000",
                    }],
                    "filesets": [{
                        "inline": { "credentials.json": "{\"token\":\"some_LNSPLACEHOLDER0000000000\"}" },
                        "guestPath": "/home/agent/.some-provider",
                    }],
                }],
            },
        })
        .to_string()
        .into_bytes();
        rig.set.put("some-provider", "sha256:abc", &doc).unwrap();

        let listed = list(&rig.store()).unwrap();
        let method = &listed[0].methods[0];
        assert_eq!(
            method.opens,
            ["other.example", "db.example:5432"],
            "a method's egress is not bounded by `serves`, so both tables reach the card"
        );
        assert_eq!(method.writes, ["/home/agent/.some-provider"]);
        assert_eq!(method.env, ["SOME_REGION"]);
        assert_eq!(method.credentials, ["SOME_TOKEN"]);
    }

    #[tokio::test]
    async fn a_credential_with_no_env_var_is_disclosed_by_its_placeholder() {
        let rig = Rig::new();
        let doc = serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": "some-provider",
            "spec": {
                "serves": ["api.some-provider.example"],
                "methods": [{
                    "name": "token",
                    "auth": { "kind": "token" },
                    "credentials": [{
                        "placeholder": "some_LNSPLACEHOLDER0000000000",
                        "injections": [{ "kind": "bearer_header", "domain": "api.some-provider.example" }],
                    }],
                }],
            },
        })
        .to_string()
        .into_bytes();
        rig.set.put("some-provider", "sha256:abc", &doc).unwrap();
        let listed = list(&rig.store()).unwrap();
        assert_eq!(
            listed[0].methods[0].credentials,
            ["some_LNSPLACEHOLDER0000000000"],
            "a credential with no envVar is still something the user must be told about"
        );
    }
    #[tokio::test]
    async fn a_deny_rule_is_not_disclosed_as_something_the_method_opens() {
        // "opens" is what the user consents to; a rule that closes a destination belongs under neither that label nor a silent omission of the allow beside it.
        let rig = Rig::new();
        let doc = serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": "some-provider",
            "spec": {
                "serves": ["api.some-provider.example"],
                "methods": [{
                    "name": "token",
                    "auth": { "kind": "token" },
                    "egress": {
                        "http": [
                            { "match": "blocked.example", "verdict": "deny" },
                            { "match": "allowed.example", "verdict": "allow" },
                        ],
                    },
                }],
            },
        })
        .to_string()
        .into_bytes();
        rig.set.put("some-provider", "sha256:abc", &doc).unwrap();
        let listed = list(&rig.store()).unwrap();
        assert_eq!(listed[0].methods[0].opens, ["allowed.example"]);
    }

    fn installed_as(rig: &Rig, name: &str, host: &str, digest: &str) {
        rig.set.put(name, digest, &document(name, host)).unwrap();
    }

    #[test]
    fn an_undecided_connector_holds_every_destination_it_serves() {
        let rig = Rig::new();
        installed_as(
            &rig,
            "some-provider",
            "api.some-provider.example",
            "sha256:abc",
        );
        assert_eq!(
            held_patterns(&rig.store(), "/work").unwrap(),
            ["api.some-provider.example"]
        );
    }

    #[test]
    fn a_granted_connector_holds_nothing_because_its_egress_applies_instead() {
        let rig = Rig::new();
        installed_as(
            &rig,
            "some-provider",
            "api.some-provider.example",
            "sha256:abc",
        );
        rig.store()
            .decide(
                "/work",
                "some-provider",
                ProjectDecision::Granted {
                    digest: "sha256:abc".to_string(),
                    method: "open".to_string(),
                    profile: None,
                    authority: Default::default(),
                },
            )
            .unwrap();
        assert!(held_patterns(&rig.store(), "/work").unwrap().is_empty());
    }

    #[test]
    fn a_declined_connector_holds_nothing() {
        let rig = Rig::new();
        installed_as(
            &rig,
            "some-provider",
            "api.some-provider.example",
            "sha256:abc",
        );
        rig.store()
            .decide("/work", "some-provider", ProjectDecision::Declined)
            .unwrap();
        assert!(held_patterns(&rig.store(), "/work").unwrap().is_empty());
    }

    #[test]
    fn a_grant_made_against_other_bytes_holds_again_so_the_update_is_offered() {
        // §7.1: a grant does not carry to a different digest. Reading the decision alone would silence the offer for bytes nobody consented to.
        let rig = Rig::new();
        installed_as(
            &rig,
            "some-provider",
            "api.some-provider.example",
            "sha256:new",
        );
        rig.store()
            .decide(
                "/work",
                "some-provider",
                ProjectDecision::Granted {
                    digest: "sha256:old".to_string(),
                    method: "open".to_string(),
                    profile: None,
                    authority: Default::default(),
                },
            )
            .unwrap();
        assert_eq!(
            held_patterns(&rig.store(), "/work").unwrap(),
            ["api.some-provider.example"]
        );
    }

    #[test]
    fn one_project_s_decision_does_not_release_the_hold_in_another() {
        let rig = Rig::new();
        installed_as(
            &rig,
            "some-provider",
            "api.some-provider.example",
            "sha256:abc",
        );
        rig.store()
            .decide("/work", "some-provider", ProjectDecision::Declined)
            .unwrap();
        assert_eq!(
            held_patterns(&rig.store(), "/elsewhere").unwrap(),
            ["api.some-provider.example"]
        );
    }

    #[test]
    fn a_document_that_will_not_parse_holds_nothing_and_does_not_fail_the_run() {
        // A run must launch beside a connector this build cannot read; holding nothing is the safe direction, since a destination is asked about only when an offer could follow.
        let rig = Rig::new();
        rig.set
            .put("broken", "sha256:abc", b"{\"kind\":\"connector\"}")
            .unwrap();
        installed_as(
            &rig,
            "some-provider",
            "api.some-provider.example",
            "sha256:abc",
        );
        let messages = crate::test_env::captured_messages(|| {
            assert_eq!(
                held_patterns(&rig.store(), "/work").unwrap(),
                ["api.some-provider.example"],
                "the readable connector still holds what it serves"
            );
        });
        assert!(
            messages.iter().any(|m| m.contains("broken")),
            "the operator must learn which connector was skipped, or an offer that never arrives has no explanation: {messages:?}"
        );
    }

    #[test]
    fn a_decision_file_this_run_cannot_read_holds_rather_than_letting_the_destination_through() {
        // Holding asks, and a grant answers the ask; letting it through cannot be taken back, so an unreadable decision must not read as consent.
        let rig = Rig::new();
        installed_as(
            &rig,
            "some-provider",
            "api.some-provider.example",
            "sha256:abc",
        );
        rig.grants.make_unreadable();

        let messages = crate::test_env::captured_messages(|| {
            assert_eq!(
                held_patterns(&rig.store(), "/work").unwrap(),
                ["api.some-provider.example"]
            );
        });
        assert!(
            messages.iter().any(|m| m.contains("some-provider")),
            "the operator must learn the decision could not be read: {messages:?}"
        );
    }

    #[test]
    fn a_connector_this_project_declined_raises_no_unreadable_warning() {
        // Deciding before parsing keeps the warning true: a declined connector is not one this run failed to read.
        let rig = Rig::new();
        rig.set
            .put("broken", "sha256:abc", b"{\"kind\":\"connector\"}")
            .unwrap();
        rig.store()
            .decide("/work", "broken", ProjectDecision::Declined)
            .unwrap();

        let messages = crate::test_env::captured_messages(|| {
            assert!(held_patterns(&rig.store(), "/work").unwrap().is_empty());
        });
        assert!(
            messages.is_empty(),
            "nothing was skipped for being unreadable: {messages:?}"
        );
    }

    #[test]
    fn nothing_installed_holds_nothing() {
        let rig = Rig::new();
        assert!(held_patterns(&rig.store(), "/work").unwrap().is_empty());
    }
}
