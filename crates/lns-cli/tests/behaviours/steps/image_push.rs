use cucumber::{given, then};

use crate::world::{BehaviourWorld, StagedBuild, StagedRequest, StagedResolution};

/// The image config a staged build answers with: two history entries, so the size refusal has an instruction to name.
const STAGED_CONFIG: &str = r#"{"history":[{"created_by":"FROM docker.io/library/node:24-bookworm"},{"created_by":"RUN npm install -g @anthropic-ai/claude-code"}]}"#;

fn staged_image(layer_bytes: &[u64]) -> lns_ipc::PushableImage {
    lns_ipc::PushableImage {
        reference: format!("lns-build.local/built@sha256:{}", "cc".repeat(32)),
        digest: format!("sha256:{}", "cc".repeat(32)),
        manifest: "{}".to_string(),
        manifest_media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
        config: STAGED_CONFIG.to_string(),
        config_digest: format!("sha256:{}", "dd".repeat(32)),
        config_media_type: "application/vnd.oci.image.config.v1+json".to_string(),
        layers: layer_bytes
            .iter()
            .enumerate()
            .map(|(index, size)| lns_ipc::PushableLayer {
                digest: format!("sha256:{}", format!("{index:02}").repeat(32)),
                media_type: "application/vnd.oci.image.layer.v1.tar+gzip".to_string(),
                size: *size,
                path: format!("/layers/{index}"),
            })
            .collect(),
    }
}

fn stage(w: &mut BehaviourWorld, reused: bool, image: Option<lns_ipc::PushableImage>) {
    w.built_image = Some(StagedBuild {
        key: format!("sha256:{}", "5f".repeat(32)),
        label: "./image/Containerfile".to_string(),
        reused,
        image,
    });
}

#[given(regex = r"^the build answers with an image of (\d+) layers$")]
fn build_answers_with_an_image(w: &mut BehaviourWorld, layers: usize) {
    stage(w, false, Some(staged_image(&vec![64; layers])));
}

#[given(regex = r"^the build answers with an image of (\d+) layers it did not have to build$")]
fn build_answers_with_a_reused_image(w: &mut BehaviourWorld, layers: usize) {
    stage(w, true, Some(staged_image(&vec![64; layers])));
}

#[given("the build answers with a key and no image")]
fn build_answers_with_no_image(w: &mut BehaviourWorld) {
    stage(w, false, None);
}

#[given(regex = r"^this machine lets a built image weigh (\d+) bytes$")]
fn the_image_limit_is(w: &mut BehaviourWorld, bytes: u64) {
    w.image_limit = Some(bytes);
}

#[given(regex = r"^the build answers with an image whose second layer is (\d+) bytes$")]
fn build_answers_with_a_heavy_layer(w: &mut BehaviourWorld, bytes: u64) {
    stage(w, false, Some(staged_image(&[64, bytes])));
}

#[then(regex = r#"^the image was published into "([^"]+)"$"#)]
fn the_image_was_published_into(w: &mut BehaviourWorld, repository: String) {
    let published: Vec<&str> = w
        .pushed_images
        .iter()
        .map(|(repository, _)| repository.as_str())
        .collect();
    assert_eq!(
        published,
        vec![repository.as_str()],
        "§6: the image publishes where the artifact does, so one grant covers both"
    );
}

fn published_document(w: &BehaviourWorld) -> serde_json::Value {
    let doc = w
        .pushed_doc
        .as_ref()
        .expect("the push uploaded a config blob");
    serde_json::from_slice(doc).expect("the config blob is json")
}

#[then(r#"the published document's "spec.image" is the digest of the published image"#)]
fn the_published_image_is_a_digest(w: &mut BehaviourWorld) {
    let (repository, image) = w
        .pushed_images
        .first()
        .expect("the push published an image");
    assert_eq!(
        published_document(w)["spec"]["image"],
        serde_json::Value::String(format!("{repository}@{}", image.digest)),
        "a consumer must never receive a document it would have to build"
    );
}

