//! The connector document — `docs/sandbox-spec.md` §3.2.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use lns_policy::Egress;
use serde::{Deserialize, Serialize};

use crate::sandbox::FilesetEntry;
use crate::spec;

/// The `auth.kind` values this version can offer; any other parses and leaves its method unofferable (§3.2.2).
const KNOWN_AUTH_KINDS: [&str; 1] = ["token"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorDefinition {
    pub name: String,
    pub spec: ConnectorSpec,
}

/// Detection in `serves`, and the alternative ways to connect in `methods`.
///
/// `deny_unknown_fields` cannot ride with `flatten`, so `misplaced` collects what
/// this shape does not name and [`refuse_a_payload_block_outside_a_method`] holds
/// the line — which is what lets the message say where a block belongs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorSpec {
    #[serde(default)]
    pub serves: Vec<String>,
    #[serde(default)]
    pub methods: Vec<Method>,
    /// Every block a method owns, refused here so the message can say where it belongs (§3.2.2).
    #[serde(flatten)]
    misplaced: BTreeMap<String, serde_json::Value>,
}

/// One way to connect: an optional `auth`, plus the payload a grant applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Method {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<Auth>,
    #[serde(default)]
    pub egress: Egress,
    #[serde(default)]
    pub credentials: Vec<lns_spec::Credential>,
    #[serde(default)]
    pub filesets: Vec<FilesetEntry>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// A block a running guest cannot be given, kept only so the refusal can name the mechanism that decides it.
    #[serde(flatten)]
    refused: BTreeMap<String, serde_json::Value>,
}

impl Method {
    /// The card can offer this method only if this version implements its mechanism; an unknown one is not an error (§3.2.2).
    pub fn is_offerable(&self) -> bool {
        self.auth
            .as_ref()
            .is_none_or(|auth| KNOWN_AUTH_KINDS.contains(&auth.kind.as_str()))
    }

    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
}

/// How the user proves they may use a method. `kind` decides what else it accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Auth {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Tolerated only for a `kind` this version does not know; strict decoding holds for one it does (§1.2).
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

/// The blocks a method may not carry, each naming the mechanism that decides it rather than the rule.
const REFUSED_BLOCKS: [(&str, &str); 10] = [
    (
        "tools",
        "a connector is applied to a guest that is already running, and tools are installed before the guest boots",
    ),
    (
        "scripts",
        "a connector is applied to a guest that is already running, and a script runs once before the workload — connecting a connector must not mean running code",
    ),
    (
        "volumes",
        "which mounts exist is fixed when the guest is created, and a connector is applied to a guest that is already running",
    ),
    (
        "ports",
        "which listeners exist is fixed when the guest is created, and a connector is applied to a guest that is already running",
    ),
    (
        "mixins",
        "the card shows one document, and a graph would apply egress and credentials from documents the user never saw",
    ),
    ("image", "it describes one launch, and the sandbox owns it"),
    (
        "command",
        "it describes one launch, and the sandbox owns it",
    ),
    (
        "workdir",
        "it describes one launch, and the sandbox owns it",
    ),
    ("user", "it describes one launch, and the sandbox owns it"),
    (
        "resources",
        "it describes one launch, and the sandbox owns it",
    ),
];

/// Every block a method owns, so `spec` carrying one can point at where it belongs.
const METHOD_BLOCKS: [&str; 4] = ["egress", "credentials", "filesets", "env"];

/// Parse and cross-field-validate a `lns.run/v1` connector against the document alone; a caller holding the bytes of a `path` fileset passes them to [`parse_with_path_files`] instead.
pub fn parse(config_json: &[u8]) -> Result<ConnectorDefinition> {
    parse_with_path_files(config_json, &BTreeMap::new())
}

