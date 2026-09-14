//! Which file a path-form `spec.image` names, and which directory is the build context.
//!
//! Slice 0 of lensapp/lens-sandbox#393 owns the path rules in `lns_artifact::image`; that branch is
//! not in this one's base, so the same rules are read here and this module goes when it lands.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::context::{ContextFs, EntryKind};

/// The names a build context may hold, in the order a directory is searched — `Containerfile` first, as Podman does.
const CONTAINERFILE_NAMES: [&str; 2] = ["Containerfile", "Dockerfile"];

/// Where the build reads its instructions and its files, and how the run summary names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Located {
    pub containerfile: PathBuf,
    pub context: PathBuf,
    pub label: String,
}

impl Located {
    /// The Containerfile's path inside the context, which is how another engine is told where to find it.
    pub(crate) fn in_context(&self) -> String {
        self.containerfile
            .strip_prefix(&self.context)
            .unwrap_or(&self.containerfile)
            .to_string_lossy()
            .into_owned()
    }
}

/// A value beginning `.`, `/` or `~` is a path — no OCI reference starts that way — so one predicate decides the form.
pub(crate) fn names_a_containerfile(image: &str) -> bool {
    matches!(image.trim_start().chars().next(), Some('.' | '/' | '~'))
}

pub(crate) fn locate<F: ContextFs>(fs: &F, definition_dir: &Path, image: &str) -> Result<Located> {
    let path = image.trim();
    if path.starts_with('~') {
        bail!(
            "spec.image {path:?} is built from a Containerfile beside the document, so it cannot be home-anchored; publish the image yourself and name it by reference instead"
        );
    }
    if path.starts_with('/') {
        bail!(
            "spec.image {path:?} is built from a Containerfile beside the document, so it must be relative, such as ./image"
        );
    }
    if path.split('/').any(|segment| segment == "..") {
        bail!(
            "spec.image {path:?} leaves the document's directory, and a build sends only what the context holds"
        );
    }
    // `components` drops every `.` the author wrote, so one directory named two ways is one path.
    let named: PathBuf = definition_dir.join(path).components().collect();
    let Some(meta) = fs.meta(&named)? else {
        bail!("spec.image {path:?} names nothing beside the document");
    };
    match meta.kind {
        EntryKind::Directory => in_directory(fs, &named, path),
        _ => Ok(Located {
            containerfile: named.clone(),
            context: named.parent().unwrap_or(definition_dir).to_path_buf(),
            label: path.to_string(),
        }),
    }
}

