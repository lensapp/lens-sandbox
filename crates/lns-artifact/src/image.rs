use anyhow::{Result, bail};

/// The names a build context may hold, in the order a directory is searched — `Containerfile` first, as Podman does.
pub const CONTAINERFILE_NAMES: [&str; 2] = ["Containerfile", "Dockerfile"];

/// What `spec.image` names: an image to pull, or a Containerfile beside the document for lns to build (§3.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSource<'a> {
    Reference(&'a str),
    Containerfile(&'a str),
}

/// A value beginning `.`, `/` or `~` is a path — no OCI reference starts that way — so one predicate decides the form for validation, the run, and the push.
pub fn source(image: &str) -> ImageSource<'_> {
    let leading = image.trim_start().chars().next();
    if matches!(leading, Some('.' | '/' | '~')) {
        ImageSource::Containerfile(image)
    } else {
        ImageSource::Reference(image)
    }
}

/// The path rules a Containerfile-form `spec.image` is held to: the artifact ships what it names, so it names something beside the document (§3.1.1).
pub fn validate(image: &str) -> Result<()> {
    let ImageSource::Containerfile(path) = source(image) else {
        return Ok(());
    };
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
            "spec.image {path:?} leaves the document's directory, and the artifact ships what it names; move the Containerfile into the project"
        );
    }
    if path.chars().any(char::is_control) {
        bail!("spec.image {path:?} must not contain control characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registry_coordinate_is_an_image_to_pull() {
        for reference in [
            "alpine:3.20",
            "ghcr.io/acme/base@sha256:abc",
            "docker.io/library/node:24-bookworm",
        ] {
            assert_eq!(
                source(reference),
                ImageSource::Reference(reference),
                "{reference} is a reference, so nothing about it is built"
            );
        }
    }

    #[test]
    fn a_path_shaped_value_names_a_containerfile_to_build() {
        for path in [
            "./image",
            ".",
            "./build/api",
            "../elsewhere",
            "~/image",
            "/srv/image",
        ] {
            assert_eq!(
                source(path),
                ImageSource::Containerfile(path),
                "{path} is path-shaped, so it names a build rather than a registry coordinate"
            );
        }
    }

    #[test]
    fn a_containerfile_path_that_leaves_the_document_is_refused_by_the_field_it_was_written_in() {
        for path in ["../elsewhere", "./build/../../elsewhere"] {
            let refusal = format!("{:#}", validate(path).unwrap_err());
            assert!(
                refusal.contains("spec.image") && refusal.contains(path),
                "the author has to see which value is refused: {refusal}"
            );
        }
    }

    #[test]
    fn a_home_anchored_containerfile_path_is_refused_because_the_artifact_ships_what_it_names() {
        let refusal = format!("{:#}", validate("~/image").unwrap_err());
        assert!(
            refusal.contains("spec.image") && refusal.contains("~/image"),
            "got: {refusal}"
        );
    }

    #[test]
    fn an_absolute_containerfile_path_is_refused_and_the_relative_spelling_is_named() {
        let refusal = format!("{:#}", validate("/srv/image").unwrap_err());
        assert!(refusal.contains("./"), "got: {refusal}");
    }

    #[test]
    fn a_control_character_is_refused_wherever_it_sits_in_the_path() {
        assert!(validate("./im\nage").is_err());
    }

    #[test]
    fn the_paths_beside_the_document_are_accepted() {
        for path in ["./image", ".", "./image/Containerfile", "./a/b/c"] {
            validate(path).unwrap_or_else(|e| panic!("{path} is beside the document: {e:#}"));
        }
    }

    #[test]
    fn a_reference_is_held_to_no_path_rule() {
        validate("ghcr.io/acme/base:1").expect("a reference is published as written");
    }

    #[test]
    fn a_directory_holding_both_names_builds_the_containerfile_as_podman_does() {
        assert_eq!(CONTAINERFILE_NAMES, ["Containerfile", "Dockerfile"]);
    }
}