/// [`parse`], plus the §3.2.5 read of every `path` fileset the caller could read beside the document, keyed by the `path` the entry writes.
pub fn parse_with_path_files(
    config_json: &[u8],
    path_files: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<ConnectorDefinition> {
    let doc = spec::parse_envelope(config_json, spec::Kind::Connector)?;
    let connector: ConnectorSpec =
        serde_json::from_value(doc.spec).context("parsing connector spec")?;
    validate(&connector, path_files)?;
    Ok(ConnectorDefinition {
        name: doc.name,
        spec: connector,
    })
}

fn validate(
    connector: &ConnectorSpec,
    path_files: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<()> {
    refuse_a_payload_block_outside_a_method(connector)?;
    validate_serves(&connector.serves)?;
    if connector.methods.is_empty() {
        bail!("a connector must declare at least one method: methods are how it is connected");
    }
    let mut named = BTreeSet::new();
    for method in &connector.methods {
        validate_method(method, path_files)
            .with_context(|| format!("connector method {}", method.name))?;
        if !named.insert(&method.name) {
            bail!("duplicate connector method {:?}", method.name);
        }
    }
    Ok(())
}

fn refuse_a_payload_block_outside_a_method(connector: &ConnectorSpec) -> Result<()> {
    if let Some(block) = METHOD_BLOCKS
        .iter()
        .find(|block| connector.misplaced.contains_key(**block))
    {
        bail!(
            "a connector must declare {block} inside a method, not beside methods: a grant applies one method, so a block outside them belongs to nothing"
        );
    }
    if let Some((block, why)) = REFUSED_BLOCKS
        .iter()
        .find(|(block, _)| connector.misplaced.contains_key(*block))
    {
        bail!("a connector must not declare {block}: {why}");
    }
    if let Some(unknown) = connector.misplaced.keys().next() {
        bail!("unknown field {unknown:?} in connector spec");
    }
    Ok(())
}

fn validate_serves(serves: &[String]) -> Result<()> {
    if serves.is_empty() {
        bail!(
            "a connector must declare at least one serves entry: serves is what decides when it is offered"
        );
    }
    for pattern in serves {
        if pattern.trim().is_empty() {
            bail!("a serves entry must name a destination");
        }
        if pattern.chars().any(char::is_whitespace) {
            bail!("serves entry {pattern:?} must not contain whitespace");
        }
    }
    Ok(())
}

fn validate_method(
    method: &Method,
    path_files: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<()> {
    if !spec::is_valid_name(&method.name) {
        bail!("invalid method name {:?}", method.name);
    }
    if let Some((block, why)) = REFUSED_BLOCKS
        .iter()
        .find(|(block, _)| method.refused.contains_key(*block))
    {
        bail!("a connector must not declare {block}: {why}");
    }
    if let Some(unknown) = method.refused.keys().next() {
        bail!("unknown field {unknown:?} in connector method");
    }
    if let Some(auth) = &method.auth {
        validate_auth(auth)?;
    } else if !method.credentials.is_empty() {
        bail!(
            "a method that declares credentials must declare auth: nothing would produce the value, and the credential would ship permanently unarmed"
        );
    }
    lns_spec::credential::validate_all(&method.credentials, lns_spec::credential::Source::Method)
        .map_err(anyhow::Error::msg)?;
    for key in method.env.keys() {
        if !lns_spec::is_legal_env_var_name(key) {
            bail!(
                "invalid env key {key:?}: env keys must be non-empty and free of '=', whitespace, and control characters"
            );
        }
    }
    method
        .egress
        .validate_local_transport()
        .context("connector policy")?;
    method
        .egress
        .validate_binary_scopes()
        .context("connector policy")?;
    let mut written = BTreeSet::new();
    for fileset in &method.filesets {
        crate::sandbox::validate_fileset(fileset, crate::spec::GuestAnchor::Home)?;
        if fileset.host_path.is_some() {
            bail!(
                "a connector must not declare a fileset hostPath: a connector is installed once and used in every project, so reading a path off whichever machine happens to be running it is a sandbox concern"
            );
        }
        // Per method, not per document: methods are alternatives, so only one ever writes.
        if !written.insert(&fileset.guest_path) {
            bail!("duplicate guest path {}", fileset.guest_path);
        }
    }
    refuse_a_secret_shaped_file_carrying_no_declared_placeholder(method, path_files)
}

fn validate_auth(auth: &Auth) -> Result<()> {
    if auth.kind.trim().is_empty() {
        bail!("a method's auth must name its kind");
    }
    if KNOWN_AUTH_KINDS.contains(&auth.kind.as_str())
        && let Some(unknown) = auth.extra.keys().next()
    {
        bail!(
            "unknown field {unknown:?} in a {:?} auth: strict decoding holds for a kind this version knows",
            auth.kind
        );
    }
    Ok(())
}

/// §3.2.5's checkable half: a secret-shaped name earns its exception only by carrying a placeholder **this method** declares.
pub fn refuse_a_secret_shaped_file_carrying_no_declared_placeholder(
    method: &Method,
    path_files: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<()> {
    let declared: Vec<&str> = method
        .credentials
        .iter()
        .map(|credential| credential.placeholder.as_str())
        .collect();
    let readable = method.filesets.iter().flat_map(|fileset| {
        fileset
            .inline
            .iter()
            .chain(fileset.path.as_ref().and_then(|path| path_files.get(path)))
    });
    for files in readable {
        for (name, content) in files {
            if !name.split('/').any(crate::sandbox::looks_like_secret_name) {
                continue;
            }
            if !declared
                .iter()
                .any(|placeholder| content.contains(placeholder))
            {
                bail!(
                    "connector fileset file {name} is secret-shaped and carries no placeholder this method declares; a connector writes the placeholder and the real value stays on the host"
                );
            }
        }
    }
    Ok(())
}

/// Every `path` fileset a connector declares, with the method that owns it.
pub fn path_filesets(connector: &ConnectorSpec) -> Vec<(&str, &str)> {
    connector
        .methods
        .iter()
        .flat_map(|method| {
            method
                .filesets
                .iter()
                .filter_map(move |fileset| Some((method.name.as_str(), fileset.path.as_deref()?)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(spec: &str) -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"connector","name":"some-provider","spec":{spec}}}"#
        )
        .into_bytes()
    }

    const SERVES: &str = r#""serves":["api.some-provider.example"]"#;
    const CREDENTIAL: &str = r#"{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000","injections":[{"kind":"bearer_header","domain":"api.some-provider.example"}]}"#;

    fn with_methods(methods: &str) -> Vec<u8> {
        document(&format!(r#"{{{SERVES},"methods":{methods}}}"#))
    }

    #[test]
    fn a_method_carries_the_payload_a_grant_applies() {
        let def = parse(&with_methods(&format!(
            r#"[{{"name":"token","label":"API token","auth":{{"kind":"token","help":"where to get one"}},"egress":{{"http":[{{"match":"api.some-provider.example","verdict":"allow"}}]}},"credentials":[{CREDENTIAL}],"env":{{"SOME_PROVIDER_REGION":"eu"}},"filesets":[{{"guestPath":"~/.some-provider","inline":{{"config.json":"{{}}"}}}}]}}]"#
        )))
        .expect("§3.2.2 lets a method carry egress, credentials, env and a fileset");
        assert_eq!(def.name, "some-provider");
        assert_eq!(def.spec.serves, ["api.some-provider.example"]);
        let method = &def.spec.methods[0];
        assert_eq!(method.name, "token");
        assert_eq!(method.label(), "API token");
        assert_eq!(method.egress.http.len(), 1);
        assert_eq!(method.credentials[0].env_var.as_deref(), Some("SOME_TOKEN"));
        assert_eq!(
            method.env.get("SOME_PROVIDER_REGION").map(String::as_str),
            Some("eu")
        );
        assert_eq!(method.filesets[0].guest_path, "~/.some-provider");
        assert!(method.is_offerable());
    }

    #[test]
    fn a_method_with_no_auth_is_granted_rather_than_connected() {
        let def = parse(&document(
            r#"{"serves":["docs.rs"],"methods":[{"name":"default","egress":{"http":[{"match":"docs.rs","verdict":"allow"}]}}]}"#,
        ))
        .expect("a connector that only opens a destination has nothing to sign in to");
        let method = &def.spec.methods[0];
        assert!(method.auth.is_none());
        assert!(
            method.is_offerable(),
            "a method with nothing to authenticate is offerable, not lesser: the card grants it and never connects it"
        );
        assert_eq!(method.label(), "default", "label defaults to name");
    }

    #[test]
    fn a_method_refuses_each_block_a_running_guest_cannot_be_given() {
        for (block, payload) in [
            ("tools", r#"["postgresql@17"]"#),
            ("scripts", r#"[{"when":"pre-start","run":"echo hi"}]"#),
            (
                "volumes",
                r#"[{"name":"cache","target":"/home/agent/.cache"}]"#,
            ),
            ("ports", r#"[{"container":8080}]"#),
            ("mixins", r#"["./local"]"#),
            ("image", r#""ghcr.io/team/base:1""#),
            ("command", r#""agent --serve""#),
            ("workdir", r#""/workspace""#),
            ("user", r#""node""#),
            ("resources", r#"{"cpu":2}"#),
        ] {
            let err = parse(&with_methods(&format!(
                r#"[{{"name":"token","auth":{{"kind":"token"}},"{block}":{payload}}}]"#
            )))
            .unwrap_err();
            assert!(
                format!("{err:#}").contains(&format!("a connector must not declare {block}")),
                "§3.2.3 refuses a block by name so an author learns where it belongs; got: {err:#}"
            );
        }
    }

    #[test]
    fn a_payload_block_beside_the_methods_is_refused_where_it_belongs() {
        for block in METHOD_BLOCKS {
            let payload = match block {
                "egress" => r#"{"http":[]}"#.to_string(),
                "credentials" => format!("[{CREDENTIAL}]"),
                "filesets" => r#"[{"guestPath":"~/a","inline":{"a":"b"}}]"#.to_string(),
                _ => r#"{"A":"b"}"#.to_string(),
            };
            let err = parse(&document(&format!(
                r#"{{{SERVES},"{block}":{payload},"methods":[{{"name":"token","auth":{{"kind":"token"}}}}]}}"#
            )))
            .unwrap_err();
            assert!(
                format!("{err:#}")
                    .contains(&format!("a connector must declare {block} inside a method")),
                "a grant applies one method, so a payload block outside them belongs to nothing; got: {err:#}"
            );
        }
    }

    #[test]
    fn a_connector_that_serves_nothing_is_refused() {
        let err = parse(&document(
            r#"{"methods":[{"name":"token","auth":{"kind":"token"}}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("at least one serves entry"),
            "serves is what decides when a connector is offered, so one that serves nothing can never be offered; got: {err:#}"
        );
    }

    #[test]
    fn a_connector_with_no_method_is_refused() {
        let err = parse(&document(&format!(r#"{{{SERVES},"methods":[]}}"#))).unwrap_err();
        assert!(
            format!("{err:#}").contains("at least one method"),
            "methods are how a connector is connected; got: {err:#}"
        );
    }

    #[test]
    fn two_methods_sharing_a_name_are_refused() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"}},{"name":"token","auth":{"kind":"token"}}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate connector method"),
            "a grant records the method by name, so two methods answering to one name would make a grant ambiguous; got: {err:#}"
        );
    }

    #[test]
    fn a_method_name_that_is_not_a_dns_label_is_refused() {
        let err = parse(&with_methods(
            r#"[{"name":"Token Method","auth":{"kind":"token"}}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid method name"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_method_declaring_credentials_without_auth_is_refused() {
        let err = parse(&with_methods(&format!(
            r#"[{{"name":"token","credentials":[{CREDENTIAL}]}}]"#
        )))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must declare auth"),
            "nothing would produce the value, so the credential would ship permanently unarmed; got: {err:#}"
        );
    }

    #[test]
    fn an_unknown_auth_kind_parses_and_leaves_its_method_unofferable() {
        let def = parse(&with_methods(
            r#"[{"name":"browser","auth":{"kind":"oauth_device","clientId":"abc","scopes":["read"]}},{"name":"token","auth":{"kind":"token"}}]"#,
        ))
        .expect(
            "§3.2.2 makes an unknown kind a stated exception to strict decoding: refusing the document would make every improved connector uninstallable on a machine that had not upgraded",
        );
        assert!(
            !def.spec.methods[0].is_offerable(),
            "a mechanism this version cannot run must not be offered"
        );
        assert!(
            def.spec.methods[1].is_offerable(),
            "every other method in the document is still offered"
        );
    }

    #[test]
    fn a_typo_in_a_known_auth_kind_still_fails_the_load() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token","helpp":"typo"}}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("unknown field \"helpp\""),
            "the exception is bounded to an auth whose kind this version does not know; got: {err:#}"
        );
    }

    #[test]
    fn a_method_refuses_a_fileset_that_reads_the_machine_running_it() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"hostPath":"~/.some-provider/config.json","guestPath":"~/.some-provider/config.json"}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must not declare a fileset hostPath"),
            "a connector is installed once and used in every project, so reading a path off whichever machine runs it is a sandbox concern; got: {err:#}"
        );
    }

    #[test]
    fn a_connector_fileset_writes_under_the_guests_home() {
        let def = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"guestPath":"~/.some-provider","inline":{"config.json":"{}"}}]}]"#,
        ))
        .expect("§3.2.3 anchors a connector fileset at the home the running guest reports");
        assert_eq!(
            def.spec.methods[0].filesets[0].guest_path,
            "~/.some-provider"
        );
    }

    #[test]
    fn a_connector_fileset_may_not_name_an_absolute_guest_path() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"guestPath":"/home/agent/.some-provider","inline":{"config.json":"{}"}}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must start with `~/`"),
            "a method is applied on a policy change, which reaches only the home of the running guest, so an absolute path is refused where the document is parsed rather than at grant; got: {err:#}"
        );
    }

    #[test]
    fn a_connector_fileset_may_not_name_another_users_home() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"guestPath":"~alice/.some-provider","inline":{"config.json":"{}"}}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must start with `~/`"),
            "a document does not choose whose files it writes; got: {err:#}"
        );
    }

    #[test]
    fn two_filesets_in_one_method_may_not_claim_one_guest_path() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"guestPath":"~/.x","inline":{"a":"1"}},{"guestPath":"~/.x","inline":{"b":"2"}}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate guest path"),
            "the card names every fileset a method writes, so two entries claiming one path would disclose two writes where one happens; got: {err:#}"
        );
    }

    #[test]
    fn two_methods_may_each_write_the_same_guest_path() {
        parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"guestPath":"~/.x","inline":{"a":"1"}}]},{"name":"sso","auth":{"kind":"token"},"filesets":[{"guestPath":"~/.x","inline":{"b":"2"}}]}]"#,
        ))
        .expect("methods are alternatives, so only one ever writes and the paths cannot collide");
    }

    #[test]
    fn a_placeholder_that_could_pass_for_a_real_token_is_refused() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"sk-live-0123456789"}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must self-identify as fake"),
            "these documents are pushed to registries, so a placeholder that reads like a token is a credential one push from being public; got: {err:#}"
        );
    }

    #[test]
    fn a_credential_inside_a_method_may_carry_no_env_var() {
        let def = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"credentials":[{"placeholder":"some_LNSPLACEHOLDER0000000000","injections":[{"kind":"bearer_header","domain":"api.some-provider.example"}]}],"filesets":[{"guestPath":"~/.some-provider/credentials.json","inline":{"credentials.json":"{\"token\":\"some_LNSPLACEHOLDER0000000000\"}"}}]}]"#,
        ))
        .expect("a fileset-delivered credential serves a client that never reads the environment");
        let credential = &def.spec.methods[0].credentials[0];
        assert!(credential.env_var.is_none());
        assert_eq!(
            credential.owner(),
            "some_LNSPLACEHOLDER0000000000",
            "the placeholder is the entry's identity when no variable names it"
        );
    }

    #[test]
    fn two_credentials_in_one_method_sharing_a_placeholder_are_refused() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"A","placeholder":"some_LNSPLACEHOLDER0000000000"},{"envVar":"B","placeholder":"some_LNSPLACEHOLDER0000000000"}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate credential placeholder"),
            "the placeholder is what the merge keys on for an entry with no envVar; got: {err:#}"
        );
    }

    #[test]
    fn two_methods_may_reuse_one_variable_and_one_marker() {
        parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}]},{"name":"sso","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}]}]"#,
        ))
        .expect(
            "methods are alternatives and only one ever enters the merge, so a token method and a sign-in method serve the same variable of the same service",
        );
    }

    #[test]
    fn a_secret_shaped_inline_file_must_carry_a_placeholder_its_own_method_declares() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}],"filesets":[{"guestPath":"~/.some-provider","inline":{"credentials.json":"{\"token\":\"sk-live-real\"}"}}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("carries no placeholder this method declares"),
            "§3.2.5's checkable half: a secret-shaped name earns its exception only by carrying the marker; got: {err:#}"
        );
    }

    #[test]
    fn a_secret_shaped_file_cannot_borrow_a_sibling_methods_marker() {
        let err = parse(&with_methods(
            r#"[{"name":"a","auth":{"kind":"token"},"credentials":[{"envVar":"A_TOKEN","placeholder":"a_LNSPLACEHOLDER00000000"}]},{"name":"b","auth":{"kind":"token"},"credentials":[{"envVar":"B_TOKEN","placeholder":"b_LNSPLACEHOLDER00000000"}],"filesets":[{"guestPath":"~/.p","inline":{"credentials.json":"{\"token\":\"a_LNSPLACEHOLDER00000000\"}"}}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("carries no placeholder this method declares"),
            "a sibling's injection is not armed when this method applies, so its marker would reach the wire unsubstituted; got: {err:#}"
        );
    }

    #[test]
    fn a_path_fileset_is_checked_against_the_bytes_beside_the_document() {
        let json = with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}],"filesets":[{"path":"./some-provider","guestPath":"~/.some-provider"}]}]"#,
        );
        let beside = BTreeMap::from([(
            "./some-provider".to_string(),
            BTreeMap::from([(
                "credentials.json".to_string(),
                r#"{"token":"sk-live-real"}"#.to_string(),
            )]),
        )]);
        let err = parse_with_path_files(&json, &beside).unwrap_err();
        assert!(
            format!("{err:#}").contains("carries no placeholder this method declares"),
            "got: {err:#}"
        );
        assert!(
            parse(&json).is_ok(),
            "a caller that cannot read the directory leaves the content check to the one that can"
        );
    }

    #[test]
    fn path_filesets_names_the_method_that_packs_each_directory() {
        let def = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"path":"./a","guestPath":"~/a"}]},{"name":"sso","auth":{"kind":"token"},"filesets":[{"path":"./b","guestPath":"~/b"}]}]"#,
        ))
        .expect("two methods may each pack a directory");
        assert_eq!(
            path_filesets(&def.spec),
            [("token", "./a"), ("sso", "./b")],
            "an artifact carries one layer per entry, in declaration order"
        );
    }

    #[test]
    fn a_serves_entry_that_names_no_destination_is_refused() {
        for serves in [r#"[""]"#, r#"["  "]"#, r#"["api.example .com"]"#] {
            let err = parse(&document(&format!(
                r#"{{"serves":{serves},"methods":[{{"name":"token","auth":{{"kind":"token"}}}}]}}"#
            )))
            .unwrap_err();
            assert!(
                format!("{err:#}").contains("serves entry"),
                "{serves}: got {err:#}"
            );
        }
    }

    #[test]
    fn an_unknown_field_in_a_connector_spec_fails_the_load() {
        let err = parse(&document(&format!(
            r#"{{{SERVES},"methods":[{{"name":"token","auth":{{"kind":"token"}}}}],"servs":["typo.example"]}}"#
        )))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("unknown field \"servs\""),
            "a misspelled key must fail on its line rather than load with a default; got: {err:#}"
        );
    }

    #[test]
    fn an_unknown_field_in_a_method_fails_the_load() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"labell":"typo"}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("unknown field \"labell\""),
            "got: {err:#}"
        );
    }

    #[test]
    fn an_auth_that_names_no_kind_is_refused() {
        let err = parse(&with_methods(r#"[{"name":"token","auth":{"kind":"  "}}]"#)).unwrap_err();
        assert!(
            format!("{err:#}").contains("must name its kind"),
            "kind decides what else auth accepts and what the method produces; got: {err:#}"
        );
    }

    #[test]
    fn a_refused_block_beside_the_methods_is_still_refused_by_name() {
        let err = parse(&document(&format!(
            r#"{{{SERVES},"tools":["postgresql@17"],"methods":[{{"name":"token","auth":{{"kind":"token"}}}}]}}"#
        )))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("a connector must not declare tools"),
            "a block a running guest cannot take is refused wherever it sits, not only inside a method; got: {err:#}"
        );
    }

    #[test]
    fn a_refused_block_is_refused_however_it_is_written() {
        for payload in ["8080", "[]", "null"] {
            let err = parse(&with_methods(&format!(
                r#"[{{"name":"token","auth":{{"kind":"token"}},"ports":{payload}}}]"#
            )))
            .unwrap_err();
            assert!(
                format!("{err:#}").contains("a connector must not declare ports"),
                "writing the block at all is what a method may not do, so an empty or null one earns the same named refusal rather than a bare unknown-field error; got: {err:#}"
            );
        }
    }

    #[test]
    fn an_env_key_no_process_could_carry_is_refused() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"env":{"SOME KEY":"v"}}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid env key"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_document_of_another_kind_is_refused() {
        let err = parse(
            br#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"x","spec":{"serves":["a.example"]}}"#,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("expected kind connector"),
            "got: {err:#}"
        );
    }
}
