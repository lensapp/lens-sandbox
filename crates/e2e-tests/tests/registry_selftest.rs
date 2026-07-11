//! Validates the in-process OCI registry speaks the exact protocol subset that
//! `lns push` (oci_client chunked push) and `lns run` (manifest/config
//! pull) use — without booting a VM, so the registry can be iterated fast.

#[path = "support/registry.rs"]
mod registry;

use http::HeaderValue;
use oci_client::{
    Reference,
    client::{ClientConfig, ClientProtocol},
    secrets::RegistryAuth,
};

fn sandbox_manifest() -> Vec<u8> {
    format!(
        r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{{"name":"some-sandbox"}},"spec":{{"isolation":"microvm","baseImage":"reg/base@sha256:{}"}}}}"#,
        "a".repeat(64)
    )
    .into_bytes()
}

#[tokio::test]
async fn build_push_and_pull_round_trip_through_the_local_registry() {
    let reg = registry::LocalRegistry::start();
    let built = lns_artifact::build::build_artifact(&sandbox_manifest()).expect("build sandbox");

    let reference: Reference = format!("{}/some/sandbox:1", reg.host())
        .parse()
        .expect("ref parses");
    let client = oci_client::Client::new(ClientConfig {
        protocol: ClientProtocol::Http,
        ..Default::default()
    });

    for blob in &built.blobs {
        client
            .push_blob(&reference, blob.data.clone(), &blob.digest)
            .await
            .unwrap_or_else(|e| panic!("push blob {}: {e}", blob.digest));
    }
    let content_type = HeaderValue::from_str(&built.manifest_media_type).unwrap();
    client
        .push_manifest_raw(&reference, built.manifest.clone(), content_type)
        .await
        .expect("push manifest");

    let (manifest, _digest, config) = client
        .pull_manifest_and_config(&reference, &RegistryAuth::Anonymous)
        .await
        .expect("pull manifest+config");

    assert_eq!(
        manifest.artifact_type.as_deref(),
        Some("application/vnd.lens.sandbox.v1+json"),
        "the pulled manifest must carry the lens artifactType"
    );
    let parsed = lns_artifact::spec::parse_sandbox(config.as_bytes())
        .expect("the pulled config is the sandbox spec");
    assert_eq!(parsed.metadata.name, "some-sandbox");
}
