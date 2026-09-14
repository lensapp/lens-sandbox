use anyhow::{Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpperKind {
    Directory,
    Regular,
    Symlink,
    /// Overlayfs marks a deleted lower entry with a character device, so the upper's only char devices are the deletions this run made.
    Whiteout,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpperEntry {
    pub name: String,
    pub kind: UpperKind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

pub(crate) trait UpperTree {
    fn read_dir(&self, path: &str) -> Result<Vec<UpperEntry>>;
    fn read_file(&self, path: &str) -> Result<Vec<u8>>;
    fn read_link(&self, path: &str) -> Result<String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    Directory {
        path: String,
        mode: u32,
        uid: u32,
        gid: u32,
    },
    Regular {
        path: String,
        mode: u32,
        uid: u32,
        gid: u32,
        bytes: Vec<u8>,
    },
    Symlink {
        path: String,
        target: String,
        uid: u32,
        gid: u32,
    },
    Removed {
        path: String,
    },
}

impl Change {
    pub(crate) fn path(&self) -> &str {
        match self {
            Self::Directory { path, .. }
            | Self::Regular { path, .. }
            | Self::Symlink { path, .. }
            | Self::Removed { path } => path,
        }
    }
}

/// What one guest left in the overlay's upper layer, parents before children so a tar of it can be unpacked in order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ChangeSet {
    pub changes: Vec<Change>,
}

pub(crate) fn capture(tree: &dyn UpperTree) -> Result<ChangeSet> {
    let mut changes = Vec::new();
    walk(tree, "", &mut changes)?;
    Ok(ChangeSet { changes })
}

fn walk(tree: &dyn UpperTree, dir: &str, changes: &mut Vec<Change>) -> Result<()> {
    let mut entries = tree
        .read_dir(dir)
        .with_context(|| format!("reading the upper layer's {}", display_dir(dir)))?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in entries {
        let path = join(dir, &entry.name);
        match entry.kind {
            UpperKind::Whiteout => changes.push(Change::Removed { path }),
            UpperKind::Directory => {
                changes.push(Change::Directory {
                    path: path.clone(),
                    mode: entry.mode,
                    uid: entry.uid,
                    gid: entry.gid,
                });
                walk(tree, &path, changes)?;
            }
            UpperKind::Regular => {
                let bytes = tree
                    .read_file(&path)
                    .with_context(|| format!("reading {path} out of the upper layer"))?;
                changes.push(Change::Regular {
                    path,
                    mode: entry.mode,
                    uid: entry.uid,
                    gid: entry.gid,
                    bytes,
                });
            }
            UpperKind::Symlink => {
                let target = tree
                    .read_link(&path)
                    .with_context(|| format!("reading the symlink {path} in the upper layer"))?;
                changes.push(Change::Symlink {
                    path,
                    target,
                    uid: entry.uid,
                    gid: entry.gid,
                });
            }
            UpperKind::Unsupported => {}
        }
    }
    Ok(())
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

fn display_dir(dir: &str) -> String {
    if dir.is_empty() {
        "root".to_string()
    } else {
        dir.to_string()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    pub(crate) struct FakeUpper {
        dirs: BTreeMap<String, Vec<UpperEntry>>,
        files: BTreeMap<String, Vec<u8>>,
        links: BTreeMap<String, String>,
    }

    impl FakeUpper {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        fn entry(&mut self, dir: &str, name: &str, kind: UpperKind, mode: u32) -> &mut Self {
            self.dirs
                .entry(dir.to_string())
                .or_default()
                .push(UpperEntry {
                    name: name.to_string(),
                    kind,
                    mode,
                    uid: 0,
                    gid: 0,
                });
            self
        }

        pub(crate) fn dir(&mut self, path: &str, mode: u32) -> &mut Self {
            let (parent, name) = split(path);
            self.dirs.entry(path.to_string()).or_default();
            self.entry(parent, name, UpperKind::Directory, mode)
        }

        pub(crate) fn file(&mut self, path: &str, mode: u32, bytes: &[u8]) -> &mut Self {
            let (parent, name) = split(path);
            self.files.insert(path.to_string(), bytes.to_vec());
            self.entry(parent, name, UpperKind::Regular, mode)
        }

        pub(crate) fn symlink(&mut self, path: &str, target: &str) -> &mut Self {
            let (parent, name) = split(path);
            self.links.insert(path.to_string(), target.to_string());
            self.entry(parent, name, UpperKind::Symlink, 0o777)
        }

        pub(crate) fn whiteout(&mut self, path: &str) -> &mut Self {
            let (parent, name) = split(path);
            self.entry(parent, name, UpperKind::Whiteout, 0)
        }

        pub(crate) fn unsupported(&mut self, path: &str) -> &mut Self {
            let (parent, name) = split(path);
            self.entry(parent, name, UpperKind::Unsupported, 0o644)
        }
    }

    fn split(path: &str) -> (&str, &str) {
        match path.rsplit_once('/') {
            Some((parent, name)) => (parent, name),
            None => ("", path),
        }
    }

    impl UpperTree for FakeUpper {
        fn read_dir(&self, path: &str) -> Result<Vec<UpperEntry>> {
            self.dirs
                .get(path)
                .cloned()
                .with_context(|| format!("no such directory {path}"))
        }

        fn read_file(&self, path: &str) -> Result<Vec<u8>> {
            self.files
                .get(path)
                .cloned()
                .with_context(|| format!("no such file {path}"))
        }

        fn read_link(&self, path: &str) -> Result<String> {
            self.links
                .get(path)
                .cloned()
                .with_context(|| format!("no such symlink {path}"))
        }
    }

    #[test]
    fn a_created_file_and_a_deleted_one_are_the_whole_change_set() {
        let mut upper = FakeUpper::new();
        upper
            .file("spike-created", 0o644, b"built-by-lns\n")
            .dir("etc", 0o755)
            .whiteout("etc/alpine-release");

        let captured = capture(&upper).unwrap();

        assert_eq!(
            captured.changes,
            vec![
                Change::Directory {
                    path: "etc".into(),
                    mode: 0o755,
                    uid: 0,
                    gid: 0,
                },
                Change::Removed {
                    path: "etc/alpine-release".into(),
                },
                Change::Regular {
                    path: "spike-created".into(),
                    mode: 0o644,
                    uid: 0,
                    gid: 0,
                    bytes: b"built-by-lns\n".to_vec(),
                },
            ]
        );
    }

    #[test]
    fn a_directory_is_emitted_before_the_children_it_holds() {
        let mut upper = FakeUpper::new();
        upper
            .dir("opt", 0o755)
            .dir("opt/tool", 0o750)
            .file("opt/tool/bin", 0o755, b"#!/bin/sh\n");

        let captured = capture(&upper).unwrap();
        let paths: Vec<&str> = captured.changes.iter().map(Change::path).collect();

        assert_eq!(paths, vec!["opt", "opt/tool", "opt/tool/bin"]);
    }

    #[test]
    fn a_symlink_keeps_its_target() {
        let mut upper = FakeUpper::new();
        upper.symlink("bin-sh", "busybox");

        assert_eq!(
            capture(&upper).unwrap().changes,
            vec![Change::Symlink {
                path: "bin-sh".into(),
                target: "busybox".into(),
                uid: 0,
                gid: 0,
            }]
        );
    }

    #[test]
    fn an_entry_kind_no_oci_layer_can_carry_is_dropped() {
        let mut upper = FakeUpper::new();
        upper.unsupported("dev-sda").file("kept", 0o644, b"x");

        let captured = capture(&upper).unwrap();
        let paths: Vec<&str> = captured.changes.iter().map(Change::path).collect();

        assert_eq!(paths, vec!["kept"]);
    }

    #[test]
    fn an_unreadable_root_names_the_upper_layer() {
        let err = capture(&FakeUpper::new()).unwrap_err();
        assert!(
            format!("{err:#}").contains("the upper layer's root"),
            "{err:#}"
        );
    }

    #[test]
    fn an_unreadable_subdirectory_names_itself() {
        let mut upper = FakeUpper::new();
        upper.dirs.insert(
            String::new(),
            vec![UpperEntry {
                name: "gone".into(),
                kind: UpperKind::Directory,
                mode: 0o755,
                uid: 0,
                gid: 0,
            }],
        );

        let err = capture(&upper).unwrap_err();
        assert!(format!("{err:#}").contains("upper layer's gone"), "{err:#}");
    }

    #[test]
    fn a_file_whose_content_will_not_read_names_the_file() {
        let mut upper = FakeUpper::new();
        upper.entry("", "vanished", UpperKind::Regular, 0o644);

        let err = capture(&upper).unwrap_err();
        assert!(
            format!("{err:#}").contains("reading vanished out of the upper layer"),
            "{err:#}"
        );
    }

    #[test]
    fn a_symlink_whose_target_will_not_read_names_the_symlink() {
        let mut upper = FakeUpper::new();
        upper.entry("", "dangling", UpperKind::Symlink, 0o777);

        let err = capture(&upper).unwrap_err();
        assert!(
            format!("{err:#}").contains("the symlink dangling in the upper layer"),
            "{err:#}"
        );
    }

    #[test]
    fn every_change_reports_the_path_it_is_about() {
        assert_eq!(
            Change::Removed {
                path: "etc/x".into()
            }
            .path(),
            "etc/x"
        );
        assert_eq!(
            Change::Symlink {
                path: "l".into(),
                target: "t".into(),
                uid: 0,
                gid: 0
            }
            .path(),
            "l"
        );
    }
}
