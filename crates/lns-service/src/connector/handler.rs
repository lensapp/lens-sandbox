//! The three machine verbs of `docs/cli-spec.md` §3.3, over a store and a source.

use anyhow::Result;
use lns_ipc::{ConnectorConnectionView, ConnectorMethodView, ConnectorView};

use super::source::{ConnectorSource, Source};
use super::store::{Connection, ConnectorStore, GrantHolder, Installed};
use std::collections::BTreeMap;

/// Resolve `<REF|PATH>` and install what it names. The digest comes from the resolver, never from a caller, because a grant binds to it.
pub async fn install<S: ConnectorSource + ?Sized>(
    store: &ConnectorStore<'_>,
    source: &S,
    operand: &str,
) -> Result<ConnectorView> {
    let fetched = source.fetch(&Source::of(operand)?).await?;
    let definition = store.install(&fetched.digest, &fetched.document, &fetched.filesets)?;
    Ok(view_of(
        &definition,
        &fetched.digest,
        &store.connections_of(&definition.name)?,
    ))
}

pub fn uninstall(store: &ConnectorStore<'_>, name: &str) -> Result<Option<usize>> {
    let held = store.connections_of(name)?.len();
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
    let connections = store.connections_of(&entry.name)?;
    Ok(match lns_artifact::connector::parse(&entry.document) {
        Ok(definition) => view_of(&definition, &entry.digest, &connections),
        Err(_) => ConnectorView {
            name: entry.name.clone(),
            digest: entry.digest.clone(),
            serves: Vec::new(),
            methods: Vec::new(),
            connections: connection_views(&connections),
        },
    })
}

fn view_of(
    definition: &lns_artifact::connector::ConnectorDefinition,
    digest: &str,
    connections: &BTreeMap<String, Connection>,
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
                auth_label: method.auth.as_ref().map(|auth| auth.label().to_string()),
                offerable: can_apply(method),
                opens: opened_by(method),
                writes: method
                    .filesets
                    .iter()
                    .map(|fileset| lns_artifact::connector::guest_directory(&fileset.guest_path))
                    .collect(),
                env: method.env.keys().cloned().collect(),
                credentials: method
                    .credentials
                    .iter()
                    .map(|credential| credential.owner().to_string())
                    .collect(),
                asks: asked_of(method),
                help: method.auth.as_ref().and_then(|auth| auth.help.clone()),
            })
            .collect(),
        connections: connection_views(connections),
    }
}

/// The `auth` outputs a connect must supply, which is what [`super::payload`] later reads the values back under, so the ask and the read cannot drift apart (§4.1).
fn asked_of(method: &lns_artifact::connector::Method) -> Vec<String> {
    method
        .credentials
        .iter()
        .filter_map(|credential| lns_artifact::connector::input_of(method, credential))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
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

fn connection_views(connections: &BTreeMap<String, Connection>) -> Vec<ConnectorConnectionView> {
    connections
        .iter()
        .map(|(label, connection)| ConnectorConnectionView {
            label: label.clone(),
            method: connection.method.clone(),
            authority: connection.authority.0.iter().cloned().collect(),
        })
        .collect()
}

/// What one connect produced: the connection it stored, and the runs whose grant its authority no longer matches (§3.2.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connected {
    pub connection: String,
    pub invalidated: Vec<GrantHolder>,
}

/// What one grant recorded, and the method it displaced — a run holds one grant per connector (§3.2.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Granted {
    pub method: String,
    pub connection: Option<String>,
    pub displaced: Option<String>,
    /// True when the run already held exactly this grant, which cli-spec §3.3 makes an exit-1 answer rather than a change.
    pub unchanged: bool,
}

/// Store what an authentication returned as a connection. A method with no `auth` has nothing to connect and is granted instead (cli-spec §3.3).
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
        super::store::Connection {
            method: method.name.clone(),
            // A `kind: token` exchange reports no authority (§3.2.4).
            authority: super::store::Authority::default(),
            values,
        },
    )?;
    Ok(Connected {
        connection: label.to_string(),
        invalidated,
    })
}

/// Drop one connection, or every connection of a connector. The connector stays installed and grants naming a dropped connection stay (cli-spec §3.3).
pub fn disconnect(
    store: &ConnectorStore<'_>,
    name: &str,
    connection: Option<&str>,
) -> Result<usize> {
    installed_entry(store, name)?;
    Ok(store.drop_connections(name, connection)?)
}

/// Record one run's grant of one method, replacing whatever it decided before.
pub fn grant(
    store: &ConnectorStore<'_>,
    name: &str,
    holder: &GrantHolder,
    method: &str,
    connection: Option<&str>,
) -> Result<Granted> {
    let entry = installed_entry(store, name)?;
    let definition = lns_artifact::connector::parse(&entry.document)?;
    let method = offerable_method(&definition, method)?;
    refuse_a_path_another_connector_writes(store, holder, name, &entry, &method.name)?;
    let connection = behind_the_method(store, name, method, connection)?;
    let authority = match &connection {
        Some(label) => store
            .connections_of(name)?
            .get(label)
            .map(|held| held.authority.clone())
            .unwrap_or_default(),
        None => super::store::Authority::default(),
    };
    let decision = super::store::RunDecision::Granted {
        digest: entry.digest.clone(),
        method: method.name.clone(),
        connection: connection.clone(),
        authority,
    };
    let held = store.decision(holder, name)?;
    if held.as_ref() == Some(&decision) {
        return Ok(Granted {
            method: method.name.clone(),
            connection,
            displaced: None,
            unchanged: true,
        });
    }
    let displaced = store.decide(holder, name, decision)?;
    Ok(Granted {
        method: method.name.clone(),
        connection,
        displaced: displaced.and_then(displaced_method),
        unchanged: false,
    })
}

