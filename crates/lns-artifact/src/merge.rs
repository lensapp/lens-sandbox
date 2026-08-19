//! Resolution order and per-block merge for a sandbox and the mixins it layers on (`docs/sandbox-spec.md` §3.3.2), kept pure so the walk's depth limit, its cycle refusal and every merge rule are decided without a fetch.

use std::collections::BTreeMap;

use anyhow::{Result, bail};

use crate::sandbox::{FilesetEntry, SandboxSpec, Volume};
use crate::spec::Port;

/// One layer of a resolved sandbox, labelled with where it came from so a disclosure can attribute every entry it shows. It borrows the document the walk already holds, because a source list names an order rather than owning a copy of every spec in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source<'a> {
    pub label: &'a str,
    pub spec: &'a SandboxSpec,
}

/// The label the sandbox's own spec resolves under; every other source is labelled by the reference that named it.
pub const ROOT_LABEL: &str = "the sandbox";

/// The blocks a disclosure attributes, which are the ones §1.5 names plus `ports`, whose winners already have to answer for one another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    Credential,
    Tool,
    Mount,
    Port,
    Egress,
}

/// What one source decided, and what deciding it replaced, so a reader sees where every line of a resolved sandbox came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contribution {
    pub block: Block,
    pub key: String,
    pub source: String,
    /// What the entry says about itself, when it says anything (§4.2), so the disclosure explains an entry the way its own document does.
    pub note: Option<String>,
    pub displaced: Vec<Displaced>,
}

/// An entry a later source replaced, named as the reader would have seen it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Displaced {
    pub source: String,
    pub summary: String,
}

/// Where a merged `path` fileset came from: the source document that declared it, and its index among that document's own `path` entries — the coordinates §7 addresses its layer by, since a merged document's own order says nothing about any one artifact's layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesetOrigin {
    pub guest_path: String,
    pub source: String,
    pub layer_index: usize,
}

/// A merged sandbox and the record of which source decided each thing in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    pub spec: SandboxSpec,
    pub contributions: Vec<Contribution>,
    /// Every surviving `path` fileset, addressed by the source that declared it.
    pub fileset_origins: Vec<FilesetOrigin>,
}

/// The graph is walked this deep and refused beyond, so a chain nobody can read cannot stall a launch.
pub const MAX_DEPTH: usize = 5;

/// Flatten a sandbox and its mixins into the one ordered source list §3.3.2 merges: the sandbox first, then each of its `mixins` in order with that mixin's own `mixins` expanded right after it, then each extra reference in the order the user gave it, then the directory's own decisions last.
///
/// A mixin's own mixins come after it, so they beat it — a mixin that pulls in another is asking for that other's version of a shared setting. The local source is the exception: §8.1 puts it after every other source outright, so what it pulled merges before it.
/// A source many documents name appears once, at the last place it was named: an earlier appearance can decide nothing a later one does not, since either a source after it sets a key or the source itself sets it again.
pub fn flatten<'a>(
    root: &'a SandboxSpec,
    extra: &'a [String],
    local: Option<Source<'a>>,
    graph: &'a BTreeMap<String, SandboxSpec>,
) -> Result<Vec<Source<'a>>> {
    let reachable: Vec<String> = local
        .iter()
        .flat_map(|local| local.spec.mixins.iter().cloned())
        .chain(extra.iter().cloned())
        .collect();
    refuse_what_sits_out_of_reach(&root.mixins, &reachable, graph)?;
    let mut sources = Vec::with_capacity(2 + graph.len());
    let mut seen = std::collections::BTreeSet::new();
    // Pushed first because the list is built in reverse, so this is what lands last.
    if let Some(local) = local {
        let own = &local.spec.mixins;
        sources.push(local);
        expand(own, graph, &mut seen, &mut sources)?;
    }
    expand(extra, graph, &mut seen, &mut sources)?;
    expand(&root.mixins, graph, &mut seen, &mut sources)?;
    sources.push(Source {
        label: ROOT_LABEL,
        spec: root,
    });
    sources.reverse();
    Ok(sources)
}

/// One reference to reach, and the source to emit once everything it names has been.
enum Step<'a> {
    Reach(&'a str),
    Emit(&'a str, &'a SandboxSpec),
}

/// Walk the references in reverse, each one's own mixins before it, and keep the first arrival — which is the last place the forward order names it. The walk carries its own stack, because the graph's longest chain is bounded by how many documents were fetched and not by [`MAX_DEPTH`].
fn expand<'a>(
    references: &'a [String],
    graph: &'a BTreeMap<String, SandboxSpec>,
    seen: &mut std::collections::BTreeSet<&'a str>,
    sources: &mut Vec<Source<'a>>,
) -> Result<()> {
    let mut trail: Vec<&'a str> = Vec::new();
    let mut ancestors: std::collections::BTreeSet<&'a str> = std::collections::BTreeSet::new();
    let mut stack: Vec<Step<'a>> = references.iter().map(|r| Step::Reach(r)).collect();
    while let Some(step) = stack.pop() {
        let key = match step {
            Step::Emit(key, spec) => {
                trail.pop();
                ancestors.remove(key);
                sources.push(Source { label: key, spec });
                continue;
            }
            Step::Reach(key) => key,
        };
        if ancestors.contains(key) {
            bail!(
                "mixin {key} is reachable from itself ({} → {key}); a cycle cannot resolve",
                trail.join(" → ")
            );
        }
        let Some(spec) = graph.get(key) else {
            bail!("mixin {key} is not resolved; it has to be pulled before the merge");
        };
        if !seen.insert(key) {
            continue;
        }
        trail.push(key);
        ancestors.insert(key);
        stack.push(Step::Emit(key, spec));
        stack.extend(spec.mixins.iter().map(|child| Step::Reach(child)));
    }
    Ok(())
}

