use anyhow::{Context, Result, bail};

/// The names a build context may hold, in the order a directory is searched — `Containerfile` first, as Podman does.
pub const CONTAINERFILE_NAMES: [&str; 2] = ["Containerfile", "Dockerfile"];

/// What a built image may weigh before a push refuses it, unless this machine's `push.imageLimit` says otherwise (§6).
pub const DEFAULT_IMAGE_LIMIT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

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

/// The rules `spec.imageSource` is held to: `lns push` writes it beside the digest of the image it built, so it names a path and never stands beside one (§6).
pub fn validate_source(image: &str, image_source: Option<&str>) -> Result<()> {
    let Some(written) = image_source else {
        return Ok(());
    };
    if let ImageSource::Containerfile(path) = source(image) {
        bail!(
            "spec.imageSource {written:?} stands beside spec.image {path:?}, which is itself a Containerfile; \
             imageSource records what a published image was built from, and `lns push` writes it"
        );
    }
    let ImageSource::Containerfile(path) = source(written) else {
        bail!(
            "spec.imageSource {written:?} must be the Containerfile path the image was built from, such as ./image"
        );
    };
    validate(path).context("spec.imageSource")
}

/// The push-time rewrite of a path-form `spec.image` (§6): the document publishes the digest of the image lns built, and `imageSource` keeps the path the author wrote.
pub fn rewrite_to_built(doc: &[u8], built: &str) -> Result<Vec<u8>> {
    let mut value: serde_json::Value =
        serde_json::from_slice(doc).context("re-reading the definition for the image rewrite")?;
    let written = value["spec"]["image"].as_str().unwrap_or_default();
    let ImageSource::Containerfile(path) = source(written) else {
        return Ok(doc.to_vec());
    };
    let path = path.to_string();
    value["spec"]["image"] = serde_json::Value::String(built.to_string());
    value["spec"]["imageSource"] = serde_json::Value::String(path);
    serde_json::to_vec(&value).context("serializing the image-pinned definition")
}

/// `imageSource` is the record of the build source layer beside it, so a push that ships no such layer publishes no such record — a pulled document re-pushed as it stands must not name a path this artifact does not carry (§7.3).
pub fn forget_source(doc: &[u8]) -> Result<Vec<u8>> {
    let mut value: serde_json::Value =
        serde_json::from_slice(doc).context("re-reading the definition for the image rewrite")?;
    let Some(spec) = value["spec"].as_object_mut() else {
        return Ok(doc.to_vec());
    };
    if spec.remove("imageSource").is_none() {
        return Ok(doc.to_vec());
    }
    serde_json::to_vec(&value).context("serializing the image-pinned definition")
}

/// The tag a built image publishes under, beside the artifact that names it: a digest is not a tag, so the one character a tag cannot carry is replaced and the digest stays legible in the reference (§6).
pub fn published_tag(repository: &str, digest: &str) -> String {
    format!("{repository}:{}", digest.replace(':', "-"))
}

/// One layer of a built image, as its manifest addresses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageLayer {
    pub digest: String,
    pub bytes: u64,
}

/// A built image over the limit fails the push naming the layer that grew it, the way a fileset over its limit fails validate (§6).
pub fn refuse_an_image_over(limit: u64, layers: &[ImageLayer], config: &str) -> Result<()> {
    let total: u64 = layers.iter().map(|layer| layer.bytes).sum();
    if total <= limit {
        return Ok(());
    }
    let heaviest = layers
        .iter()
        .max_by_key(|layer| layer.bytes)
        .expect("a total over the limit needs at least one layer to have made it");
    bail!(
        "the built image is {total} bytes, over the {limit}-byte limit a push may publish; \
         its largest layer is {} at {} bytes, written by {}; \
         make that instruction write less, or raise push.imageLimit",
        heaviest.digest,
        heaviest.bytes,
        wrote(config, layers, &heaviest.digest),
    );
}

