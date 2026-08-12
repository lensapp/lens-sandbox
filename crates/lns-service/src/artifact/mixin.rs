use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use lns_artifact::merge::{MAX_DEPTH, Source, flatten, merge};
use lns_artifact::sandbox::{Definition, SandboxSpec};

/// Where a declared mixin's document comes from: a registry for a digest-pinned reference, the local filesystem for a directory beside the definition.
pub trait MixinSource: Send + Sync {
    fn fetch(&self, reference: &str) -> impl std::future::Future<Output = Result<String>> + Send;
}

/// Whether a pulled manifest is a mixin artifact, decided on the media types alone so the refusal reads the same whether the artifact is mistyped or another kind entirely.
pub fn is_a_mixin_artifact(artifact_type: Option<&str>, config_media_type: Option<&str>) -> bool {
    let mixin = lns_artifact::spec::Kind::Mixin;
    match artifact_type.filter(|t| !t.is_empty()) {
        Some(declared) => declared == mixin.artifact_type(),
        None => config_media_type.is_some_and(|t| t == mixin.config_media_type()),
    }
}

/// Fetch every mixin the ordering rules can reach, so they decide against a complete graph rather than one that is still arriving.
async fn collect<S: MixinSource>(
    roots: &[String],
    source: &S,
) -> Result<BTreeMap<String, SandboxSpec>> {
    let mut graph: BTreeMap<String, SandboxSpec> = BTreeMap::new();
    let mut frontier: Vec<String> = roots.to_vec();
    let mut depth = 1;
    while !frontier.is_empty() && depth <= MAX_DEPTH {
        let mut next = Vec::new();
        for reference in frontier {
            if graph.contains_key(&reference) {
                continue;
            }
            if lns_artifact::sandbox::names_a_local_directory(&reference) {
                bail!(
                    "mixin {reference} names a local directory, which has no meaning on the machine that pulled this sandbox; a published document pins every mixin by digest"
                );
            }
            let document = source
                .fetch(&reference)
                .await
                .with_context(|| format!("resolving mixin {reference}"))?;
            let mixin = lns_artifact::sandbox::parse_mixin(document.as_bytes())
                .with_context(|| format!("reading mixin {reference}"))?;
            next.extend(mixin.spec.mixins.iter().cloned());
            graph.insert(reference, mixin.spec);
        }
        frontier = next;
        depth += 1;
    }
    Ok(graph)
}

/// Resolve a pulled artifact's mixins when it is a sandbox; a plain image has no document to merge and is returned untouched.
pub async fn resolve_if_a_sandbox<S: MixinSource>(
    artifact_type: Option<&str>,
    config_media_type: Option<&str>,
    config_json: &[u8],
    source: &S,
) -> Result<Resolution> {
    if !matches!(
        crate::artifact::dispatch(artifact_type, config_media_type),
        Ok(crate::artifact::RunPath::Sandbox)
    ) {
        return Ok(Resolution {
            document: config_json.to_vec(),
            mixins: Vec::new(),
        });
    }
    resolve(config_json, source).await
}

/// What a run boots, and the mixins that produced it — the merged document declares none of its own, so the references travel beside it.
#[derive(Debug)]
pub struct Resolution {
    pub document: Vec<u8>,
    pub mixins: Vec<String>,
}

/// Resolve a published definition and every mixin it layers on into the one document a run boots (`docs/sandbox-spec.md` §3.3), returned as a definition the rest of the plan path reads exactly as it reads an authored one.
pub async fn resolve<S: MixinSource>(config_json: &[u8], source: &S) -> Result<Resolution> {
    let def = lns_artifact::sandbox::parse(config_json)
        .context("reading the published sandbox document")?;
    if def.spec.mixins.is_empty() {
        return Ok(Resolution {
            document: config_json.to_vec(),
            mixins: Vec::new(),
        });
    }
    let graph = collect(&def.spec.mixins, source).await?;
    let sources = flatten(&def.spec, &[], &graph)?;
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
    })
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
                fetched: Mutex::new(Vec::new()),
            }
        }
    }

    impl MixinSource for Fake {
        async fn fetch(&self, reference: &str) -> Result<String> {
            self.fetched
                .lock()
                .expect("fetch log poisoned")
                .push(reference.to_string());
            self.documents
                .get(reference)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no such mixin here"))
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
        let out = resolve(&sandbox(&spec), &Fake::new(&refs)).await?;
        lns_artifact::sandbox::parse(&out.document)
    }

    #[tokio::test]
    async fn a_document_with_no_mixins_is_returned_untouched() {
        let source = Fake::new(&[]);
        let original = sandbox(r#"{"image":"x:1"}"#);
        let out = resolve(&original, &source).await.unwrap();
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
        let out = resolve_if_a_sandbox(None, None, b"{}", &source)
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
        let resolution = resolve(&sandbox(&spec), &source)
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
            documents: BTreeMap::from([(
                reference.clone(),
                String::from_utf8(sandbox(r#"{"image":"x:1"}"#)).expect("utf-8 fixture"),
            )]),
            fetched: Mutex::new(Vec::new()),
        };
        let err = resolve(
            &sandbox(&format!(r#"{{"image":"x:1","mixins":["{reference}"]}}"#)),
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
        let err = resolve(b"{}", &Fake::new(&[])).await.unwrap_err();
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