fn in_directory<F: ContextFs>(fs: &F, directory: &Path, written: &str) -> Result<Located> {
    for name in CONTAINERFILE_NAMES {
        let candidate = directory.join(name);
        if fs.meta(&candidate)?.is_some() {
            return Ok(Located {
                containerfile: candidate,
                context: directory.to_path_buf(),
                label: format!("{}/{name}", written.trim_end_matches('/')),
            });
        }
    }
    bail!(
        "spec.image {written:?} is a directory that holds neither Containerfile nor Dockerfile, so there is nothing to build"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::containerfile::context::tests::FakeContext;

    fn located(context: &FakeContext, image: &str) -> Located {
        locate(context, Path::new("/ctx"), image).expect("this spec.image names a Containerfile")
    }

    fn refusal(context: &FakeContext, image: &str) -> String {
        format!(
            "{:#}",
            locate(context, Path::new("/ctx"), image).expect_err("this spec.image is refused")
        )
    }

    #[test]
    fn a_registry_coordinate_is_no_containerfile_and_a_path_is() {
        for reference in ["alpine:3.20", "ghcr.io/acme/base@sha256:abc"] {
            assert!(!names_a_containerfile(reference), "{reference}");
        }
        for path in ["./image", ".", "../elsewhere", "~/image", "/srv/image"] {
            assert!(names_a_containerfile(path), "{path}");
        }
    }

    #[test]
    fn a_directory_holding_both_names_builds_the_containerfile_as_podman_does() {
        let mut context = FakeContext::new();
        context
            .dir("image", 0o755)
            .file("image/Containerfile", 0o644, b"FROM alpine\n")
            .file("image/Dockerfile", 0o644, b"FROM node\n");

        assert_eq!(
            located(&context, "./image"),
            Located {
                containerfile: PathBuf::from("/ctx/image/Containerfile"),
                context: PathBuf::from("/ctx/image"),
                label: "./image/Containerfile".into(),
            },
        );
    }

    #[test]
    fn a_directory_holding_only_a_dockerfile_builds_that_and_says_so() {
        let mut context = FakeContext::new();
        context
            .dir("image", 0o755)
            .file("image/Dockerfile", 0o644, b"FROM alpine\n");

        assert_eq!(located(&context, "./image").label, "./image/Dockerfile");
    }

    #[test]
    fn a_trailing_slash_does_not_double_in_the_name_the_summary_prints() {
        let mut context = FakeContext::new();
        context
            .dir("image", 0o755)
            .file("image/Dockerfile", 0o644, b"FROM alpine\n");

        assert_eq!(located(&context, "./image/").label, "./image/Dockerfile");
    }

    #[test]
    fn a_file_of_any_name_is_the_containerfile_and_its_directory_is_the_context() {
        let mut context = FakeContext::new();
        context
            .dir("build", 0o755)
            .file("build/agent.containerfile", 0o644, b"FROM alpine\n");

        assert_eq!(
            located(&context, "./build/agent.containerfile"),
            Located {
                containerfile: PathBuf::from("/ctx/build/agent.containerfile"),
                context: PathBuf::from("/ctx/build"),
                label: "./build/agent.containerfile".into(),
            },
        );
    }

    #[test]
    fn the_document_s_own_directory_is_a_context_too() {
        let mut context = FakeContext::new();
        context.file("Dockerfile", 0o644, b"FROM alpine\n");

        assert_eq!(
            located(&context, "."),
            Located {
                containerfile: PathBuf::from("/ctx/Dockerfile"),
                context: PathBuf::from("/ctx"),
                label: "./Dockerfile".into(),
            },
        );
    }

    #[test]
    fn a_directory_holding_neither_name_is_refused_by_the_field_it_was_written_in() {
        let mut context = FakeContext::new();
        context.dir("image", 0o755);

        let refusal = refusal(&context, "./image");
        assert!(refusal.contains("spec.image"), "{refusal}");
        assert!(
            refusal.contains("neither Containerfile nor Dockerfile"),
            "{refusal}"
        );
    }

    #[test]
    fn a_path_that_names_nothing_is_refused_by_the_field_it_was_written_in() {
        let refusal = refusal(&FakeContext::new(), "./image");
        assert!(refusal.contains("spec.image"), "{refusal}");
        assert!(
            refusal.contains("names nothing beside the document"),
            "{refusal}"
        );
    }

    #[test]
    fn a_path_that_leaves_the_document_a_home_anchor_and_an_absolute_path_are_each_refused() {
        for (image, said) in [
            ("../elsewhere", "leaves the document's directory"),
            ("~/image", "home-anchored"),
            ("/srv/image", "must be relative"),
        ] {
            let refusal = refusal(&FakeContext::new(), image);
            assert!(refusal.contains("spec.image"), "{refusal}");
            assert!(refusal.contains(said), "{refusal}");
        }
    }

    #[test]
    fn a_context_the_host_cannot_read_is_reported_rather_than_taken_as_absent() {
        let mut context = FakeContext::new();
        context.dir("image", 0o755);
        context.unreadable = Some("/ctx/image".into());

        let refusal = refusal(&context, "./image");
        assert!(refusal.contains("permission denied"), "{refusal}");
    }

    #[test]
    fn a_containerfile_the_host_cannot_stat_is_reported_rather_than_taken_as_absent() {
        let mut context = FakeContext::new();
        context.dir("image", 0o755);
        context.unreadable = Some("/ctx/image/Containerfile".into());

        let refusal = refusal(&context, "./image");
        assert!(refusal.contains("permission denied"), "{refusal}");
    }

    /// A daemon reads the file out of the context tar, so it is named relative to that context and never by this machine's path.
    #[test]
    fn the_file_is_named_inside_the_context_for_an_engine_that_reads_only_the_tar() {
        let mut context = FakeContext::new();
        context.file("image/Containerfile", 0o644, b"FROM alpine\n");
        assert_eq!(located(&context, "./image").in_context(), "Containerfile");

        let mut beside = FakeContext::new();
        beside.file("Dockerfile.web", 0o644, b"FROM alpine\n");
        assert_eq!(
            located(&beside, "./Dockerfile.web").in_context(),
            "Dockerfile.web"
        );
    }
}