pub fn forget(store: &ConnectorStore<'_>, name: &str, holder: &GrantHolder) -> Result<bool> {
    Ok(store.forget(holder, name)?)
}

/// Grants a method from the card, refusing bytes other than the ones the card disclosed, and answers with what the guest is to be given (§3.2.4).
pub fn grant_disclosed(
    store: &ConnectorStore<'_>,
    name: &str,
    disclosed_digest: &str,
    holder: &GrantHolder,
    method: &str,
    connection: Option<&str>,
) -> Result<crate::approval_flow::protocol::GrantedPayload> {
    let entry = installed_entry(store, name)?;
    if entry.digest != disclosed_digest {
        anyhow::bail!(
            "{name} was replaced since this card was raised, so it now opens something you were not shown; the next run offers the new version"
        );
    }
    // The connection `grant` settled on, not the one asked for: a caller naming none still gets the only account held, and the payload must be armed with that one.
    let settled = grant(store, name, holder, method, connection)?;
    supplied_by(store, &entry, method, settled.connection.as_deref())
}

/// Two grants claiming one guest path reach the guest as two creates: the second fails, the batch rolls back, and every granted file for the run goes with it. So the collision is refused where a user can still answer it.
fn refuse_a_path_another_connector_writes(
    store: &ConnectorStore<'_>,
    holder: &GrantHolder,
    name: &str,
    entry: &Installed,
    method: &str,
) -> Result<()> {
    let taken = paths_written_by_other_connectors(store, holder, name)?;
    for path in written_paths(&supplied_by(store, entry, method, None)?) {
        if let Some(writer) = taken.get(&path) {
            anyhow::bail!(
                "{writer} already writes {path} in this run: two connectors writing one file would leave the guest with neither, so disconnect {writer} from this run first"
            );
        }
    }
    Ok(())
}

fn paths_written_by_other_connectors(
    store: &ConnectorStore<'_>,
    holder: &GrantHolder,
    granting: &str,
) -> Result<BTreeMap<String, String>> {
    let mut taken = BTreeMap::new();
    for (connector, payload) in granted_supply(store, holder)? {
        if connector == granting {
            continue;
        }
        for path in written_paths(&payload) {
            taken.insert(path, connector.clone());
        }
    }
    Ok(taken)
}

fn written_paths(payload: &crate::approval_flow::protocol::GrantedPayload) -> Vec<String> {
    payload.files.iter().map(|file| file.path.clone()).collect()
}

/// What every recorded grant gives this run, by connector, so a run that granted yesterday is not asked again and is not left empty-handed (§7.1).
pub fn granted_supply(
    store: &ConnectorStore<'_>,
    holder: &GrantHolder,
) -> Result<BTreeMap<String, crate::approval_flow::protocol::GrantedPayload>> {
    let mut supplied = BTreeMap::new();
    for entry in store.installed()? {
        let Some(super::store::RunDecision::Granted {
            digest,
            method,
            connection,
            ..
        }) = store.decision(holder, &entry.name)?
        else {
            continue;
        };
        // A grant bound to other bytes is not one, so an update supplies nothing and offers again.
        if digest != entry.digest {
            continue;
        }
        match supplied_by(store, &entry, &method, connection.as_deref()) {
            Ok(payload) => {
                supplied.insert(entry.name.clone(), payload);
            }
            // A grant is per connector, so one this run cannot supply must not take the others with it.
            Err(e) => crate::log::error!("{} supplies nothing to this run: {e:#}", entry.name),
        }
    }
    drop_a_connector_writing_a_path_another_already_holds(&mut supplied);
    Ok(supplied)
}

/// Decided per connector, not per path: a dropped connector writes nothing, so the paths it claimed are free again and must not cost a third connector its grant.
fn drop_a_connector_writing_a_path_another_already_holds(
    supplied: &mut BTreeMap<String, crate::approval_flow::protocol::GrantedPayload>,
) {
    let mut held: BTreeMap<String, String> = BTreeMap::new();
    let mut dropped = Vec::new();
    for (connector, payload) in supplied.iter() {
        let paths = written_paths(payload);
        match paths.iter().find_map(|path| held.get_key_value(path)) {
            Some((path, holder)) => {
                crate::log::error!(
                    "{connector} does not supply anything to this run: it writes {path}, which {holder} already writes here — disconnect one of them"
                );
                dropped.push(connector.clone());
            }
            None => held.extend(paths.into_iter().map(|path| (path, connector.clone()))),
        }
    }
    for connector in dropped {
        supplied.remove(&connector);
    }
}

