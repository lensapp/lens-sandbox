//! The build context: what a `COPY` or an `ADD` moves out of the directory beside the document,
//! and where it lands in the image. One layer per instruction, made on the host, so a copied file
//! never passes through a guest that could see it.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use super::executor::CopyStep;
use super::upper::{Change, ChangeSet};

/// The mode a directory the copy has to create is given, as the classic builder does.
const CREATED_DIRECTORY_MODE: u32 = 0o755;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryKind {
    Directory,
    Regular,
    Symlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Meta {
    pub kind: EntryKind,
    pub mode: u32,
}

/// The build context as the host reads it: absent is an answer, not an error.
pub(crate) trait ContextFs {
    fn meta(&self, path: &Path) -> Result<Option<Meta>>;
    fn entries(&self, path: &Path) -> Result<Vec<String>>;
    fn read(&self, path: &Path) -> Result<Vec<u8>>;
    fn read_link(&self, path: &Path) -> Result<String>;
}

/// Who a copied file belongs to in the image: `--chown` decides, and root owns what it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Owner {
    uid: u32,
    gid: u32,
}

pub(crate) fn stage<F: ContextFs>(fs: &F, context: &Path, step: &CopyStep) -> Result<ChangeSet> {
    let owner = owner_of(step.owner.as_deref())?;
    let mut changes = Vec::new();
    for parent in ancestors_of(&step.destination) {
        changes.push(Change::Directory {
            path: parent,
            mode: CREATED_DIRECTORY_MODE,
            uid: owner.uid,
            gid: owner.gid,
        });
    }
    let into_directory = step.sources.len() > 1 || step.destination.ends_with('/');
    for source in &step.sources {
        let from = rooted(context, source)?;
        let meta = fs
            .meta(&from)
            .with_context(|| format!("reading {source} in the build context"))?
            .with_context(|| format!("the build context holds no {source}"))?;
        let target = if into_directory {
            format!("{}/{}", trimmed(&step.destination), base_name(source))
        } else {
            trimmed(&step.destination).to_string()
        };
        match meta.kind {
            EntryKind::Directory => {
                changes.push(Change::Directory {
                    path: guest_path(&target),
                    mode: meta.mode,
                    uid: owner.uid,
                    gid: owner.gid,
                });
                walk(fs, &from, &target, owner, &mut changes)?;
            }
            _ => changes.push(one_entry(fs, &from, &target, meta, owner)?),
        }
    }
    Ok(ChangeSet { changes })
}

fn walk<F: ContextFs>(
    fs: &F,
    from: &Path,
    target: &str,
    owner: Owner,
    changes: &mut Vec<Change>,
) -> Result<()> {
    let mut names = fs
        .entries(from)
        .with_context(|| format!("reading the build context's {}", from.display()))?;
    names.sort();
    for name in names {
        let child = from.join(&name);
        let target = format!("{target}/{name}");
        let meta = fs
            .meta(&child)
            .with_context(|| format!("reading {} in the build context", child.display()))?
            .with_context(|| format!("{} left the build context mid-copy", child.display()))?;
        match meta.kind {
            EntryKind::Directory => {
                changes.push(Change::Directory {
                    path: guest_path(&target),
                    mode: meta.mode,
                    uid: owner.uid,
                    gid: owner.gid,
                });
                walk(fs, &child, &target, owner, changes)?;
            }
            _ => changes.push(one_entry(fs, &child, &target, meta, owner)?),
        }
    }
    Ok(())
}

fn one_entry<F: ContextFs>(
    fs: &F,
    from: &Path,
    target: &str,
    meta: Meta,
    owner: Owner,
) -> Result<Change> {
    let path = guest_path(target);
    match meta.kind {
        EntryKind::Symlink => Ok(Change::Symlink {
            path,
            target: fs.read_link(from).with_context(|| {
                format!(
                    "reading the symlink {} in the build context",
                    from.display()
                )
            })?,
            uid: owner.uid,
            gid: owner.gid,
        }),
        _ => Ok(Change::Regular {
            path,
            mode: meta.mode,
            uid: owner.uid,
            gid: owner.gid,
            bytes: fs
                .read(from)
                .with_context(|| format!("reading {} out of the build context", from.display()))?,
        }),
    }
}

