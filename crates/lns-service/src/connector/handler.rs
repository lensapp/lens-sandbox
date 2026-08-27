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
            })
            .collect(),
        profiles: profile_views(profiles),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connector::source::FetchedConnector;
    use crate::connector::store::{Authority, InstalledSet};
    use lns_policy::decision_store::{DecisionFile, DecisionStore};
    use std::sync::Mutex;

    struct FakeMap<T> {
        state: Mutex<DecisionFile<T>>,
    }

    impl<T> Default for FakeMap<T> {
        fn default() -> Self {
            Self {
                state: Mutex::new(DecisionFile::new()),
            }
        }
    }

    impl<T: Clone + Send + Sync> DecisionStore<T> for FakeMap<T> {
        fn load(&self) -> std::io::Result<DecisionFile<T>> {
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
}
