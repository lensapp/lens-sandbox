use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use lns_artifact::merge::{MAX_DEPTH, Source, flatten, merge};
use lns_artifact::sandbox::{Definition, SandboxSpec};

/// What a document was read from, which is both its identity in the graph and what roots any directory it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Locator {
    Reference(String),
    /// The document on this machine a source was read from. A reference it declares joins onto the directory that document sits in, the way a reference keys on the digest a registry answered with.
    Local(std::path::PathBuf),
}

impl Locator {
    /// The one string every rule reads this source by: the reference the registry answered with, or the directory's absolute path.
    pub fn key(&self) -> String {
        match self {
            Locator::Reference(reference) => reference.clone(),
            Locator::Local(path) => path.display().to_string(),
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

/// Resolve one reference against the document that named it: a path the user typed is one this machine read whatever the run is, while a path a document declares roots at that document and so needs one this machine read.
fn locate(reference: &str, home: &Locator, the_user_named_it: bool) -> Result<Locator> {
    if !lns_artifact::sandbox::names_a_local_path(reference) {
        return Ok(Locator::Reference(reference.to_string()));
    }
    if the_user_named_it {
        if !std::path::Path::new(reference).is_absolute() {
            bail!(
                "mixin {reference} reached the run as a relative path; a run merges the absolute path its preflight showed"
            );
        }
        return Ok(Locator::Local(lns_artifact::sandbox::fold_path(
            std::path::Path::new(reference),
        )));
    }
    let Locator::Local(document) = home else {
        bail!(
            "mixin {reference} is a local path declared by a published document, and a consumer has no copy of the machine that wrote it; publish that mixin and name it by digest"
        );
    };
    let beside = document
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(reference);
    Ok(Locator::Local(lns_artifact::sandbox::fold_path(&beside)))
}

/// The graph every ordering rule decides against, keyed by the identity each source resolved to, with every reference that names a source translated to the same key.
struct Fetched {
    graph: BTreeMap<String, SandboxSpec>,
    pinned_roots: Vec<String>,
    pinned_extra: Vec<String>,
    pinned_local: Vec<String>,
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
    local: Option<(&[String], &Locator)>,
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
    let mut pinned_local = Vec::new();
    if let Some((references, decided_in)) = local {
        for reference in references {
            pinned_local.push(visit(&mut walk, reference, decided_in, false, source).await?);
        }
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
        pinned_local,
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
    // A local source keys on the document the path resolved to, exactly as a reference keys on the digest the registry answered with, so two spellings of one document are one source.
    let key = fetched.pinned;
    walk.visited.insert(locator.key(), key.clone());
    walk.keys.insert(seen, key.clone());
    if !walk.graph.contains_key(&key) {
        let mixin = lns_artifact::sandbox::parse_mixin(fetched.document.as_bytes())
            .with_context(|| format!("reading mixin {reference}"))?;
        let child_home = match locator {
            Locator::Local(_) => Locator::Local(std::path::PathBuf::from(&key)),
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
    local: Option<LocalSource>,
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
            authored_egress: lns_policy::Egress::default(),
            fileset_origins: Vec::new(),
        });
    }
    resolve(config_json, extra, home, source, local).await
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
    /// What every source but the directory's own decided about egress: the run folds the decisions file over this live, so an approval made mid-run applies and a rule deleted mid-run retracts.
    pub authored_egress: lns_policy::Egress,
    /// Which document ships each `path` fileset of the merged document, since the merged bytes no longer say.
    pub fileset_origins: Vec<lns_ipc::FilesetOrigin>,
}

/// What the directory decided, as the merge reads it: the label a disclosure names it by, and everything it decided.
#[derive(Debug)]
pub struct LocalSource {
    pub label: String,
    /// The directory this file was read from, which is what any directory it names roots at (§3.3.1) — not the directory the run happens to be about.
    home: Locator,
    spec: SandboxSpec,
}

impl LocalSource {
    /// Read the directory's decisions as one merge source; a file nobody has written has nothing to contribute.
    pub fn read(fetched: Option<FetchedMixin>, home: Locator) -> Result<Option<Self>> {
        let Some(fetched) = fetched else {
            return Ok(None);
        };
        let document = lns_artifact::sandbox::parse_mixin(fetched.document.as_bytes())
            .with_context(|| format!("reading {}", fetched.pinned))?;
        Ok(Some(Self {
            label: fetched.pinned,
            home,
            spec: document.spec,
        }))
    }

    fn decides_nothing(&self) -> bool {
        self.spec == SandboxSpec::default()
    }
}

/// Resolve a published definition and every mixin it layers on into the one document a run boots (`docs/sandbox-spec.md` §3.3), returned as a definition the rest of the plan path reads exactly as it reads an authored one.
pub async fn resolve<S: MixinSource>(
    config_json: &[u8],
    extra: &[String],
    home: &Locator,
    source: &S,
    local: Option<LocalSource>,
) -> Result<Resolution> {
    let def = lns_artifact::sandbox::parse(config_json).context("reading the sandbox document")?;
    let local = local.filter(|local| !local.decides_nothing());
    if def.spec.mixins.is_empty() && extra.is_empty() && local.is_none() {
        return Ok(Resolution {
            document: config_json.to_vec(),
            mixins: Vec::new(),
            pinned_extra: Vec::new(),
            contributions: lns_artifact::merge::own_egress(&def.spec),
            authored_egress: def.spec.egress.clone(),
            fileset_origins: fileset_origins_on_the_wire(
                &lns_artifact::merge::own_fileset_origins(&def.spec),
            ),
        });
    }
    let decided = local
        .as_ref()
        .map(|local| (local.spec.mixins.clone(), local.home.clone()));
    let fetched = collect(
        &def.spec.mixins,
        extra,
        decided
            .as_ref()
            .map(|(references, decided_in)| (references.as_slice(), decided_in)),
        home,
        source,
    )
    .await?;
    let mut root = def.spec.clone();
    root.mixins = fetched.pinned_roots;
    let local = local.map(|mut local| {
        local.spec.mixins = fetched.pinned_local.clone();
        local
    });
    let sources = flatten(
        &root,
        &fetched.pinned_extra,
        local.as_ref().map(|local| Source {
            label: &local.label,
            spec: &local.spec,
        }),
        &fetched.graph,
    )?;
    let merged = merge(&sources)?;
    let document = document(&def, &merged.spec)?;
    refuse_what_no_sandbox_could_be(&document, &sources)?;
    let authored_egress = authored_egress(&sources, local.is_some());
    let mixins = sources
        .iter()
        .skip(1)
        .map(|source| source.label.to_string())
        .collect();
    Ok(Resolution {
        document,
        mixins,
        pinned_extra: fetched.pinned_extra,
        contributions: merged.contributions,
        authored_egress,
        fileset_origins: fileset_origins_on_the_wire(&merged.fileset_origins),
    })
}

/// The egress every source but the directory's own decided, which is what the gate folds the live decisions file over; §8.1 puts that source last, so it is the one the fold leaves out.
fn authored_egress(sources: &[Source], the_directory_decided: bool) -> lns_policy::Egress {
    lns_artifact::merge::egress_of(&sources[..sources.len() - usize::from(the_directory_decided)])
}

/// Restate where each `path` fileset's files live for the wire, since lns-ipc names the document format without depending on the crate that merges it.
pub fn fileset_origins_on_the_wire(
    origins: &std::collections::BTreeMap<String, lns_artifact::merge::FilesetOrigin>,
) -> Vec<lns_ipc::FilesetOrigin> {
    origins
        .iter()
        .map(|(mount_path, origin)| lns_ipc::FilesetOrigin {
            mount_path: mount_path.clone(),
            source: origin.source.clone(),
            layer: origin.layer,
        })
        .collect()
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
    let fetched = collect(roots, &[], None, &home, source).await?;
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
        "name": &def.name,
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
                                r#"{{"apiVersion":"lns.run/v1","kind":"mixin","name":"some-mixin","spec":{spec}}}"#
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
            // A local path answers under the document it names, the same as the reader this stands in for; a directory holds `lns.yaml`.
            let pinned = match locator {
                Locator::Local(path) if path.extension().is_none() => {
                    path.join("lns.yaml").display().to_string()
                }
                _ => self.pins.get(&reference).cloned().unwrap_or(reference),
            };
            let document = self
                .documents
                .get(&pinned)
                .or_else(|| self.documents.get(&locator.key()))
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no such mixin here"))?;
            Ok(FetchedMixin { pinned, document })
        }
    }

    /// Every scenario here resolves a published document unless it says otherwise.
    /// A source read from a document on this machine, which is what a local reference joins onto.
    fn decided_in(document: &str) -> Locator {
        Locator::Local(std::path::PathBuf::from(document))
    }

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
        format!(r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{spec}}}"#)
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
        let out = resolve(&sandbox(&spec), &[], &published(), &Fake::new(&refs), None).await?;
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
        let resolution = resolve(&sandbox(&spec), &[], &published(), &Fake::new(&refs), None)
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
                r#"{"tools":["node@22"],"volumes":[{"type":"volume","name":"cache","target":"/cache"}],"ports":[{"container":8080}],"credentials":[{"envVar":"SOME_TOKEN","placeholder":"lns-placeholder-some","injections":[{"kind":"bearer_header","domain":"api.some-provider.example"}]}],"egress":{"http":[{"match":"api.some-provider.example","verdict":"allow"}]}}"#,
            )],
        );
        let refs: Vec<(&str, &str)> = documents
            .iter()
            .map(|(r, b)| (r.as_str(), b.as_str()))
            .collect();
        let resolution = resolve(&sandbox(&spec), &[], &published(), &Fake::new(&refs), None)
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
            None,
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
        let err = resolve(&sandbox(&spec), &[], &published(), &Fake::new(&refs), None)
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
        let out = resolve(&original, &[], &published(), &source, None)
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
        let out = resolve_if_a_sandbox(None, None, b"{}", &[], &published(), &source, None)
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
            None,
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
        let resolution = resolve(&sandbox(&spec), &[], &published(), &source, None)
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
            None,
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
            None,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains(&format!("reading mixin {reference}")),
            "a sandbox published where a mixin was expected must be refused by the reader, not merged as one; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_local_path_declared_by_a_published_document_refuses_the_run() {
        let err = resolve(
            &sandbox(r#"{"image":"x:1","mixins":["./mixins/postgres-tools/"]}"#),
            &[],
            &published(),
            &Fake::new(&[]),
            None,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("a consumer has no copy of the machine that wrote it"),
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
        let err = resolve(b"{}", &[], &published(), &Fake::new(&[]), None)
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
            None,
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
            None,
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
            None,
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
            &decided_in("/work/lns.yaml"),
            &source,
            None,
        )
        .await
        .expect("each mixin roots its own references");
        assert!(
            out.mixins.contains(&"/work/a/m/lns.yaml".to_string())
                && out.mixins.contains(&"/work/b/m/lns.yaml".to_string()),
            "`./m` means a different directory under each mixin that names it, so one identity for both would merge the wrong document; got {:?}",
            out.mixins
        );
    }

    #[tokio::test]
    async fn a_relative_path_the_user_names_refuses_rather_than_guessing_a_root() {
        let err = resolve(
            &sandbox(r#"{"image":"x:1"}"#),
            &["./mixins/pg".to_string()],
            &decided_in("/work/lns.yaml"),
            &Fake::new(&[]),
            None,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("reached the run as a relative path"),
            "only the caller knows the directory the user typed it from, so rooting it here would read some other directory with the same name; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_directory_the_user_names_is_read_from_this_machine() {
        let source = Fake::new(&[("/work/mixins/pg", r#"{"tools":["python@3.12"]}"#)]);
        let out = resolve(
            &sandbox(r#"{"image":"x:1"}"#),
            &["/work/mixins/./pg".to_string()],
            &decided_in("/work/lns.yaml"),
            &source,
            None,
        )
        .await
        .expect("a directory the user named beside their own definition is theirs to merge");
        assert_eq!(
            out.mixins,
            ["/work/mixins/pg/lns.yaml"],
            "a local source has no digest, so the document it resolved to is the identity the disclosure names"
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

    #[tokio::test]
    async fn a_sandbox_that_layers_on_nothing_still_attributes_the_egress_it_ships() {
        let out = resolve(
            &sandbox(
                r#"{"image":"x:1","egress":{"tcp":[{"match":"db.vendor.example:5432","verdict":"allow"}]}}"#,
            ),
            &[],
            &published(),
            &Fake::new(&[]),
            None,
        )
        .await
        .expect("a document with no mixins resolves to itself");
        assert_eq!(
            out.contributions
                .iter()
                .map(|c| (c.key.as_str(), c.source.as_str()))
                .collect::<Vec<_>>(),
            [(
                "allow db.vendor.example:5432",
                lns_artifact::merge::ROOT_LABEL
            )],
            "§1.5 has a run disclose every rule it will enforce, and a run that pulled no mixin enforces these"
        );
    }

    #[test]
    fn a_decisions_file_that_is_no_mixin_refuses_the_run_naming_the_file() {
        let err = LocalSource::read(
            Some(FetchedMixin {
                pinned: "lns-local-mixin.yaml".to_string(),
                document: r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"x","spec":{"image":"x:1"}}"#.to_string(),
            }),
            decided_in("/work/lns.yaml"),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("lns-local-mixin.yaml"),
            "a run that dropped a decisions file it could not read would boot without what the developer decided; got: {err:#}"
        );
    }

    #[test]
    fn a_directory_that_decided_only_destinations_is_still_a_source_of_the_merge() {
        let local = LocalSource::read(
            Some(FetchedMixin {
                pinned: "lns-local-mixin.yaml".to_string(),
                document: r#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"lns-local-mixin","spec":{"egress":{"http":[{"match":"api.example.test","verdict":"allow"}]}}}"#.to_string(),
            }),
            decided_in("/work/lns.yaml"),
        )
        .expect("a written file reads")
        .expect("a written file contributes");
        assert!(
            !local.decides_nothing(),
            "§3.3.2 merges this file's egress like any other source's, so a file holding only egress still decides something"
        );
    }

    /// The decisions file as the run reads it, holding whatever `spec` a scenario is about.
    fn decided(spec: &str) -> Option<LocalSource> {
        LocalSource::read(
            Some(FetchedMixin {
                pinned: "lns-local-mixin.yaml".to_string(),
                document: format!(
                    r#"{{"apiVersion":"lns.run/v1","kind":"mixin","name":"lns-local-mixin","spec":{spec}}}"#
                ),
            }),
            decided_in("/work"),
        )
        .expect("a written file reads")
    }

    #[tokio::test]
    async fn what_the_directory_decided_reaches_the_document_that_boots_attributed_to_the_file() {
        let out = resolve(
            &sandbox(
                r#"{"image":"x:1","egress":{"http":[{"match":"docs.some-vendor.example","verdict":"deny"}]}}"#,
            ),
            &[],
            &published(),
            &Fake::new(&[]),
            decided(
                r#"{"egress":{"http":[{"match":"docs.some-vendor.example","verdict":"allow"}]}}"#,
            ),
        )
        .await
        .expect("a directory that decided a destination resolves");
        let def = lns_artifact::sandbox::parse(&out.document).expect("the merge is a sandbox");
        assert_eq!(
            def.spec
                .egress
                .http
                .iter()
                .map(|rule| (rule.match_pattern.as_str(), rule.verdict))
                .collect::<Vec<_>>(),
            [
                ("docs.some-vendor.example", lns_policy::Verdict::Allow),
                ("docs.some-vendor.example", lns_policy::Verdict::Deny)
            ],
            "§8.1 puts the directory's decisions last, and §4.2 places a later source ahead, so the developer's allow is what a first-match gate reaches"
        );
        assert_eq!(
            out.contributions
                .iter()
                .map(|c| (c.key.as_str(), c.source.as_str()))
                .collect::<Vec<_>>(),
            [
                ("allow docs.some-vendor.example", "lns-local-mixin.yaml"),
                (
                    "deny docs.some-vendor.example",
                    lns_artifact::merge::ROOT_LABEL
                )
            ],
            "§1.5 has the disclosure name what each source contributed, so an allow beating a pulled deny is visible while it can still be refused"
        );
        assert_eq!(
            out.mixins,
            ["lns-local-mixin.yaml"],
            "a source the run merged is one the disclosure names"
        );
    }

    #[tokio::test]
    async fn the_gates_baseline_leaves_out_what_the_directory_decided() {
        let out = resolve(
            &sandbox(
                r#"{"image":"x:1","egress":{"http":[{"match":"api.some-vendor.example","verdict":"allow"}],"tcp":[{"match":"db.some-vendor.example:5432","verdict":"allow"}]}}"#,
            ),
            &[],
            &published(),
            &Fake::new(&[]),
            decided(
                r#"{"egress":{"http":[{"match":"docs.some-vendor.example","verdict":"allow"}],"tcp":[{"match":"cache.some-vendor.example:6379","verdict":"allow"}]}}"#,
            ),
        )
        .await
        .expect("a directory that decided a destination resolves");
        assert_eq!(
            out.authored_egress
                .http
                .iter()
                .map(|rule| rule.match_pattern.as_str())
                .collect::<Vec<_>>(),
            ["api.some-vendor.example"],
            "the run folds the live decisions file over this baseline, and a frozen copy of the file in it would outlive a rule the developer deletes mid-run"
        );
        assert_eq!(
            out.authored_egress
                .tcp
                .iter()
                .map(|rule| rule.match_pattern.as_str())
                .collect::<Vec<_>>(),
            ["db.some-vendor.example:5432"],
            "the raw table is folded live by the same rule as the inspected one"
        );
    }

    #[tokio::test]
    async fn a_run_that_layered_on_nothing_hands_the_gate_its_own_egress_as_the_baseline() {
        let out = resolve(
            &sandbox(
                r#"{"image":"x:1","egress":{"http":[{"match":"api.some-vendor.example","verdict":"allow"}]}}"#,
            ),
            &[],
            &published(),
            &Fake::new(&[]),
            None,
        )
        .await
        .expect("a document with no mixins resolves to itself");
        assert_eq!(
            out.authored_egress
                .http
                .iter()
                .map(|rule| rule.match_pattern.as_str())
                .collect::<Vec<_>>(),
            ["api.some-vendor.example"],
            "a run that resolved nothing still enforces what its own document said, so a baseline of nothing would drop it"
        );
    }

    #[tokio::test]
    async fn a_plain_image_hands_the_gate_no_baseline_at_all() {
        let out = resolve_if_a_sandbox(None, None, b"{}", &[], &published(), &Fake::new(&[]), None)
            .await
            .expect("an image has no document to merge");
        assert_eq!(
            out.authored_egress,
            lns_policy::Egress::default(),
            "an image declares no egress, so the directory's decisions govern it verbatim"
        );
    }

    #[tokio::test]
    async fn a_directory_the_decisions_file_names_is_read_beside_that_file() {
        let source = Fake::new(&[("/decisions/tools", r#"{"tools":["ripgrep@14"]}"#)]);
        let local = LocalSource::read(
            Some(FetchedMixin {
                pinned: "lns-local-mixin.yaml".to_string(),
                document: r#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"lns-local-mixin","spec":{"mixins":["./tools"]}}"#.to_string(),
            }),
            decided_in("/decisions/lns-local-mixin.yaml"),
        )
        .expect("a written file reads")
        .expect("a written file contributes");

        let out = resolve(
            &sandbox(r#"{"image":"x:1"}"#),
            &[],
            &published(),
            &source,
            Some(local),
        )
        .await
        .expect("a directory beside the decisions file is one this machine read");
        assert!(
            String::from_utf8_lossy(&out.document).contains("ripgrep@14"),
            "§3.3.1 roots a declared entry at the directory of the document that named it, and that document is the decisions file; got: {}",
            String::from_utf8_lossy(&out.document)
        );
    }

    #[tokio::test]
    async fn a_decisions_file_kept_outside_the_project_still_roots_its_own_references() {
        let source = Fake::new(&[("/decisions/tools", r#"{"tools":["ripgrep@14"]}"#)]);
        let local = LocalSource::read(
            Some(FetchedMixin {
                pinned: "dev.yaml".to_string(),
                document: r#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"dev","spec":{"mixins":["./tools"]}}"#.to_string(),
            }),
            decided_in("/decisions/lns-local-mixin.yaml"),
        )
        .expect("a written file reads")
        .expect("a written file contributes");

        let out = resolve(
            &sandbox(r#"{"image":"x:1"}"#),
            &[],
            &decided_in("/projdir/lns.yaml"),
            &source,
            Some(local),
        )
        .await
        .expect("a --policy file elsewhere still names directories beside itself");
        assert!(
            String::from_utf8_lossy(&out.document).contains("ripgrep@14"),
            "rooting at the project instead would merge whatever /projdir/tools happens to hold; got: {}",
            String::from_utf8_lossy(&out.document)
        );
    }

    #[tokio::test]
    async fn a_path_the_user_named_merges_into_a_published_run() {
        let source = Fake::new(&[("/work/mixins/debug", r#"{"tools":["ripgrep@14"]}"#)]);
        let out = resolve(
            &sandbox(r#"{"image":"x:1"}"#),
            &["/work/mixins/debug".to_string()],
            &published(),
            &source,
            None,
        )
        .await
        .expect("a path the user typed is one this machine read, whatever the sandbox is");
        assert!(
            String::from_utf8_lossy(&out.document).contains("ripgrep@14"),
            "a directory the developer names for their own run is theirs to name; only a published document may not name one, because its consumer has no copy; got: {}",
            String::from_utf8_lossy(&out.document)
        );
    }

    #[tokio::test]
    async fn a_mixin_a_document_path_names_roots_its_own_references_beside_the_document() {
        let source = Fake::new(&[
            ("/work/pg/lns.yaml", r#"{"mixins":["./sibling"]}"#),
            ("/work/pg/sibling", r#"{"tools":["python@3.12"]}"#),
        ]);
        let out = resolve(
            &sandbox(r#"{"image":"x:1","mixins":["./pg/lns.yaml"]}"#),
            &[],
            &decided_in("/work/lns.yaml"),
            &source,
            None,
        )
        .await
        .expect("a document names its own siblings beside itself");
        assert!(
            String::from_utf8_lossy(&out.document).contains("python@3.12"),
            "§3.3.1 roots a declared entry at the directory of the document that named it, and naming that document by its own path does not move it; got: {}",
            String::from_utf8_lossy(&out.document)
        );
    }

    #[tokio::test]
    async fn one_document_named_two_ways_is_one_source() {
        let source = Fake::new(&[("/work/pg/lns.yaml", r#"{"tools":["python@3.12"]}"#)]);
        let out = resolve(
            &sandbox(r#"{"image":"x:1","mixins":["./pg","./pg/lns.yaml"]}"#),
            &[],
            &decided_in("/work/lns.yaml"),
            &source,
            None,
        )
        .await
        .expect("both spellings resolve");
        assert_eq!(
            out.mixins.len(),
            1,
            "§3.3.2 has a source many documents name appear once, and two spellings of one path are not two sources; got {:?}",
            out.mixins
        );
    }
}
