use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use lns_artifact::merge::{MAX_DEPTH, Source, flatten, merge};
use lns_artifact::sandbox::{Definition, SandboxSpec};

/// A mixin's document and the reference that names exactly those bytes, since a user may ask for one by tag and what they approve has to name the bytes.
#[derive(Debug)]
pub struct FetchedMixin {
    pub pinned: String,
    pub document: String,
}

/// Where a mixin's document comes from: a registry for a digest-pinned reference, the local filesystem for a directory beside the definition.
pub trait MixinSource: Send + Sync {
    fn fetch(
        &self,
        reference: &str,
    ) -> impl std::future::Future<Output = Result<FetchedMixin>> + Send;
}

/// Whether a pulled manifest is a mixin artifact, decided on the media types alone so the refusal reads the same whether the artifact is mistyped or another kind entirely.
pub fn is_a_mixin_artifact(artifact_type: Option<&str>, config_media_type: Option<&str>) -> bool {
    let mixin = lns_artifact::spec::Kind::Mixin;
    match artifact_type.filter(|t| !t.is_empty()) {
        Some(declared) => declared == mixin.artifact_type(),
        None => config_media_type.is_some_and(|t| t == mixin.config_media_type()),
    }
}

/// The graph every ordering rule decides against, keyed by the pinned reference each source resolved to, with every reference that names a source translated to the same key.
struct Fetched {
    graph: BTreeMap<String, SandboxSpec>,
    pinned_roots: Vec<String>,
    pinned_extra: Vec<String>,
}

/// What one walk of the graph carries: the sources fetched so far, keyed by the reference that pins them, and the references still to visit.
struct Walk {
    graph: BTreeMap<String, SandboxSpec>,
    pins: BTreeMap<String, String>,
    frontier: Vec<String>,
}

/// Fetch every mixin the ordering rules can reach, so they decide against a complete graph rather than one that is still arriving.
async fn collect<S: MixinSource>(
    roots: &[String],
    extra: &[String],
    source: &S,
) -> Result<Fetched> {
    let mut walk = Walk {
        graph: BTreeMap::new(),
        pins: BTreeMap::new(),
        frontier: Vec::new(),
    };
    let mut pinned_extra = Vec::new();
    for reference in extra {
        pinned_extra.push(visit(&mut walk, reference, true, source).await?);
    }
    for reference in roots {
        visit(&mut walk, reference, false, source).await?;
    }
    let mut depth = 2;
    while !walk.frontier.is_empty() && depth <= MAX_DEPTH {
        for reference in std::mem::take(&mut walk.frontier) {
            visit(&mut walk, &reference, false, source).await?;
        }
        depth += 1;
    }
    let mut graph = walk.graph;
    for spec in graph.values_mut() {
        spec.mixins = pinned(&spec.mixins, &walk.pins);
    }
    Ok(Fetched {
        pinned_roots: pinned(roots, &walk.pins),
        graph,
        pinned_extra,
    })
}

/// Say every reference the way the registry answered for it, so the walk and the graph agree on one identity per source; a reference nothing fetched is left alone for the depth limit to refuse.
fn pinned(references: &[String], pins: &BTreeMap<String, String>) -> Vec<String> {
    references
        .iter()
        .map(|reference| pins.get(reference).unwrap_or(reference).clone())
        .collect()
}

/// Fetch one reference and answer with what pins it, reading each reference once however many documents name it.
async fn visit<S: MixinSource>(
    walk: &mut Walk,
    reference: &str,
    the_user_named_it: bool,
    source: &S,
) -> Result<String> {
    if let Some(pinned) = walk.pins.get(reference) {
        return Ok(pinned.clone());
    }
    refuse_a_directory(reference, the_user_named_it)?;
    let fetched = source
        .fetch(reference)
        .await
        .with_context(|| format!("resolving mixin {reference}"))?;
    walk.pins
        .insert(reference.to_string(), fetched.pinned.clone());
    if !walk.graph.contains_key(&fetched.pinned) {
        let mixin = lns_artifact::sandbox::parse_mixin(fetched.document.as_bytes())
            .with_context(|| format!("reading mixin {reference}"))?;
        walk.frontier.extend(mixin.spec.mixins.iter().cloned());
        walk.graph.insert(fetched.pinned.clone(), mixin.spec);
    }
    Ok(fetched.pinned)
}