/// Refuse a source no walk of depth [`MAX_DEPTH`] reaches, measured by shortest path so a mixin one document names deep does not refuse a graph another names shallow. It is the same frontier the fetch walks, so what a fetch pulled is what resolves.
fn refuse_what_sits_out_of_reach(
    declared: &[String],
    extra: &[String],
    graph: &BTreeMap<String, SandboxSpec>,
) -> Result<()> {
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut frontier: Vec<&str> = declared.iter().chain(extra).map(String::as_str).collect();
    let mut depth = 1;
    loop {
        frontier.retain(|reference| !seen.contains(reference));
        let Some(first) = frontier.first() else {
            return Ok(());
        };
        if depth > MAX_DEPTH {
            bail!(
                "mixin {first} sits deeper than {MAX_DEPTH} mixins from the sandbox; refusing to resolve further"
            );
        }
        let mut next = Vec::new();
        for reference in std::mem::take(&mut frontier) {
            if seen.insert(reference)
                && let Some(spec) = graph.get(reference)
            {
                next.extend(spec.mixins.iter().map(String::as_str));
            }
        }
        frontier = next;
        depth += 1;
    }
}

/// Merge an ordered source list per §3.3.2: the last source to say something about a thing wins, and every keyed block unions by its key.
pub fn merge(sources: &[Source]) -> Result<Merged> {
    let mut spec = SandboxSpec::default();

    for source in sources {
        spec.env
            .extend(source.spec.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        // Only a sandbox can carry the blocks that describe one launch; a mixin declaring one is refused at parse.
        if !source.spec.image.is_empty() {
            spec.image = source.spec.image.clone();
        }
        take_over(&mut spec.command, &source.spec.command);
        take_over(&mut spec.workdir, &source.spec.workdir);
        take_over(&mut spec.user, &source.spec.user);
        take_over(&mut spec.resources, &source.spec.resources);
        if !source.spec.connectors.is_empty() {
            spec.connectors = source.spec.connectors.clone();
        }
    }

    let mut contributions = Vec::new();
    spec.credentials = fold(
        sources,
        |s| &s.credentials,
        |c| c.env_var.clone(),
        |c| c.env_var.clone(),
        Block::Credential,
        &mut contributions,
    );
    spec.tools = fold(
        sources,
        |s| &s.tools,
        |t| tool_name(t).to_string(),
        Clone::clone,
        Block::Tool,
        &mut contributions,
    );
    let mut fileset_origins = Vec::new();
    (spec.volumes, spec.filesets) = fold_mounts(sources, &mut contributions, &mut fileset_origins);
    let ports = fold_labelled(
        sources,
        |s| &s.ports,
        |p| p.container.to_string(),
        |p| p.container.to_string(),
    );
    refuse_a_host_port_two_sources_publish(&ports)?;
    for won in &ports {
        contributions.push(won.contribution(Block::Port));
    }
    spec.ports = ports.into_iter().map(|won| won.item).collect();

    spec.egress = egress_of(sources);
    for source in sources.iter().rev() {
        // Egress is a union rather than a keyed replacement, so every entry is attributed and none displaces another.
        for rule in source.spec.egress.http.iter() {
            contributions.push(egress_contribution(
                rule.verdict,
                &rule.match_pattern,
                rule.description.as_deref(),
                source.label,
            ));
        }
        for rule in source.spec.egress.tcp.iter() {
            contributions.push(egress_contribution(
                rule.verdict,
                &rule.match_pattern,
                rule.description.as_deref(),
                source.label,
            ));
        }
    }

    // The mixins are what produced this document, so the resolved one declares none of its own.
    spec.mixins = Vec::new();
    Ok(Merged {
        spec,
        contributions,
        fileset_origins,
    })
}

/// The egress an ordered source list decides: a union with each later source's entries ahead of an earlier one's, so the latest entry matching a destination is the one that decides (§4.2).
pub fn egress_of(sources: &[Source]) -> lns_policy::Egress {
    let mut egress = lns_policy::Egress::default();
    for source in sources.iter().rev() {
        egress.http.extend(source.spec.egress.http.iter().cloned());
        egress.tcp.extend(source.spec.egress.tcp.iter().cloned());
    }
    egress
}

/// Where a document's own `path` filesets came from when nothing merges into it, so a run that layered on nothing still addresses each one's layer by its source.
pub fn own_fileset_origins(spec: &SandboxSpec) -> Vec<FilesetOrigin> {
    path_filesets(spec)
        .map(|(layer_index, fileset)| FilesetOrigin {
            guest_path: fileset.guest_path.clone(),
            source: ROOT_LABEL.to_string(),
            layer_index,
        })
        .collect()
}

/// Every `path` fileset a document declares, numbered in declaration order — the numbering publish packs layers in (§6).
pub fn path_filesets(spec: &SandboxSpec) -> impl Iterator<Item = (usize, &FilesetEntry)> {
    spec.filesets
        .iter()
        .filter(|fileset| fileset.path.is_some())
        .enumerate()
}

/// What a document's own egress contributes when nothing merges into it, so a run that layered on nothing still discloses every rule it will enforce (§1.5).
pub fn own_egress(spec: &SandboxSpec) -> Vec<Contribution> {
    let http = spec.egress.http.iter().map(|rule| {
        egress_contribution(
            rule.verdict,
            &rule.match_pattern,
            rule.description.as_deref(),
            ROOT_LABEL,
        )
    });
    let tcp = spec.egress.tcp.iter().map(|rule| {
        egress_contribution(
            rule.verdict,
            &rule.match_pattern,
            rule.description.as_deref(),
            ROOT_LABEL,
        )
    });
    http.chain(tcp).collect()
}

/// A rule is its verdict and its pattern together, because the verdict is the half that decides and two sources may disagree about one destination.
fn egress_contribution(
    verdict: lns_policy::Verdict,
    match_pattern: &str,
    note: Option<&str>,
    source: &str,
) -> Contribution {
    let verdict = match verdict {
        lns_policy::Verdict::Allow => "allow",
        lns_policy::Verdict::Deny => "deny",
    };
    Contribution {
        block: Block::Egress,
        key: format!("{verdict} {match_pattern}"),
        source: source.to_string(),
        note: note.map(str::to_string),
        displaced: Vec::new(),
    }
}

/// A mount, whichever block claimed it: one target is one keyspace across `volumes` and `filesets`, so a later source's claim of either kind displaces an earlier source's claim of either kind. A packed fileset carries its index among its own document's `path` entries, because that is what names its layer.
#[derive(Clone)]
enum Mount {
    Volume(Volume),
    Fileset(FilesetEntry, Option<usize>),
}

impl Mount {
    /// How a replaced mount reads back to the developer: the kind that held the target, and what it mounted there.
    fn summary(&self) -> String {
        match self {
            Mount::Volume(volume) => format!("volume {}", volume.source()),
            Mount::Fileset(..) => "fileset".to_string(),
        }
    }
}

fn fold_mounts(
    sources: &[Source],
    into: &mut Vec<Contribution>,
    origins: &mut Vec<FilesetOrigin>,
) -> (Vec<Volume>, Vec<FilesetEntry>) {
    let mut order: Vec<String> = Vec::new();
    let mut winners: BTreeMap<String, Won<Mount>> = BTreeMap::new();
    for source in sources {
        for volume in &source.spec.volumes {
            claim(
                &mut order,
                &mut winners,
                volume.target.clone(),
                Mount::Volume(volume.clone()),
                source.label,
                Mount::summary,
            );
        }
        let mut layers = 0;
        for fileset in &source.spec.filesets {
            let layer_index = fileset.path.as_ref().map(|_| {
                layers += 1;
                layers - 1
            });
            claim(
                &mut order,
                &mut winners,
                fileset.guest_path.clone(),
                Mount::Fileset(fileset.clone(), layer_index),
                source.label,
                Mount::summary,
            );
        }
    }
    let mut volumes = Vec::new();
    let mut filesets = Vec::new();
    for won in order
        .into_iter()
        .filter_map(|target| winners.remove(&target))
    {
        into.push(won.contribution(Block::Mount));
        let source = won.source;
        match won.item {
            Mount::Volume(volume) => volumes.push(volume),
            Mount::Fileset(fileset, layer_index) => {
                if let Some(layer_index) = layer_index {
                    origins.push(FilesetOrigin {
                        guest_path: fileset.guest_path.clone(),
                        source,
                        layer_index,
                    });
                }
                filesets.push(fileset);
            }
        }
    }
    (volumes, filesets)
}

fn take_over<T: Clone>(into: &mut Option<T>, from: &Option<T>) {
    if from.is_some() {
        *into = from.clone();
    }
}

/// The tool a `name@version` entry names, so a later source's version of one tool replaces an earlier source's rather than joining it.
fn tool_name(entry: &str) -> &str {
    entry.split_once('@').map_or(entry, |(name, _)| name)
}

/// A key's winner, the source that decided it, and every entry that decision replaced.
struct Won<T> {
    item: T,
    key: String,
    source: String,
    displaced: Vec<Displaced>,
}

impl<T> Won<T> {
    fn contribution(&self, block: Block) -> Contribution {
        Contribution {
            block,
            key: self.key.clone(),
            source: self.source.clone(),
            note: None,
            displaced: self.displaced.clone(),
        }
    }
}

/// Union one keyed block across every source, last-wins, in first-appearance order of the key so a reader sees the document's own entries before what a mixin added.
fn fold<T: Clone>(
    sources: &[Source],
    items: fn(&SandboxSpec) -> &Vec<T>,
    key: fn(&T) -> String,
    summary: fn(&T) -> String,
    block: Block,
    into: &mut Vec<Contribution>,
) -> Vec<T> {
    fold_labelled(sources, items, key, summary)
        .into_iter()
        .map(|won| {
            into.push(won.contribution(block));
            won.item
        })
        .collect()
}

/// [`fold`], keeping each winner whole, for a block whose winners still have to answer for one another.
fn fold_labelled<T: Clone>(
    sources: &[Source],
    items: fn(&SandboxSpec) -> &Vec<T>,
    key: fn(&T) -> String,
    summary: fn(&T) -> String,
) -> Vec<Won<T>> {
    let mut order: Vec<String> = Vec::new();
    let mut winners: BTreeMap<String, Won<T>> = BTreeMap::new();
    for source in sources {
        for item in items(source.spec) {
            claim(
                &mut order,
                &mut winners,
                key(item),
                item.clone(),
                source.label,
                summary,
            );
        }
    }
    order
        .into_iter()
        .filter_map(|key| winners.remove(&key))
        .collect()
}

/// Record one source's claim of a key, keeping what it replaced so the disclosure can say what changed rather than only what won.
fn claim<T: Clone>(
    order: &mut Vec<String>,
    winners: &mut BTreeMap<String, Won<T>>,
    key: String,
    item: T,
    source: &str,
    summary: fn(&T) -> String,
) {
    match winners.get_mut(&key) {
        Some(won) => {
            won.displaced.push(Displaced {
                source: std::mem::replace(&mut won.source, source.to_string()),
                summary: summary(&std::mem::replace(&mut won.item, item)),
            });
        }
        None => {
            order.push(key.clone());
            winners.insert(
                key.clone(),
                Won {
                    item,
                    key,
                    source: source.to_string(),
                    displaced: Vec::new(),
                },
            );
        }
    }
}

/// A host port is one socket, so precedence cannot settle two sources publishing onto it: keeping the later mapping would silently unpublish a port the sandbox declared, and the resolved document would claim a host port twice — which `sandbox::parse` itself refuses.
fn refuse_a_host_port_two_sources_publish(ports: &[Won<Port>]) -> Result<()> {
    let mut held: BTreeMap<i64, &str> = BTreeMap::new();
    for won in ports {
        let Some(host) = won.item.host else { continue };
        let label = won.source.as_str();
        if let Some(holder) = held.insert(host, label) {
            bail!("{holder} and {label} both publish host port {host}; one of them has to move it");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(json: &str) -> SandboxSpec {
        serde_json::from_str(json).expect("valid spec fixture")
    }

    fn graph(entries: &[(&str, &str)]) -> BTreeMap<String, SandboxSpec> {
        entries
            .iter()
            .map(|(name, json)| ((*name).to_string(), spec(json)))
            .collect()
    }

    fn labels<'a>(sources: &[Source<'a>]) -> Vec<&'a str> {
        sources.iter().map(|s| s.label).collect()
    }

    /// Four levels, five mixins each, every one naming all of the next level — the shape a stranger's artifact can hand the service.
    fn wide_dag() -> (SandboxSpec, BTreeMap<String, SandboxSpec>) {
        let level = |n: usize| -> Vec<String> { (0..5).map(|i| format!("l{n}-{i}")).collect() };
        let names = |n: usize| -> String {
            serde_json::to_string(&level(n)).expect("a reference list serializes")
        };
        let mut graph = BTreeMap::new();
        for depth in 1..=4 {
            let body = if depth == 4 {
                r#"{"tools":["node@22"]}"#.to_string()
            } else {
                format!(r#"{{"mixins":{}}}"#, names(depth + 1))
            };
            for key in level(depth) {
                graph.insert(key, spec(&body));
            }
        }
        (
            spec(&format!(r#"{{"image":"x:1","mixins":{}}}"#, names(1))),
            graph,
        )
    }

    /// Every source is named by the sandbox itself, so each sits one step away and no depth rule refuses the graph — and each also names the one before it, so the walk has a chain as long as the list to get down.
    fn a_chain_the_sandbox_names_flat(
        length: usize,
    ) -> (SandboxSpec, BTreeMap<String, SandboxSpec>) {
        let names: Vec<String> = (0..length).map(|i| format!("m{i}")).collect();
        let graph = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let body = match i {
                    0 => "{}".to_string(),
                    _ => format!(r#"{{"mixins":["m{}"]}}"#, i - 1),
                };
                (name.clone(), spec(&body))
            })
            .collect();
        let listed = serde_json::to_string(&names).expect("a reference list serializes");
        (
            spec(&format!(r#"{{"image":"x:1","mixins":{listed}}}"#)),
            graph,
        )
    }

    #[test]
    fn a_long_chain_of_sources_costs_no_stack() {
        let (root, graph) = a_chain_the_sandbox_names_flat(5_000);
        let walked = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || flatten(&root, &[], None, &graph).map(|sources| sources.len()))
            .expect("the walk gets its own thread")
            .join()
            .expect(
                "a walk that takes a frame per source overflows the stack, and that aborts the service rather than refusing one run",
            );
        assert_eq!(
            walked.expect("every reference resolves"),
            5_001,
            "the depth rule measures the shortest path, so a chain the sandbox names flat is legal however long it is"
        );
    }

    #[test]
    fn a_mixin_many_documents_reach_is_one_source_and_not_one_per_path() {
        let (root, graph) = wide_dag();
        let sources = flatten(&root, &[], None, &graph).expect("every reference resolves");
        assert_eq!(
            labels(&sources).len(),
            1 + graph.len(),
            "a source list built per path grows as a power of the fan-out, so this twenty-document graph would cost the service hundreds of spec copies to merge one document"
        );
    }

    #[test]
    fn the_source_list_walks_the_document_then_its_mixins_then_the_flags() {
        let root = spec(r#"{"image":"x:1","mixins":["own"]}"#);
        let extra = ["first".to_string(), "second".to_string()];
        let graph = graph(&[
            ("own", r#"{}"#),
            ("first", r#"{"mixins":["firsts-own"]}"#),
            ("firsts-own", r#"{}"#),
            ("second", r#"{}"#),
        ]);
        let sources = flatten(&root, &extra, None, &graph).expect("every reference resolves");
        assert_eq!(
            labels(&sources),
            [ROOT_LABEL, "own", "first", "firsts-own", "second"],
            "a mixin's own mixins follow it, and the user's flags are appended last, so each beats what came before it"
        );
    }

    #[test]
    fn the_source_list_ends_with_the_local_mixin_after_every_flag() {
        let root = spec(r#"{"image":"x:1","mixins":["own"]}"#);
        let extra = ["flag".to_string()];
        let local = spec(r#"{"tools":["ripgrep@14"]}"#);
        let graph = graph(&[("own", r#"{}"#), ("flag", r#"{}"#)]);
        let sources = flatten(
            &root,
            &extra,
            Some(Source {
                label: "lns-local-mixin.yaml",
                spec: &local,
            }),
            &graph,
        )
        .expect("every reference resolves");
        assert_eq!(
            labels(&sources),
            [ROOT_LABEL, "own", "flag", "lns-local-mixin.yaml"],
            "the developer's own decisions are last, so nothing they pulled can overrule them (docs/sandbox-spec.md §8.1)"
        );
    }

    #[test]
    fn a_local_mixins_own_mixins_are_expanded_before_it() {
        let root = spec(r#"{"image":"x:1"}"#);
        let local = spec(r#"{"mixins":["locals-own"]}"#);
        let graph = graph(&[("locals-own", r#"{}"#)]);
        let sources = flatten(
            &root,
            &[],
            Some(Source {
                label: "lns-local-mixin.yaml",
                spec: &local,
            }),
            &graph,
        )
        .expect("every reference resolves");
        assert_eq!(
            labels(&sources),
            [ROOT_LABEL, "locals-own", "lns-local-mixin.yaml"],
            "§3.3.2 puts a mixin's own mixins after it, but §8.1 says the local one is last outright, and what it pulled is still something pulled"
        );
    }

    #[test]
    fn a_chain_five_deep_resolves_and_a_sixth_refuses() {
        let chain: Vec<(&str, &str)> = vec![
            ("m1", r#"{"mixins":["m2"]}"#),
            ("m2", r#"{"mixins":["m3"]}"#),
            ("m3", r#"{"mixins":["m4"]}"#),
            ("m4", r#"{"mixins":["m5"]}"#),
            ("m5", r#"{}"#),
        ];
        let root = spec(r#"{"image":"x:1","mixins":["m1"]}"#);
        assert_eq!(flatten(&root, &[], None, &graph(&chain)).unwrap().len(), 6);

        let mut deeper = chain;
        deeper[4] = ("m5", r#"{"mixins":["m6"]}"#);
        deeper.push(("m6", r#"{}"#));
        let err = flatten(&root, &[], None, &graph(&deeper)).unwrap_err();
        assert!(
            format!("{err:#}").contains("deeper than 5 mixins"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_mixin_reachable_from_itself_refuses_the_resolution() {
        let root = spec(r#"{"image":"x:1","mixins":["a"]}"#);
        let err = flatten(
            &root,
            &[],
            None,
            &graph(&[("a", r#"{"mixins":["b"]}"#), ("b", r#"{"mixins":["a"]}"#)]),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("reachable from itself"),
            "a cycle would resolve forever, and a digest-pinned reference makes one detectable by identity; got: {err:#}"
        );
    }

    #[test]
    fn a_reference_nothing_pulled_refuses_rather_than_being_skipped() {
        let root = spec(r#"{"image":"x:1","mixins":["absent"]}"#);
        let err = flatten(&root, &[], None, &BTreeMap::new()).unwrap_err();
        assert!(
            format!("{err:#}").contains("is not resolved"),
            "skipping it would boot a sandbox without what its own document declared; got: {err:#}"
        );
    }

    fn sources(entries: &[(&str, &str)]) -> Vec<(String, SandboxSpec)> {
        entries
            .iter()
            .map(|(label, json)| ((*label).to_string(), spec(json)))
            .collect()
    }

    fn as_sources(owned: &[(String, SandboxSpec)]) -> Vec<Source<'_>> {
        owned
            .iter()
            .map(|(label, spec)| Source { label, spec })
            .collect()
    }

    fn merged(owned: &[(String, SandboxSpec)]) -> SandboxSpec {
        merge(&as_sources(owned))
            .expect("these sources resolve")
            .spec
    }

    fn contributions(owned: &[(String, SandboxSpec)]) -> Vec<Contribution> {
        merge(&as_sources(owned))
            .expect("these sources resolve")
            .contributions
    }

    fn contribution(owned: &[(String, SandboxSpec)], block: Block, key: &str) -> Contribution {
        contributions(owned)
            .into_iter()
            .find(|c| c.block == block && c.key == key)
            .unwrap_or_else(|| panic!("no {block:?} contribution for {key}"))
    }

    #[test]
    fn a_tool_a_later_source_redefines_names_the_source_and_what_it_replaced() {
        let found = contribution(
            &sources(&[
                (ROOT_LABEL, r#"{"tools":["node@20","python@3.12"]}"#),
                ("obs", r#"{"tools":["node@22"]}"#),
            ]),
            Block::Tool,
            "node",
        );
        assert_eq!(found.source, "obs");
        assert_eq!(
            found.displaced,
            [Displaced {
                source: ROOT_LABEL.to_string(),
                summary: "node@20".to_string()
            }],
            "a tool the developer read in the document is gone from the run, so the disclosure has to say which source took it and what it took it from"
        );
    }

    #[test]
    fn a_source_nothing_replaced_is_attributed_with_an_empty_displacement() {
        let found = contribution(
            &sources(&[
                (ROOT_LABEL, r#"{"tools":["node@20"]}"#),
                ("obs", r#"{"tools":["ripgrep@14"]}"#),
            ]),
            Block::Tool,
            "ripgrep",
        );
        assert_eq!(found.source, "obs");
        assert!(
            found.displaced.is_empty(),
            "a mixin that adds a tool replaced nothing, and saying otherwise would invent a conflict"
        );
    }

    #[test]
    fn a_mount_records_the_kind_that_held_the_target_before_it() {
        let found = contribution(
            &sources(&[
                (
                    ROOT_LABEL,
                    r#"{"volumes":[{"type":"volume","name":"cache","target":"/cache"}]}"#,
                ),
                (
                    "obs",
                    r#"{"filesets":[{"inline":{"a.md":"x"},"guestPath":"/cache"}]}"#,
                ),
            ]),
            Block::Mount,
            "/cache",
        );
        assert_eq!(found.source, "obs");
        assert_eq!(
            found.displaced,
            [Displaced {
                source: ROOT_LABEL.to_string(),
                summary: "volume cache".to_string()
            }],
            "one target is one keyspace, so a fileset taking a volume's target has to read as the replacement it is"
        );
    }

    #[test]
    fn a_credential_a_mixin_contributes_names_the_mixin() {
        let found = contribution(
            &sources(&[
                (ROOT_LABEL, r#"{"image":"x:1"}"#),
                (
                    "obs",
                    r#"{"credentials":[{"envVar":"SOME_TOKEN","placeholder":"lns-placeholder-some","injections":[{"kind":"bearer_header","domain":"api.some-provider.example"}]}]}"#,
                ),
            ]),
            Block::Credential,
            "SOME_TOKEN",
        );
        assert_eq!(
            found.source, "obs",
            "a credential the sandbox never asked for is exactly what a reader has to be able to trace to its mixin"
        );
    }

    const EXPLAINED_EGRESS: &str = r#"{"egress":{"http":[{"match":"api.base.example","verdict":"allow","description":"approved during a run"}],"tcp":[{"match":"db.base.example:5432","verdict":"allow","description":"the project database"}]}}"#;

    #[test]
    fn an_egress_entry_carries_what_its_own_rule_says_about_itself() {
        // §4.2 shows a description wherever the entry is explained, and the disclosure before boot is where a reader meets the entry.
        let owned = sources(&[(ROOT_LABEL, EXPLAINED_EGRESS)]);
        let noted = |key: &str| {
            contribution(&owned, Block::Egress, key)
                .note
                .unwrap_or_default()
        };
        assert_eq!(noted("allow api.base.example"), "approved during a run");
        assert_eq!(noted("allow db.base.example:5432"), "the project database");
    }

    #[test]
    fn a_document_that_layers_on_nothing_still_carries_what_its_rules_explain() {
        // The uncomposed run reports through own_egress rather than the merge, so an explanation dropped here is one no reader of that run ever sees.
        let explained: Vec<(String, Option<String>)> = own_egress(&spec(EXPLAINED_EGRESS))
            .into_iter()
            .map(|c| (c.key, c.note))
            .collect();
        assert_eq!(
            explained,
            [
                (
                    "allow api.base.example".to_string(),
                    Some("approved during a run".to_string())
                ),
                (
                    "allow db.base.example:5432".to_string(),
                    Some("the project database".to_string())
                ),
            ]
        );
    }

    #[test]
    fn every_egress_entry_is_attributed_and_none_displaces_another() {
        let found = contributions(&sources(&[
            (
                ROOT_LABEL,
                r#"{"egress":{"http":[{"match":"api.base.example","verdict":"allow"}]}}"#,
            ),
            (
                "obs",
                r#"{"egress":{"http":[{"match":"api.obs.example","verdict":"allow"}]}}"#,
            ),
        ]));
        let rules: Vec<(&str, &str)> = found
            .iter()
            .filter(|c| c.block == Block::Egress)
            .map(|c| (c.key.as_str(), c.source.as_str()))
            .collect();
        assert_eq!(
            rules,
            [
                ("allow api.obs.example", "obs"),
                ("allow api.base.example", ROOT_LABEL)
            ],
            "egress is a union in decision order, so every entry keeps its own source and nothing is reported as replaced"
        );
        assert!(
            found
                .iter()
                .filter(|c| c.block == Block::Egress)
                .all(|c| c.displaced.is_empty())
        );
    }

    #[test]
    fn a_third_source_records_both_the_sources_it_displaced() {
        let found = contribution(
            &sources(&[
                (ROOT_LABEL, r#"{"tools":["node@20"]}"#),
                ("obs", r#"{"tools":["node@21"]}"#),
                ("pg", r#"{"tools":["node@22"]}"#),
            ]),
            Block::Tool,
            "node",
        );
        assert_eq!(found.source, "pg");
        assert_eq!(
            found.displaced,
            [
                Displaced {
                    source: ROOT_LABEL.to_string(),
                    summary: "node@20".to_string()
                },
                Displaced {
                    source: "obs".to_string(),
                    summary: "node@21".to_string()
                }
            ],
            "reporting only the last loser would hide that the document's own version lost too"
        );
    }

    #[test]
    fn env_unions_by_key_and_the_last_source_to_set_one_wins() {
        let merged = merged(&sources(&[
            ("base", r#"{"env":{"MODE":"research","KEEP":"1"}}"#),
            ("later", r#"{"env":{"MODE":"strict"}}"#),
        ]));
        assert_eq!(merged.env.get("MODE").map(String::as_str), Some("strict"));
        assert_eq!(merged.env.get("KEEP").map(String::as_str), Some("1"));
    }

    fn origins(owned: &[(String, SandboxSpec)]) -> Vec<FilesetOrigin> {
        merge(&as_sources(owned))
            .expect("these sources resolve")
            .fileset_origins
    }

    #[test]
    fn a_packed_fileset_is_addressed_by_its_own_documents_layer_order() {
        let found = origins(&sources(&[
            (
                ROOT_LABEL,
                r#"{"image":"x:1","filesets":[{"inline":{"a.md":"x"},"guestPath":"/notes"},{"path":"./skills","guestPath":"/skills"},{"path":"./hooks","guestPath":"/hooks"}]}"#,
            ),
            (
                "obs",
                r#"{"filesets":[{"path":"./tools","guestPath":"/tools"}]}"#,
            ),
        ]));
        assert_eq!(
            found,
            [
                FilesetOrigin {
                    guest_path: "/skills".into(),
                    source: ROOT_LABEL.into(),
                    layer_index: 0
                },
                FilesetOrigin {
                    guest_path: "/hooks".into(),
                    source: ROOT_LABEL.into(),
                    layer_index: 1
                },
                FilesetOrigin {
                    guest_path: "/tools".into(),
                    source: "obs".into(),
                    layer_index: 0
                },
            ],
            "a layer is named by (source document, index among that document's own path entries) — inline entries carry no layer, and a mixin's first path entry is its own artifact's first layer, not the merged document's third"
        );
    }

    #[test]
    fn a_fileset_a_later_source_replaced_is_addressed_by_the_source_that_won() {
        let found = origins(&sources(&[
            (
                ROOT_LABEL,
                r#"{"image":"x:1","filesets":[{"path":"./skills","guestPath":"/skills"}]}"#,
            ),
            (
                "obs",
                r#"{"filesets":[{"path":"./better-skills","guestPath":"/skills"}]}"#,
            ),
        ]));
        assert_eq!(
            found,
            [FilesetOrigin {
                guest_path: "/skills".into(),
                source: "obs".into(),
                layer_index: 0
            }],
            "pulling the layer from the displaced source's artifact would mount files the run does not declare"
        );
    }

    #[test]
    fn a_fileset_a_volume_took_the_target_from_leaves_no_layer_to_pull() {
        assert!(
            origins(&sources(&[
                (
                    ROOT_LABEL,
                    r#"{"image":"x:1","filesets":[{"path":"./skills","guestPath":"/skills"}]}"#,
                ),
                (
                    "obs",
                    r#"{"volumes":[{"type":"volume","name":"cache","target":"/skills"}]}"#,
                ),
            ]))
            .is_empty(),
            "the fileset lost the target, so materializing its layer would write files under a volume the run mounts instead"
        );
    }

    #[test]
    fn a_document_that_layers_on_nothing_still_addresses_its_own_packed_filesets() {
        let spec = spec(
            r#"{"image":"x:1","filesets":[{"hostPath":"~/.gitconfig","guestPath":"/g"},{"path":"./skills","guestPath":"/skills"}]}"#,
        );
        assert_eq!(
            own_fileset_origins(&spec),
            [FilesetOrigin {
                guest_path: "/skills".into(),
                source: ROOT_LABEL.into(),
                layer_index: 0
            }],
            "the no-mixin path skips the merge, so it has to number the layers the same way (docs/sandbox-spec.md §6)"
        );
    }

    #[test]
    fn a_credential_is_replaced_whole_rather_than_field_by_field() {
        let merged = merged(&sources(&[
            (
                "base",
                r#"{"credentials":[{"envVar":"SOME_TOKEN","placeholder":"lns-placeholder-base","injections":[{"kind":"bearer_header","domain":"api.base.example"}]}]}"#,
            ),
            (
                "later",
                r#"{"credentials":[{"envVar":"SOME_TOKEN","placeholder":"lns-placeholder-later"}]}"#,
            ),
        ]));
        assert_eq!(merged.credentials.len(), 1);
        assert_eq!(merged.credentials[0].placeholder, "lns-placeholder-later");
        assert!(
            merged.credentials[0].injections.is_empty(),
            "half of one entry and half of another would inject a placeholder the workload never holds"
        );
    }

    #[test]
    fn a_tool_is_keyed_by_name_so_a_later_version_replaces_it() {
        let merged = merged(&sources(&[
            ("base", r#"{"tools":["node@20","python@3.12"]}"#),
            ("later", r#"{"tools":["node@22"]}"#),
        ]));
        assert_eq!(merged.tools, ["node@22", "python@3.12"]);
    }

    #[test]
    fn a_mount_target_is_owned_by_the_last_source_to_claim_it() {
        let merged = merged(&sources(&[
            (
                "base",
                r#"{"volumes":[{"type":"volume","name":"base","target":"/cache"}],"filesets":[{"inline":{"a.md":"base"},"guestPath":"/notes"}]}"#,
            ),
            (
                "later",
                r#"{"volumes":[{"type":"volume","name":"later","target":"/cache"}],"filesets":[{"inline":{"a.md":"later"},"guestPath":"/notes"}]}"#,
            ),
        ]));
        assert_eq!(merged.volumes.len(), 1);
        assert_eq!(merged.volumes[0].source(), "later");
        assert_eq!(merged.filesets.len(), 1);
        assert_eq!(merged.filesets[0].inline.as_ref().unwrap()["a.md"], "later");
    }

    #[test]
    fn a_fileset_can_take_over_a_target_an_earlier_source_gave_a_volume() {
        let merged = merged(&sources(&[
            (
                "base",
                r#"{"volumes":[{"type":"volume","name":"base","target":"/notes"}]}"#,
            ),
            (
                "later",
                r#"{"filesets":[{"inline":{"a.md":"later"},"guestPath":"/notes"}]}"#,
            ),
        ]));
        assert!(
            merged.volumes.is_empty(),
            "one target is one keyspace across both blocks, and a document claiming it twice is one the parser itself refuses"
        );
        assert_eq!(
            merged.filesets.len(),
            1,
            "the later source's fileset displaced the volume that held the target"
        );
    }

    #[test]
    fn a_volume_can_take_over_a_target_an_earlier_source_gave_a_fileset() {
        let merged = merged(&sources(&[
            (
                "base",
                r#"{"filesets":[{"inline":{"a.md":"base"},"guestPath":"/notes"}]}"#,
            ),
            (
                "later",
                r#"{"volumes":[{"type":"volume","name":"later","target":"/notes"}]}"#,
            ),
        ]));
        assert!(merged.filesets.is_empty());
        assert_eq!(merged.volumes.len(), 1);
        assert_eq!(merged.volumes[0].source(), "later");
    }

    #[test]
    fn a_third_source_displaces_the_second_rather_than_the_first() {
        let merged = merged(&sources(&[
            ("first", r#"{"env":{"MODE":"a"}}"#),
            ("second", r#"{"env":{"MODE":"b"}}"#),
            ("third", r#"{"env":{"MODE":"c"}}"#),
        ]));
        assert_eq!(
            merged.env.get("MODE").map(String::as_str),
            Some("c"),
            "the last source to set a key wins, however many held it before"
        );
    }

    #[test]
    fn a_port_is_keyed_by_its_container_number() {
        let merged = merged(&sources(&[
            ("base", r#"{"ports":[{"container":8080,"host":18080}]}"#),
            ("later", r#"{"ports":[{"container":8080,"host":28080}]}"#),
        ]));
        assert_eq!(merged.ports.len(), 1);
        assert_eq!(merged.ports[0].host, Some(28080));
    }

    #[test]
    fn two_sources_publishing_one_host_port_refuse_the_resolution() {
        let err = merge(&as_sources(&sources(&[
            (
                ROOT_LABEL,
                r#"{"image":"x:1","ports":[{"container":8080,"host":18080}]}"#,
            ),
            ("later", r#"{"ports":[{"container":9090,"host":18080}]}"#),
        ])))
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("host port 18080")
                && message.contains(ROOT_LABEL)
                && message.contains("later"),
            "a host port is one socket, so precedence cannot settle two claims on it — and keeping the later mapping would silently unpublish a port the sandbox declared; the refusal has to name both sources so the author knows which to move: got {message}"
        );
    }

    #[test]
    fn a_host_port_a_later_source_remaps_is_an_override_rather_than_a_collision() {
        let merged = merged(&sources(&[
            ("base", r#"{"ports":[{"container":8080,"host":18080}]}"#),
            ("later", r#"{"ports":[{"container":8080,"host":18080}]}"#),
        ]));
        assert_eq!(
            merged.ports.len(),
            1,
            "the later entry replaced the earlier one by container, so one mapping holds the host port and nothing collides"
        );
    }

    #[test]
    fn a_host_port_only_an_earlier_source_still_holds_is_kept() {
        let merged = merged(&sources(&[
            ("base", r#"{"ports":[{"container":8080,"host":18080}]}"#),
            ("later", r#"{"ports":[{"container":8080,"host":28080}]}"#),
            ("last", r#"{"ports":[{"container":9090,"host":18080}]}"#),
        ]));
        assert_eq!(
            merged.ports.len(),
            2,
            "the source that held 18080 was overridden off it, so the claim it no longer makes must not refuse a later one"
        );
    }

    #[test]
    fn a_later_sources_egress_is_placed_ahead_so_it_decides() {
        let merged = merged(&sources(&[
            (
                "base",
                r#"{"egress":{"http":[{"match":"api.example.test","verdict":"deny"}]}}"#,
            ),
            (
                "later",
                r#"{"egress":{"http":[{"match":"api.example.test","verdict":"allow"}]}}"#,
            ),
        ]));
        let table = &merged.egress.http;
        assert_eq!(table.len(), 2, "both entries survive the union");
        assert_eq!(
            table[0].verdict,
            lns_policy::Verdict::Allow,
            "the gate reads first-match, so the later source's entry has to sit ahead for it to decide"
        );
    }

    #[test]
    fn the_egress_a_source_list_folds_to_is_the_one_the_merged_document_carries() {
        let owned = sources(&[
            (
                "base",
                r#"{"egress":{"http":[{"match":"api.example.test","verdict":"deny"}],"tcp":[{"match":"db.example.test:5432","verdict":"allow"}]}}"#,
            ),
            (
                "later",
                r#"{"egress":{"http":[{"match":"api.example.test","verdict":"allow"}]}}"#,
            ),
        ]);
        let list = as_sources(&owned);
        assert_eq!(
            egress_of(&list),
            merge(&list).expect("these sources resolve").spec.egress,
            "the gate re-folds a prefix of this list against a live source, so the fold has to be the one that produced the document"
        );
        assert_eq!(
            egress_of(&list[..1]).http.len(),
            1,
            "a prefix folds to what those sources alone decided, which is what makes a source that is still being edited foldable over it"
        );
    }

    #[test]
    fn a_raw_table_merges_by_the_same_rule_as_the_http_one() {
        let merged = merged(&sources(&[
            (
                "base",
                r#"{"egress":{"tcp":[{"match":"db.example.com:5432","verdict":"deny"}]}}"#,
            ),
            (
                "later",
                r#"{"egress":{"tcp":[{"match":"db.example.com:5432","verdict":"allow"}]}}"#,
            ),
        ]));
        let table = &merged.egress.tcp;
        assert_eq!(table.len(), 2);
        assert_eq!(
            table[0].verdict,
            lns_policy::Verdict::Allow,
            "a raw destination merges by the same rule, so the later source's entry decides"
        );
    }

    #[test]
    fn a_resolved_sandbox_keeps_the_connector_list_its_own_document_declared() {
        let merged = merged(&sources(&[
            (
                "the sandbox",
                r#"{"image":"x:1","connectors":["some-provider"]}"#,
            ),
            ("later", r#"{"tools":["node@22"]}"#),
        ]));
        assert_eq!(
            merged.connectors,
            ["some-provider"],
            "no mixin can name a connector, so resolution must not lose the list the sandbox itself carries"
        );
    }

    #[test]
    fn one_documents_own_order_survives_inside_its_own_entries() {
        let merged = merged(&sources(&[(
            "base",
            r#"{"egress":{"http":[{"match":"api.example.test","verdict":"allow"},{"match":"*","verdict":"deny"}]}}"#,
        )]));
        let patterns: Vec<&str> = merged
            .egress
            .http
            .iter()
            .map(|r| r.match_pattern.as_str())
            .collect();
        assert_eq!(
            patterns,
            ["api.example.test", "*"],
            "an author writes an ordered table and reads it top to bottom"
        );
    }

    #[test]
    fn the_resolved_document_declares_no_mixins_of_its_own() {
        let merged = merged(&sources(&[("base", r#"{"image":"x:1","mixins":["own"]}"#)]));
        assert!(
            merged.mixins.is_empty(),
            "the mixins are what produced this document, so carrying them would ask a run to resolve them twice"
        );
    }

    #[test]
    fn only_the_sandbox_contributes_the_blocks_that_describe_one_launch() {
        let merged = merged(&sources(&[
            (
                "the sandbox",
                r#"{"image":"x:1","command":"agent","workdir":"/w","user":"node","resources":{"cpu":2}}"#,
            ),
            ("later", r#"{"tools":["node@22"]}"#),
        ]));
        assert_eq!(merged.image, "x:1");
        assert_eq!(merged.command.as_deref(), Some("agent"));
        assert_eq!(merged.workdir.as_deref(), Some("/w"));
        assert_eq!(merged.user.as_deref(), Some("node"));
        assert!(merged.resources.is_some());
    }

    #[test]
    fn a_merged_document_round_trips_through_its_own_serialization() {
        let merged = merged(&sources(&[(
            "base",
            r#"{"image":"x:1","command":"agent","workdir":"/w","user":"node","env":{"MODE":"research"},"resources":{"cpu":2,"memory":"1Gi"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"lns-placeholder-some"}],"tools":["node@22"],"volumes":[{"type":"bind","source":".","target":"/w"}],"filesets":[{"inline":{"a.md":"x"},"guestPath":"/notes"}],"ports":[{"container":8080}],"egress":{"http":[{"match":"api.example.test","verdict":"allow"}]}}"#,
        )]));
        let json = serde_json::to_string(&merged).expect("a merged spec serializes");
        assert_eq!(
            serde_json::from_str::<SandboxSpec>(&json).expect("and decodes again"),
            merged,
            "the resolved document has to survive the trip to a disclosure and to the service, or the merge silently drops what it re-encoded badly"
        );
    }
}
