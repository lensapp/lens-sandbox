use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::author::Fs;
use super::fileset::local_mixin_document;

/// The decisions file holds one machine's answers, so §6.1 refuses an entry naming it rather than publishing it.
const LOCAL_MIXIN_FILENAME: &str = "lns-local-mixin.yaml";

/// One local mixin a push publishes, with the repository the parent reference and its own name earn it.
#[derive(Debug, PartialEq, Eq)]
pub struct PlannedMixin {
    pub declared: String,
    pub document: PathBuf,
    pub root: PathBuf,
    pub repository: String,
    pub bytes: Vec<u8>,
}

/// The local-mixin subtree one push publishes, children before the parents that pin them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MixinPlan {
    pub nodes: Vec<PlannedMixin>,
}

impl MixinPlan {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// Strip the release coordinate from a reference, so a tag or digest cannot leak into a repository derived from it.
fn repository_of(reference: &str) -> &str {
    let without_digest = reference
        .split_once('@')
        .map_or(reference, |(head, _)| head);
    match without_digest.rfind(':') {
        Some(colon) if !without_digest[colon..].contains('/') => &without_digest[..colon],
        _ => without_digest,
    }
}

/// The repository a child publishes to: the parent reference up to its last path segment, with the child's own name.
pub fn sibling_repository(parent_reference: &str, name: &str) -> Result<String> {
    let (namespace, _) = repository_of(parent_reference)
        .rsplit_once('/')
        .with_context(|| {
            format!(
                "{parent_reference} names no namespace to publish a mixin beside; qualify it with the registry and owner the mixin should publish under"
            )
        })?;
    Ok(format!("{namespace}/{name}"))
}

/// Walk the local entries of `doc`, depth first, so every child is planned before the document that pins it.
pub fn plan_local_mixins<F: Fs + ?Sized>(
    fs: &F,
    project_dir: &Path,
    doc: &[u8],
    parent_reference: &str,
) -> Result<MixinPlan> {
    let def = lns_artifact::sandbox::parse_document(doc)?;
    let mut walk = Walk {
        fs,
        parent_reference,
        nodes: Vec::new(),
        trail: Vec::new(),
        children: std::collections::BTreeMap::new(),
        roots: Vec::new(),
    };
    walk.descend(def.mixins(), project_dir, None)?;
    refuse_what_sits_too_deep(&walk.roots, &walk.children)?;
    Ok(MixinPlan { nodes: walk.nodes })
}

struct Walk<'a, F: Fs + ?Sized> {
    fs: &'a F,
    parent_reference: &'a str,
    nodes: Vec<PlannedMixin>,
    trail: Vec<PathBuf>,
    children: std::collections::BTreeMap<PathBuf, Vec<PathBuf>>,
    roots: Vec<PathBuf>,
}

impl<F: Fs + ?Sized> Walk<'_, F> {
    fn descend(&mut self, mixins: &[String], root: &Path, owner: Option<PathBuf>) -> Result<()> {
        let mut reached = Vec::new();
        for declared in mixins {
            if !lns_artifact::sandbox::names_a_local_path(declared) {
                continue;
            }
            reached.push(self.plan_one(declared, root)?);
        }
        match owner {
            Some(owner) => {
                self.children.insert(owner, reached);
            }
            None => self.roots = reached,
        }
        Ok(())
    }

    fn plan_one(&mut self, declared: &str, root: &Path) -> Result<PathBuf> {
        let document = local_mixin_document(self.fs, root, declared);
        if document.file_name() == Some(LOCAL_MIXIN_FILENAME.as_ref()) {
            bail!(
                "mixin {declared} names {LOCAL_MIXIN_FILENAME}, which holds this machine's own decisions and is never published"
            );
        }
        if let Some(position) = self.trail.iter().position(|seen| seen == &document) {
            let trail: Vec<String> = self.trail[position..]
                .iter()
                .chain(std::iter::once(&document))
                .map(|path| path.display().to_string())
                .collect();
            bail!(
                "mixin {declared} is reachable from itself ({}); a cycle has no digest to pin, because the digest would depend on itself",
                trail.join(" → ")
            );
        }
        if self.nodes.iter().any(|node| node.document == document) {
            return Ok(document);
        }
        let doc = super::author::load_definition_json_at(self.fs, &document)?;
        let child = lns_artifact::sandbox::parse_mixin(&doc)
            .with_context(|| format!("mixin {declared} must be a mixin to publish as one"))?;
        let child_root = document.parent().unwrap_or(root).to_path_buf();
        self.trail.push(document.clone());
        self.descend(&child.spec.mixins, &child_root, Some(document.clone()))?;
        self.trail.pop();
        self.nodes.push(PlannedMixin {
            declared: declared.to_string(),
            repository: sibling_repository(self.parent_reference, &child.name)?,
            document: document.clone(),
            root: child_root,
            bytes: doc,
        });
        Ok(document)
    }
}