/// A directory has no published identity, so neither kind of reference may name one — but only one of them is the user's to correct.
fn refuse_a_directory(reference: &str, the_user_named_it: bool) -> Result<()> {
    if !lns_artifact::sandbox::names_a_local_directory(reference) {
        return Ok(());
    }
    if the_user_named_it {
        bail!(
            "mixin {reference} cannot name a local directory: a run merges published bytes, so name a reference the disclosure can pin"
        );
    }
    bail!(
        "mixin {reference} names a local directory, which has no meaning on the machine that pulled this sandbox; a published document pins every mixin by digest"
    )
}

/// Resolve a pulled artifact's mixins when it is a sandbox; a plain image has no document to merge and is returned untouched.
pub async fn resolve_if_a_sandbox<S: MixinSource>(
    artifact_type: Option<&str>,
    config_media_type: Option<&str>,
    config_json: &[u8],
    extra: &[String],
    source: &S,
) -> Result<Resolution> {
    if !matches!(
        crate::artifact::dispatch(artifact_type, config_media_type),
        Ok(crate::artifact::RunPath::Sandbox)
    ) {
        refuse_mixins_without_a_document(extra)?;
        return Ok(Resolution {
            document: config_json.to_vec(),
            mixins: Vec::new(),
            pinned_extra: Vec::new(),
        });
    }
    resolve(config_json, extra, source).await
}

/// What a run boots, and the mixins that produced it — the merged document declares none of its own, so the references travel beside it.
#[derive(Debug)]
pub struct Resolution {
    pub document: Vec<u8>,
    pub mixins: Vec<String>,
    /// What each reference the user named resolved to, in the order they named them, so the boot merges the bytes the preflight showed.
    pub pinned_extra: Vec<String>,
}

/// Resolve a published definition and every mixin it layers on into the one document a run boots (`docs/sandbox-spec.md` §3.3), returned as a definition the rest of the plan path reads exactly as it reads an authored one.
pub async fn resolve<S: MixinSource>(
    config_json: &[u8],
    extra: &[String],
    source: &S,
) -> Result<Resolution> {
    let def = lns_artifact::sandbox::parse(config_json)
        .context("reading the published sandbox document")?;
    if def.spec.mixins.is_empty() && extra.is_empty() {
        return Ok(Resolution {
            document: config_json.to_vec(),
            mixins: Vec::new(),
            pinned_extra: Vec::new(),
        });
    }
    let fetched = collect(&def.spec.mixins, extra, source).await?;
    let mut root = def.spec.clone();
    root.mixins = fetched.pinned_roots;
    let sources = flatten(&root, &fetched.pinned_extra, &fetched.graph)?;
    let merged = merge(&sources)?;
    let document = document(&def, &merged.spec)?;
    refuse_what_no_sandbox_could_be(&document, &sources)?;
    Ok(Resolution {
        document,
        mixins: sources
            .iter()
            .skip(1)
            .map(|source| source.label.clone())
            .collect(),
        pinned_extra: fetched.pinned_extra,
    })
}

/// Only a sandbox document has blocks to merge into, so a run of anything else has to refuse what it was given rather than boot without it.
pub fn refuse_mixins_without_a_document(extra: &[String]) -> Result<()> {
    if extra.is_empty() {
        return Ok(());
    }
    bail!(
        "this reference has no sandbox document to merge into, so it cannot take the {} mixin(s) named for it",
        extra.len()
    )
}

/// The preflight pins what it showed, and the boot merges that — so a reference reaching the boot unpinned was never disclosed, whoever sent it.
pub fn require_pinned_extras(extra: &[String]) -> Result<()> {
    match extra
        .iter()
        .find(|reference| !lns_artifact::spec::is_digest_pinned_image(reference))
    {
        None => Ok(()),
        Some(unpinned) => bail!(
            "mixin {unpinned} reached the run unpinned; a run merges the digest its preflight showed, so pin it by digest"
        ),
    }
}