/// The egress, credentials, `env` and files one granted method contributes, armed with the values the named connection holds.
fn supplied_by(
    store: &ConnectorStore<'_>,
    entry: &Installed,
    method: &str,
    connection: Option<&str>,
) -> Result<crate::approval_flow::protocol::GrantedPayload> {
    let definition = lns_artifact::connector::parse(&entry.document)?;
    let method = offerable_method(&definition, method)?;
    let values = match connection {
        Some(label) => store
            .connections_of(&entry.name)?
            .remove(label)
            .map(|held| lns_ipc::SecretValues(held.values))
            .unwrap_or_default(),
        None => lns_ipc::SecretValues::default(),
    };
    Ok(super::payload::granted_payload(method, &values))
}

/// Every connector this run has not decided, as the card and `lns connector grant` both disclose it, so a run holds what they serve and can offer them (§3.2.1).
pub fn offerable(store: &ConnectorStore<'_>, holder: &GrantHolder) -> Result<Vec<ConnectorView>> {
    let mut offers = Vec::new();
    let mut unreadable = Vec::new();
    for entry in store.installed()? {
        if decided_here(store, holder, &entry) {
            continue;
        }
        match lns_artifact::connector::parse(&entry.document) {
            Ok(definition) => offers.push(view_of(
                &definition,
                &entry.digest,
                &store.connections_of(&entry.name)?,
            )),
            Err(_) => unreadable.push(entry.name),
        }
    }
    if !unreadable.is_empty() {
        let names = unreadable.join(", ");
        crate::log::warn!(
            "cannot read the installed connector(s) {names}, so this run is not offered what they serve"
        );
    }
    Ok(offers)
}

/// A decision this run cannot read is not one: holding asks, which a grant then answers, where letting the destination through cannot be taken back (§3.2.1).
fn decided_here(store: &ConnectorStore<'_>, holder: &GrantHolder, entry: &Installed) -> bool {
    match store.decision(holder, &entry.name) {
        Ok(decision) => decision.is_some_and(|decision| decision.decides(&entry.digest)),
        Err(e) => {
            crate::log::warn!(
                "cannot read what this run decided about {}, so its destinations are held: {e}",
                entry.name
            );
            false
        }
    }
}

/// Which connection stands behind a method: none for one that does not authenticate, the named one, or the only one held.
fn behind_the_method(
    store: &ConnectorStore<'_>,
    name: &str,
    method: &lns_artifact::connector::Method,
    named: Option<&str>,
) -> Result<Option<String>> {
    if method.auth.is_none() {
        if let Some(named) = named {
            anyhow::bail!(
                "method {} of {name} does not authenticate, so it takes no connection; drop --connection {named}",
                method.name
            );
        }
        return Ok(None);
    }
    // A connection records the method that produced it, so one made for another method holds values for another method's credentials.
    let held: BTreeMap<String, Connection> = store
        .connections_of(name)?
        .into_iter()
        .filter(|(_, connection)| connection.method == method.name)
        .collect();
    match named {
        Some(named) if held.contains_key(named) => Ok(Some(named.to_string())),
        Some(named) => anyhow::bail!(
            "{name} holds no connection named {named} for method {}",
            method.name
        ),
        None if held.len() == 1 => Ok(held.into_keys().next()),
        None if held.is_empty() => anyhow::bail!(
            "{name} authenticates and this machine holds no connection for method {}; run `lns connector connect {name} --method {}` first",
            method.name,
            method.name
        ),
        None => anyhow::bail!(
            "{name} holds {} connections for method {}, so name one with --connection",
            held.len(),
            method.name
        ),
    }
}

