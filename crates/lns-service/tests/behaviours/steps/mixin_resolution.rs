use cucumber::{given, then, when};
use lns_service::artifact::mixin::{FetchedMixin, Locator, MixinSource};

use crate::world::BehaviourWorld;

/// The one mixin every scenario declares, digest-pinned because a published document may reference nothing else.
pub(crate) const MIXIN: &str = "ghcr.io/acme/some-mixin@sha256:c41e8b7d20a95f6c3d84b1e07f92a5c8d63b40e19a7c25f8b0d3e6a94c17f582";

/// Serves the mixin documents a scenario installed, answering for a tag under the pinned identity a registry would report.
pub(crate) struct Installed {
    documents: std::collections::BTreeMap<String, String>,
    pins: std::collections::BTreeMap<String, String>,
    layers: std::collections::BTreeMap<String, Vec<lns_service::artifact::PackedLayer>>,
}

impl Installed {
    pub(crate) fn from_rig(rig: &crate::declared_rig::DeclaredRig) -> Self {
        Self {
            documents: rig.mixins.clone(),
            pins: rig.mixin_pins.clone(),
            layers: rig.mixin_layers.clone(),
        }
    }
}

impl MixinSource for Installed {
    async fn fetch(&self, locator: &Locator) -> anyhow::Result<FetchedMixin> {
        let reference = locator.key();
        // A local path answers under the document it names, as the reader this stands in for does; a directory holds `lns.yaml`.
        let pinned = match locator {
            Locator::Local(path) if path.extension().is_none() => {
                path.join("lns.yaml").display().to_string()
            }
            _ => self
                .pins
                .get(&reference)
                .cloned()
                .unwrap_or(reference.clone()),
        };
        let document = self
            .documents
            .get(&pinned)
            .or_else(|| self.documents.get(&reference))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("nothing here answers for {reference}"))?;
        let layers = self.layers.get(&reference).cloned().unwrap_or_default();
        Ok(FetchedMixin {
            pinned,
            document,
            layers,
        })
    }
}

/// A scenario that does not say otherwise resolves a published document.
pub(crate) fn published() -> Locator {
    Locator::Reference("registry.example.test/some-sandbox:1".to_string())
}

pub(crate) fn install_at(w: &mut BehaviourWorld, reference: &str, name: &str, spec: &str) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.mixins.insert(
        reference.to_string(),
        format!(r#"{{"apiVersion":"lns.run/v1","kind":"mixin","name":"{name}","spec":{spec}}}"#),
    );
}

fn install(w: &mut BehaviourWorld, spec: &str) {
    install_at(w, MIXIN, "some-mixin", spec);
}

fn definition(w: &mut BehaviourWorld, spec: &str) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.definition = Some(format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{spec}}}"#
    ));
}

#[given(regex = r#"^a mixin declaring the tool "([^"]+)"$"#)]
fn mixin_declaring_a_tool(w: &mut BehaviourWorld, tool: String) {
    install(w, &format!(r#"{{"tools":["{tool}"]}}"#));
}

#[given(regex = r#"^a mixin declaring the tools "([^"]+)" and "([^"]+)"$"#)]
fn mixin_declaring_two_tools(w: &mut BehaviourWorld, first: String, second: String) {
    install(w, &format!(r#"{{"tools":["{first}","{second}"]}}"#));
}

#[given(regex = r#"^a mixin allowing "([^"]+)"$"#)]
fn mixin_allowing_a_destination(w: &mut BehaviourWorld, host: String) {
    install(
        w,
        &format!(r#"{{"egress":{{"http":[{{"match":"{host}","verdict":"allow"}}]}}}}"#),
    );
}

#[given(regex = r#"^a mixin declaring the credential "([^"]+)" for "([^"]+)"$"#)]
fn mixin_declaring_a_credential(w: &mut BehaviourWorld, env_var: String, domain: String) {
    install(
        w,
        &format!(
            r#"{{"credentials":[{{"envVar":"{env_var}","placeholder":"lns-placeholder-{env_var}","injections":[{{"kind":"bearer_header","domain":"{domain}"}}]}}]}}"#
        ),
    );
}

#[given(regex = r#"^the directory's own decisions declare the tool "([^"]+)"$"#)]
fn local_mixin_declaring_a_tool(w: &mut BehaviourWorld, tool: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.local_mixin = Some(format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"mixin","name":"lns-local-mixin","spec":{{"tools":["{tool}"]}}}}"#
    ));
}

/// The decisions file in both the roles it plays: the merge's last source, and the live table the guest's gate folds over what the run pulled.
fn decided(w: &mut BehaviourWorld, spec: &str, allowing: &str) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.local_mixin = Some(format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"mixin","name":"lns-local-mixin","spec":{spec}}}"#
    ));
    rig.overlay
        .add_rule(lns_policy::RouteRule::allow_host(allowing));
}

#[given(regex = r#"^the directory's own decisions allow "([^"]+)"$"#)]
fn local_mixin_allowing_a_destination(w: &mut BehaviourWorld, host: String) {
    decided(
        w,
        &format!(
            r#"{{"tools":["curl@8"],"egress":{{"http":[{{"match":"{host}","verdict":"allow"}}]}}}}"#
        ),
        &host,
    );
}

