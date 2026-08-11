//! Resolution order and per-block merge for a sandbox and the mixins it layers on (`docs/sandbox-spec.md` §3.3.2), kept pure so the walk's depth limit, its cycle refusal and every merge rule are decided without a fetch.

use std::collections::BTreeMap;

use anyhow::{Result, bail};

use crate::sandbox::{FilesetEntry, SandboxSpec, Volume};

/// One layer of a resolved sandbox, labelled with where it came from so a disclosure can attribute every entry it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub label: String,
    pub spec: SandboxSpec,
}

/// The label the sandbox's own spec resolves under; every other source is labelled by the reference that named it.
pub const ROOT_LABEL: &str = "the sandbox";

/// The graph is walked this deep and refused beyond, so a chain nobody can read cannot stall a launch.
pub const MAX_DEPTH: usize = 5;

/// What one source contributed to the merged document, and what it displaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contribution {
    pub source: String,
    pub block: &'static str,
    pub key: String,
    pub replaced: Option<String>,
}

/// A resolved sandbox and the record of who contributed each part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    pub spec: SandboxSpec,
    pub contributions: Vec<Contribution>,
}

/// Flatten a sandbox and its mixins into the one ordered source list §3.3.2 merges: the sandbox first, then each of its `mixins` in order with that mixin's own `mixins` expanded right after it, then each extra reference in the order the user gave it.
///
/// A mixin's own mixins come after it, so they beat it — a mixin that pulls in another is asking for that other's version of a shared setting.
pub fn flatten(
    root: &SandboxSpec,
    extra: &[String],
    graph: &BTreeMap<String, SandboxSpec>,
) -> Result<Vec<Source>> {
    let mut sources = vec![Source {
        label: ROOT_LABEL.to_string(),
        spec: root.clone(),
    }];
    let mut trail = Vec::new();
    expand(&root.mixins, graph, 1, &mut trail, &mut sources)?;
    expand(extra, graph, 1, &mut trail, &mut sources)?;
    Ok(sources)
}

fn expand(
    references: &[String],
    graph: &BTreeMap<String, SandboxSpec>,
    depth: usize,
    trail: &mut Vec<String>,
    sources: &mut Vec<Source>,
) -> Result<()> {
    for reference in references {
        if trail.contains(reference) {
            bail!(
                "mixin {reference} is reachable from itself ({} → {reference}); a cycle cannot resolve",
                trail.join(" → ")
            );
        }
        if depth > MAX_DEPTH {
            bail!(
                "mixin {reference} sits deeper than {MAX_DEPTH} mixins from the sandbox ({}); refusing to resolve further",
                trail.join(" → ")
            );
        }
        let Some(spec) = graph.get(reference) else {
            bail!("mixin {reference} is not resolved; it has to be pulled before the merge");
        };
        sources.push(Source {
            label: reference.clone(),
            spec: spec.clone(),
        });
        trail.push(reference.clone());
        expand(&spec.mixins, graph, depth + 1, trail, sources)?;
        trail.pop();
    }
    Ok(())
}