fn displaced_method(decision: super::store::RunDecision) -> Option<String> {
    match decision {
        super::store::RunDecision::Granted { method, .. } => Some(method),
        super::store::RunDecision::Declined => None,
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

/// What this version can deliver, which is narrower than what a document may declare: install keeps a packed fileset's bytes, but nothing reads them back into the files a grant sends, so a method writing one cannot be applied yet (§3.2.2).
fn can_apply(method: &lns_artifact::connector::Method) -> bool {
    method.is_offerable()
        && method
            .filesets
            .iter()
            .all(|fileset| fileset.inline.is_some())
}

/// The named method, refused when this version cannot deliver it — the card could not either (§3.2.2).
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
    if !can_apply(found) {
        anyhow::bail!("method {method} of {} needs a newer lns", definition.name);
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connector::source::FetchedConnector;
    use crate::connector::store::{Authority, GrantHolder, InstalledSet, RunDecision};
    use lns_policy::decision_store::{DecisionFile, DecisionStore};
    use std::sync::Mutex;

    fn a_run() -> GrantHolder {
        GrantHolder::Run("1a2b3c4d0000000000000000000000aa".to_string())
    }

    fn another_run() -> GrantHolder {
        GrantHolder::Run("9f8e7d6c0000000000000000000000bb".to_string())
    }

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
        layers: Mutex<BTreeMap<String, Vec<Vec<u8>>>>,
    }

    impl InstalledSet for FakeSet {
        fn list(&self) -> std::io::Result<Vec<Installed>> {
            Ok(self.entries.lock().unwrap().clone())
        }
        fn put(
            &self,
            name: &str,
            digest: &str,
            document: &[u8],
            filesets: &[Vec<u8>],
        ) -> std::io::Result<()> {
            let mut held = self.entries.lock().unwrap();
            held.retain(|e| e.name != name);
            held.push(Installed {
                name: name.to_string(),
                digest: digest.to_string(),
                document: document.to_vec(),
            });
            self.layers
                .lock()
                .unwrap()
                .insert(name.to_string(), filesets.to_vec());
            Ok(())
        }
        fn fileset_layer(&self, name: &str, index: usize) -> std::io::Result<Vec<u8>> {
            self.layers
                .lock()
                .unwrap()
                .get(name)
                .and_then(|layers| layers.get(index))
                .cloned()
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotFound, "no such fileset layer")
                })
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
        filesets: Vec<Vec<u8>>,
        asked: Mutex<Vec<Source>>,
    }

    impl ConnectorSource for FakeSource {
        async fn fetch(&self, source: &Source) -> Result<FetchedConnector> {
            self.asked.lock().unwrap().push(source.clone());
            Ok(FetchedConnector {
                digest: self.digest.clone(),
                document: self.document.clone(),
                filesets: self.filesets.clone(),
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
        values: FakeMap<Connection>,
        grants: FakeMap<super::super::store::RunDecision>,
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
            filesets: Vec::new(),
            asked: Mutex::new(Vec::new()),
        }
    }

    #[tokio::test]
    async fn installing_keeps_the_packed_fileset_the_source_brought() {
        // The digest binds a grant to these bytes, so the bytes have to be here when a later run grants the method that writes them.
        let rig = Rig::new();
        let layer = crate::connector::layer::fixtures::raw_layer("config.json", b"{}");
        let doc = serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": "some-provider",
            "spec": {
                "serves": ["api.some-provider.example"],
                "methods": [{
                    "name": "open",
                    "filesets": [{ "path": "./seed", "guestPath": "~/.some-provider" }],
                }],
            },
        })
        .to_string()
        .into_bytes();
        let src = FakeSource {
            digest: "sha256:abc".to_string(),
            document: doc,
            filesets: vec![layer.clone()],
            asked: Mutex::new(Vec::new()),
        };

        install(&rig.store(), &src, "ghcr.io/acme/some-provider:1")
            .await
            .expect("install accepts a connector that packs one directory");

        assert_eq!(
            rig.store().fileset_layer("some-provider", 0).unwrap(),
            layer,
            "install passes the source's layers to the store, or a grant has nothing to write"
        );
        assert!(
            rig.store().fileset_layer("some-provider", 1).is_err(),
            "an index nothing packed is not a layer"
        );
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
                .map(|m| (m.name.as_str(), m.label.as_str(), m.auth_label.is_some()))
                .collect::<Vec<_>>(),
            [("token", "API token", true), ("open", "open", false)],
            "a method with no auth has nothing to connect, and one with no label falls back to its name"
        );
        assert!(view.connections.is_empty(), "installing connects nothing");
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
                Source::Local("/work/some-provider/lns.yaml".into()),
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
    async fn uninstalling_reports_how_many_connections_it_dropped() {
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
                    Connection {
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
    async fn the_list_names_each_connector_with_the_connections_this_machine_holds() {
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
                Connection {
                    method: "token".to_string(),
                    authority: Authority::of(["repo:read"]),
                    values: Default::default(),
                },
            )
            .unwrap();

        let listed = list(&rig.store()).unwrap();

        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].connections,
            [ConnectorConnectionView {
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
            .put("mystery", "sha256:xyz", b"not a document", &[])
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
    async fn connecting_stores_a_connection_the_machine_then_holds() {
        let rig = Rig::new();
        installed(&rig).await;
        let connected = connect(&rig.store(), "some-provider", "token", "work", values())
            .expect("token is an offerable method that authenticates");
        assert_eq!(connected.connection, "work");
        assert!(connected.invalidated.is_empty());
        assert_eq!(
            rig.store().connections_of("some-provider").unwrap()["work"].method,
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
    async fn disconnecting_drops_the_connections_and_leaves_the_connector_installed() {
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
    async fn granting_a_method_that_authenticates_takes_the_only_connection_held() {
        let rig = Rig::new();
        installed(&rig).await;
        connect(&rig.store(), "some-provider", "token", "work", values()).unwrap();
        let granted = grant(&rig.store(), "some-provider", &a_run(), "token", None).expect("grant");
        assert_eq!(granted.method, "token");
        assert_eq!(granted.connection.as_deref(), Some("work"));
        assert_eq!(granted.displaced, None);
    }

    #[tokio::test]
    async fn granting_binds_to_the_digest_installed_now() {
        // §7.1: a grant bound to other bytes is not one, so the grant has to record the digest it consented to.
        let rig = Rig::new();
        installed(&rig).await;
        grant(&rig.store(), "some-provider", &a_run(), "open", None).expect("grant");
        assert!(
            rig.store()
                .grant_for(&a_run(), "some-provider", "sha256:abc")
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn granting_a_method_that_authenticates_with_no_connection_held_says_to_connect_first() {
        let rig = Rig::new();
        installed(&rig).await;
        let err = grant(&rig.store(), "some-provider", &a_run(), "token", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("connector connect"), "{err}");
    }

    #[tokio::test]
    async fn granting_with_several_connections_held_and_none_named_asks_for_one() {
        let rig = Rig::new();
        installed(&rig).await;
        for label in ["work", "personal"] {
            connect(&rig.store(), "some-provider", "token", label, values()).unwrap();
        }
        let err = grant(&rig.store(), "some-provider", &a_run(), "token", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--connection"), "{err}");
    }

    #[tokio::test]
    async fn granting_a_connection_the_machine_does_not_hold_is_refused() {
        let rig = Rig::new();
        installed(&rig).await;
        connect(&rig.store(), "some-provider", "token", "work", values()).unwrap();
        let err = grant(
            &rig.store(),
            "some-provider",
            &a_run(),
            "token",
            Some("other"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("no connection named other"), "{err}");
    }

    #[tokio::test]
    async fn granting_a_method_that_does_not_authenticate_takes_no_connection() {
        let rig = Rig::new();
        installed(&rig).await;
        let granted = grant(&rig.store(), "some-provider", &a_run(), "open", None).expect("grant");
        assert_eq!(granted.connection, None);
        let err = grant(
            &rig.store(),
            "some-provider",
            &a_run(),
            "open",
            Some("work"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("takes no connection"), "{err}");
    }

    #[tokio::test]
    async fn granting_again_names_the_method_it_displaces() {
        // §3.2.4: a run holds one grant per connector, so what it prints names what it replaced.
        let rig = Rig::new();
        installed(&rig).await;
        connect(&rig.store(), "some-provider", "token", "work", values()).unwrap();
        grant(&rig.store(), "some-provider", &a_run(), "token", None).unwrap();
        let granted = grant(&rig.store(), "some-provider", &a_run(), "open", None).expect("grant");
        assert_eq!(granted.displaced.as_deref(), Some("token"));
    }

    #[tokio::test]
    async fn granting_over_a_decline_displaces_no_method() {
        let rig = Rig::new();
        installed(&rig).await;
        rig.store()
            .decide(
                &a_run(),
                "some-provider",
                super::super::store::RunDecision::Declined,
            )
            .unwrap();
        let granted = grant(&rig.store(), "some-provider", &a_run(), "open", None).expect("grant");
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
        rig.set
            .put("some-provider", "sha256:abc", &doc, &[])
            .unwrap();
        let err = grant(&rig.store(), "some-provider", &a_run(), "future", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("needs a newer lns"), "{err}");
    }

    #[tokio::test]
    async fn forgetting_clears_what_the_run_decided() {
        let rig = Rig::new();
        installed(&rig).await;
        grant(&rig.store(), "some-provider", &a_run(), "open", None).unwrap();
        assert!(forget(&rig.store(), "some-provider", &a_run()).unwrap());
        assert!(!forget(&rig.store(), "some-provider", &a_run()).unwrap());
        assert_eq!(
            rig.store().decision(&a_run(), "some-provider").unwrap(),
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
    async fn a_grant_will_not_take_a_connection_another_method_produced() {
        // A connection records the method that made it, so one made elsewhere holds values for another method's credentials.
        let rig = Rig::new();
        rig.set
            .put(
                "some-provider",
                "sha256:abc",
                &two_token_methods("some-provider"),
                &[],
            )
            .unwrap();
        connect(&rig.store(), "some-provider", "token", "personal", values()).unwrap();

        let err = grant(&rig.store(), "some-provider", &a_run(), "token-org", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("connector connect"), "{err}");
        assert_eq!(
            rig.store().decision(&a_run(), "some-provider").unwrap(),
            None,
            "nothing may be recorded when the connection does not belong to the method"
        );
    }

    #[tokio::test]
    async fn naming_a_connection_of_another_method_is_refused() {
        let rig = Rig::new();
        rig.set
            .put(
                "some-provider",
                "sha256:abc",
                &two_token_methods("some-provider"),
                &[],
            )
            .unwrap();
        connect(&rig.store(), "some-provider", "token", "personal", values()).unwrap();
        let err = grant(
            &rig.store(),
            "some-provider",
            &a_run(),
            "token-org",
            Some("personal"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("no connection named personal"), "{err}");
    }

    #[tokio::test]
    async fn a_grant_the_run_already_holds_is_unchanged_rather_than_a_replacement() {
        // cli-spec §3.3: exits 1 when the run already granted that method and connection — and "replaced" would be false.
        let rig = Rig::new();
        installed(&rig).await;
        connect(&rig.store(), "some-provider", "token", "work", values()).unwrap();
        let first = grant(&rig.store(), "some-provider", &a_run(), "token", None).unwrap();
        assert!(!first.unchanged);

        let again = grant(&rig.store(), "some-provider", &a_run(), "token", None).unwrap();
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
                        "guestPath": "~/.some-provider",
                    }],
                }],
            },
        })
        .to_string()
        .into_bytes();
        rig.set
            .put("some-provider", "sha256:abc", &doc, &[])
            .unwrap();

        let listed = list(&rig.store()).unwrap();
        let method = &listed[0].methods[0];
        assert_eq!(
            method.opens,
            ["other.example", "db.example:5432"],
            "a method's egress is not bounded by `serves`, so both tables reach the card"
        );
        assert_eq!(method.writes, ["~/.some-provider"]);
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
        rig.set
            .put("some-provider", "sha256:abc", &doc, &[])
            .unwrap();
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
        rig.set
            .put("some-provider", "sha256:abc", &doc, &[])
            .unwrap();
        let listed = list(&rig.store()).unwrap();
        assert_eq!(listed[0].methods[0].opens, ["allowed.example"]);
    }

    fn served_by(offers: Vec<ConnectorView>) -> Vec<String> {
        offers.into_iter().flat_map(|offer| offer.serves).collect()
    }

    fn installed_as(rig: &Rig, name: &str, host: &str, digest: &str) {
        rig.set
            .put(name, digest, &document(name, host), &[])
            .unwrap();
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
            served_by(offerable(&rig.store(), &a_run()).unwrap()),
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
                &a_run(),
                "some-provider",
                RunDecision::Granted {
                    digest: "sha256:abc".to_string(),
                    method: "open".to_string(),
                    connection: None,
                    authority: Default::default(),
                },
            )
            .unwrap();
        assert!(served_by(offerable(&rig.store(), &a_run()).unwrap()).is_empty());
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
            .decide(&a_run(), "some-provider", RunDecision::Declined)
            .unwrap();
        assert!(served_by(offerable(&rig.store(), &a_run()).unwrap()).is_empty());
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
                &a_run(),
                "some-provider",
                RunDecision::Granted {
                    digest: "sha256:old".to_string(),
                    method: "open".to_string(),
                    connection: None,
                    authority: Default::default(),
                },
            )
            .unwrap();
        assert_eq!(
            served_by(offerable(&rig.store(), &a_run()).unwrap()),
            ["api.some-provider.example"]
        );
    }

    #[test]
    fn one_runs_decision_does_not_release_the_hold_in_another() {
        let rig = Rig::new();
        installed_as(
            &rig,
            "some-provider",
            "api.some-provider.example",
            "sha256:abc",
        );
        rig.store()
            .decide(&a_run(), "some-provider", RunDecision::Declined)
            .unwrap();
        assert_eq!(
            served_by(offerable(&rig.store(), &another_run()).unwrap()),
            ["api.some-provider.example"]
        );
    }

    #[test]
    fn a_document_that_will_not_parse_holds_nothing_and_does_not_fail_the_run() {
        // A run must launch beside a connector this build cannot read; holding nothing is the safe direction, since a destination is asked about only when an offer could follow.
        let rig = Rig::new();
        rig.set
            .put("broken", "sha256:abc", b"{\"kind\":\"connector\"}", &[])
            .unwrap();
        installed_as(
            &rig,
            "some-provider",
            "api.some-provider.example",
            "sha256:abc",
        );
        let messages = crate::test_env::captured_messages(|| {
            assert_eq!(
                served_by(offerable(&rig.store(), &a_run()).unwrap()),
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
                served_by(offerable(&rig.store(), &a_run()).unwrap()),
                ["api.some-provider.example"]
            );
        });
        assert!(
            messages.iter().any(|m| m.contains("some-provider")),
            "the operator must learn the decision could not be read: {messages:?}"
        );
    }

    #[test]
    fn a_connector_this_run_declined_raises_no_unreadable_warning() {
        // Deciding before parsing keeps the warning true: a declined connector is not one this run failed to read.
        let rig = Rig::new();
        rig.set
            .put("broken", "sha256:abc", b"{\"kind\":\"connector\"}", &[])
            .unwrap();
        rig.store()
            .decide(&a_run(), "broken", RunDecision::Declined)
            .unwrap();

        let messages = crate::test_env::captured_messages(|| {
            assert!(served_by(offerable(&rig.store(), &a_run()).unwrap()).is_empty());
        });
        assert!(
            messages.is_empty(),
            "nothing was skipped for being unreadable: {messages:?}"
        );
    }

    #[test]
    fn granting_from_a_card_answers_with_what_the_guest_is_given() {
        let rig = Rig::new();
        let doc = serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": "some-provider",
            "spec": {
                "serves": ["api.some-provider.example"],
                "methods": [{
                    "name": "open",
                    "egress": { "http": [{ "match": "api.some-provider.example", "verdict": "allow" }] },
                    "env": { "SOME_REGION": "eu" },
                }],
            },
        })
        .to_string()
        .into_bytes();
        rig.set
            .put("some-provider", "sha256:abc", &doc, &[])
            .unwrap();

        let payload = grant_disclosed(
            &rig.store(),
            "some-provider",
            "sha256:abc",
            &a_run(),
            "open",
            None,
        )
        .expect("grant");

        assert_eq!(
            payload.egress.network.egress.http[0].match_pattern,
            "api.some-provider.example"
        );
        assert_eq!(payload.env.get("SOME_REGION"), Some(&"eu".to_string()));
        let recorded = rig.store().decision(&a_run(), "some-provider").unwrap();
        assert!(
            matches!(recorded, Some(RunDecision::Granted { .. })),
            "the run's answer is recorded, not just returned: {recorded:?}"
        );
    }

    fn writing(name: &str, guest_path: &str) -> Vec<u8> {
        writing_all(name, &[guest_path])
    }

    fn writing_all(name: &str, guest_paths: &[&str]) -> Vec<u8> {
        let filesets = guest_paths
            .iter()
            .map(|guest_path| {
                serde_json::json!({ "guestPath": guest_path, "inline": { "config.json": "{}" } })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": name,
            "spec": {
                "serves": [format!("api.{name}.example")],
                "methods": [{ "name": "open", "filesets": filesets }],
            },
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn a_method_writing_an_inline_fileset_is_offered_and_supplies_it() {
        let rig = Rig::new();
        rig.set
            .put(
                "some-provider",
                "sha256:abc",
                &writing("some-provider", "~/.some-provider"),
                &[],
            )
            .unwrap();

        let offered = offerable(&rig.store(), &a_run()).expect("offerable");
        assert!(
            offered[0].methods[0].offerable,
            "an inline fileset's content is inside the document install already keeps, so nothing is missing"
        );

        let payload = grant_disclosed(
            &rig.store(),
            "some-provider",
            "sha256:abc",
            &a_run(),
            "open",
            None,
        )
        .expect("grant");
        assert_eq!(
            payload
                .files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["~/.some-provider/config.json"]
        );
    }

    #[test]
    fn a_grant_is_refused_when_another_connector_already_writes_that_file() {
        // Two entries claiming one path reach the guest as two creates. The second fails, the batch rolls back, and every granted file for the run goes with it.
        let rig = Rig::new();
        rig.set
            .put("alpha", "sha256:a", &writing("alpha", "~/.shared"), &[])
            .unwrap();
        rig.set
            .put("beta", "sha256:b", &writing("beta", "~/.shared"), &[])
            .unwrap();
        grant_disclosed(&rig.store(), "alpha", "sha256:a", &a_run(), "open", None).expect("first");

        let err = grant_disclosed(&rig.store(), "beta", "sha256:b", &a_run(), "open", None)
            .expect_err("the second grant claims a path the first already writes");

        assert!(
            format!("{err:#}").contains("alpha") && format!("{err:#}").contains("~/.shared"),
            "the refusal must name the connector that holds the path and the path itself; got: {err:#}"
        );
        assert!(
            rig.store().decision(&a_run(), "beta").unwrap().is_none(),
            "a refused grant must not be recorded, or the next run restores the collision"
        );
    }

    #[test]
    fn the_cli_verb_refuses_a_path_another_connector_writes_too() {
        // `lns connector grant` reaches `grant` without passing the card, so a check only the card ran would let the CLI record what the card refuses.
        let rig = Rig::new();
        rig.set
            .put("alpha", "sha256:a", &writing("alpha", "~/.shared"), &[])
            .unwrap();
        rig.set
            .put("beta", "sha256:b", &writing("beta", "~/.shared"), &[])
            .unwrap();
        grant(&rig.store(), "alpha", &a_run(), "open", None).expect("first");

        let err = grant(&rig.store(), "beta", &a_run(), "open", None)
            .expect_err("the second grant claims a path the first already writes");

        assert!(
            format!("{err:#}").contains("alpha") && format!("{err:#}").contains("~/.shared"),
            "got: {err:#}"
        );
        assert!(
            rig.store().decision(&a_run(), "beta").unwrap().is_none(),
            "a refused grant must not be recorded, or the next run restores the collision"
        );
    }

    #[test]
    fn one_connector_this_run_cannot_supply_costs_only_its_own_grant() {
        // A grant is per connector, so one unreadable connector must not take the rest of the run's grants with it — the run would start with no egress it was promised and no card to ask again.
        let rig = Rig::new();
        rig.set
            .put("alpha", "sha256:a", b"not a document", &[])
            .unwrap();
        rig.set
            .put("beta", "sha256:b", &writing("beta", "~/.beta"), &[])
            .unwrap();
        for (name, digest) in [("alpha", "sha256:a"), ("beta", "sha256:b")] {
            rig.store()
                .decide(
                    &a_run(),
                    name,
                    RunDecision::Granted {
                        digest: digest.to_string(),
                        method: "open".to_string(),
                        connection: None,
                        authority: Default::default(),
                    },
                )
                .unwrap();
        }

        let mut supplied = BTreeMap::new();
        let messages = crate::test_env::captured_messages(|| {
            supplied =
                granted_supply(&rig.store(), &a_run()).expect("the store itself is readable");
        });

        assert_eq!(
            supplied.keys().collect::<Vec<_>>(),
            ["beta"],
            "beta granted its own method and must still get it"
        );
        assert!(
            messages.iter().any(|m| m.contains("alpha")),
            "a grant this run drops must say which connector it was; got: {messages:?}"
        );
    }

    #[test]
    fn a_path_spelled_around_a_dot_segment_is_the_same_path() {
        // `.` is a legal guestPath segment, so a second connector could name a file the first already writes and be refused by neither guard.
        let rig = Rig::new();
        rig.set
            .put("alpha", "sha256:a", &writing("alpha", "~/shared/x"), &[])
            .unwrap();
        rig.set
            .put("beta", "sha256:b", &writing("beta", "~/shared/./x"), &[])
            .unwrap();
        grant(&rig.store(), "alpha", &a_run(), "open", None).expect("first");

        let err = grant(&rig.store(), "beta", &a_run(), "open", None)
            .expect_err("the same file, spelled around a dot");

        assert!(
            format!("{err:#}").contains("alpha") && format!("{err:#}").contains("~/shared/x"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_connector_dropped_for_a_collision_stops_claiming_the_paths_it_will_not_write() {
        // Dropping beta frees every path beta held, so gamma collides with nothing. Charging gamma for a conflict that no longer exists would cost a third connector its files and name a holder that supplies none.
        let rig = Rig::new();
        for (name, digest, paths) in [
            ("alpha", "sha256:a", vec!["~/.shared"]),
            ("beta", "sha256:b", vec!["~/.shared", "~/.own"]),
            ("gamma", "sha256:g", vec!["~/.own"]),
        ] {
            rig.set
                .put(name, digest, &writing_all(name, &paths), &[])
                .unwrap();
            rig.store()
                .decide(
                    &a_run(),
                    name,
                    RunDecision::Granted {
                        digest: digest.to_string(),
                        method: "open".to_string(),
                        connection: None,
                        authority: Default::default(),
                    },
                )
                .unwrap();
        }

        let mut supplied = BTreeMap::new();
        let messages = crate::test_env::captured_messages(|| {
            supplied = granted_supply(&rig.store(), &a_run()).unwrap();
        });

        assert_eq!(
            supplied.keys().collect::<Vec<_>>(),
            ["alpha", "gamma"],
            "only the connector that actually collides loses its grant"
        );
        assert!(
            messages.iter().all(|m| !m.contains("gamma")),
            "gamma writes nothing another connector writes, so accusing it would send the user to disconnect the wrong pair; got: {messages:?}"
        );
    }

    #[test]
    fn a_collision_between_two_recorded_grants_costs_the_later_connector_and_says_so() {
        // An older lns could record what grant now refuses. Restoring both would take the whole run's files away, so one connector loses its grant rather than every connector losing its files.
        let rig = Rig::new();
        for (name, digest) in [("alpha", "sha256:a"), ("beta", "sha256:b")] {
            rig.set
                .put(name, digest, &writing(name, "~/.shared"), &[])
                .unwrap();
            rig.store()
                .decide(
                    &a_run(),
                    name,
                    RunDecision::Granted {
                        digest: digest.to_string(),
                        method: "open".to_string(),
                        connection: None,
                        authority: Default::default(),
                    },
                )
                .unwrap();
        }

        let mut supplied = BTreeMap::new();
        let messages = crate::test_env::captured_messages(|| {
            supplied = granted_supply(&rig.store(), &a_run()).unwrap();
        });

        assert_eq!(
            supplied.keys().collect::<Vec<_>>(),
            ["alpha"],
            "one connector keeps the path, and it is the same one on every run"
        );
        assert!(
            messages
                .iter()
                .any(|m| m.contains("beta") && m.contains("alpha") && m.contains("~/.shared")),
            "a connector that stops supplying must say which path it lost and to whom; got: {messages:?}"
        );
    }

    #[test]
    fn a_method_packing_a_directory_is_not_offered_because_its_bytes_are_not_kept() {
        // Install stores the document alone, so a packed layer's content is not on this machine. Offering it would disclose a write that never happens.
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
                    "filesets": [{ "guestPath": "~/.some-provider", "path": "./seed" }],
                }],
            },
        })
        .to_string()
        .into_bytes();
        rig.set
            .put("some-provider", "sha256:abc", &doc, &[])
            .unwrap();

        let offered = offerable(&rig.store(), &a_run()).expect("offerable");
        assert!(
            !offered[0].methods[0].offerable,
            "the card must not promise a file it cannot write"
        );

        let err = grant_disclosed(
            &rig.store(),
            "some-provider",
            "sha256:abc",
            &a_run(),
            "token",
            None,
        )
        .expect_err("and granting it is refused for the same reason");
        assert!(format!("{err:#}").contains("newer lns"), "{err:#}");
    }

    #[test]
    fn granting_bytes_other_than_the_ones_disclosed_is_refused() {
        // §3.2.4: a grant must not be silently widened. A reinstall between the card and the click changes what the method opens.
        let rig = Rig::new();
        installed_as(
            &rig,
            "some-provider",
            "api.some-provider.example",
            "sha256:new",
        );

        let err = grant_disclosed(
            &rig.store(),
            "some-provider",
            "sha256:the-one-the-card-showed",
            &a_run(),
            "open",
            None,
        )
        .expect_err("the bytes changed under the card");

        assert!(format!("{err:#}").contains("replaced"), "{err:#}");
        assert!(
            rig.store()
                .decision(&a_run(), "some-provider")
                .unwrap()
                .is_none(),
            "nothing was recorded for bytes the user never saw"
        );
    }

    #[test]
    fn granting_a_connected_method_arms_it_with_the_value_that_connection_holds() {
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
                        "envVar": "SOME_TOKEN",
                        "placeholder": "some-provider-LNSPLACEHOLDER00",
                        "injections": [{ "kind": "bearer_header", "domain": "api.some-provider.example" }],
                    }],
                }],
            },
        })
        .to_string()
        .into_bytes();
        rig.set
            .put("some-provider", "sha256:abc", &doc, &[])
            .unwrap();
        // Keyed by what the method's own view asks for, because a test that guessed the key would pass while the CLI and the card asked under another one.
        let asked = list(&rig.store()).expect("list")[0].methods[0].asks.clone();
        connect(
            &rig.store(),
            "some-provider",
            "token",
            "work",
            asked
                .into_iter()
                .map(|ask| (ask, "sk-live".to_string()))
                .collect(),
        )
        .expect("connect");

        let payload = grant_disclosed(
            &rig.store(),
            "some-provider",
            "sha256:abc",
            &a_run(),
            "token",
            Some("work"),
        )
        .expect("grant");

        assert_eq!(
            payload.credentials[0].injections[0].value(),
            "Bearer sk-live",
            "the value the user connected with is what the boundary substitutes"
        );
    }

    #[test]
    fn nothing_installed_holds_nothing() {
        let rig = Rig::new();
        assert!(served_by(offerable(&rig.store(), &a_run()).unwrap()).is_empty());
    }
}