#[given(regex = r#"^the directory's own decisions allow "([^"]+)" and nothing else$"#)]
fn local_mixin_allowing_only_a_destination(w: &mut BehaviourWorld, host: String) {
    decided(
        w,
        &format!(r#"{{"egress":{{"http":[{{"match":"{host}","verdict":"allow"}}]}}}}"#),
        &host,
    );
}

#[given(regex = r#"^the directory's own decisions declare the tool "([^"]+)" and that mixin$"#)]
fn local_mixin_declaring_a_tool_and_that_mixin(w: &mut BehaviourWorld, tool: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.local_mixin = Some(format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"mixin","name":"lns-local-mixin","spec":{{"tools":["{tool}"],"mixins":["{MIXIN}"]}}}}"#
    ));
}

#[given("the sandbox definition declares nothing but its image")]
fn definition_declares_only_its_image(w: &mut BehaviourWorld) {
    definition(w, r#"{"image":"ghcr.io/team/base:1"}"#);
}

#[given(regex = r#"^the sandbox definition denies "([^"]+)"$"#)]
fn definition_denying_a_destination(w: &mut BehaviourWorld, host: String) {
    definition(
        w,
        &format!(
            r#"{{"image":"ghcr.io/team/base:1","egress":{{"http":[{{"match":"{host}","verdict":"deny"}}]}}}}"#
        ),
    );
}

#[given("the sandbox definition declares that mixin")]
fn definition_declares_that_mixin(w: &mut BehaviourWorld) {
    definition(
        w,
        &format!(r#"{{"image":"ghcr.io/team/base:1","mixins":["{MIXIN}"]}}"#),
    );
}

#[given(regex = r#"^the sandbox definition declares the tool "([^"]+)" and that mixin$"#)]
fn definition_declares_a_tool_and_that_mixin(w: &mut BehaviourWorld, tool: String) {
    definition(
        w,
        &format!(r#"{{"image":"ghcr.io/team/base:1","tools":["{tool}"],"mixins":["{MIXIN}"]}}"#),
    );
}

#[given(
    regex = r#"^a published sandbox layering on a mixin that ships a path fileset at "([^"]+)"$"#
)]
fn sandbox_layering_on_a_mixin_that_ships_files(w: &mut BehaviourWorld, mount: String) {
    install(
        w,
        &format!(r#"{{"filesets":[{{"path":"./skills","mountPath":"{mount}"}}]}}"#),
    );
    definition(
        w,
        &format!(r#"{{"image":"ghcr.io/team/base:1","mixins":["{MIXIN}"]}}"#),
    );
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.mixin_layers.insert(
        MIXIN.to_string(),
        vec![lns_service::artifact::PackedLayer {
            digest: format!("sha256:{}", "e".repeat(64)),
            size: 512,
        }],
    );
}

#[then(regex = r#"^the run pulls "([^"]+)" from the mixin's own artifact$"#)]
fn the_run_pulls_from_the_mixins_own_artifact(
    w: &mut BehaviourWorld,
    mount: String,
) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    if let Some(err) = &rig.error {
        return Err(format!("the launch failed: {err}"));
    }
    match rig.packed_filesets.get(&mount) {
        Some(source) if source.reference == MIXIN => Ok(()),
        other => Err(format!(
            "sharing a directory across sandboxes is publishing a mixin that carries it, so the layer has to come from the mixin's own digest — the one the disclosure named; got {other:?}"
        )),
    }
}

#[given("the sandbox definition declares a mixin nothing can resolve")]
fn definition_declares_an_unresolvable_mixin(w: &mut BehaviourWorld) {
    definition(
        w,
        &format!(r#"{{"image":"ghcr.io/team/base:1","mixins":["{MIXIN}"]}}"#),
    );
}

#[when("the published sandbox is resolved and launched")]
async fn sandbox_is_resolved_and_launched(w: &mut BehaviourWorld) {
    let (definition, installed, local) = {
        let rig = w.declared.get_or_insert_with(Default::default);
        (
            rig.definition
                .clone()
                .expect("a Given step must declare the definition"),
            Installed::from_rig(rig),
            rig.local_mixin.clone(),
        )
    };
    let local = lns_service::artifact::mixin::LocalSource::read(
        local.map(|document| FetchedMixin {
            pinned: "lns-local-mixin.yaml".to_string(),
            document,
            layers: Vec::new(),
        }),
        Locator::Local(std::path::PathBuf::from("/work")),
    )
    .expect("the directory's decisions read");
    let planned = match lns_service::artifact::mixin::resolve(
        definition.as_bytes(),
        &[],
        &published(),
        &installed,
        local,
    )
    .await
    {
        Ok(resolution) => {
            let packed = lns_service::artifact::correlate_packed_filesets(
                &resolution.fileset_origins,
                &resolution.declared_path_filesets,
                &resolution.carriers,
            )
            .expect("every packed fileset this scenario declares has a layer behind it");
            let rig = w.declared.get_or_insert_with(Default::default);
            rig.resolved_document =
                Some(String::from_utf8_lossy(&resolution.document).into_owned());
            rig.resolved_mixins.clone_from(&resolution.mixins);
            rig.contributions =
                lns_service::artifact::mixin::on_the_wire(&resolution.contributions);
            rig.packed_filesets = packed.clone();
            lns_service::artifact::plan_published_sandbox(
                &resolution.document,
                "registry.example.test/some-sandbox:1",
                &packed,
            )
            .map(|resolved| {
                lns_service::artifact::with_authored_baseline(resolved, &resolution.authored_egress)
            })
        }
        Err(e) => Err(e),
    };
    crate::steps::declared_connectors::launch_resolved(w, planned);
}

#[then(regex = r#"^the run installs "([^"]+)"$"#)]
fn run_installs(w: &mut BehaviourWorld, tool: String) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    if let Some(err) = &rig.error {
        return Err(format!("the launch failed: {err}"));
    }
    if rig.tools.contains(&tool) {
        Ok(())
    } else {
        Err(format!(
            "expected the resolved sandbox to ask for {tool}, got {:?}",
            rig.tools
        ))
    }
}

/// What the guest's gate decides for a host: the first entry that matches it, since a merged table only means anything read in order. `None` is the destination nothing decided, which is asked about rather than refused.
pub fn gate_verdict(policy: &lns_policy::Policy, host: &str) -> Option<lns_policy::Verdict> {
    policy
        .network
        .egress
        .http
        .iter()
        .find(|rule| lns_policy::matching::domain_matches(&rule.match_pattern, host))
        .map(|rule| rule.verdict)
}

#[then(regex = r#"^a workload request to "([^"]+)" is allowed by policy$"#)]
fn request_allowed_by_policy(w: &mut BehaviourWorld, host: String) -> Result<(), String> {
    let policy = w
        .declared
        .as_ref()
        .and_then(|r| r.running_policy.as_ref())
        .ok_or("no running policy was produced")?;
    match gate_verdict(policy, &host) {
        Some(lns_policy::Verdict::Allow) => Ok(()),
        other => Err(format!(
            "expected {host} to be allowed by what the mixin contributed, the gate's first match gave {other:?}; routes: {:?}",
            policy.network.egress.http
        )),
    }
}

#[then("the error says the mixin could not be resolved")]
fn error_says_unresolvable(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    let error = rig.error.as_deref().ok_or("no launch error was recorded")?;
    if error.contains("resolving mixin") {
        Ok(())
    } else {
        Err(format!(
            "a run that dropped a mixin it could not fetch would boot without what its document asked for: {error}"
        ))
    }
}

#[then("the error says the definition reached the plan unresolved")]
fn error_says_the_definition_reached_the_plan_unresolved(
    w: &mut BehaviourWorld,
) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    let error = rig.error.as_deref().ok_or("no launch error was recorded")?;
    if error.contains("reached the plan without being resolved") {
        Ok(())
    } else {
        Err(format!(
            "a run whose document still declares a mixin never merged it, so booting would drop what it contributes without a word: {error}"
        ))
    }
}

#[then(regex = r#"^the resolved document allows "([^"]+)"$"#)]
fn resolved_document_allows(w: &mut BehaviourWorld, host: String) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    let document = rig
        .resolved_document
        .as_ref()
        .ok_or("the resolution produced no document")?;
    let def = lns_artifact::sandbox::parse(document.as_bytes()).map_err(|e| format!("{e:#}"))?;
    match super::mixin_resolution::gate_verdict(
        &lns_policy::Policy {
            network: lns_policy::NetworkPolicy {
                egress: def.spec.egress,
            },
            ..lns_policy::Policy::default()
        },
        &host,
    ) {
        Some(lns_policy::Verdict::Allow) => Ok(()),
        other => Err(format!(
            "the document a run boots is the whole merge, and a decision missing from it has nowhere to be disclosed; the document's first match for {host} gave {other:?}: {document}"
        )),
    }
}

#[then(regex = r#"^the disclosure attributes "([^"]+)" to "([^"]+)"$"#)]
fn disclosure_attributes(
    w: &mut BehaviourWorld,
    entry: String,
    source: String,
) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    let found: Vec<(&str, &str)> = rig
        .contributions
        .iter()
        .map(|c| (c.key.as_str(), c.source.as_str()))
        .collect();
    if found.contains(&(entry.as_str(), source.as_str())) {
        Ok(())
    } else {
        Err(format!(
            "§1.5 has the disclosure name what each source contributed, so an override nobody intended has to be visible before boot; got {found:?}"
        ))
    }
}

#[then("the run resolved the directory's own decisions as a source")]
fn run_resolved_the_directorys_decisions(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    if rig
        .resolved_mixins
        .iter()
        .any(|source| source == "lns-local-mixin.yaml")
    {
        Ok(())
    } else {
        Err(format!(
            "a file holding only destinations still decides them, so a run that named no source for them leaves 'why did this run reach that host' unanswered; got {:?}",
            rig.resolved_mixins
        ))
    }
}
