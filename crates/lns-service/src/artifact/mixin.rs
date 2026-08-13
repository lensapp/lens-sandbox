use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use lns_artifact::merge::{MAX_DEPTH, Source, flatten, merge};
use lns_artifact::sandbox::{Definition, SandboxSpec};

/// What a document was read from, which is both its identity in the graph and what roots any directory it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Locator {
    Reference(String),
    Directory(std::path::PathBuf),
}

impl Locator {
    /// The one string every rule reads this source by: the reference the registry answered with, or the directory's absolute path.
    pub fn key(&self) -> String {
        match self {
            Locator::Reference(reference) => reference.clone(),
            Locator::Directory(path) => path.display().to_string(),
        }
    }
}

/// A mixin's document and the reference that names exactly those bytes, since a user may ask for one by tag and what they approve has to name the bytes.
#[derive(Debug)]
pub struct FetchedMixin {
    pub pinned: String,
    pub document: String,
}

/// Where a mixin's document comes from: a registry for a reference, this machine's filesystem for a directory.
pub trait MixinSource: Send + Sync {
    fn fetch(
        &self,
        locator: &Locator,
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

/// Resolve one reference against the document that named it: a directory is meaningful only to a document this machine read, and it roots where that document lives.
fn locate(reference: &str, home: &Locator, the_user_named_it: bool) -> Result<Locator> {
    if !lns_artifact::sandbox::names_a_local_directory(reference) {
        return Ok(Locator::Reference(reference.to_string()));
    }
    let Locator::Directory(dir) = home else {
        bail!(
            "mixin {reference} is a directory, and this run's sandbox is published; a directory merges only into a document this machine read, so run the definition it belongs to"
        );
    };
    if the_user_named_it {
        if !std::path::Path::new(reference).is_absolute() {
            bail!(
                "mixin {reference} reached the run as a relative directory; a run merges the absolute path its preflight showed"
            );
        }
        return Ok(Locator::Directory(lns_artifact::sandbox::fold_path(
            std::path::Path::new(reference),
        )));
    }
    Ok(Locator::Directory(lns_artifact::sandbox::fold_path(
        &dir.join(reference),
    )))
}

/// The graph every ordering rule decides against, keyed by the identity each source resolved to, with every reference that names a source translated to the same key.
struct Fetched {
    graph: BTreeMap<String, SandboxSpec>,
    pinned_roots: Vec<String>,
    pinned_extra: Vec<String>,
}

/// What one walk of the graph carries: the sources fetched so far, what each reference resolved to under the document that named it, and the references still to visit.
struct Walk {
    graph: BTreeMap<String, SandboxSpec>,
    visited: BTreeMap<String, String>,
    keys: BTreeMap<(String, String), String>,
    frontier: Vec<(String, Locator)>,
}

/// Fetch every mixin the ordering rules can reach, so they decide against a complete graph rather than one that is still arriving.
async fn collect<S: MixinSource>(
    roots: &[String],
    extra: &[String],
    home: &Locator,
    source: &S,
) -> Result<Fetched> {
    let mut walk = Walk {
        graph: BTreeMap::new(),
        visited: BTreeMap::new(),
        keys: BTreeMap::new(),
        frontier: Vec::new(),
    };
    let mut pinned_extra = Vec::new();
    for reference in extra {
        pinned_extra.push(visit(&mut walk, reference, home, true, source).await?);
    }
    let mut pinned_roots = Vec::new();
    for reference in roots {
        pinned_roots.push(visit(&mut walk, reference, home, false, source).await?);
    }
    let mut depth = 2;
    while !walk.frontier.is_empty() && depth <= MAX_DEPTH {
        for (reference, parent) in std::mem::take(&mut walk.frontier) {
            visit(&mut walk, &reference, &parent, false, source).await?;
        }
        depth += 1;
    }
    let keys = walk.keys;
    let mut graph = walk.graph;
    for (key, spec) in graph.iter_mut() {
        spec.mixins = spec
            .mixins
            .iter()
            .map(|reference| {
                keys.get(&(key.clone(), reference.clone()))
                    .unwrap_or(reference)
                    .clone()
            })
            .collect();
    }
    Ok(Fetched {
        graph,
        pinned_roots,
        pinned_extra,
    })
}

/// Fetch one reference and answer with the identity every rule reads it by, reading each one once however many documents name it.
async fn visit<S: MixinSource>(
    walk: &mut Walk,
    reference: &str,
    home: &Locator,
    the_user_named_it: bool,
    source: &S,
) -> Result<String> {
    let seen = (home.key(), reference.to_string());
    let locator = locate(reference, home, the_user_named_it)?;
    if let Some(key) = walk.visited.get(&locator.key()) {
        let key = key.clone();
        walk.keys.insert(seen, key.clone());
        return Ok(key);
    }
    let fetched = source
        .fetch(&locator)
        .await
        .with_context(|| format!("resolving mixin {reference}"))?;
    let key = match &locator {
        Locator::Reference(_) => fetched.pinned,
        Locator::Directory(path) => path.display().to_string(),
    };
    walk.visited.insert(locator.key(), key.clone());
    walk.keys.insert(seen, key.clone());
    if !walk.graph.contains_key(&key) {
        let mixin = lns_artifact::sandbox::parse_mixin(fetched.document.as_bytes())
            .with_context(|| format!("reading mixin {reference}"))?;
        let child_home = match locator {
            Locator::Directory(path) => Locator::Directory(path),
            Locator::Reference(_) => Locator::Reference(key.clone()),
        };
        walk.frontier.extend(
            mixin
                .spec
                .mixins
                .iter()
                .map(|child| (child.clone(), child_home.clone())),
        );
        walk.graph.insert(key.clone(), mixin.spec);
    }
    Ok(key)
}

/// Resolve a pulled artifact's mixins when it is a sandbox; a plain image has no document to merge and is returned untouched.
pub async fn resolve_if_a_sandbox<S: MixinSource>(
    artifact_type: Option<&str>,
    config_media_type: Option<&str>,
    config_json: &[u8],
    extra: &[String],
    home: &Locator,
    source: &S,
) -> Result<Resolution> {
    if !matches!(
        crate::artifact::dispatch(artifact_type, config_media_type),
        Ok(Some(lns_artifact::spec::Kind::Sandbox))
    ) {
        refuse_mixins_without_a_document(extra)?;
        return Ok(Resolution {
            document: config_json.to_vec(),
            mixins: Vec::new(),
            pinned_extra: Vec::new(),
            contributions: Vec::new(),
        });
    }
    resolve(config_json, extra, home, source).await
}

/// What a run boots, and the mixins that produced it — the merged document declares none of its own, so the references travel beside it.
#[derive(Debug)]
pub struct Resolution {
    pub document: Vec<u8>,
    pub mixins: Vec<String>,
    /// What each reference the user named resolved to, in the order they named them, so the boot merges the bytes the preflight showed.
    pub pinned_extra: Vec<String>,
    /// Which source decided each entry of the merged document, and what that decision replaced.
    pub contributions: Vec<lns_artifact::merge::Contribution>,
}

/// Resolve a published definition and every mixin it layers on into the one document a run boots (`docs/sandbox-spec.md` §3.3), returned as a definition the rest of the plan path reads exactly as it reads an authored one.
pub async fn resolve<S: MixinSource>(
    config_json: &[u8],
    extra: &[String],
    home: &Locator,
    source: &S,
) -> Result<Resolution> {
    let def = lns_artifact::sandbox::parse(config_json).context("reading the sandbox document")?;
    if def.spec.mixins.is_empty() && extra.is_empty() {
        return Ok(Resolution {
            document: config_json.to_vec(),
            mixins: Vec::new(),
            pinned_extra: Vec::new(),
            contributions: Vec::new(),
        });
    }
    let fetched = collect(&def.spec.mixins, extra, home, source).await?;
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
            .map(|source| source.label.to_string())
            .collect(),
        pinned_extra: fetched.pinned_extra,
        contributions: merged.contributions,
    })
}