/// A source that leaves the context is refused: the artifact ships what the document names, and nothing else.
fn rooted(context: &Path, source: &str) -> Result<PathBuf> {
    if source.starts_with('/') || source.starts_with('~') {
        bail!(
            "the source {source:?} is not in the build context; a COPY names a path beside the document, such as ./app"
        );
    }
    if source.split('/').any(|segment| segment == "..") {
        bail!(
            "the source {source:?} leaves the build context, and a build sends only what the context holds"
        );
    }
    Ok(context.join(source))
}

fn owner_of(chown: Option<&str>) -> Result<Owner> {
    let Some(chown) = chown else {
        return Ok(Owner { uid: 0, gid: 0 });
    };
    let (user, group) = match chown.split_once(':') {
        Some((user, group)) => (user, group),
        None => (chown, chown),
    };
    match (user.parse::<u32>(), group.parse::<u32>()) {
        (Ok(uid), Ok(gid)) => Ok(Owner { uid, gid }),
        _ => bail!(
            "--chown={chown} names an identity only the image can resolve; write it as uid:gid, such as --chown=1000:1000"
        ),
    }
}

/// A layer's paths are relative to the image root, the way a captured upper's are.
fn guest_path(target: &str) -> String {
    target.trim_start_matches('/').to_string()
}

fn trimmed(destination: &str) -> &str {
    let trimmed = destination.trim_end_matches('/');
    if trimmed.is_empty() { "/" } else { trimmed }
}

fn base_name(source: &str) -> &str {
    source
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(source)
}

