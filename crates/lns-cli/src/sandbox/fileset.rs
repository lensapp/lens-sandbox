use std::path::Path;

use anyhow::{Context, Result, bail};
use lns_artifact::build::FileEntry;

use super::author::Fs;

/// Snapshot a fileset directory into pack-ready entries, refusing secret-shaped files anywhere in the tree — a fileset is baked into the artifact, so there is no keep/drop prompt to catch one later.
pub fn walk<F: Fs + ?Sized>(fs: &F, root: &Path) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    walk_into(fs, root, Path::new(""), &mut entries)?;
    Ok(entries)
}

fn walk_into<F: Fs + ?Sized>(
    fs: &F,
    dir: &Path,
    rel: &Path,
    out: &mut Vec<FileEntry>,
) -> Result<()> {
    let listed = fs
        .dir_entries(dir)
        .with_context(|| format!("reading fileset directory {}", dir.display()))?;
    for entry in listed {
        let entry_rel = rel.join(&entry.name);
        if crate::run::host_bind::looks_like_secret(&entry.name) {
            bail!(
                "fileset contains a secret-shaped file: {} — real secrets stay outside the workload",
                entry_rel.display()
            );
        }
        let entry_abs = dir.join(&entry.name);
        if entry.dir {
            walk_into(fs, &entry_abs, &entry_rel, out)?;
        } else {
            out.push(FileEntry {
                path: entry_rel
                    .to_str()
                    .with_context(|| format!("non-utf8 fileset path {}", entry_rel.display()))?
                    .to_string(),
                data: fs
                    .read(&entry_abs)
                    .with_context(|| format!("reading fileset file {}", entry_abs.display()))?,
            });
        }
    }
    Ok(())
}

/// The offline validate/run guard: every path fileset must name a readable, secret-free directory in the project.
pub fn path_fileset_problems<F: Fs + ?Sized>(
    fs: &F,
    project_dir: &Path,
    definition: &lns_artifact::sandbox::Definition,
) -> Vec<String> {
    definition
        .spec
        .filesets
        .iter()
        .filter_map(|fileset| {
            let path = fileset.path.as_deref()?;
            walk(fs, &project_dir.join(path))
                .err()
                .map(|e| format!("fileset {path}: {e:#}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::test_support::MapFs;

    #[test]
    fn walk_snapshots_nested_files_with_project_relative_paths() {
        let fs = MapFs::with(&[
            ("/work/skills/prompts.md", "p"),
            ("/work/skills/deep/notes.md", "n"),
        ]);
        let entries = walk(&fs, Path::new("/work/skills")).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["deep/notes.md", "prompts.md"]);
        assert_eq!(entries[1].data, b"p");
    }

    #[test]
    fn walk_refuses_a_secret_shaped_file_anywhere_in_the_tree() {
        let fs = MapFs::with(&[
            ("/work/skills/prompts.md", "p"),
            ("/work/skills/deep/.env", "TOKEN=x"),
        ]);
        let err = walk(&fs, Path::new("/work/skills")).unwrap_err();
        assert!(
            format!("{err:#}").contains("secret-shaped file: deep/.env"),
            "got: {err:#}"
        );
    }

    #[test]
    fn walk_surfaces_a_missing_directory_with_its_path() {
        let fs = MapFs::default();
        let err = walk(&fs, Path::new("/work/skills")).unwrap_err();
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
        let err = walk(&fs, Path::new("/work/skills")).unwrap_err();
        assert!(
            format!("{err:#}").contains("reading fileset file /work/skills/prompts.md"),
            "got: {err:#}"
        );
    }

    #[test]
    fn path_fileset_problems_reports_only_broken_path_entries() {
        let fs = MapFs::with(&[("/work/skills/prompts.md", "p")]);
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"s"},"spec":{"image":"x:1","filesets":[{"path":"./skills","mountPath":"/a"},{"path":"./missing","mountPath":"/b"},{"ref":"reg/skills@sha256:abc","mountPath":"/c"}]}}"#,
        )
        .unwrap();
        let problems = path_fileset_problems(&fs, Path::new("/work"), &def);
        assert_eq!(problems.len(), 1, "got: {problems:?}");
        assert!(problems[0].contains("./missing"), "got: {problems:?}");
    }
}