/// Restate a merge's attribution for the wire, since lns-ipc names the document format without depending on the crate that merges it.
pub fn on_the_wire(
    contributions: &[lns_artifact::merge::Contribution],
) -> Vec<lns_ipc::SourceContribution> {
    contributions
        .iter()
        .map(|c| lns_ipc::SourceContribution {
            block: match c.block {
                lns_artifact::merge::Block::Credential => lns_ipc::ContributionBlock::Credential,
                lns_artifact::merge::Block::Tool => lns_ipc::ContributionBlock::Tool,
                lns_artifact::merge::Block::Mount => lns_ipc::ContributionBlock::Mount,
                lns_artifact::merge::Block::Port => lns_ipc::ContributionBlock::Port,
                lns_artifact::merge::Block::Egress => lns_ipc::ContributionBlock::Egress,
            },
            key: c.key.clone(),
            source: c.source.clone(),
            displaced: c
                .displaced
                .iter()
                .map(|d| lns_ipc::DisplacedEntry {
                    source: d.source.clone(),
                    summary: d.summary.clone(),
                })
                .collect(),
        })
        .collect()
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

/// Cache every mixin a pulled one names, so a digest-pinned graph pulled once resolves offline afterwards; answers with how many documents it read.
pub async fn warm<S: MixinSource>(roots: &[String], source: &S) -> Result<usize> {
    let home = Locator::Reference(String::new());
    let fetched = collect(roots, &[], &home, source).await?;
    Ok(fetched.graph.len())
}

/// A declared directory roots at the directory the definition was read from, so that root has to name one directory on this machine, whoever sent it.
pub fn require_a_rooted_project_dir(project_dir: &std::path::Path) -> Result<()> {
    if project_dir.is_absolute() {
        return Ok(());
    }
    bail!(
        "the definition's directory {} is not an absolute directory; a definition's mixins root where it was read from",
        project_dir.display()
    )
}

/// The preflight pins what it showed, and the boot merges that — so a reference reaching the boot unpinned was never disclosed, whoever sent it.
pub fn require_pinned_extras(extra: &[String]) -> Result<()> {
    match extra.iter().find(|reference| {
        !lns_artifact::spec::is_digest_pinned_image(reference)
            && !std::path::Path::new(reference).is_absolute()
    }) {
        None => Ok(()),
        Some(unpinned) => bail!(
            "mixin {unpinned} reached the run neither pinned nor rooted; a run merges the digest or the absolute path its preflight showed"
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
        "kind": lns_artifact::spec::Kind::Sandbox.as_str(),
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
                                r#"{{"apiVersion":"lns.run/v1","kind":"mixin","metadata":{{"name":"some-mixin"}},"spec":{spec}}}"#
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
        async fn fetch(&self, locator: &Locator) -> Result<FetchedMixin> {
            let reference = locator.key();
            self.fetched
                .lock()
                .expect("fetch log poisoned")
                .push(reference.clone());
            let document = self
                .documents
                .get(&reference)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no such mixin here"))?;
            Ok(FetchedMixin {
                pinned: self.pins.get(&reference).cloned().unwrap_or(reference),
                document,
            })
        }
    }

    /// Every scenario here resolves a published document unless it says otherwise.
    fn published() -> Locator {
        Locator::Reference("registry.example.test/team/sandbox:1".to_string())
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
            r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","metadata":{{"name":"hermes"}},"spec":{spec}}}"#
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
        let out = resolve(&sandbox(&spec), &[], &published(), &Fake::new(&refs)).await?;
        lns_artifact::sandbox::parse(&out.document)
    }

    #[tokio::test]
    async fn a_resolution_carries_which_source_decided_each_entry() {
        let (spec, documents) = resolve_named(
            r#"{"image":"x:1","tools":["node@20"],"mixins":["obs"]}"#,
            &[("obs", r#"{"tools":["node@22"]}"#)],
        );
        let refs: Vec<(&str, &str)> = documents
            .iter()
            .map(|(r, b)| (r.as_str(), b.as_str()))
            .collect();
        let resolution = resolve(&sandbox(&spec), &[], &published(), &Fake::new(&refs))
            .await
            .expect("a declared mixin resolves");
        let wire = on_the_wire(&resolution.contributions);
        let tool = wire
            .iter()
            .find(|c| c.block == lns_ipc::ContributionBlock::Tool && c.key == "node")
            .expect("the merged document's tool is attributed");
        assert_eq!(tool.source, pinned("obs"));
        assert_eq!(
            tool.displaced,
            [lns_ipc::DisplacedEntry {
                source: lns_artifact::merge::ROOT_LABEL.to_string(),
                summary: "node@20".to_string()
            }],
            "the disclosure the CLI prints is built from this, so what a mixin replaced has to survive the wire"
        );
    }

    #[tokio::test]
    async fn every_block_a_merge_attributes_survives_the_wire() {
        let (spec, documents) = resolve_named(
            r#"{"image":"x:1","mixins":["obs"]}"#,
            &[(
                "obs",
                r#"{"tools":["node@22"],"volumes":[{"type":"volume","name":"cache","target":"/cache"}],"ports":[{"container":8080}],"credentials":[{"envVar":"SOME_TOKEN","placeholder":"lns-placeholder-some","injections":[{"kind":"bearer_header","domain":"api.some-provider.example"}]}],"policy":{"egress":{"http":[{"match":"api.some-provider.example","verdict":"allow"}]}}}"#,
            )],
        );
        let refs: Vec<(&str, &str)> = documents
            .iter()
            .map(|(r, b)| (r.as_str(), b.as_str()))
            .collect();
        let resolution = resolve(&sandbox(&spec), &[], &published(), &Fake::new(&refs))
            .await
            .expect("a mixin declaring every block resolves");
        let wire = on_the_wire(&resolution.contributions);
        let found: Vec<(lns_ipc::ContributionBlock, &str)> =
            wire.iter().map(|c| (c.block, c.key.as_str())).collect();
        for expected in [
            (lns_ipc::ContributionBlock::Tool, "node"),
            (lns_ipc::ContributionBlock::Mount, "/cache"),
            (lns_ipc::ContributionBlock::Port, "8080"),
            (lns_ipc::ContributionBlock::Credential, "SOME_TOKEN"),
            (
                lns_ipc::ContributionBlock::Egress,
                "allow api.some-provider.example",
            ),
        ] {
            assert!(
                found.contains(&expected),
                "§1.5 names every rule, mount, tool and credential, so a block that never reaches the wire is a line the disclosure cannot attribute; missing {expected:?} from {found:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_published_document_declaring_an_unpinned_mixin_refuses_before_it_is_fetched() {
        let source = Fake::new(&[]);
        let err = resolve(
            &sandbox(r#"{"image":"x:1","mixins":["ghcr.io/acme/obs:2"]}"#),
            &[],
            &published(),
            &source,
        )
        .await
        .expect_err("a tag a published document declares resolves differently for each consumer");
        assert!(
            format!("{err:#}").contains("must be digest-pinned"),
            "got: {err:#}"
        );
        assert!(
            source
                .fetched
                .lock()
                .expect("fetch log poisoned")
                .is_empty(),
            "the refusal has to land before the network, or a stranger's document chooses what this run pulls"
        );
    }

    #[tokio::test]
    async fn a_mixin_declaring_an_unpinned_mixin_refuses_the_graph_below_it() {
        let (spec, documents) = resolve_named(
            r#"{"image":"x:1","mixins":["a"]}"#,
            &[("a", r#"{"mixins":["ghcr.io/acme/obs:2"]}"#)],
        );
        let refs: Vec<(&str, &str)> = documents
            .iter()
            .map(|(r, b)| (r.as_str(), b.as_str()))
            .collect();
        let err = resolve(&sandbox(&spec), &[], &published(), &Fake::new(&refs))
            .await
            .expect_err("a pinned mixin naming a tag reopens exactly what pinning it closed");
        assert!(
            format!("{err:#}").contains("must be digest-pinned"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_document_with_no_mixins_is_returned_untouched() {
        let source = Fake::new(&[]);
        let original = sandbox(r#"{"image":"x:1"}"#);
        let out = resolve(&original, &[], &published(), &source)
            .await
            .unwrap();
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
        let out = resolve_if_a_sandbox(None, None, b"{}", &[], &published(), &source)
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
            &published(),
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
        let resolution = resolve(&sandbox(&spec), &[], &published(), &source)
            .await
            .expect("a diamond resolves");
        let shared = pinned("shared");
        assert_eq!(
            resolution.mixins,
            [pinned("a"), pinned("b"), shared.clone()],
            "the disclosure names every source that contributed, and a mixin two documents reach contributed once — naming it twice would tell a reader it merged twice"
        );
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
            &published(),
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
            &published(),
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
            &published(),
            &Fake::new(&[]),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}")
                .contains("a directory merges only into a document this machine read"),
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
        let err = resolve(b"{}", &[], &published(), &Fake::new(&[]))
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("reading the sandbox document"),
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
        let sandbox_only = resolve_if_a_sandbox(
            None,
            None,
            b"{}",
            &[pinned("m")],
            &published(),
            &Fake::new(&[]),
        )
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
            &published(),
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
            &published(),
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

    #[tokio::test]
    async fn one_relative_reference_under_two_mixins_names_two_directories() {
        let source = Fake::new(&[
            ("/work/a", r#"{"mixins":["./m"]}"#),
            ("/work/b", r#"{"mixins":["./m"]}"#),
            ("/work/a/m", r#"{"tools":["python@3.12"]}"#),
            ("/work/b/m", r#"{"tools":["node@22"]}"#),
        ]);
        let out = resolve(
            &sandbox(r#"{"image":"x:1","mixins":["./a","./b"]}"#),
            &[],
            &Locator::Directory(std::path::PathBuf::from("/work")),
            &source,
        )
        .await
        .expect("each mixin roots its own references");
        assert!(
            out.mixins.contains(&"/work/a/m".to_string())
                && out.mixins.contains(&"/work/b/m".to_string()),
            "`./m` means a different directory under each mixin that names it, so one identity for both would merge the wrong document; got {:?}",
            out.mixins
        );
    }

    #[tokio::test]
    async fn a_relative_directory_the_user_names_refuses_rather_than_guessing_a_root() {
        let err = resolve(
            &sandbox(r#"{"image":"x:1"}"#),
            &["./mixins/pg".to_string()],
            &Locator::Directory(std::path::PathBuf::from("/work")),
            &Fake::new(&[]),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("relative directory"),
            "only the caller knows the directory the user typed it from, so rooting it here would read some other directory with the same name; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_directory_the_user_names_is_read_from_this_machine() {
        let source = Fake::new(&[("/work/mixins/pg", r#"{"tools":["python@3.12"]}"#)]);
        let out = resolve(
            &sandbox(r#"{"image":"x:1"}"#),
            &["/work/mixins/./pg".to_string()],
            &Locator::Directory(std::path::PathBuf::from("/work")),
            &source,
        )
        .await
        .expect("a directory the user named beside their own definition is theirs to merge");
        assert_eq!(
            out.mixins,
            ["/work/mixins/pg"],
            "a directory has no digest, so the folded absolute path is the identity the disclosure names"
        );
    }

    #[tokio::test]
    async fn warming_a_pulled_mixin_reads_every_document_its_graph_reaches() {
        let source = Fake::new(&[
            (
                pinned("a").as_str(),
                r#"{"mixins":["REPLACED"]}"#.replace("REPLACED", &pinned("b")).as_str(),
            ),
            (pinned("b").as_str(), r#"{"tools":["node@22"]}"#),
        ]);
        let read = warm(&[pinned("a")], &source)
            .await
            .expect("a pulled mixin's graph is what makes it resolve offline later");
        assert_eq!(
            read, 2,
            "a pull that stopped at the mixin itself would still need the network the first time something merges it"
        );
    }

    #[test]
    fn a_definition_rooted_at_no_one_directory_is_refused_before_anything_is_read() {
        require_a_rooted_project_dir(std::path::Path::new("/work"))
            .expect("an absolute directory is what a caller reads a definition from");
        let err = require_a_rooted_project_dir(std::path::Path::new("work")).unwrap_err();
        assert!(
            format!("{err:#}").contains("is not an absolute directory"),
            "a relative root reads whichever directory of that name the service happens to sit beside, and roots that mixin's filesets and binds under it; got: {err:#}"
        );
    }

    #[test]
    fn a_reference_reaching_the_run_unpinned_is_refused_before_it_merges() {
        require_pinned_extras(&[pinned("m")])
            .expect("a pinned reference is what a preflight sends");
        let err = require_pinned_extras(&["ghcr.io/acme/obs-tools:2".to_string()]).unwrap_err();
        assert!(
            format!("{err:#}").contains("neither pinned nor rooted"),
            "a client that skipped the preflight would boot bytes nobody disclosed; got: {err:#}"
        );
    }

    #[test]
    fn a_merge_that_could_not_have_been_authored_refuses_the_run() {
        let document = sandbox(
            r#"{"image":"x:1","volumes":[{"name":"data","target":"/data"}],"filesets":[{"path":"./skills","mountPath":"/data"}]}"#,
        );
        let empty = SandboxSpec::default();
        let mixin = pinned("m");
        let sources = [
            Source {
                label: lns_artifact::merge::ROOT_LABEL,
                spec: &empty,
            },
            Source {
                label: &mixin,
                spec: &empty,
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