/// Every directory above the destination, so a copy into a path the base image lacks lands whole.
fn ancestors_of(destination: &str) -> Vec<String> {
    let mut segments: Vec<&str> = trimmed(destination)
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    segments.pop();
    let mut prefix = String::new();
    let mut parents = Vec::new();
    for segment in segments {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(segment);
        parents.push(prefix.clone());
    }
    parents
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// One build context, and one way to make each read of it fail: every error path a real
    /// directory has is a field here, so no test needs a double of its own.
    #[derive(Default)]
    pub(crate) struct FakeContext {
        files: BTreeMap<String, (u32, Vec<u8>)>,
        links: BTreeMap<String, String>,
        dirs: BTreeMap<String, u32>,
        pub(crate) unreadable: Option<String>,
        unlistable: Option<String>,
        unreadable_bytes: Option<String>,
        unreadable_link: Option<String>,
        /// A name a directory lists and nothing answers for, the way a file removed mid-copy reads.
        ghost: Option<(String, String)>,
    }

    impl FakeContext {
        pub(crate) fn new() -> Self {
            let mut context = Self::default();
            context.dirs.insert("/ctx".into(), 0o755);
            context
        }

        pub(crate) fn file(&mut self, path: &str, mode: u32, bytes: &[u8]) -> &mut Self {
            self.dir_chain(path);
            self.files
                .insert(format!("/ctx/{path}"), (mode, bytes.to_vec()));
            self
        }

        pub(crate) fn symlink(&mut self, path: &str, target: &str) -> &mut Self {
            self.dir_chain(path);
            self.links
                .insert(format!("/ctx/{path}"), target.to_string());
            self
        }

        pub(crate) fn dir(&mut self, path: &str, mode: u32) -> &mut Self {
            self.dirs.insert(format!("/ctx/{path}"), mode);
            self
        }

        fn unlistable(&mut self, path: &str) -> &mut Self {
            self.unlistable = Some(format!("/ctx/{path}"));
            self
        }

        fn unreadable_bytes(&mut self, path: &str) -> &mut Self {
            self.unreadable_bytes = Some(format!("/ctx/{path}"));
            self
        }

        fn unreadable_link(&mut self, path: &str) -> &mut Self {
            self.unreadable_link = Some(format!("/ctx/{path}"));
            self
        }

        fn ghost(&mut self, dir: &str, name: &str) -> &mut Self {
            self.ghost = Some((format!("/ctx/{dir}"), name.to_string()));
            self
        }

        fn dir_chain(&mut self, path: &str) {
            let mut prefix = String::from("/ctx");
            let mut segments: Vec<&str> = path.split('/').collect();
            segments.pop();
            for segment in segments {
                prefix.push('/');
                prefix.push_str(segment);
                self.dirs.entry(prefix.clone()).or_insert(0o755);
            }
        }
    }

    impl ContextFs for FakeContext {
        fn meta(&self, path: &Path) -> Result<Option<Meta>> {
            let key = path.to_string_lossy().to_string();
            if self.unreadable.as_deref() == Some(key.as_str()) {
                bail!("permission denied");
            }
            if let Some(mode) = self.dirs.get(&key) {
                return Ok(Some(Meta {
                    kind: EntryKind::Directory,
                    mode: *mode,
                }));
            }
            if let Some((mode, _)) = self.files.get(&key) {
                return Ok(Some(Meta {
                    kind: EntryKind::Regular,
                    mode: *mode,
                }));
            }
            if self.links.contains_key(&key) {
                return Ok(Some(Meta {
                    kind: EntryKind::Symlink,
                    mode: 0o777,
                }));
            }
            Ok(None)
        }

        fn entries(&self, path: &Path) -> Result<Vec<String>> {
            let key = path.to_string_lossy().to_string();
            let prefix = format!("{key}/");
            if self.unlistable.as_deref() == Some(key.as_str()) {
                bail!("permission denied");
            }
            let mut names: Vec<String> = self
                .dirs
                .keys()
                .chain(self.files.keys())
                .chain(self.links.keys())
                .filter_map(|key| key.strip_prefix(&prefix))
                .filter(|rest| !rest.contains('/'))
                .map(str::to_string)
                .collect();
            if let Some((dir, name)) = &self.ghost
                && *dir == key
            {
                names.push(name.clone());
            }
            names.sort();
            names.dedup();
            Ok(names)
        }

        fn read(&self, path: &Path) -> Result<Vec<u8>> {
            let key = path.to_string_lossy().to_string();
            if self.unreadable_bytes.as_deref() == Some(key.as_str()) {
                bail!("input/output error");
            }
            self.files
                .get(&key)
                .map(|(_, bytes)| bytes.clone())
                .with_context(|| format!("no such file {}", path.display()))
        }

        fn read_link(&self, path: &Path) -> Result<String> {
            let key = path.to_string_lossy().to_string();
            if self.unreadable_link.as_deref() == Some(key.as_str()) {
                bail!("input/output error");
            }
            self.links
                .get(&key)
                .cloned()
                .with_context(|| format!("no such symlink {}", path.display()))
        }
    }

    /// Who one change belongs to, or nothing where the change is a removal — a copy makes none.
    fn owner(change: &Change) -> Option<(u32, u32)> {
        match change {
            Change::Directory { uid, gid, .. }
            | Change::Regular { uid, gid, .. }
            | Change::Symlink { uid, gid, .. } => Some((*uid, *gid)),
            Change::Removed { .. } => None,
        }
    }

    fn step(sources: &[&str], destination: &str) -> CopyStep {
        CopyStep {
            sources: sources.iter().map(|s| s.to_string()).collect(),
            destination: destination.to_string(),
            owner: None,
            line: 4,
        }
    }

    fn staged(context: &FakeContext, step: &CopyStep) -> Vec<Change> {
        stage(context, Path::new("/ctx"), step)
            .expect("the context answers this copy")
            .changes
    }

    fn refusal(context: &FakeContext, step: &CopyStep) -> String {
        format!(
            "{:#}",
            stage(context, Path::new("/ctx"), step).expect_err("this copy must be refused")
        )
    }

    #[test]
    fn one_file_lands_at_the_destination_with_its_mode_and_content() {
        let mut context = FakeContext::new();
        context.file("entrypoint.sh", 0o755, b"#!/bin/sh\n");

        assert_eq!(
            staged(
                &context,
                &step(&["entrypoint.sh"], "/usr/local/bin/entrypoint.sh")
            ),
            vec![
                Change::Directory {
                    path: "usr".into(),
                    mode: CREATED_DIRECTORY_MODE,
                    uid: 0,
                    gid: 0,
                },
                Change::Directory {
                    path: "usr/local".into(),
                    mode: CREATED_DIRECTORY_MODE,
                    uid: 0,
                    gid: 0,
                },
                Change::Directory {
                    path: "usr/local/bin".into(),
                    mode: CREATED_DIRECTORY_MODE,
                    uid: 0,
                    gid: 0,
                },
                Change::Regular {
                    path: "usr/local/bin/entrypoint.sh".into(),
                    mode: 0o755,
                    uid: 0,
                    gid: 0,
                    bytes: b"#!/bin/sh\n".to_vec(),
                },
            ],
        );
    }

    #[test]
    fn a_directory_source_lands_with_everything_under_it_parents_first() {
        let mut context = FakeContext::new();
        context
            .dir("app", 0o750)
            .file("app/index.js", 0o644, b"main\n")
            .dir("app/lib", 0o755)
            .file("app/lib/util.js", 0o644, b"lib\n")
            .symlink("app/current", "lib");

        let paths: Vec<String> = staged(&context, &step(&["app"], "/srv/app"))
            .iter()
            .map(|change| change.path().to_string())
            .collect();

        assert_eq!(
            paths,
            vec![
                "srv".to_string(),
                "srv/app".to_string(),
                "srv/app/current".to_string(),
                "srv/app/index.js".to_string(),
                "srv/app/lib".to_string(),
                "srv/app/lib/util.js".to_string(),
            ],
        );
    }

    #[test]
    fn a_symlink_keeps_the_target_it_pointed_at_rather_than_the_file_it_named() {
        let mut context = FakeContext::new();
        context.symlink("node", "/usr/local/bin/node-24");

        assert_eq!(
            staged(&context, &step(&["node"], "/usr/bin/node")),
            vec![
                Change::Directory {
                    path: "usr".into(),
                    mode: CREATED_DIRECTORY_MODE,
                    uid: 0,
                    gid: 0,
                },
                Change::Directory {
                    path: "usr/bin".into(),
                    mode: CREATED_DIRECTORY_MODE,
                    uid: 0,
                    gid: 0,
                },
                Change::Symlink {
                    path: "usr/bin/node".into(),
                    target: "/usr/local/bin/node-24".into(),
                    uid: 0,
                    gid: 0,
                },
            ],
        );
    }

    #[test]
    fn more_than_one_source_lands_under_the_destination_by_its_own_name() {
        let mut context = FakeContext::new();
        context
            .file("package.json", 0o644, b"{}\n")
            .file("package-lock.json", 0o644, b"{}\n");

        let paths: Vec<String> = staged(
            &context,
            &step(&["package.json", "package-lock.json"], "/srv"),
        )
        .iter()
        .map(|change| change.path().to_string())
        .collect();

        assert_eq!(
            paths,
            vec![
                "srv/package.json".to_string(),
                "srv/package-lock.json".to_string()
            ],
        );
    }

    #[test]
    fn a_destination_written_as_a_directory_keeps_the_source_name() {
        let mut context = FakeContext::new();
        context.file("app.js", 0o644, b"main\n");

        let paths: Vec<String> = staged(&context, &step(&["app.js"], "/srv/"))
            .iter()
            .map(|change| change.path().to_string())
            .collect();

        assert_eq!(paths, vec!["srv/app.js".to_string()]);
    }

    #[test]
    fn a_copy_into_the_image_root_needs_no_parent_of_its_own() {
        let mut context = FakeContext::new();
        context.file("marker", 0o644, b"x\n");

        let paths: Vec<String> = staged(&context, &step(&["marker"], "/"))
            .iter()
            .map(|change| change.path().to_string())
            .collect();

        assert_eq!(paths, vec!["marker".to_string()]);
    }

    #[test]
    fn a_numeric_chown_owns_every_entry_the_copy_produces() {
        let mut context = FakeContext::new();
        context
            .dir("seed", 0o755)
            .file("seed/state.json", 0o600, b"{}\n")
            .symlink("seed/current", "state.json");
        let mut step = step(&["seed"], "/home/node/state");
        step.owner = Some("1000:1000".into());

        for change in staged(&context, &step) {
            assert_eq!(owner(&change), Some((1000, 1000)), "{change:?}");
        }
    }

    #[test]
    fn a_removal_belongs_to_nobody_because_a_copy_makes_none() {
        assert_eq!(
            owner(&Change::Removed {
                path: "srv/app".into()
            }),
            None,
        );
    }

    #[test]
    fn a_chown_naming_one_identity_uses_it_for_the_group_as_well() {
        let mut context = FakeContext::new();
        context.file("app.js", 0o644, b"x");
        let mut step = step(&["app.js"], "/srv/app.js");
        step.owner = Some("1000".into());

        assert_eq!(
            staged(&context, &step).last().and_then(owner),
            Some((1000, 1000)),
        );
    }

    #[test]
    fn a_chown_only_the_image_could_resolve_is_refused_and_names_the_numeric_form() {
        let mut context = FakeContext::new();
        context.file("app.js", 0o644, b"x");
        let mut step = step(&["app.js"], "/srv/app.js");
        step.owner = Some("node:node".into());

        let refusal = refusal(&context, &step);
        assert!(refusal.contains("--chown=node:node"), "{refusal}");
        assert!(refusal.contains("uid:gid"), "{refusal}");
    }

    #[test]
    fn a_source_the_context_does_not_hold_is_refused_by_name() {
        let refusal = refusal(&FakeContext::new(), &step(&["missing.js"], "/srv/app.js"));
        assert!(
            refusal.contains("the build context holds no missing.js"),
            "{refusal}"
        );
    }

    #[test]
    fn a_source_outside_the_context_is_refused_before_anything_is_read() {
        for source in ["../secrets/id_rsa", "/etc/shadow", "~/.aws/credentials"] {
            let refusal = refusal(&FakeContext::new(), &step(&[source], "/srv/x"));
            assert!(refusal.contains(source), "{refusal}");
            assert!(refusal.contains("build context"), "{refusal}");
        }
    }

    #[test]
    fn a_context_that_cannot_be_read_names_the_source_that_stopped_it() {
        let mut context = FakeContext::new();
        context.file("app.js", 0o644, b"x");
        context.unreadable = Some("/ctx/app.js".into());

        let refusal = refusal(&context, &step(&["app.js"], "/srv/app.js"));
        assert!(
            refusal.contains("reading app.js in the build context"),
            "{refusal}"
        );
        assert!(refusal.contains("permission denied"), "{refusal}");
    }

    #[test]
    fn a_child_that_leaves_the_context_mid_copy_stops_the_copy() {
        let mut context = FakeContext::new();
        context.dir("app", 0o755).ghost("app", "gone.js");

        let refusal = refusal(&context, &step(&["app"], "/srv/app"));
        assert!(
            refusal.contains("left the build context mid-copy"),
            "{refusal}"
        );
    }

    #[test]
    fn a_directory_whose_entries_will_not_list_names_itself() {
        let mut context = FakeContext::new();
        context.dir("app", 0o755).unlistable("app");

        let refusal = refusal(&context, &step(&["app"], "/srv/app"));
        assert!(
            refusal.contains("reading the build context's /ctx/app"),
            "{refusal}"
        );
        assert!(refusal.contains("permission denied"), "{refusal}");
    }

    #[test]
    fn a_file_whose_bytes_will_not_read_names_the_file() {
        let mut context = FakeContext::new();
        context
            .file("app.js", 0o644, b"x")
            .unreadable_bytes("app.js");

        let refusal = refusal(&context, &step(&["app.js"], "/srv/app.js"));
        assert!(
            refusal.contains("reading /ctx/app.js out of the build context"),
            "{refusal}"
        );
    }

    #[test]
    fn a_symlink_whose_target_will_not_read_names_the_symlink() {
        let mut context = FakeContext::new();
        context.symlink("link", "app.js").unreadable_link("link");

        let refusal = refusal(&context, &step(&["link"], "/srv/link"));
        assert!(
            refusal.contains("reading the symlink /ctx/link in the build context"),
            "{refusal}"
        );
    }
}
