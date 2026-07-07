use crate::world::BehaviourWorld;
use cucumber::gherkin::Step;
use cucumber::given;
use lns_ipc::{ArtifactInspection, BundleView, FilesetView, ImageView, SignatureView};

fn full_digest() -> String {
    format!("sha256:{}", "a".repeat(64))
}

fn empty_bundle(reference: &str) -> BundleView {
    BundleView {
        reference: reference.to_string(),
        sandbox_base_image: None,
        filesets: Vec::new(),
        integrations: Vec::new(),
        signature: SignatureView::Unsigned,
        policy_flags: Vec::new(),
    }
}

#[given(regex = r#"^the service inspects "([^"]+)" as a plain image$"#)]
fn inspects_plain_image(world: &mut BehaviourWorld, reference: String) {
    world.image.inspect_result = Some(ArtifactInspection::Image(ImageView {
        reference,
        digest: full_digest(),
    }));
}

#[given(regex = r#"^the service inspects "([^"]+)" as a bundle composing:$"#)]
fn inspects_bundle_composing(world: &mut BehaviourWorld, step: &Step, reference: String) {
    let mut bundle = empty_bundle(&reference);
    let rows = &step.table().expect("the composing step needs a table").rows;
    for row in rows {
        let kind = row[0].trim();
        let value = row[1].trim();
        match kind {
            "sandbox base" => bundle.sandbox_base_image = Some(value.to_string()),
            "fileset" => {
                let (name, mount) = value
                    .split_once("->")
                    .expect("a fileset row reads `name -> mount`");
                bundle.filesets.push(FilesetView {
                    name: name.trim().to_string(),
                    mount_path: mount.trim().to_string(),
                });
            }
            "integration" => bundle.integrations.push(value.to_string()),
            other => panic!("unknown composing row kind {other:?}"),
        }
    }
    world.image.inspect_result = Some(ArtifactInspection::Bundle(bundle));
}

#[given(regex = r#"^the service inspects "([^"]+)" as a bundle signed by a trusted key$"#)]
fn inspects_signed_bundle(world: &mut BehaviourWorld, reference: String) {
    let mut bundle = empty_bundle(&reference);
    bundle.signature = SignatureView::SignedTrusted;
    world.image.inspect_result = Some(ArtifactInspection::Bundle(bundle));
}

#[given(regex = r#"^the service inspects "([^"]+)" as a bundle whose policy defaults to allow$"#)]
fn inspects_permissive_bundle(world: &mut BehaviourWorld, reference: String) {
    let mut bundle = empty_bundle(&reference);
    bundle
        .policy_flags
        .push("permissive defaultVerdict: allow — the sandbox is open by default".to_string());
    world.image.inspect_result = Some(ArtifactInspection::Bundle(bundle));
}

#[given(regex = r#"^the service reports "inspect" needs a login for host "([^"]+)"$"#)]
fn inspect_needs_login(world: &mut BehaviourWorld, host: String) {
    world.image.refuse_message = Some(format!(
        "inspecting the bundle needs a login for {host}: run `lns login {host}`"
    ));
}
