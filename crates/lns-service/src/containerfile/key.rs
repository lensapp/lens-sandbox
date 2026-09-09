//! What a build is keyed by: slice 4 of lensapp/lens-sandbox#393.
//!
//! An image is a function of the base it stands on, the Containerfile's own text, the bytes of the
//! context beside it and the architecture a guest boots — so those four are the key, and a build
//! whose key this machine already holds is not run again. One instruction is keyed the same way,
//! by the image it stands on and its own text, plus what it copies when it copies anything.

use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use super::context::{ContextFs, EntryKind};
use super::executor::{ConfigDraft, RunStep};
use super::upper::{Change, ChangeSet};

/// The whole image: what a second `lns sandbox build` of an untouched document finds and does not run again.
pub(crate) fn image_key(base: &str, text: &str, context: &str, arch: &str) -> String {
    let mut fields = Fields::over("lns.build.image.v1");
    fields.field(base.as_bytes());
    fields.field(text.as_bytes());
    fields.field(context.as_bytes());
    fields.field(arch.as_bytes());
    fields.finish()
}

/// One instruction over the image before it, which is what makes a build reuse its leading steps.
pub(crate) fn step_key(parent: &str, instruction: &str, copied: Option<&str>) -> String {
    let mut fields = Fields::over("lns.build.step.v1");
    fields.field(parent.as_bytes());
    fields.field(instruction.as_bytes());
    match copied {
        Some(copied) => {
            fields.field(b"copied");
            fields.field(copied.as_bytes());
        }
        None => fields.field(b"nothing copied"),
    }
    fields.finish()
}

/// A `RUN` as the guest is actually asked to run it: `ARG` reaches a command and its environment
/// without committing anything of its own, so the written text alone does not tell two runs apart.
pub(crate) fn run_key(parent: &str, step: &RunStep) -> String {
    let mut fields = Fields::over("lns.build.step.v1");
    fields.field(parent.as_bytes());
    fields.field(b"RUN");
    fields.each(&step.argv);
    fields.field(b"env");
    fields.each(&step.env);
    fields.field(step.user.as_bytes());
    fields.field(step.workdir.as_bytes());
    fields.finish()
}

/// A `COPY` or an `ADD` by what it puts in its layer, which is where its sources, its destination and its `--chown` all end up.
pub(crate) fn transfer_key(parent: &str, instruction: &str, copied: &ChangeSet) -> String {
    step_key(parent, instruction, Some(&changes_hash(copied)))
}

/// An instruction that writes only config, by the config the image then carries — the same reason a `RUN` is keyed by its resolved command.
pub(crate) fn config_key(parent: &str, instruction: &str, config: &ConfigDraft) -> String {
    let mut fields = Fields::over("lns.build.step.v1");
    fields.field(parent.as_bytes());
    fields.field(instruction.as_bytes());
    fields.field(b"config");
    fields.pairs(&config.env);
    fields.pairs(&config.labels);
    for optional in [&config.user, &config.workdir] {
        fields.field(optional.as_deref().unwrap_or_default().as_bytes());
    }
    for argv in [&config.entrypoint, &config.cmd, &config.shell] {
        match argv {
            Some(argv) => fields.each(argv),
            None => fields.field(b"unset"),
        }
    }
    fields.each(&config.exposed_ports);
    fields.each(&config.volumes);
    fields.finish()
}

/// Every byte the context holds, so a file edited beside the Containerfile is a different build and a file only touched is not.
pub(crate) fn context_hash<F: ContextFs>(fs: &F, context: &Path) -> Result<String> {
    let mut fields = Fields::over("lns.build.context.v1");
    walk(fs, context, "", &mut fields)?;
    Ok(fields.finish())
}

