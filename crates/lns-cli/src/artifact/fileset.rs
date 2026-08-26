use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use lns_artifact::build::FileEntry;
use lns_artifact::spec::Kind;

use super::author::Fs;

/// Snapshot a fileset directory into pack-ready entries, refusing secret-shaped files anywhere in the tree — a fileset is baked into the artifact, so there is no keep/drop prompt to catch one later. A connector is §3.2.3's one exception: it may name such a file, and the caller checks the content for a declared placeholder instead.
pub fn walk<F: Fs + ?Sized>(fs: &F, root: &Path, kind: Kind) -> Result<Vec<FileEntry>> {
    walk_under(
        fs,
        root,
        &WalkRules {
            kind,
            max_bytes: lns_artifact::build::MAX_FILESET_BYTES,
            max_entries: lns_artifact::build::MAX_FILESET_ENTRIES,
        },
    )
}

/// What one walk enforces on the tree it reads: which kind is walking, and the two limits a packed layer may not exceed.
struct WalkRules {
    kind: Kind,
    max_bytes: u64,
    max_entries: usize,
}

fn walk_under<F: Fs + ?Sized>(fs: &F, root: &Path, rules: &WalkRules) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    let mut total_bytes = 0;
    walk_into(
        fs,
        root,
        Path::new(""),
        rules,
        &mut entries,
        &mut total_bytes,
    )?;
    Ok(entries)
}

fn walk_into<F: Fs + ?Sized>(
    fs: &F,
    dir: &Path,
    rel: &Path,
    rules: &WalkRules,
    out: &mut Vec<FileEntry>,
    total_bytes: &mut u64,
) -> Result<()> {
    let listed = fs
        .dir_entries(dir)
        .with_context(|| format!("reading fileset directory {}", dir.display()))?;
    for entry in listed {
        let entry_rel = rel.join(&entry.name);
        if rules.kind != Kind::Connector && crate::run::host_bind::looks_like_secret(&entry.name) {
            bail!(
                "fileset contains a secret-shaped file: {} — real secrets stay outside the workload",
                entry_rel.display()
            );
        }
        let entry_abs = dir.join(&entry.name);
        if entry.dir {
            walk_into(fs, &entry_abs, &entry_rel, rules, out, total_bytes)?;
        } else {
            if out.len() >= rules.max_entries {
                bail!("fileset contains more than {} files", rules.max_entries);
            }
            let remaining = rules.max_bytes.saturating_sub(*total_bytes);
            let data = fs
                .read_limited(&entry_abs, remaining)
                .with_context(|| format!("reading fileset file {}", entry_abs.display()))?;
            if data.len() as u64 > remaining {
                bail!("fileset content exceeds the {}-byte limit", rules.max_bytes);
            }
            *total_bytes += data.len() as u64;
            out.push(FileEntry {
                path: entry_rel
                    .to_str()
                    .with_context(|| format!("non-utf8 fileset path {}", entry_rel.display()))?
                    .to_string(),
                data,
                mode: entry.mode,
            });
        }
    }
    Ok(())
}

/// The offline validate/run guard: every path fileset must name a readable directory in the project that its kind is allowed to ship. Reading the directory is what lets a connector be held to §3.2.3 here rather than only at push.
pub fn path_fileset_problems<F: Fs + ?Sized>(
    fs: &F,
    project_dir: &Path,
    paths: &[&str],
    methods: &[lns_artifact::connector::Method],
    kind: Kind,
) -> Vec<String> {
    let mut problems = Vec::new();
    let mut read: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for path in paths {
        match walk(fs, &project_dir.join(path), kind) {
            Ok(entries) => {
                read.insert(
                    (*path).to_string(),
                    lns_artifact::build::text_by_name(&entries),
                );
            }
            Err(e) => problems.push(format!("fileset {path}: {e:#}")),
        }
    }
    if problems.is_empty() {
        for method in methods {
            if let Err(e) =
                lns_artifact::connector::refuse_a_secret_shaped_file_carrying_no_declared_placeholder(
                    method, &read,
                )
            {
                problems.push(format!("{e:#}"));
            }
        }
    }
    problems
}

/// The document a local mixin reference names: a directory is read as the `lns.yaml` inside it, and a path naming the document is that document.
pub fn local_mixin_document<F: Fs + ?Sized>(
    fs: &F,
    project_dir: &Path,
    reference: &str,
) -> std::path::PathBuf {
    let named = lns_artifact::sandbox::fold_path(&project_dir.join(reference));
    if fs.is_dir(&named) {
        named.join("lns.yaml")
    } else {
        named
    }
}

