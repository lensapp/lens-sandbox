//! Snapshotting a fileset directory into pack-ready entries, here rather than in
//! a caller because §3.2.3's connector exception living in two walkers is how a
//! secret eventually ships.

use std::io;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::build::FileEntry;
use crate::spec::Kind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub dir: bool,
    pub mode: u32,
}

/// The two reads a snapshot needs, kept narrow so a caller's wider filesystem port is not a prerequisite.
pub trait SnapshotFs {
    fn read_limited(&self, path: &Path, max_bytes: u64) -> io::Result<Vec<u8>>;
    fn dir_entries(&self, dir: &Path) -> io::Result<Vec<DirEntry>>;
}

/// Snapshot a fileset directory into pack-ready entries, refusing secret-shaped files anywhere in the tree — a fileset is baked into the artifact, so there is no keep/drop prompt to catch one later. A connector is §3.2.3's one exception: it may name such a file, and the caller checks the content for a declared placeholder instead.
pub fn walk<F: SnapshotFs + ?Sized>(fs: &F, root: &Path, kind: Kind) -> Result<Vec<FileEntry>> {
    walk_under(
        fs,
        root,
        &WalkRules {
            kind,
            max_bytes: crate::build::MAX_FILESET_BYTES,
            max_entries: crate::build::MAX_FILESET_ENTRIES,
        },
    )
}

/// What one walk enforces on the tree it reads: which kind is walking, and the two limits a packed layer may not exceed.
struct WalkRules {
    kind: Kind,
    max_bytes: u64,
    max_entries: usize,
}