/// What a `COPY` or an `ADD` puts in its layer, which is the part of its key the instruction text cannot see.
pub(crate) fn changes_hash(changes: &ChangeSet) -> String {
    let mut fields = Fields::over("lns.build.copied.v1");
    for change in &changes.changes {
        match change {
            Change::Directory {
                path,
                mode,
                uid,
                gid,
            } => {
                fields.field(b"directory");
                fields.entry(path, *mode, *uid, *gid);
            }
            Change::Regular {
                path,
                mode,
                uid,
                gid,
                bytes,
            } => {
                fields.field(b"regular");
                fields.entry(path, *mode, *uid, *gid);
                fields.field(bytes);
            }
            Change::Symlink {
                path,
                target,
                uid,
                gid,
            } => {
                fields.field(b"symlink");
                fields.entry(path, 0, *uid, *gid);
                fields.field(target.as_bytes());
            }
            Change::Removed { path } => {
                fields.field(b"removed");
                fields.field(path.as_bytes());
            }
        }
    }
    fields.finish()
}

fn walk<F: ContextFs>(fs: &F, directory: &Path, prefix: &str, fields: &mut Fields) -> Result<()> {
    let mut names = fs
        .entries(directory)
        .with_context(|| format!("reading the build context at {}", directory.display()))?;
    names.sort();
    for name in names {
        let path = directory.join(&name);
        let relative = match prefix.is_empty() {
            true => name.clone(),
            false => format!("{prefix}/{name}"),
        };
        let Some(meta) = fs
            .meta(&path)
            .with_context(|| format!("reading {} in the build context", path.display()))?
        else {
            continue;
        };
        fields.field(relative.as_bytes());
        match meta.kind {
            EntryKind::Directory => {
                fields.field(b"directory");
                fields.field(&meta.mode.to_be_bytes());
                walk(fs, &path, &relative, fields)?;
            }
            EntryKind::Regular => {
                fields.field(b"regular");
                fields.field(&meta.mode.to_be_bytes());
                fields.field(
                    &fs.read(&path).with_context(|| {
                        format!("reading {} in the build context", path.display())
                    })?,
                );
            }
            EntryKind::Symlink => {
                fields.field(b"symlink");
                fields.field(
                    fs.read_link(&path)
                        .with_context(|| {
                            format!("reading {} in the build context", path.display())
                        })?
                        .as_bytes(),
                );
            }
        }
    }
    Ok(())
}

/// A digest over fields rather than over their concatenation: every field carries its own length, so no two different inputs can spell one key.
struct Fields(Sha256);

impl Fields {
    fn over(domain: &str) -> Self {
        let mut fields = Self(Sha256::new());
        fields.field(domain.as_bytes());
        fields
    }

    fn field(&mut self, bytes: &[u8]) {
        self.0.update((bytes.len() as u64).to_be_bytes());
        self.0.update(bytes);
    }

    fn each(&mut self, values: &[String]) {
        self.field(&(values.len() as u64).to_be_bytes());
        for value in values {
            self.field(value.as_bytes());
        }
    }

    fn pairs(&mut self, values: &[(String, String)]) {
        self.field(&(values.len() as u64).to_be_bytes());
        for (key, value) in values {
            self.field(key.as_bytes());
            self.field(value.as_bytes());
        }
    }

    fn entry(&mut self, path: &str, mode: u32, uid: u32, gid: u32) {
        self.field(path.as_bytes());
        self.field(&mode.to_be_bytes());
        self.field(&uid.to_be_bytes());
        self.field(&gid.to_be_bytes());
    }