/// Merge an ordered source list per §3.3.2: the last source to say something about a thing wins, and every keyed block unions by its key.
pub fn merge(sources: &[Source]) -> Merged {
    let mut contributions = Vec::new();
    let mut spec = SandboxSpec::default();

    for source in sources {
        for (key, value) in &source.spec.env {
            let replaced = spec.env.insert(key.clone(), value.clone());
            record(
                &mut contributions,
                source,
                "env",
                key.clone(),
                replaced.map(|_| ()),
            );
        }
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

    spec.credentials = fold(
        sources,
        "credentials",
        |s| &s.credentials,
        |c| c.env_var.clone(),
        &mut contributions,
    );
    spec.tools = fold(
        sources,
        "tools",
        |s| &s.tools,
        |t| tool_name(t).to_string(),
        &mut contributions,
    );
    (spec.volumes, spec.filesets) = fold_mounts(sources, &mut contributions);
    spec.ports = fold(
        sources,
        "ports",
        |s| &s.ports,
        |p| p.container.to_string(),
        &mut contributions,
    );

    // A later source's entries are placed ahead of an earlier one's, so the latest entry matching a destination is the one that decides.
    for source in sources.iter().rev() {
        spec.policy
            .egress
            .http
            .extend(source.spec.policy.egress.http.iter().cloned());
        spec.policy
            .egress
            .tcp
            .extend(source.spec.policy.egress.tcp.iter().cloned());
        for rule in &source.spec.policy.egress.http {
            record(
                &mut contributions,
                source,
                "egress.http",
                rule.match_pattern.clone(),
                None,
            );
        }
        for rule in &source.spec.policy.egress.tcp {
            record(
                &mut contributions,
                source,
                "egress.tcp",
                rule.match_pattern.clone(),
                None,
            );
        }
    }

    // The mixins are what produced this document, so the resolved one declares none of its own.
    spec.mixins = Vec::new();
    Merged {
        spec,
        contributions,
    }
}

/// A mount, whichever block claimed it: one target is one keyspace across `volumes` and `filesets`, so a later source's claim of either kind displaces an earlier source's claim of either kind.
enum Mount {
    Volume(Volume),
    Fileset(FilesetEntry),
}

fn fold_mounts(
    sources: &[Source],
    contributions: &mut Vec<Contribution>,
) -> (Vec<Volume>, Vec<FilesetEntry>) {
    let mut order: Vec<String> = Vec::new();
    let mut winners: BTreeMap<String, (Mount, String)> = BTreeMap::new();
    let mut claim = |target: String, mount: Mount, block: &'static str, label: &str| {
        let replaced = winners.insert(target.clone(), (mount, label.to_string()));
        if replaced.is_none() {
            order.push(target.clone());
        }
        contributions.push(Contribution {
            source: label.to_string(),
            block,
            key: target,
            replaced: replaced.map(|(_, held)| held),
        });
    };
    for source in sources {
        for volume in &source.spec.volumes {
            claim(
                volume.target.clone(),
                Mount::Volume(volume.clone()),
                "volumes",
                &source.label,
            );
        }
        for fileset in &source.spec.filesets {
            claim(
                fileset.mount_path.clone(),
                Mount::Fileset(fileset.clone()),
                "filesets",
                &source.label,
            );
        }
    }
    let mut volumes = Vec::new();
    let mut filesets = Vec::new();
    for (mount, _) in order
        .into_iter()
        .filter_map(|target| winners.remove(&target))
    {
        match mount {
            Mount::Volume(volume) => volumes.push(volume),
            Mount::Fileset(fileset) => filesets.push(fileset),
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

/// Union one keyed block across every source, last-wins, in first-appearance order of the key so a reader sees the document's own entries before what a mixin added.
fn fold<T: Clone>(
    sources: &[Source],
    block: &'static str,
    items: fn(&SandboxSpec) -> &Vec<T>,
    key: fn(&T) -> String,
    contributions: &mut Vec<Contribution>,
) -> Vec<T> {
    let mut order: Vec<String> = Vec::new();
    let mut winners: BTreeMap<String, (T, String)> = BTreeMap::new();
    for source in sources {
        for item in items(&source.spec) {
            let key = key(item);
            let replaced = winners.insert(key.clone(), (item.clone(), source.label.clone()));
            if replaced.is_none() {
                order.push(key.clone());
            }
            contributions.push(Contribution {
                source: source.label.clone(),
                block,
                key,
                replaced: replaced.map(|(_, label)| label),
            });
        }
    }
    order
        .into_iter()
        .filter_map(|key| winners.remove(&key).map(|(item, _)| item))
        .collect()
}

fn record(
    contributions: &mut Vec<Contribution>,
    source: &Source,
    block: &'static str,
    key: String,
    replaced: Option<()>,
) {
    let replaced = replaced.and_then(|()| {
        contributions
            .iter()
            .rev()
            .find(|c| c.block == block && c.key == key)
            .map(|c| c.source.clone())
    });
    contributions.push(Contribution {
        source: source.label.clone(),
        block,
        key,
        replaced,
    });
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

    fn labels(sources: &[Source]) -> Vec<&str> {
        sources.iter().map(|s| s.label.as_str()).collect()
    }

    #[test]
    fn the_source_list_walks_the_document_then_its_mixins_then_the_flags() {
        let root = spec(r#"{"image":"x:1","mixins":["own"]}"#);
        let sources = flatten(
            &root,
            &["first".into(), "second".into()],
            &graph(&[
                ("own", r#"{}"#),
                ("first", r#"{"mixins":["firsts-own"]}"#),
                ("firsts-own", r#"{}"#),
                ("second", r#"{}"#),
            ]),
        )
        .expect("every reference resolves");
        assert_eq!(
            labels(&sources),
            [ROOT_LABEL, "own", "first", "firsts-own", "second"],
            "a mixin's own mixins follow it, and the user's flags are appended last, so each beats what came before it"
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
        assert_eq!(flatten(&root, &[], &graph(&chain)).unwrap().len(), 6);

        let mut deeper = chain;
        deeper[4] = ("m5", r#"{"mixins":["m6"]}"#);
        deeper.push(("m6", r#"{}"#));
        let err = flatten(&root, &[], &graph(&deeper)).unwrap_err();
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
        let err = flatten(&root, &[], &BTreeMap::new()).unwrap_err();
        assert!(
            format!("{err:#}").contains("is not resolved"),
            "skipping it would boot a sandbox without what its own document declared; got: {err:#}"
        );
    }

    fn sources(entries: &[(&str, &str)]) -> Vec<Source> {
        entries
            .iter()
            .map(|(label, json)| Source {
                label: (*label).to_string(),
                spec: spec(json),
            })
            .collect()
    }

    #[test]
    fn env_unions_by_key_and_the_last_source_to_set_one_wins() {
        let merged = merge(&sources(&[
            ("base", r#"{"env":{"MODE":"research","KEEP":"1"}}"#),
            ("later", r#"{"env":{"MODE":"strict"}}"#),
        ]));
        assert_eq!(
            merged.spec.env.get("MODE").map(String::as_str),
            Some("strict")
        );
        assert_eq!(merged.spec.env.get("KEEP").map(String::as_str), Some("1"));
    }

    #[test]
    fn a_credential_is_replaced_whole_rather_than_field_by_field() {
        let merged = merge(&sources(&[
            (
                "base",
                r#"{"credentials":[{"envVar":"SOME_TOKEN","placeholder":"lns-placeholder-base","injections":[{"kind":"bearer_header","domain":"api.base.example"}]}]}"#,
            ),
            (
                "later",
                r#"{"credentials":[{"envVar":"SOME_TOKEN","placeholder":"lns-placeholder-later"}]}"#,
            ),
        ]));
        assert_eq!(merged.spec.credentials.len(), 1);
        assert_eq!(
            merged.spec.credentials[0].placeholder,
            "lns-placeholder-later"
        );
        assert!(
            merged.spec.credentials[0].injections.is_empty(),
            "half of one entry and half of another would inject a placeholder the workload never holds"
        );
    }

    #[test]
    fn a_tool_is_keyed_by_name_so_a_later_version_replaces_it() {
        let merged = merge(&sources(&[
            ("base", r#"{"tools":["node@20","python@3.12"]}"#),
            ("later", r#"{"tools":["node@22"]}"#),
        ]));
        assert_eq!(merged.spec.tools, ["node@22", "python@3.12"]);
    }

    #[test]
    fn a_mount_target_is_owned_by_the_last_source_to_claim_it() {
        let merged = merge(&sources(&[
            (
                "base",
                r#"{"volumes":[{"type":"volume","name":"base","target":"/cache"}],"filesets":[{"inline":{"a.md":"base"},"mountPath":"/notes"}]}"#,
            ),
            (
                "later",
                r#"{"volumes":[{"type":"volume","name":"later","target":"/cache"}],"filesets":[{"inline":{"a.md":"later"},"mountPath":"/notes"}]}"#,
            ),
        ]));
        assert_eq!(merged.spec.volumes.len(), 1);
        assert_eq!(merged.spec.volumes[0].source(), "later");
        assert_eq!(merged.spec.filesets.len(), 1);
        assert_eq!(
            merged.spec.filesets[0].inline.as_ref().unwrap()["a.md"],
            "later"
        );
    }

    #[test]
    fn a_fileset_can_take_over_a_target_an_earlier_source_gave_a_volume() {
        let merged = merge(&sources(&[
            (
                "base",
                r#"{"volumes":[{"type":"volume","name":"base","target":"/notes"}]}"#,
            ),
            (
                "later",
                r#"{"filesets":[{"inline":{"a.md":"later"},"mountPath":"/notes"}]}"#,
            ),
        ]));
        assert!(
            merged.spec.volumes.is_empty(),
            "one target is one keyspace across both blocks, and a document claiming it twice is one the parser itself refuses"
        );
        assert_eq!(merged.spec.filesets.len(), 1);
        let claim = merged
            .contributions
            .iter()
            .find(|c| c.key == "/notes" && c.source == "later")
            .expect("the winning claim is recorded");
        assert_eq!(
            claim.replaced.as_deref(),
            Some("base"),
            "the disclosure has to name the volume the fileset displaced"
        );
    }

    #[test]
    fn a_volume_can_take_over_a_target_an_earlier_source_gave_a_fileset() {
        let merged = merge(&sources(&[
            (
                "base",
                r#"{"filesets":[{"inline":{"a.md":"base"},"mountPath":"/notes"}]}"#,
            ),
            (
                "later",
                r#"{"volumes":[{"type":"volume","name":"later","target":"/notes"}]}"#,
            ),
        ]));
        assert!(merged.spec.filesets.is_empty());
        assert_eq!(merged.spec.volumes.len(), 1);
        assert_eq!(merged.spec.volumes[0].source(), "later");
    }

    #[test]
    fn a_third_source_displaces_the_second_rather_than_the_first() {
        let merged = merge(&sources(&[
            ("first", r#"{"env":{"MODE":"a"}}"#),
            ("second", r#"{"env":{"MODE":"b"}}"#),
            ("third", r#"{"env":{"MODE":"c"}}"#),
        ]));
        assert_eq!(merged.spec.env.get("MODE").map(String::as_str), Some("c"));
        let third = merged
            .contributions
            .iter()
            .find(|c| c.source == "third")
            .expect("the last writer is recorded");
        assert_eq!(
            third.replaced.as_deref(),
            Some("second"),
            "a displacement names whoever held the key, not whoever set it first"
        );
    }

    #[test]
    fn a_port_is_keyed_by_its_container_number() {
        let merged = merge(&sources(&[
            ("base", r#"{"ports":[{"container":8080,"host":18080}]}"#),
            ("later", r#"{"ports":[{"container":8080,"host":28080}]}"#),
        ]));
        assert_eq!(merged.spec.ports.len(), 1);
        assert_eq!(merged.spec.ports[0].host, Some(28080));
    }

    #[test]
    fn a_later_sources_egress_is_placed_ahead_so_it_decides() {
        let merged = merge(&sources(&[
            (
                "base",
                r#"{"policy":{"egress":{"http":[{"match":"api.example.test","verdict":"deny"}]}}}"#,
            ),
            (
                "later",
                r#"{"policy":{"egress":{"http":[{"match":"api.example.test","verdict":"allow"}]}}}"#,
            ),
        ]));
        let table = &merged.spec.policy.egress.http;
        assert_eq!(table.len(), 2, "both entries survive the union");
        assert_eq!(
            table[0].verdict,
            lns_policy::Verdict::Allow,
            "the gate reads first-match, so the later source's entry has to sit ahead for it to decide"
        );
    }

    #[test]
    fn a_raw_table_merges_by_the_same_rule_as_the_http_one() {
        let merged = merge(&sources(&[
            (
                "base",
                r#"{"policy":{"egress":{"tcp":[{"match":"db.example.com:5432","verdict":"deny"}]}}}"#,
            ),
            (
                "later",
                r#"{"policy":{"egress":{"tcp":[{"match":"db.example.com:5432","verdict":"allow"}]}}}"#,
            ),
        ]));
        let table = &merged.spec.policy.egress.tcp;
        assert_eq!(table.len(), 2);
        assert_eq!(table[0].verdict, lns_policy::Verdict::Allow);
        assert!(
            merged
                .contributions
                .iter()
                .any(|c| c.block == "egress.tcp" && c.source == "later"),
            "a raw destination is disclosed like any other, so the merge has to record who asked for it"
        );
    }

    #[test]
    fn a_resolved_sandbox_keeps_the_connector_list_its_own_document_declared() {
        let merged = merge(&sources(&[
            (
                "the sandbox",
                r#"{"image":"x:1","connectors":["some-provider"]}"#,
            ),
            ("later", r#"{"tools":["node@22"]}"#),
        ]));
        assert_eq!(
            merged.spec.connectors,
            ["some-provider"],
            "no mixin can name a connector, so resolution must not lose the list the sandbox itself carries"
        );
    }

    #[test]
    fn one_documents_own_order_survives_inside_its_own_entries() {
        let merged = merge(&sources(&[(
            "base",
            r#"{"policy":{"egress":{"http":[{"match":"api.example.test","verdict":"allow"},{"match":"*","verdict":"deny"}]}}}"#,
        )]));
        let patterns: Vec<&str> = merged
            .spec
            .policy
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
        let merged = merge(&sources(&[("base", r#"{"image":"x:1","mixins":["own"]}"#)]));
        assert!(
            merged.spec.mixins.is_empty(),
            "the mixins are what produced this document, so carrying them would ask a run to resolve them twice"
        );
    }

    #[test]
    fn only_the_sandbox_contributes_the_blocks_that_describe_one_launch() {
        let merged = merge(&sources(&[
            (
                "the sandbox",
                r#"{"image":"x:1","command":"agent","workdir":"/w","user":"node","resources":{"cpu":2}}"#,
            ),
            ("later", r#"{"tools":["node@22"]}"#),
        ]));
        assert_eq!(merged.spec.image, "x:1");
        assert_eq!(merged.spec.command.as_deref(), Some("agent"));
        assert_eq!(merged.spec.workdir.as_deref(), Some("/w"));
        assert_eq!(merged.spec.user.as_deref(), Some("node"));
        assert!(merged.spec.resources.is_some());
    }

    #[test]
    fn a_contribution_names_its_source_and_what_it_displaced() {
        let merged = merge(&sources(&[
            ("base", r#"{"tools":["node@20"],"env":{"MODE":"research"}}"#),
            ("later", r#"{"tools":["node@22"],"env":{"MODE":"strict"}}"#),
        ]));
        let tool = merged
            .contributions
            .iter()
            .find(|c| c.block == "tools" && c.source == "later")
            .expect("the winning tool is recorded");
        assert_eq!(tool.key, "node");
        assert_eq!(
            tool.replaced.as_deref(),
            Some("base"),
            "the disclosure has to show what a source replaced, so an override nobody intended is visible while it can still be refused"
        );
        let env = merged
            .contributions
            .iter()
            .find(|c| c.block == "env" && c.source == "later")
            .expect("the winning env key is recorded");
        assert_eq!(env.replaced.as_deref(), Some("base"));
    }

    #[test]
    fn a_first_contribution_displaces_nothing() {
        let merged = merge(&sources(&[("base", r#"{"tools":["node@20"]}"#)]));
        assert_eq!(merged.contributions.len(), 1);
        assert_eq!(merged.contributions[0].replaced, None);
    }

    #[test]
    fn a_merged_document_round_trips_through_its_own_serialization() {
        let merged = merge(&sources(&[(
            "base",
            r#"{"image":"x:1","command":"agent","workdir":"/w","user":"node","env":{"MODE":"research"},"resources":{"cpu":2,"memory":"1Gi"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"lns-placeholder-some"}],"tools":["node@22"],"volumes":[{"type":"bind","source":".","target":"/w"}],"filesets":[{"inline":{"a.md":"x"},"mountPath":"/notes"}],"ports":[{"container":8080}],"policy":{"egress":{"http":[{"match":"api.example.test","verdict":"allow"}]}}}"#,
        )]));
        let json = serde_json::to_string(&merged.spec).expect("a merged spec serializes");
        assert_eq!(
            serde_json::from_str::<SandboxSpec>(&json).expect("and decodes again"),
            merged.spec,
            "the resolved document has to survive the trip to a disclosure and to the service, or the merge silently drops what it re-encoded badly"
        );
    }
}