/// The resolved document is what boots, so it has to be one an author could have written — an override is normal, but its result still has to hold.
fn refuse_what_no_sandbox_could_be(document: &[u8], sources: &[Source]) -> Result<()> {
    let contributors = sources.len().saturating_sub(1);
    lns_artifact::sandbox::parse(document).with_context(|| {
        format!("the {contributors} mixin(s) this sandbox layers on merge into a document that is not a valid sandbox")
    })?;
    Ok(())
}

/// Re-emit the merged spec under the sandbox's own identity, so what the plan path reads is the document that will boot.
fn document(def: &Definition, spec: &SandboxSpec) -> Result<Vec<u8>> {
    serde_json::to_vec(&serde_json::json!({
        "apiVersion": lns_artifact::sandbox::API_VERSION,
        "kind": lns_artifact::sandbox::KIND,
        "metadata": &def.metadata,
        "spec": spec,
    }))
    .context("serializing the resolved sandbox")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake {
        documents: BTreeMap<String, String>,
        pins: BTreeMap<String, String>,
        fetched: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(documents: &[(&str, &str)]) -> Self {
            Self {
                documents: documents
                    .iter()
                    .map(|(r, spec)| {
                        (
                            (*r).to_string(),
                            format!(
                                r#"{{"apiVersion":"lns.run/v1","kind":"Mixin","metadata":{{"name":"some-mixin"}},"spec":{spec}}}"#
                            ),
                        )
                    })
                    .collect(),
                pins: BTreeMap::new(),
                fetched: Mutex::new(Vec::new()),
            }
        }

        fn pinning(mut self, reference: &str, pinned: &str) -> Self {
            self.pins.insert(reference.to_string(), pinned.to_string());
            self
        }
    }

    impl MixinSource for Fake {
        async fn fetch(&self, reference: &str) -> Result<FetchedMixin> {
            self.fetched
                .lock()
                .expect("fetch log poisoned")
                .push(reference.to_string());
            let document = self
                .documents
                .get(reference)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no such mixin here"))?;
            Ok(FetchedMixin {
                pinned: self
                    .pins
                    .get(reference)
                    .cloned()
                    .unwrap_or_else(|| reference.to_string()),
                document,
            })
        }
    }

    /// A digest-pinned reference for a fixture name, since validation refuses anything a consumer could not resolve identically.
    fn pinned(name: &str) -> String {
        let hex: String = name
            .bytes()
            .cycle()
            .take(64)
            .map(|b| char::from_digit(u32::from(b % 16), 16).unwrap_or('a'))
            .collect();
        format!("ghcr.io/acme/{name}@sha256:{hex}")
    }

    fn sandbox(spec: &str) -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{spec}}}"#
        )
        .into_bytes()
    }

    /// Names in a fixture stand for digest-pinned references, so a scenario reads as a graph rather than as sixty-four hex characters.
    fn resolve_named(spec: &str, mixins: &[(&str, &str)]) -> (String, Vec<(String, String)>) {
        let mut rewritten = spec.to_string();
        let mut documents = Vec::new();
        for (name, body) in mixins {
            rewritten = rewritten.replace(&format!("\"{name}\""), &format!("\"{}\"", pinned(name)));
            let mut body = (*body).to_string();
            for (other, _) in mixins {
                body = body.replace(&format!("\"{other}\""), &format!("\"{}\"", pinned(other)));
            }
            documents.push((pinned(name), body));
        }
        (rewritten, documents)
    }

    async fn resolved(spec: &str, mixins: &[(&str, &str)]) -> Result<Definition> {
        let (spec, documents) = resolve_named(spec, mixins);
        let refs: Vec<(&str, &str)> = documents
            .iter()
            .map(|(r, b)| (r.as_str(), b.as_str()))
            .collect();
        let out = resolve(&sandbox(&spec), &[], &Fake::new(&refs)).await?;
        lns_artifact::sandbox::parse(&out.document)
    }

    #[tokio::test]
    async fn a_document_with_no_mixins_is_returned_untouched() {
        let source = Fake::new(&[]);
        let original = sandbox(r#"{"image":"x:1"}"#);
        let out = resolve(&original, &[], &source).await.unwrap();
        assert_eq!(
            out.document, original,
            "nothing to resolve means nothing to rewrite"
        );
        assert!(out.mixins.is_empty());
        assert!(
            source
                .fetched
                .lock()
                .expect("fetch log poisoned")
                .is_empty(),
            "a document declaring no mixins must not reach for the network at all"
        );
    }

    #[tokio::test]
    async fn a_plain_image_is_passed_through_without_a_fetch() {
        let source = Fake::new(&[]);
        let out = resolve_if_a_sandbox(None, None, b"{}", &[], &source)
            .await
            .expect("an image has no document to merge");
        assert_eq!(out.document, b"{}");
        assert!(
            source
                .fetched
                .lock()
                .expect("fetch log poisoned")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_sandbox_artifact_is_the_one_kind_that_resolves() {
        let sandbox_type = lns_artifact::spec::Kind::Sandbox.artifact_type();
        let out = resolve_if_a_sandbox(
            Some(&sandbox_type),
            None,
            &sandbox(r#"{"image":"x:1"}"#),
            &[],
            &Fake::new(&[]),
        )
        .await
        .expect("a sandbox resolves");
        assert!(out.mixins.is_empty());
    }

    #[tokio::test]
    async fn a_declared_mixin_contributes_to_the_document_that_boots() {
        let def = resolved(
            r#"{"image":"x:1","tools":["node@20"],"mixins":["m"]}"#,
            &[("m", r#"{"tools":["node@22","python@3.12"]}"#)],
        )
        .await
        .expect("the mixin resolves");
        assert_eq!(def.spec.image, "x:1", "the sandbox still owns its launch");
        assert_eq!(
            def.spec.tools,
            ["node@22", "python@3.12"],
            "the mixin's version of a shared tool wins, and what only it declares arrives"
        );
        assert!(
            def.spec.mixins.is_empty(),
            "the resolved document declares no mixins, so nothing downstream tries to resolve it twice"
        );
    }

    #[tokio::test]
    async fn a_mixins_own_mixins_are_fetched_and_merged_too() {
        let def = resolved(
            r#"{"image":"x:1","mixins":["outer"]}"#,
            &[
                ("outer", r#"{"tools":["node@20"],"mixins":["inner"]}"#),
                ("inner", r#"{"tools":["node@22"]}"#),
            ],
        )
        .await
        .expect("the transitive mixin resolves");
        assert_eq!(
            def.spec.tools,
            ["node@22"],
            "a mixin that pulls in another is asking for that other's version"
        );
    }

    #[tokio::test]
    async fn each_reference_is_fetched_once_however_many_documents_name_it() {
        let mixins = [
            ("a", r#"{"mixins":["shared"]}"#),
            ("b", r#"{"mixins":["shared"]}"#),
            ("shared", r#"{"tools":["node@22"]}"#),
        ];
        let (spec, documents) = resolve_named(r#"{"image":"x:1","mixins":["a","b"]}"#, &mixins);
        let refs: Vec<(&str, &str)> = documents
            .iter()
            .map(|(r, b)| (r.as_str(), b.as_str()))
            .collect();
        let source = Fake::new(&refs);
        let resolution = resolve(&sandbox(&spec), &[], &source)
            .await
            .expect("a diamond resolves");
        assert_eq!(
            resolution.mixins.len(),
            4,
            "the disclosure names every source that contributed, including a mixin reached twice"
        );
        let shared = pinned("shared");
        let fetched = source.fetched.lock().expect("fetch log poisoned");
        assert_eq!(
            fetched.iter().filter(|r| **r == shared).count(),
            1,
            "two documents naming one mixin is one fetch, not two"
        );
    }

    #[tokio::test]
    async fn a_reference_that_cannot_be_fetched_refuses_the_run_naming_it() {
        let absent = pinned("absent");
        let err = resolve(
            &sandbox(&format!(r#"{{"image":"x:1","mixins":["{absent}"]}}"#)),
            &[],
            &Fake::new(&[]),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains(&format!("resolving mixin {absent}")),
            "a run that dropped what it could not fetch would boot without what its document asked for; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_reference_that_is_not_a_mixin_document_refuses_the_run() {
        let reference = pinned("a-sandbox");
        let source = Fake {
            pins: BTreeMap::new(),
            documents: BTreeMap::from([(
                reference.clone(),
                String::from_utf8(sandbox(r#"{"image":"x:1"}"#)).expect("utf-8 fixture"),
            )]),
            fetched: Mutex::new(Vec::new()),
        };
        let err = resolve(
            &sandbox(&format!(r#"{{"image":"x:1","mixins":["{reference}"]}}"#)),
            &[],
            &source,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains(&format!("reading mixin {reference}")),
            "a sandbox published where a mixin was expected must be refused by the reader, not merged as one; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_local_directory_reached_from_a_published_document_refuses_the_run() {
        let err = resolve(
            &sandbox(r#"{"image":"x:1","mixins":["./mixins/postgres-tools/"]}"#),
            &[],
            &Fake::new(&[]),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("has no meaning on the machine that pulled this sandbox"),
            "a directory beside the author's file is not something a consumer can resolve, and reading whatever host path it names would be worse than refusing; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_cycle_refuses_the_run_rather_than_fetching_forever() {
        let err = resolved(
            r#"{"image":"x:1","mixins":["a"]}"#,
            &[("a", r#"{"mixins":["b"]}"#), ("b", r#"{"mixins":["a"]}"#)],
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("reachable from itself"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_chain_deeper_than_the_limit_refuses_the_run() {
        let err = resolved(
            r#"{"image":"x:1","mixins":["m1"]}"#,
            &[
                ("m1", r#"{"mixins":["m2"]}"#),
                ("m2", r#"{"mixins":["m3"]}"#),
                ("m3", r#"{"mixins":["m4"]}"#),
                ("m4", r#"{"mixins":["m5"]}"#),
                ("m5", r#"{"mixins":["m6"]}"#),
                ("m6", r#"{}"#),
            ],
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("deeper than 5 mixins"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_document_that_will_not_parse_is_blamed_on_itself_and_not_on_its_mixins() {
        let err = resolve(b"{}", &[], &Fake::new(&[])).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("reading the published sandbox document"),
            "a document that will not parse has no mixins to blame, and naming them sends a publisher looking in the wrong place; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn two_sources_publishing_one_host_port_refuse_the_run() {
        let err = resolved(
            r#"{"image":"x:1","ports":[{"container":8080,"host":18080}],"mixins":["m"]}"#,
            &[("m", r#"{"ports":[{"container":9090,"host":18080}]}"#)],
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("both publish host port 18080"),
            "ports merge by container number, so two sources can claim one host port without either document repeating one, and the run has to refuse rather than silently unpublish one of them; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_plain_image_refuses_the_mixins_the_user_named_rather_than_dropping_them() {
        let sandbox_only = resolve_if_a_sandbox(None, None, b"{}", &[pinned("m")], &Fake::new(&[]))
            .await
            .unwrap_err();
        assert!(
            format!("{sandbox_only:#}").contains("no sandbox document to merge into"),
            "an image has nothing to merge, so silently running it without what the user asked for is the refusal this exists to make; got: {sandbox_only:#}"
        );
    }

    #[tokio::test]
    async fn a_declared_pin_the_registry_renders_differently_still_resolves() {
        let declared = format!("ghcr.io/acme/m:17@sha256:{}", "c".repeat(64));
        let rendered = format!("ghcr.io/acme/m@sha256:{}", "c".repeat(64));
        let source = Fake::new(&[(declared.as_str(), r#"{"tools":["python@3.12"]}"#)])
            .pinning(&declared, &rendered);
        let out = resolve(
            &sandbox(&format!(r#"{{"image":"x:1","mixins":["{declared}"]}}"#)),
            &[],
            &source,
        )
        .await
        .expect("a document may pin a mixin in any form the registry accepts");
        assert_eq!(
            out.mixins,
            [rendered],
            "the disclosure names the identity the registry answered with, and the declared spelling has to reach it"
        );
    }

    #[tokio::test]
    async fn a_tag_the_user_names_reaches_the_merge_pinned() {
        let tagged = "ghcr.io/acme/obs-tools:2";
        let source =
            Fake::new(&[(tagged, r#"{"tools":["python@3.12"]}"#)]).pinning(tagged, &pinned("obs"));
        let out = resolve(
            &sandbox(r#"{"image":"x:1"}"#),
            &[tagged.to_string()],
            &source,
        )
        .await
        .expect("a tag resolves");
        assert_eq!(
            out.pinned_extra,
            [pinned("obs")],
            "the boot merges what the preflight pinned, so the tag has to be answered here and not resolved twice"
        );
        assert_eq!(
            out.mixins,
            [pinned("obs")],
            "the disclosure names bytes, so a tag that reached it unresolved could move after the user approved it"
        );
    }

    #[test]
    fn a_run_with_no_document_to_merge_into_refuses_the_mixins_named_for_it() {
        refuse_mixins_without_a_document(&[]).expect("a run naming none is untouched");
        let err = refuse_mixins_without_a_document(&[pinned("m")]).unwrap_err();
        assert!(
            format!("{err:#}").contains("no sandbox document to merge into"),
            "a plain image has no blocks to merge, so running it while dropping what the request named would be a silent lie; got: {err:#}"
        );
    }

    #[test]
    fn a_reference_reaching_the_run_unpinned_is_refused_before_it_merges() {
        require_pinned_extras(&[pinned("m")])
            .expect("a pinned reference is what a preflight sends");
        let err = require_pinned_extras(&["ghcr.io/acme/obs-tools:2".to_string()]).unwrap_err();
        assert!(
            format!("{err:#}").contains("reached the run unpinned"),
            "a client that skipped the preflight would boot bytes nobody disclosed; got: {err:#}"
        );
    }

    #[test]
    fn a_merge_that_could_not_have_been_authored_refuses_the_run() {
        let document = sandbox(
            r#"{"image":"x:1","volumes":[{"name":"data","target":"/data"}],"filesets":[{"path":"./skills","mountPath":"/data"}]}"#,
        );
        let sources = [
            Source {
                label: lns_artifact::merge::ROOT_LABEL.to_string(),
                spec: SandboxSpec::default(),
            },
            Source {
                label: pinned("m"),
                spec: SandboxSpec::default(),
            },
        ];
        let err = refuse_what_no_sandbox_could_be(&document, &sources).unwrap_err();
        assert!(
            format!("{err:#}").contains("1 mixin(s) this sandbox layers on merge into a document that is not a valid sandbox"),
            "a merge that ever produced a document no author could have written has to name the mixins that produced it, or it reads as a malformed publish; got: {err:#}"
        );
    }

    #[test]
    fn a_mixin_artifact_is_recognised_by_its_declared_type_and_only_then_by_its_config() {
        let mixin = lns_artifact::spec::Kind::Mixin;
        let sandbox = lns_artifact::spec::Kind::Sandbox;
        assert!(is_a_mixin_artifact(
            Some(&mixin.artifact_type()),
            Some(&sandbox.config_media_type())
        ));
        assert!(
            !is_a_mixin_artifact(
                Some(&sandbox.artifact_type()),
                Some(&mixin.config_media_type())
            ),
            "a present artifactType is the answer; the config type must never second-guess it"
        );
        assert!(
            is_a_mixin_artifact(None, Some(&mixin.config_media_type())),
            "an artifact pushed by a tool that writes no artifactType is still readable"
        );
        assert!(!is_a_mixin_artifact(None, None));
        assert!(!is_a_mixin_artifact(Some(""), None));
    }
}
