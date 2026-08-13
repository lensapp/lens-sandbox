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
use std::io::{Read, Write};
use std::net::TcpStream;

fn sandbox_manifest() -> Vec<u8> {
    format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","metadata":{{"name":"some-sandbox"}},"spec":{{"image":"reg/base@sha256:{}"}}}}"#,
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
    assert_eq!(reg.manifest_repositories(), vec!["some/sandbox"]);

    let (manifest, _digest, config) = client
        .pull_manifest_and_config(&reference, &RegistryAuth::Anonymous)
        .await
        .expect("pull manifest+config");

    assert_eq!(
        manifest.artifact_type.as_deref(),
        Some("application/vnd.lens.sandbox.v1+json"),
        "the pulled manifest must carry the lens artifactType"
    );
    let parsed = lns_artifact::sandbox::parse(config.as_bytes())
        .expect("the pulled config loads through the same reader run and inspect use");
    assert_eq!(parsed.metadata.name, "some-sandbox");
}

#[test]
fn registry_offline_mode_rejects_requests() {
    let reg = registry::LocalRegistry::start();
    reg.set_online(false);
    let mut stream = TcpStream::connect(reg.host()).expect("connect to offline registry");
    stream
        .write_all(b"GET /v2/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write registry request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read registry response");
    assert!(response.starts_with("HTTP/1.1 503 Service Unavailable"));
}

#[tokio::test]
async fn the_version_index_stub_serves_its_list_and_404s_unknown_tools() {
    let index = registry::VersionIndex::start("node", "22.9.0\n22.11.0");
    let body = reqwest_free_get(&format!("{}/node", index.url()));
    assert!(
        body.contains("200 OK") && body.contains("22.11.0"),
        "got: {body}"
    );
    let missing = reqwest_free_get(&format!("{}/definitely-not-a-tool", index.url()));
    assert!(missing.contains("404"), "got: {missing}");
}

fn reqwest_free_get(url: &str) -> String {
    let without_scheme = url.trim_start_matches("http://");
    let (addr, path) = without_scheme.split_once('/').expect("url has a path");
    let mut stream = TcpStream::connect(addr).expect("connect to the stub");
    let request = format!("GET /{path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("send request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");
    response
}