/// Which instruction produced one layer: the image config's history, whose entries that are not `empty_layer` line up with the layers in order.
fn wrote(config: &str, layers: &[ImageLayer], digest: &str) -> String {
    let position = layers.iter().position(|layer| layer.digest == digest);
    let history = serde_json::from_str::<serde_json::Value>(config)
        .ok()
        .and_then(|config| config["history"].as_array().cloned())
        .unwrap_or_default();
    let mut produced = history
        .iter()
        .filter(|entry| !entry["empty_layer"].as_bool().unwrap_or(false));
    match position.and_then(|position| produced.nth(position)) {
        Some(entry) => entry["created_by"]
            .as_str()
            .unwrap_or("an instruction the image config does not name")
            .to_string(),
        None => "an instruction the image config does not name".to_string(),
    }
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

    const BUILT: &str = "ghcr.io/team/hermes@sha256:abc";

    fn doc(image: &str) -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{{"image":"{image}"}}}}"#
        )
        .into_bytes()
    }

    fn rewritten(image: &str) -> serde_json::Value {
        serde_json::from_slice(&rewrite_to_built(&doc(image), BUILT).expect("rewriting"))
            .expect("the rewritten document is json")
    }

    #[test]
    fn a_path_form_image_publishes_as_the_digest_and_keeps_the_path_the_author_wrote() {
        let published = rewritten("./image");
        assert_eq!(published["spec"]["image"], BUILT);
        assert_eq!(
            published["spec"]["imageSource"], "./image",
            "§6: the path is what says where the image came from, and a consumer never builds it"
        );
    }

    #[test]
    fn a_reference_form_image_publishes_untouched_and_gains_no_source() {
        let published = rewritten("ghcr.io/team/base:1");
        assert_eq!(published["spec"]["image"], "ghcr.io/team/base:1");
        assert!(
            published["spec"].get("imageSource").is_none(),
            "an image nobody built has no source to record: {published}"
        );
    }

    /// A pulled document already carries the path its first push recorded, and re-pushing it ships no build source layer — so the record has to go with the layer it describes.
    #[test]
    fn a_republished_document_that_ships_no_build_source_records_none() {
        let pulled = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/hermes@sha256:abc","imageSource":"./image"}}"#;
        let published: serde_json::Value =
            serde_json::from_slice(&forget_source(pulled).expect("forgetting"))
                .expect("the republished document is json");
        assert!(
            published["spec"].get("imageSource").is_none(),
            "a document that discloses no build must claim none: {published}"
        );
        assert_eq!(published["spec"]["image"], "ghcr.io/team/hermes@sha256:abc");
    }

    #[test]
    fn a_document_with_no_spec_object_is_published_exactly_as_it_arrived() {
        let bare = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes"}"#;
        assert_eq!(forget_source(bare).expect("forgetting"), bare.to_vec());
    }

    #[test]
    fn a_document_that_never_recorded_a_source_is_published_as_it_stands() {
        let published: serde_json::Value = serde_json::from_slice(
            &forget_source(&doc("ghcr.io/team/base:1")).expect("forgetting"),
        )
        .expect("json");
        assert_eq!(published["spec"]["image"], "ghcr.io/team/base:1");
    }

    #[test]
    fn a_built_image_publishes_under_a_tag_its_digest_is_still_readable_in() {
        assert_eq!(
            published_tag("ghcr.io/team/hermes", "sha256:abc"),
            "ghcr.io/team/hermes:sha256-abc",
            "a tag carries no colon, and an operator still has to recognize the image"
        );
    }

    #[test]
    fn a_document_that_is_not_json_names_the_rewrite_it_failed_in() {
        let err = rewrite_to_built(b"not json", BUILT).unwrap_err();
        assert!(
            format!("{err:#}").contains("re-reading the definition"),
            "{err:#}"
        );
    }

    #[test]
    fn an_image_source_beside_a_containerfile_image_is_refused_as_the_contradiction_it_is() {
        let err = validate_source("./image", Some("./image")).unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("spec.imageSource") && message.contains("lns push"),
            "a document cannot both say build this and say this was built from that: {message}"
        );
    }

    #[test]
    fn an_image_source_that_is_no_path_is_refused_naming_the_spelling_to_use() {
        let err = validate_source(BUILT, Some("ghcr.io/team/base:1")).unwrap_err();
        assert!(format!("{err:#}").contains("./image"), "{err:#}");
    }

    #[test]
    fn an_image_source_leaving_the_document_is_held_to_the_same_path_rules_as_an_image() {
        let err = validate_source(BUILT, Some("../elsewhere")).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("spec.imageSource"), "{message}");
    }

    #[test]
    fn a_published_document_carrying_both_a_digest_and_a_source_is_what_push_writes() {
        validate_source(BUILT, Some("./image")).expect("this is exactly what §6 produces");
        validate_source(BUILT, None).expect("an image nobody built carries no source");
    }

    fn layer(digest: &str, bytes: u64) -> ImageLayer {
        ImageLayer {
            digest: digest.to_string(),
            bytes,
        }
    }

    const HISTORY: &str = r#"{"history":[
        {"created_by":"ADD alpine.tar"},
        {"created_by":"ENV MODE=research","empty_layer":true},
        {"created_by":"RUN npm install -g @anthropic-ai/claude-code"}
    ]}"#;

    #[test]
    fn an_image_inside_the_limit_publishes() {
        refuse_an_image_over(
            100,
            &[layer("sha256:a", 60), layer("sha256:b", 40)],
            HISTORY,
        )
        .expect("a push at the limit is a push");
    }

    #[test]
    fn an_image_over_the_limit_names_the_layer_that_grew_it_and_the_instruction_that_wrote_it() {
        let err = refuse_an_image_over(
            100,
            &[layer("sha256:base", 10), layer("sha256:heavy", 95)],
            HISTORY,
        )
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("105 bytes"), "{message}");
        assert!(message.contains("100-byte limit"), "{message}");
        assert!(message.contains("sha256:heavy"), "{message}");
        assert!(
            message.contains("RUN npm install -g @anthropic-ai/claude-code"),
            "the author has to know which instruction to shrink: {message}"
        );
        assert!(message.contains("push.imageLimit"), "{message}");
    }

    #[test]
    fn a_layer_no_history_entry_accounts_for_is_still_named_by_its_digest() {
        let err = refuse_an_image_over(1, &[layer("sha256:only", 2)], "{}").unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("sha256:only"), "{message}");
        assert!(
            message.contains("does not name"),
            "a config that accounts for no layer must not make the refusal invent one: {message}"
        );
    }

    #[test]
    fn a_config_that_is_not_json_leaves_the_refusal_standing() {
        let err = refuse_an_image_over(1, &[layer("sha256:only", 2)], "not json").unwrap_err();
        assert!(format!("{err:#}").contains("does not name"), "{err:#}");
    }
}