fn walk_under<F: SnapshotFs + ?Sized>(
    fs: &F,
    root: &Path,
    rules: &WalkRules,
) -> Result<Vec<FileEntry>> {
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

fn walk_into<F: SnapshotFs + ?Sized>(
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
        if rules.kind != Kind::Connector && crate::sandbox::looks_like_secret_name(&entry.name) {
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

/// Derive a directory listing from a flat path-keyed map, so every in-memory fake shares one implementation.
pub fn map_dir_entries<'a>(
    paths: impl Iterator<Item = &'a std::path::PathBuf>,
    dir: &Path,
) -> io::Result<Vec<DirEntry>> {
    let mut seen: std::collections::BTreeMap<String, bool> = Default::default();
    for key in paths {
        let Ok(rest) = key.strip_prefix(dir) else {
            continue;
        };
        let mut components = rest.components();
        let Some(std::path::Component::Normal(first)) = components.next() else {
            continue;
        };
        let nested = components.next().is_some();
        let slot = seen
            .entry(first.to_string_lossy().into_owned())
            .or_default();
        *slot = *slot || nested;
    }
    if seen.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no such directory"));
    }
    Ok(seen
        .into_iter()
        .map(|(name, dir)| DirEntry {
            name,
            dir,
            mode: if dir { 0o755 } else { 0o644 },
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[derive(Default)]
    struct MapFs {
        files: BTreeMap<PathBuf, Vec<u8>>,
    }

    impl MapFs {
        fn with(entries: &[(&str, &[u8])]) -> Self {
            Self {
                files: entries
                    .iter()
                    .map(|(p, d)| (PathBuf::from(p), d.to_vec()))
                    .collect(),
            }
        }
    }

    impl SnapshotFs for MapFs {
        fn read_limited(&self, path: &Path, _max_bytes: u64) -> io::Result<Vec<u8>> {
            self.files
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such file"))
        }

        fn dir_entries(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
            map_dir_entries(self.files.keys(), dir)
        }
    }

    #[test]
    fn a_nested_tree_snapshots_with_paths_relative_to_the_root() {
        let fs = MapFs::with(&[
            ("/work/files/top.txt", b"a"),
            ("/work/files/nested/deep.txt", b"b"),
        ]);
        let entries = walk(&fs, Path::new("/work/files"), Kind::Sandbox).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["nested/deep.txt", "top.txt"]);
        assert_eq!(entries[0].data, b"b");
        assert_eq!(
            entries[1].data, b"a",
            "the bytes must reach the entry verbatim"
        );
    }

    #[test]
    fn each_files_mode_carries_so_the_exec_bit_survives_packing() {
        struct Moded;
        impl SnapshotFs for Moded {
            fn read_limited(&self, _: &Path, _: u64) -> io::Result<Vec<u8>> {
                Ok(b"#!/bin/sh".to_vec())
            }
            fn dir_entries(&self, _: &Path) -> io::Result<Vec<DirEntry>> {
                Ok(vec![
                    DirEntry {
                        name: "run.sh".to_string(),
                        dir: false,
                        mode: 0o755,
                    },
                    DirEntry {
                        name: "notes.md".to_string(),
                        dir: false,
                        mode: 0o644,
                    },
                ])
            }
        }
        let modes: BTreeMap<String, u32> = walk(&Moded, Path::new("/f"), Kind::Sandbox)
            .unwrap()
            .into_iter()
            .map(|e| (e.path, e.mode))
            .collect();
        assert_eq!(
            modes["run.sh"], 0o755,
            "an executable must keep its exec bit"
        );
        assert_eq!(modes["notes.md"], 0o644);
    }

    #[test]
    fn a_secret_shaped_file_refuses_a_sandbox_fileset() {
        // A fileset is baked into the artifact, so there is no later keep/drop prompt to catch one.
        let fs = MapFs::with(&[("/work/files/id_rsa", b"key")]);
        let err = walk(&fs, Path::new("/work/files"), Kind::Sandbox)
            .unwrap_err()
            .to_string();
        assert!(err.contains("id_rsa"), "{err}");
    }

    #[test]
    fn a_secret_shaped_file_is_allowed_for_a_connector() {
        // §3.2.3's one exception: a connector may name such a file, and the caller checks its content for a declared placeholder.
        let fs = MapFs::with(&[("/work/files/credentials.json", b"{}")]);
        assert_eq!(
            walk(&fs, Path::new("/work/files"), Kind::Connector)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_secret_shaped_file_nested_deep_is_named_by_its_path_inside_the_fileset() {
        // The author needs the path they wrote, not the absolute host path the walk happens to be at.
        let fs = MapFs::with(&[("/work/files/a/b/id_rsa", b"key")]);
        let err = walk(&fs, Path::new("/work/files"), Kind::Sandbox)
            .unwrap_err()
            .to_string();
        assert!(err.contains("secret-shaped file: a/b/id_rsa"), "{err}");
    }

    #[test]
    fn a_mixin_gets_the_same_refusal_as_a_sandbox() {
        // Only a connector is excepted, so every other kind must refuse.
        let fs = MapFs::with(&[("/work/files/id_rsa", b"key")]);
        assert!(walk(&fs, Path::new("/work/files"), Kind::Mixin).is_err());
    }

    struct Failing {
        listing: bool,
    }

    impl SnapshotFs for Failing {
        fn read_limited(&self, _: &Path, _: u64) -> io::Result<Vec<u8>> {
            Err(io::Error::other("device error"))
        }

        fn dir_entries(&self, _: &Path) -> io::Result<Vec<DirEntry>> {
            if self.listing {
                return Err(io::Error::other("permission denied"));
            }
            Ok(vec![DirEntry {
                name: "top.txt".to_string(),
                dir: false,
                mode: 0o644,
            }])
        }
    }

    #[test]
    fn an_unreadable_file_names_the_path_it_could_not_read() {
        let err = walk(
            &Failing { listing: false },
            Path::new("/work/files"),
            Kind::Sandbox,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("top.txt"), "{err}");
    }

    #[test]
    fn an_unreadable_directory_names_the_directory() {
        let err = walk(
            &Failing { listing: true },
            Path::new("/work/files"),
            Kind::Sandbox,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("/work/files"), "{err}");
    }

    #[test]
    fn a_tree_over_the_entry_limit_refuses() {
        let rules = WalkRules {
            kind: Kind::Sandbox,
            max_bytes: 1024,
            max_entries: 1,
        };
        let fs = MapFs::with(&[("/f/a.txt", b"a"), ("/f/b.txt", b"b")]);
        let err = walk_under(&fs, Path::new("/f"), &rules)
            .unwrap_err()
            .to_string();
        assert!(err.contains("more than 1 files"), "{err}");
    }

    #[test]
    fn a_tree_over_the_byte_limit_refuses() {
        let rules = WalkRules {
            kind: Kind::Sandbox,
            max_bytes: 4,
            max_entries: 100,
        };
        let fs = MapFs::with(&[("/f/a.txt", b"aaaaaaaa")]);
        let err = walk_under(&fs, Path::new("/f"), &rules)
            .unwrap_err()
            .to_string();
        assert!(err.contains("4-byte limit"), "{err}");
    }

    #[test]
    fn a_flat_map_listing_reports_directories_and_files_apart() {
        let paths = [
            PathBuf::from("/f/top.txt"),
            PathBuf::from("/f/sub/deep.txt"),
        ];
        let listed = map_dir_entries(paths.iter(), Path::new("/f")).unwrap();
        assert_eq!(
            listed,
            vec![
                DirEntry {
                    name: "sub".to_string(),
                    dir: true,
                    mode: 0o755
                },
                DirEntry {
                    name: "top.txt".to_string(),
                    dir: false,
                    mode: 0o644
                },
            ]
        );
    }

    #[test]
    fn a_listing_with_nothing_under_the_directory_is_not_found() {
        // A fake must answer a missing directory the way a real one does, or a walk over it would report an empty fileset instead of an error.
        for outside in [PathBuf::from("/other/top.txt"), PathBuf::from("/f")] {
            let paths = [outside];
            assert_eq!(
                map_dir_entries(paths.iter(), Path::new("/f"))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::NotFound
            );
        }
    }

    #[test]
    fn a_name_seen_as_both_file_and_directory_is_reported_as_a_directory() {
        // Otherwise the walk would try to read a directory as a file.
        let paths = [PathBuf::from("/f/x"), PathBuf::from("/f/x/inner.txt")];
        let listed = map_dir_entries(paths.iter(), Path::new("/f")).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].dir, "{listed:?}");
    }
}