/// Refuse by shortest path, exactly as `merge` resolves by it, so publishing never rejects a graph a run would accept.
fn refuse_what_sits_too_deep(
    roots: &[PathBuf],
    children: &std::collections::BTreeMap<PathBuf, Vec<PathBuf>>,
) -> Result<()> {
    let mut seen: std::collections::BTreeSet<&Path> = std::collections::BTreeSet::new();
    let mut frontier: Vec<&Path> = roots.iter().map(PathBuf::as_path).collect();
    let mut depth = 1;
    loop {
        frontier.retain(|document| !seen.contains(document));
        let Some(first) = frontier.first() else {
            return Ok(());
        };
        if depth > lns_artifact::merge::MAX_DEPTH {
            bail!(
                "mixin {} sits deeper than {} mixins from the document being published; refusing to publish further",
                first.display(),
                lns_artifact::merge::MAX_DEPTH
            );
        }
        let mut next = Vec::new();
        for document in std::mem::take(&mut frontier) {
            if seen.insert(document)
                && let Some(reached) = children.get(document)
            {
                next.extend(reached.iter().map(PathBuf::as_path));
            }
        }
        frontier = next;
        depth += 1;
    }
}

/// Rewrite `spec.mixins` so every local entry carries the digest its child published under, keyed by the document the entry resolves to — two spellings of one document are one identity, so both get pinned.
pub fn pin_local_mixins<F: Fs + ?Sized>(
    fs: &F,
    root: &Path,
    doc: &[u8],
    published: &[(PathBuf, String)],
) -> Result<Vec<u8>> {
    let mut value: serde_json::Value =
        serde_json::from_slice(doc).context("re-reading the definition for mixin pinning")?;
    let Some(entries) = value["spec"]["mixins"].as_array_mut() else {
        return Ok(doc.to_vec());
    };
    for entry in entries {
        let declared = entry
            .as_str()
            .context("spec.mixins entry is not a string")?;
        if !lns_artifact::sandbox::names_a_local_path(declared) {
            continue;
        }
        let document = local_mixin_document(fs, root, declared);
        let (_, pinned) = published
            .iter()
            .find(|(candidate, _)| candidate == &document)
            .with_context(|| {
                format!(
                    "mixin {declared} resolves to {}, which this push did not publish, so no digest can replace it",
                    document.display()
                )
            })?;
        *entry = serde_json::Value::String(pinned.clone());
    }
    serde_json::to_vec(&value).context("serializing the mixin-pinned definition")
}

/// The tag a child publishes under, derived from the content it names so untagged pruning cannot reclaim it.
pub fn digest_derived_tag(manifest_digest: &str) -> String {
    manifest_digest.replace(':', "-")
}