    fn finish(self) -> String {
        format!("sha256:{}", hex::encode(self.0.finalize()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::containerfile::context::tests::FakeContext;
    use crate::containerfile::upper::{Change, ChangeSet};
    use std::path::Path;

    fn hashed(context: &FakeContext) -> String {
        context_hash(context, Path::new("/ctx")).expect("this context reads")
    }

    fn one_file(bytes: &[u8], mode: u32) -> FakeContext {
        let mut context = FakeContext::new();
        context.file("app.js", mode, bytes);
        context
    }

    #[test]
    fn the_from_digest_the_text_the_context_and_the_architecture_each_change_the_image_key() {
        let key = image_key("base@sha256:aa", "FROM base\n", "sha256:ctx", "arm64");

        assert_eq!(
            key,
            image_key("base@sha256:aa", "FROM base\n", "sha256:ctx", "arm64"),
            "the same four inputs are the same image, so they are the same key",
        );
        for other in [
            image_key("base@sha256:bb", "FROM base\n", "sha256:ctx", "arm64"),
            image_key(
                "base@sha256:aa",
                "FROM base\nRUN true\n",
                "sha256:ctx",
                "arm64",
            ),
            image_key("base@sha256:aa", "FROM base\n", "sha256:other", "arm64"),
            image_key("base@sha256:aa", "FROM base\n", "sha256:ctx", "amd64"),
        ] {
            assert_ne!(key, other);
        }
    }

    /// The key measures the file, not what the parser made of it, so a comment nobody executes still
    /// decides the key — a build that reused across it would be a build nobody could explain.
    #[test]
    fn the_same_containerfile_with_different_whitespace_in_a_comment_is_a_different_text_and_so_a_different_key()
     {
        let one = image_key(
            "base@sha256:aa",
            "# build the agent\nFROM base\n",
            "c",
            "arm64",
        );
        let other = image_key(
            "base@sha256:aa",
            "#  build the agent\nFROM base\n",
            "c",
            "arm64",
        );

        assert_ne!(one, other);
    }

    #[test]
    fn a_key_names_the_algorithm_that_made_it() {
        for key in [
            image_key("b", "t", "c", "arm64"),
            step_key("parent", "RUN true", None),
            hashed(&one_file(b"one\n", 0o644)),
        ] {
            assert!(key.starts_with("sha256:"), "{key}");
            assert_eq!(key.len(), "sha256:".len() + 64, "{key}");
        }
    }

    /// A context file rewritten with the bytes it already had is the same context: the key reads
    /// content, and nothing a `touch` changes reaches it.
    #[test]
    fn a_touched_but_identical_context_file_is_the_same_key() {
        let mut context = one_file(b"console.log(1)\n", 0o644);
        let before = hashed(&context);

        context.file("app.js", 0o644, b"console.log(1)\n");

        assert_eq!(hashed(&context), before);
    }

    #[test]
    fn the_content_the_name_and_the_mode_of_a_context_file_each_change_the_context_hash() {
        let before = hashed(&one_file(b"one\n", 0o644));

        assert_ne!(hashed(&one_file(b"two\n", 0o644)), before);
        assert_ne!(hashed(&one_file(b"one\n", 0o755)), before);
        let mut renamed = FakeContext::new();
        renamed.file("other.js", 0o644, b"one\n");
        assert_ne!(hashed(&renamed), before);
    }

    #[test]
    fn a_file_added_beside_the_containerfile_changes_the_context_hash() {
        let mut context = FakeContext::new();
        context.file("Containerfile", 0o644, b"FROM base\n");
        let before = hashed(&context);

        context.file("skills/prompt.md", 0o644, b"be brief\n");

        assert_ne!(hashed(&context), before);
    }

    #[test]
    fn a_context_symlink_hashes_the_target_it_names_and_not_what_it_points_at() {
        let mut one = FakeContext::new();
        one.file("app.js", 0o644, b"one\n")
            .symlink("main", "app.js");
        let mut other = FakeContext::new();
        other
            .file("app.js", 0o644, b"one\n")
            .symlink("main", "other.js");

        assert_ne!(hashed(&one), hashed(&other));
    }

    #[test]
    fn a_directory_the_context_holds_is_part_of_the_hash_even_when_it_is_empty() {
        let mut context = FakeContext::new();
        context.file("app.js", 0o644, b"one\n");
        let before = hashed(&context);

        context.dir("cache", 0o755);

        assert_ne!(hashed(&context), before);
    }

    #[test]
    fn a_context_the_host_cannot_read_stops_the_build_naming_what_it_could_not_read() {
        let mut context = one_file(b"one\n", 0o644);
        context.unreadable = Some("/ctx/app.js".into());

        let refusal = format!(
            "{:#}",
            context_hash(&context, Path::new("/ctx"))
                .expect_err("an unreadable context is refused")
        );

        assert!(refusal.contains("permission denied"), "{refusal}");
        assert!(refusal.contains("/ctx/app.js"), "{refusal}");
    }

    #[test]
    fn a_context_directory_that_will_not_list_stops_the_build_naming_it() {
        let mut context = one_file(b"one\n", 0o644);
        context.dir("skills", 0o755).unlistable("skills");

        let refusal = format!(
            "{:#}",
            context_hash(&context, Path::new("/ctx"))
                .expect_err("a context that will not list is refused")
        );

        assert!(refusal.contains("/ctx/skills"), "{refusal}");
    }

    #[test]
    fn the_parent_the_instruction_and_the_copied_content_each_change_the_instruction_key() {
        let key = step_key(
            "built@sha256:parent",
            "COPY app /srv",
            Some("sha256:copied"),
        );

        assert_eq!(
            key,
            step_key(
                "built@sha256:parent",
                "COPY app /srv",
                Some("sha256:copied")
            ),
        );
        for other in [
            step_key("built@sha256:other", "COPY app /srv", Some("sha256:copied")),
            step_key(
                "built@sha256:parent",
                "COPY app /opt",
                Some("sha256:copied"),
            ),
            step_key(
                "built@sha256:parent",
                "COPY app /srv",
                Some("sha256:edited"),
            ),
            step_key("built@sha256:parent", "COPY app /srv", None),
        ] {
            assert_ne!(key, other);
        }
    }

    /// A `RUN` has no copied content, and the key must not confuse "nothing copied" with a copy of
    /// nothing — otherwise an empty `COPY` and the `RUN` beside it would share one entry.
    #[test]
    fn an_instruction_that_copies_nothing_is_keyed_apart_from_one_that_copies_an_empty_set() {
        assert_ne!(
            step_key("parent", "RUN true", None),
            step_key(
                "parent",
                "RUN true",
                Some(&changes_hash(&ChangeSet::default()))
            ),
        );
    }

    #[test]
    fn the_copied_content_hash_follows_the_bytes_the_paths_and_the_modes() {
        let copied = |path: &str, mode: u32, bytes: &[u8]| {
            changes_hash(&ChangeSet {
                changes: vec![Change::Regular {
                    path: path.into(),
                    mode,
                    uid: 0,
                    gid: 0,
                    bytes: bytes.to_vec(),
                }],
            })
        };
        let before = copied("srv/app", 0o644, b"one\n");

        assert_eq!(copied("srv/app", 0o644, b"one\n"), before);
        assert_ne!(copied("srv/app", 0o644, b"two\n"), before);
        assert_ne!(copied("srv/other", 0o644, b"one\n"), before);
        assert_ne!(copied("srv/app", 0o755, b"one\n"), before);
    }

    fn run_step() -> RunStep {
        RunStep {
            parent: "built@sha256:parent".into(),
            argv: vec!["/bin/sh".into(), "-c".into(), "npm i -g agent@1".into()],
            env: vec!["VERSION=1".into()],
            user: "root".into(),
            workdir: "/".into(),
            line: 4,
        }
    }

    /// `ARG` commits nothing, so two builds can reach one parent with one written `RUN` line and
    /// still run two different commands; the key is over what the guest is asked to run.
    #[test]
    fn a_run_is_keyed_by_the_command_the_env_the_user_and_the_workdir_it_resolved_to() {
        let key = run_key("built@sha256:parent", &run_step());

        assert_eq!(key, run_key("built@sha256:parent", &run_step()));
        for changed in [
            RunStep {
                argv: vec!["/bin/sh".into(), "-c".into(), "npm i -g agent@2".into()],
                ..run_step()
            },
            RunStep {
                env: vec!["VERSION=2".into()],
                ..run_step()
            },
            RunStep {
                user: "node".into(),
                ..run_step()
            },
            RunStep {
                workdir: "/srv".into(),
                ..run_step()
            },
        ] {
            assert_ne!(key, run_key("built@sha256:parent", &changed));
        }
        assert_ne!(key, run_key("built@sha256:other", &run_step()));
    }

    /// The line a `RUN` was written on moves when a comment above it does, and moving a line
    /// rebuilds nothing: the guest runs the same command in the same guest.
    #[test]
    fn the_line_a_run_was_written_on_is_not_part_of_its_key() {
        assert_eq!(
            run_key("built@sha256:parent", &run_step()),
            run_key(
                "built@sha256:parent",
                &RunStep {
                    line: 9,
                    ..run_step()
                }
            ),
        );
    }

    #[test]
    fn a_transfer_is_keyed_by_the_instruction_and_by_what_it_copies() {
        let copied = |bytes: &[u8]| ChangeSet {
            changes: vec![Change::Regular {
                path: "srv/app".into(),
                mode: 0o644,
                uid: 0,
                gid: 0,
                bytes: bytes.to_vec(),
            }],
        };
        let key = transfer_key(
            "built@sha256:parent",
            "COPY app /srv/app",
            &copied(b"one\n"),
        );

        assert_eq!(
            key,
            transfer_key(
                "built@sha256:parent",
                "COPY app /srv/app",
                &copied(b"one\n")
            ),
        );
        assert_ne!(
            key,
            transfer_key(
                "built@sha256:parent",
                "COPY app /srv/app",
                &copied(b"two\n")
            ),
            "an edited context file is a different step, whatever the instruction says",
        );
        assert_ne!(
            key,
            transfer_key("built@sha256:parent", "ADD app /srv/app", &copied(b"one\n")),
        );
    }

    #[test]
    fn a_config_instruction_is_keyed_by_the_config_the_image_then_carries() {
        let draft = ConfigDraft {
            env: vec![("MODE".into(), "research".into())],
            labels: vec![("org.opencontainers.image.title".into(), "agent".into())],
            user: Some("node".into()),
            workdir: Some("/srv".into()),
            entrypoint: Some(vec!["/entry".into()]),
            cmd: Some(vec!["--help".into()]),
            shell: Some(vec!["/bin/bash".into(), "-c".into()]),
            exposed_ports: vec!["8080".into()],
            volumes: vec!["/data".into()],
        };
        let key = config_key("built@sha256:parent", "ENV MODE=research", &draft);

        assert_eq!(
            key,
            config_key("built@sha256:parent", "ENV MODE=research", &draft)
        );
        assert_ne!(
            key,
            config_key("built@sha256:other", "ENV MODE=research", &draft)
        );
        assert_ne!(
            key,
            config_key("built@sha256:parent", "ENV MODE=review", &draft)
        );
        for changed in [
            ConfigDraft {
                env: vec![("MODE".into(), "review".into())],
                ..draft.clone()
            },
            ConfigDraft {
                labels: Vec::new(),
                ..draft.clone()
            },
            ConfigDraft {
                user: None,
                ..draft.clone()
            },
            ConfigDraft {
                workdir: Some("/opt".into()),
                ..draft.clone()
            },
            ConfigDraft {
                entrypoint: None,
                ..draft.clone()
            },
            ConfigDraft {
                cmd: Some(vec!["--serve".into()]),
                ..draft.clone()
            },
            ConfigDraft {
                shell: None,
                ..draft.clone()
            },
            ConfigDraft {
                exposed_ports: vec!["9090".into()],
                ..draft.clone()
            },
            ConfigDraft {
                volumes: Vec::new(),
                ..draft.clone()
            },
        ] {
            assert_ne!(
                key,
                config_key("built@sha256:parent", "ENV MODE=research", &changed)
            );
        }
    }

    #[test]
    fn every_kind_of_copied_entry_reaches_the_content_hash() {
        let one = ChangeSet {
            changes: vec![
                Change::Directory {
                    path: "srv".into(),
                    mode: 0o755,
                    uid: 0,
                    gid: 0,
                },
                Change::Symlink {
                    path: "srv/main".into(),
                    target: "app".into(),
                    uid: 0,
                    gid: 0,
                },
                Change::Removed {
                    path: "srv/old".into(),
                },
            ],
        };
        let mut other = one.clone();
        other.changes[1] = Change::Symlink {
            path: "srv/main".into(),
            target: "elsewhere".into(),
            uid: 0,
            gid: 0,
        };

        assert_ne!(changes_hash(&one), changes_hash(&other));
        assert_eq!(changes_hash(&one), changes_hash(&one.clone()));
    }
}