#[then(regex = r#"^the published document's "spec\.imageSource" is "([^"]+)"$"#)]
fn the_published_image_source_is(w: &mut BehaviourWorld, path: String) {
    assert_eq!(published_document(w)["spec"]["imageSource"], path);
}

#[then(regex = r#"^the published artifact's build source layer holds "([^"]+)" and "([^"]+)"$"#)]
fn the_build_source_layer_holds(w: &mut BehaviourWorld, first: String, second: String) {
    let layer = w
        .pushed_build_source
        .as_ref()
        .expect("§7.3: the Containerfile and its context ship as a layer of the same artifact");
    let read = lns_artifact::build_source::read("./image/Containerfile", layer)
        .expect("the layer reads back as a build source");
    let held: Vec<&str> = read.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(held, vec![first.as_str(), second.as_str()]);
    assert!(
        read.text.contains("FROM docker.io/library/node"),
        "an approver reads the instructions themselves: {}",
        read.text
    );
}

#[then("the build was asked for a plan only")]
fn the_build_was_a_plan(w: &mut BehaviourWorld) {
    assert_eq!(
        w.build_requests,
        vec![StagedRequest {
            plan_only: true,
            rebuild: false,
        }],
        "a dry run builds nothing"
    );
}

#[then("the build was asked to ignore the cache")]
fn the_build_ignored_the_cache(w: &mut BehaviourWorld) {
    assert_eq!(
        w.build_requests,
        vec![StagedRequest {
            plan_only: false,
            rebuild: true,
        }],
    );
}

/// The egress a mixin authored, which a build step is held to exactly as the run of the same document would be.
const AUTHORED_EGRESS: &str = r#"{"http":[{"match":"registry.npmjs.org","verdict":"allow"}]}"#;

#[given("the lns.yaml also declares a mixin")]
fn the_document_declares_a_mixin(w: &mut BehaviourWorld) {
    let path = std::path::PathBuf::from("/work/lns.yaml");
    let document = w
        .author_files
        .get(&path)
        .expect("this scenario writes an lns.yaml")
        .clone();
    w.author_files.insert(
        path,
        format!("{document}  mixins:\n    - ./project-egress.yaml\n"),
    );
    w.author_files.insert(
        std::path::PathBuf::from("/work/project-egress.yaml"),
        "apiVersion: lns.run/v1\nkind: mixin\nname: project-egress\nspec: {}\n".to_string(),
    );
}

#[given("the service merges that document's mixin")]
fn the_service_resolves_that_document(w: &mut BehaviourWorld) {
    w.staged_resolution = Some(StagedResolution {
        definition: serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "sandbox",
            "name": "hermes",
            "spec": { "image": "./image" },
        })
        .to_string(),
        authored_egress: AUTHORED_EGRESS.to_string(),
    });
}

#[then("the build was handed the resolved document and what its mixins authored")]
fn the_build_was_handed_the_resolution(w: &mut BehaviourWorld) {
    let input = w
        .build_inputs
        .first()
        .expect("the push asked the builder to build");
    let document: serde_json::Value =
        serde_json::from_str(&input.definition).expect("the build carried a json document");
    assert!(
        document["spec"].get("mixins").is_none(),
        "a build step is the document's own run, so it never reaches the plan unresolved: {document}"
    );
    assert_eq!(input.authored_egress.as_deref(), Some(AUTHORED_EGRESS));
}

#[then("the builder was not asked to resolve the document")]
fn the_builder_was_not_asked_to_resolve(w: &mut BehaviourWorld) {
    assert!(
        w.resolve_requests.is_empty(),
        "a document declaring no mixin needs no resolution: {:?}",
        w.resolve_requests
    );
}

#[then("no build was asked for")]
fn no_build_was_asked_for(w: &mut BehaviourWorld) {
    assert!(
        w.build_requests.is_empty(),
        "an image named by reference publishes as written, so nothing builds: {:?}",
        w.build_requests
    );
}