pub fn confirm_mixin_publication(
    plan: &MixinPlan,
    parent_reference: &str,
    assume_yes: bool,
    terminal: &mut dyn crate::terminal::Terminal,
    output: &mut dyn Write,
) -> Result<()> {
    if plan.is_empty() || assume_yes {
        return Ok(());
    }
    let width = plan
        .nodes
        .iter()
        .map(|node| node.declared.len())
        .max()
        .unwrap_or_default();
    writeln!(
        output,
        "{parent_reference} layers on {} local mixin(s), which publish first:",
        plan.nodes.len()
    )?;
    for node in &plan.nodes {
        writeln!(
            output,
            "  Mixin: {:width$}  → {} (the sandbox pins its digest)",
            node.declared, node.repository
        )?;
    }
    if !terminal.is_available() {
        bail!(
            "{parent_reference} publishes the mixins above and there is no terminal to confirm — run interactively, or pass --yes to accept them"
        );
    }
    write!(output, "Continue? [y/N]: ")?;
    output.flush()?;
    let answer = terminal
        .read_answer()
        .context("reading the answer to the mixin publication prompt")?;
    if crate::terminal::is_affirmative(&answer) {
        return Ok(());
    }
    bail!("declined; nothing was published")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::test_support::MapFs;
    use crate::terminal::ScriptedTerminal;

    const PARENT: &str = "ghcr.io/acme/dev:1.4";
    const CHILD_DIGEST: &str = "ghcr.io/acme/postgres-tools@sha256:cccc000000000000000000000000000000000000000000000000000000000000";

    fn sandbox_naming(mixins: &str) -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{{"image":"x:1","mixins":{mixins}}}}}"#
        )
        .into_bytes()
    }

    fn mixin_yaml(name: &str) -> String {
        format!(
            "apiVersion: lns.run/v1\nkind: mixin\nname: {name}\nspec:\n  env:\n    MODE: research\n"
        )
    }

    fn mixin_yaml_naming(name: &str, mixins: &[&str]) -> String {
        if mixins.is_empty() {
            return mixin_yaml(name);
        }
        let entries: String = mixins
            .iter()
            .map(|entry| format!("    - {entry}\n"))
            .collect();
        format!("apiVersion: lns.run/v1\nkind: mixin\nname: {name}\nspec:\n  mixins:\n{entries}")
    }

    /// A chain of `links` local mixins under /work/mixins, each naming the next.
    fn chain_of(links: usize) -> MapFs {
        let entries: Vec<(String, String)> = (0..links)
            .map(|level| {
                let next: Vec<&str> = if level + 1 < links {
                    vec!["./deeper/"]
                } else {
                    vec![]
                };
                (
                    format!("/work/mixins/{}lns.yaml", "deeper/".repeat(level)),
                    mixin_yaml_naming(&format!("level{level}"), &next),
                )
            })
            .collect();
        let refs: Vec<(&str, &str)> = entries
            .iter()
            .map(|(path, body)| (path.as_str(), body.as_str()))
            .collect();
        MapFs::with(&refs)
    }

    #[test]
    fn a_local_mixin_publishes_beside_the_parent_it_is_named_by() {
        let fs = MapFs::with(&[("/work/mixins/pg/lns.yaml", &mixin_yaml("postgres-tools"))]);
        let plan = plan_local_mixins(
            &fs,
            Path::new("/work"),
            &sandbox_naming(r#"["./mixins/pg/"]"#),
            PARENT,
        )
        .expect("a local mixin beside the document is publishable");
        assert_eq!(plan.nodes.len(), 1);
        assert_eq!(
            plan.nodes[0].repository, "ghcr.io/acme/postgres-tools",
            "the namespace comes from the reference the author typed and the repository from the mixin's own name, so nothing is invented"
        );
        assert_eq!(plan.nodes[0].declared, "./mixins/pg/");
        assert_eq!(
            plan.nodes[0].document,
            PathBuf::from("/work/mixins/pg/lns.yaml")
        );
    }

    #[test]
    fn the_parents_tag_and_digest_never_reach_the_childs_repository() {
        let cases = [
            ("ghcr.io/acme/dev:1.4", "ghcr.io/acme/pg"),
            (
                "ghcr.io/acme/dev@sha256:aaaa000000000000000000000000000000000000000000000000000000000000",
                "ghcr.io/acme/pg",
            ),
            ("ghcr.io/acme/team/dev:1.4", "ghcr.io/acme/team/pg"),
            ("localhost:5000/acme/dev:1", "localhost:5000/acme/pg"),
            ("localhost:5000/acme/dev", "localhost:5000/acme/pg"),
            ("hub.lns.run/dev", "hub.lns.run/pg"),
        ];
        for (parent, expected) in cases {
            assert_eq!(
                sibling_repository(parent, "pg").expect("the reference names a namespace"),
                expected,
                "a release coordinate must never leak into the repository a mixin publishes at; parent {parent}"
            );
        }
    }

    #[test]
    fn a_reference_with_no_namespace_says_so_rather_than_guessing_one() {
        let err = sibling_repository("dev:1", "pg").unwrap_err();
        assert!(
            format!("{err:#}").contains("no namespace"),
            "inventing a namespace would create a repository the author never typed; got: {err:#}"
        );
    }

    #[test]
    fn the_walk_publishes_children_before_the_parents_that_pin_them() {
        let fs = MapFs::with(&[
            (
                "/work/mixins/outer/lns.yaml",
                &mixin_yaml_naming("outer", &["./inner/"]),
            ),
            ("/work/mixins/outer/inner/lns.yaml", &mixin_yaml("inner")),
        ]);
        let plan = plan_local_mixins(
            &fs,
            Path::new("/work"),
            &sandbox_naming(r#"["./mixins/outer/"]"#),
            PARENT,
        )
        .expect("a mixin may layer on another local mixin");
        let order: Vec<&str> = plan
            .nodes
            .iter()
            .map(|node| node.repository.as_str())
            .collect();
        assert_eq!(
            order,
            vec!["ghcr.io/acme/inner", "ghcr.io/acme/outer"],
            "a digest cannot be pinned before it exists, so the child publishes first"
        );
    }

    #[test]
    fn a_directory_and_the_document_inside_it_are_one_node() {
        let fs = MapFs::with(&[("/work/mixins/pg/lns.yaml", &mixin_yaml("postgres-tools"))]);
        let plan = plan_local_mixins(
            &fs,
            Path::new("/work"),
            &sandbox_naming(r#"["./mixins/pg/","./mixins/pg/lns.yaml"]"#),
            PARENT,
        )
        .expect("two spellings of one document are not a conflict");
        assert_eq!(
            plan.nodes.len(),
            1,
            "identity is the folded path of the document, so one document publishes once"
        );
    }

    #[test]
    fn a_mixin_reachable_from_itself_is_refused_naming_the_trail() {
        let fs = MapFs::with(&[(
            "/work/mixins/loop/lns.yaml",
            &mixin_yaml_naming("looper", &["./"]),
        )]);
        let err = plan_local_mixins(
            &fs,
            Path::new("/work"),
            &sandbox_naming(r#"["./mixins/loop/"]"#),
            PARENT,
        )
        .unwrap_err();
        let text = format!("{err:#}");
        assert!(
            text.contains("reachable from itself"),
            "a document whose digest depends on itself has no digest to pin; got: {text}"
        );
        assert!(
            text.contains("/work/mixins/loop/lns.yaml"),
            "the trail has to name the document so the author can break the loop; got: {text}"
        );
    }

    #[test]
    fn a_chain_as_deep_as_the_merge_limit_still_publishes() {
        let plan = plan_local_mixins(
            &chain_of(lns_artifact::merge::MAX_DEPTH),
            Path::new("/work"),
            &sandbox_naming(r#"["./mixins/"]"#),
            PARENT,
        )
        .expect("a graph a run resolves is a graph a push must publish");
        assert_eq!(plan.nodes.len(), lns_artifact::merge::MAX_DEPTH);
    }

    #[test]
    fn a_chain_one_deeper_than_the_merge_limit_refuses_before_anything_publishes() {
        let err = plan_local_mixins(
            &chain_of(lns_artifact::merge::MAX_DEPTH + 1),
            Path::new("/work"),
            &sandbox_naming(r#"["./mixins/"]"#),
            PARENT,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("deeper than"),
            "a graph too deep to resolve at run time would publish an artifact nobody can start; got: {err:#}"
        );
    }

    #[test]
    fn a_mixin_named_shallow_publishes_even_when_a_long_chain_also_reaches_it() {
        let deep = lns_artifact::merge::MAX_DEPTH;
        let mut entries: Vec<(String, String)> = vec![(
            "/work/mixins/shared/lns.yaml".to_string(),
            mixin_yaml("shared"),
        )];
        for level in 0..deep {
            let next = if level + 1 < deep {
                vec!["./deeper/"]
            } else {
                vec!["/work/mixins/shared/"]
            };
            entries.push((
                format!("/work/mixins/chain/{}lns.yaml", "deeper/".repeat(level)),
                mixin_yaml_naming(&format!("link{level}"), &next),
            ));
        }
        let refs: Vec<(&str, &str)> = entries
            .iter()
            .map(|(path, body)| (path.as_str(), body.as_str()))
            .collect();
        let fs = MapFs::with(&refs);
        let plan = plan_local_mixins(
            &fs,
            Path::new("/work"),
            &sandbox_naming(r#"["./mixins/shared/","./mixins/chain/"]"#),
            PARENT,
        )
        .expect(
            "the run resolves this graph by shortest path, so publishing must refuse by the same measure or it rejects what a run accepts",
        );
        assert_eq!(
            plan.nodes
                .iter()
                .filter(|node| node.repository.ends_with("/shared"))
                .count(),
            1,
            "the shared document publishes once, and both parents pin that one digest"
        );
    }

    #[test]
    fn a_local_entry_naming_the_decisions_file_is_refused() {
        let fs = MapFs::with(&[("/work/lns-local-mixin.yaml", &mixin_yaml("lns-local-mixin"))]);
        let err = plan_local_mixins(
            &fs,
            Path::new("/work"),
            &sandbox_naming(r#"["./lns-local-mixin.yaml"]"#),
            PARENT,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("never published"),
            "§8.1 keeps one machine's answers off every registry; got: {err:#}"
        );
    }

    #[test]
    fn a_local_entry_whose_document_is_a_sandbox_is_refused_by_kind() {
        let fs = MapFs::with(&[(
            "/work/mixins/pg/lns.yaml",
            "apiVersion: lns.run/v1\nkind: sandbox\nname: pg\nspec:\n  image: x:1\n",
        )]);
        let err = plan_local_mixins(
            &fs,
            Path::new("/work"),
            &sandbox_naming(r#"["./mixins/pg/"]"#),
            PARENT,
        )
        .unwrap_err();
        let text = format!("{err:#}");
        assert!(
            text.contains("./mixins/pg/") && text.contains("mixin"),
            "a sandbox merged as a mixin would carry a second launch, so the kind is checked before publishing; got: {text}"
        );
    }

    #[test]
    fn a_local_entry_that_cannot_be_read_names_the_file_it_looked_for() {
        let fs = MapFs::with(&[("/work/lns.yaml", "unused")]);
        let err = plan_local_mixins(
            &fs,
            Path::new("/work"),
            &sandbox_naming(r#"["./mixins/absent/"]"#),
            PARENT,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("/work/mixins/absent"),
            "the author needs the path that was looked for, not just that something was missing; got: {err:#}"
        );
    }

    #[test]
    fn a_digest_pinned_entry_is_not_something_the_walk_republishes() {
        let fs = MapFs::with(&[("/work/mixins/pg/lns.yaml", &mixin_yaml("postgres-tools"))]);
        let pinned = "ghcr.io/acme/remote@sha256:bbbb000000000000000000000000000000000000000000000000000000000000";
        let plan = plan_local_mixins(
            &fs,
            Path::new("/work"),
            &sandbox_naming(&format!(r#"["{pinned}","./mixins/pg/"]"#)),
            PARENT,
        )
        .expect("a document may mix pinned and local entries");
        assert_eq!(
            plan.nodes.len(),
            1,
            "an entry the author already pinned resolves for every consumer, so republishing it would put a second copy in a registry for nothing"
        );
        assert_eq!(plan.nodes[0].declared, "./mixins/pg/");
    }

    #[test]
    fn pinning_a_document_that_names_no_mixin_leaves_it_byte_for_byte() {
        let fs = MapFs::default();
        let doc =
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"x:1"}}"#;
        assert_eq!(
            pin_local_mixins(&fs, Path::new("/work"), doc, &[])
                .expect("a document with no mixins block is not an error"),
            doc.to_vec(),
            "re-serializing a document nothing changed would move its bytes, and the digest with them"
        );
    }

    #[test]
    fn pinning_rewrites_only_the_local_entries_and_the_result_still_validates() {
        let fs = MapFs::with(&[("/work/mixins/pg/lns.yaml", &mixin_yaml("postgres-tools"))]);
        let pinned = "ghcr.io/acme/remote@sha256:bbbb000000000000000000000000000000000000000000000000000000000000";
        let doc = sandbox_naming(&format!(r#"["./mixins/pg/","{pinned}"]"#));
        let published = vec![(
            PathBuf::from("/work/mixins/pg/lns.yaml"),
            CHILD_DIGEST.to_string(),
        )];
        let rewritten = pin_local_mixins(&fs, Path::new("/work"), &doc, &published)
            .expect("the local entry has a digest");
        let value: serde_json::Value = serde_json::from_slice(&rewritten).unwrap();
        let mixins = value["spec"]["mixins"].as_array().unwrap();
        assert_eq!(mixins[0], serde_json::json!(CHILD_DIGEST));
        assert_eq!(
            mixins[1],
            serde_json::json!(pinned),
            "an entry the author already pinned publishes as written"
        );
        lns_artifact::sandbox::parse_document(&rewritten).expect(
            "the rewritten document must still validate, or push would ship bytes no consumer can parse",
        );
    }

    #[test]
    fn every_spelling_of_one_published_document_is_pinned() {
        let fs = MapFs::with(&[("/work/mixins/pg/lns.yaml", &mixin_yaml("postgres-tools"))]);
        let doc =
            sandbox_naming(r#"["./mixins/pg/","./mixins/pg/lns.yaml","./mixins/../mixins/pg/"]"#);
        let plan = plan_local_mixins(&fs, Path::new("/work"), &doc, PARENT)
            .expect("three spellings of one document are one source");
        let published: Vec<(PathBuf, String)> = plan
            .nodes
            .iter()
            .map(|node| (node.document.clone(), CHILD_DIGEST.to_string()))
            .collect();
        let rewritten = pin_local_mixins(&fs, Path::new("/work"), &doc, &published)
            .expect("every entry resolves to the one document that published");
        let value: serde_json::Value = serde_json::from_slice(&rewritten).unwrap();
        for entry in value["spec"]["mixins"].as_array().unwrap() {
            let entry = entry.as_str().unwrap();
            assert!(
                !lns_artifact::sandbox::names_a_local_path(entry),
                "§6.1 lets no local path reach the published bytes, so pinning has to key on the document an entry resolves to and not on how it was spelled; got: {entry}"
            );
        }
    }

    #[test]
    fn pinning_refuses_a_local_entry_no_child_published() {
        let fs = MapFs::with(&[("/work/mixins/pg/lns.yaml", &mixin_yaml("postgres-tools"))]);
        let err = pin_local_mixins(
            &fs,
            Path::new("/work"),
            &sandbox_naming(r#"["./mixins/pg/"]"#),
            &[],
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("no digest can replace it"),
            "silently publishing a document that still carries a local path is the one outcome §6.1 forbids; got: {err:#}"
        );
    }

    #[test]
    fn the_child_tag_is_derived_from_the_bytes_it_names() {
        let digest = "sha256:dddd000000000000000000000000000000000000000000000000000000000000";
        assert_eq!(
            digest_derived_tag(digest),
            "sha256-dddd000000000000000000000000000000000000000000000000000000000000",
            "a registry that prunes untagged manifests must not be able to reclaim a pinned mixin, and a name derived from content cannot move"
        );
    }

    fn one_node_plan() -> MixinPlan {
        MixinPlan {
            nodes: vec![PlannedMixin {
                declared: "./mixins/pg/".to_string(),
                document: PathBuf::from("/work/mixins/pg/lns.yaml"),
                root: PathBuf::from("/work/mixins/pg"),
                repository: "ghcr.io/acme/postgres-tools".to_string(),
                bytes: Vec::new(),
            }],
        }
    }

    #[test]
    fn an_empty_plan_asks_nothing_and_prints_nothing() {
        let mut out = Vec::new();
        confirm_mixin_publication(
            &MixinPlan::default(),
            PARENT,
            false,
            &mut ScriptedTerminal::answering(&[]),
            &mut out,
        )
        .expect("a push with no local mixin publishes exactly one artifact, as it always did");
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.is_empty(),
            "a prompt nobody needs would change every existing push; got: {text}"
        );
    }

    #[test]
    fn the_disclosure_names_every_repository_the_push_would_create() {
        let mut out = Vec::new();
        confirm_mixin_publication(
            &one_node_plan(),
            PARENT,
            false,
            &mut ScriptedTerminal::answering(&["y\n"]),
            &mut out,
        )
        .expect("y accepts");
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("./mixins/pg/") && text.contains("ghcr.io/acme/postgres-tools"),
            "the author consents to a repository, so both the entry and the repository have to be on screen; got: {text}"
        );
    }

    #[test]
    fn a_declined_publication_says_nothing_was_published() {
        let mut out = Vec::new();
        let err = confirm_mixin_publication(
            &one_node_plan(),
            PARENT,
            false,
            &mut ScriptedTerminal::answering(&["n\n"]),
            &mut out,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("nothing was published"),
            "a push is not revertable, so a declined one has to say it uploaded nothing; got: {err:#}"
        );
    }

    #[test]
    fn a_publication_with_no_terminal_to_confirm_names_the_flag_that_accepts_it() {
        let mut out = Vec::new();
        let err = confirm_mixin_publication(
            &one_node_plan(),
            PARENT,
            false,
            &mut ScriptedTerminal::absent(),
            &mut out,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("--yes"),
            "a script cannot answer a prompt, so the refusal has to name the flag that accepts; got: {err:#}"
        );
    }

    #[test]
    fn assume_yes_publishes_without_asking() {
        let mut out = Vec::new();
        confirm_mixin_publication(
            &one_node_plan(),
            PARENT,
            true,
            &mut ScriptedTerminal::absent(),
            &mut out,
        )
        .expect("--yes accepts without a terminal");
        assert!(
            String::from_utf8_lossy(&out).is_empty(),
            "--yes is the author saying they already know; printing the question anyway is noise"
        );
    }
}