/// A directory mixin is a file on this machine, so `validate` can say the document names one that is not there rather than leaving it to the run.
pub fn directory_mixin_problems<F: Fs + ?Sized>(
    fs: &F,
    project_dir: &Path,
    mixins: &[String],
) -> Vec<String> {
    mixins
        .iter()
        .filter(|reference| lns_artifact::sandbox::names_a_local_path(reference))
        .filter_map(|reference| {
            let document = local_mixin_document(fs, project_dir, reference);
            fs.read_to_string(&document)
                .err()
                .map(|e| format!("mixin {reference}: reading {}: {e}", document.display()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::test_support::MapFs;

    fn definition(mixins: &str) -> lns_artifact::sandbox::Definition {
        lns_artifact::sandbox::parse(
            format!(
                r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{{"image":"x:1","mixins":{mixins}}}}}"#
            )
            .as_bytes(),
        )
        .expect("the fixture is a valid definition")
    }

    #[test]
    fn validate_accepts_a_mixin_named_by_its_document() {
        let fs = MapFs::with(&[("/work/mixins/pg/lns.yaml", "kind: mixin")]);
        let problems = directory_mixin_problems(
            &fs,
            Path::new("/work"),
            &definition(r#"["./mixins/pg/lns.yaml"]"#).spec.mixins,
        );
        assert!(
            problems.is_empty(),
            "a run resolves this entry, so validate calling it broken is validate being wrong; got {problems:?}"
        );
    }

    #[test]
    fn validate_names_a_mixin_path_that_holds_no_document() {
        let fs = MapFs::with(&[("/work/mixins/present/lns.yaml", "kind: mixin")]);
        let problems = directory_mixin_problems(
            &fs,
            Path::new("/work"),
            &definition(r#"["./mixins/present","./mixins/absent"]"#)
                .spec
                .mixins,
        );
        assert_eq!(
            problems.len(),
            1,
            "a typo in a directory name is cheapest to correct at author time; got {problems:?}"
        );
        assert!(
            problems[0].contains("/work/mixins/absent"),
            "a path that is neither a directory nor a document is named as the author wrote it, rather than as a file inside a directory that is not there; got {problems:?}"
        );
    }

    #[test]
    fn validate_leaves_a_published_mixin_to_the_run() {
        let problems = directory_mixin_problems(
            &MapFs::with(&[]),
            Path::new("/work"),
            &definition(&format!(
                r#"["ghcr.io/acme/obs@sha256:{}"]"#,
                "c".repeat(64)
            ))
            .spec
            .mixins,
        );
        assert!(
            problems.is_empty(),
            "validate reads one document offline, so a reference it would have to pull is not its to check; got {problems:?}"
        );
    }

    #[test]
    fn walk_snapshots_nested_files_with_project_relative_paths() {
        let fs = MapFs::with(&[
            ("/work/skills/prompts.md", "p"),
            ("/work/skills/deep/notes.md", "n"),
        ]);
        let entries = walk(&fs, Path::new("/work/skills"), Kind::Sandbox).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["deep/notes.md", "prompts.md"]);
        assert_eq!(entries[1].data, b"p");
    }

    #[test]
    fn walk_carries_each_files_mode_so_the_exec_bit_survives_packing() {
        let fs = MapFs {
            executables: ["run.sh".to_string()].into_iter().collect(),
            ..MapFs::with(&[
                ("/work/skills/run.sh", "#!/bin/sh"),
                ("/work/skills/notes.md", "n"),
            ])
        };
        let entries = walk(&fs, Path::new("/work/skills"), Kind::Sandbox).unwrap();
        let modes: std::collections::BTreeMap<&str, u32> =
            entries.iter().map(|e| (e.path.as_str(), e.mode)).collect();
        assert_eq!(
            modes["run.sh"], 0o755,
            "an executable must keep its exec bit"
        );
        assert_eq!(modes["notes.md"], 0o644);
    }

    #[test]
    fn walk_refuses_a_secret_shaped_file_anywhere_in_the_tree() {
        let fs = MapFs::with(&[
            ("/work/skills/prompts.md", "p"),
            ("/work/skills/deep/.env", "TOKEN=x"),
        ]);
        let err = walk(&fs, Path::new("/work/skills"), Kind::Sandbox).unwrap_err();
        assert!(
            format!("{err:#}").contains("secret-shaped file: deep/.env"),
            "got: {err:#}"
        );
    }

    #[test]
    fn walk_surfaces_a_missing_directory_with_its_path() {
        let fs = MapFs::default();
        let err = walk(&fs, Path::new("/work/skills"), Kind::Sandbox).unwrap_err();
        assert!(
            format!("{err:#}").contains("reading fileset directory /work/skills"),
            "got: {err:#}"
        );
    }

    #[test]
    fn walk_surfaces_an_unreadable_file_with_its_path() {
        let fs = MapFs {
            unreadable: true,
            ..MapFs::with(&[("/work/skills/prompts.md", "p")])
        };
        let err = walk(&fs, Path::new("/work/skills"), Kind::Sandbox).unwrap_err();
        assert!(
            format!("{err:#}").contains("reading fileset file /work/skills/prompts.md"),
            "got: {err:#}"
        );
    }

    #[test]
    fn path_fileset_problems_reports_only_broken_path_entries() {
        let fs = MapFs::with(&[("/work/skills/prompts.md", "p")]);
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"s","spec":{"image":"x:1","filesets":[{"path":"./skills","guestPath":"/a"},{"path":"./missing","guestPath":"/b"},{"inline":{"a.md":"x"},"guestPath":"/c"}]}}"#,
        )
        .unwrap();
        let paths: Vec<&str> = lns_artifact::merge::path_filesets(&def.spec)
            .map(|(_, _, path)| path)
            .collect();
        let problems = path_fileset_problems(&fs, Path::new("/work"), &paths, &[], Kind::Sandbox);
        assert_eq!(problems.len(), 1, "got: {problems:?}");
        assert!(problems[0].contains("./missing"), "got: {problems:?}");
    }

    fn connector_with_a_path_fileset() -> lns_artifact::connector::ConnectorDefinition {
        lns_artifact::connector::parse(
                br#"{"apiVersion":"lns.run/v1","kind":"connector","name":"some-provider","spec":{"serves":["api.some-provider.example"],"methods":[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}],"filesets":[{"path":"./some-provider","guestPath":"/home/agent/.some-provider"}]}]}}"#,
            )
        .expect("the fixture is a valid connector")
    }

    #[test]
    fn validate_accepts_a_connector_path_file_that_carries_the_declared_placeholder() {
        let fs = MapFs::with(&[(
            "/work/some-provider/credentials.json",
            r#"{"token":"some_LNSPLACEHOLDER0000000000"}"#,
        )]);
        let def = connector_with_a_path_fileset();
        let problems = path_fileset_problems(
            &fs,
            Path::new("/work"),
            &["./some-provider"],
            &def.spec.methods,
            Kind::Connector,
        );
        assert!(
            problems.is_empty(),
            "§3.2.3 makes a connector the one kind whose fileset exists to write this exact file, so refusing it by name leaves the placeholder rule with nothing to check; got {problems:?}"
        );
    }

    #[test]
    fn validate_refuses_a_connector_path_file_that_declares_no_placeholder() {
        let fs = MapFs::with(&[(
            "/work/some-provider/credentials.json",
            r#"{"token":"sk-live-real"}"#,
        )]);
        let def = connector_with_a_path_fileset();
        let problems = path_fileset_problems(
            &fs,
            Path::new("/work"),
            &["./some-provider"],
            &def.spec.methods,
            Kind::Connector,
        );
        assert_eq!(problems.len(), 1, "got {problems:?}");
        assert!(
            problems[0].contains("carries no placeholder"),
            "§5 runs the §3.2.3 read over a `path` directory beside the document, so validate must refuse this for the same reason push does rather than by the name alone; got {problems:?}"
        );
    }

    fn limits(max_bytes: u64, max_entries: usize) -> WalkRules {
        WalkRules {
            kind: Kind::Sandbox,
            max_bytes,
            max_entries,
        }
    }

    #[test]
    fn walk_refuses_content_beyond_the_aggregate_limit() {
        let fs = MapFs::with(&[("/work/skills/a", "123"), ("/work/skills/b", "456")]);
        let err = walk_under(&fs, Path::new("/work/skills"), &limits(5, 10)).unwrap_err();
        assert!(format!("{err:#}").contains("5-byte limit"));
    }

    #[test]
    fn walk_refuses_more_than_the_entry_limit() {
        let fs = MapFs::with(&[("/work/skills/a", "1"), ("/work/skills/b", "2")]);
        let err = walk_under(&fs, Path::new("/work/skills"), &limits(10, 1)).unwrap_err();
        assert!(format!("{err:#}").contains("more than 1 files"));
    }
}
