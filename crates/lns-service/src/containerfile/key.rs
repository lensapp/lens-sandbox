//! What a build is keyed by: slice 4 of lensapp/lens-sandbox#393.

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
            image_key("base@sha256:aa", "FROM base\nRUN true\n", "sha256:ctx", "arm64"),
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
        let one = image_key("base@sha256:aa", "# build the agent\nFROM base\n", "c", "arm64");
        let other = image_key("base@sha256:aa", "#  build the agent\nFROM base\n", "c", "arm64");

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
        one.file("app.js", 0o644, b"one\n").symlink("main", "app.js");
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
            context_hash(&context, Path::new("/ctx")).expect_err("an unreadable context is refused")
        );

        assert!(refusal.contains("permission denied"), "{refusal}");
        assert!(refusal.contains("/ctx/app.js"), "{refusal}");
    }

    #[test]
    fn a_context_directory_that_will_not_list_stops_the_build_naming_it() {
        let mut context = one_file(b"one\n", 0o644);
        context.unlistable("");

        let refusal = format!(
            "{:#}",
            context_hash(&context, Path::new("/ctx")).expect_err("a context that will not list is refused")
        );

        assert!(refusal.contains("/ctx"), "{refusal}");
    }

    #[test]
    fn the_parent_the_instruction_and_the_copied_content_each_change_the_instruction_key() {
        let key = step_key("built@sha256:parent", "COPY app /srv", Some("sha256:copied"));

        assert_eq!(
            key,
            step_key("built@sha256:parent", "COPY app /srv", Some("sha256:copied")),
        );
        for other in [
            step_key("built@sha256:other", "COPY app /srv", Some("sha256:copied")),
            step_key("built@sha256:parent", "COPY app /opt", Some("sha256:copied")),
            step_key("built@sha256:parent", "COPY app /srv", Some("sha256:edited")),
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
            step_key("parent", "RUN true", Some(&changes_hash(&ChangeSet::default()))),
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
