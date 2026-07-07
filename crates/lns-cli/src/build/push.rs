use anyhow::{Context, Result};
use http::HeaderValue;
use lns_artifact::build::BuiltArtifact;
use lns_policy::registry_auth::{
    JsonFileRegistryAuthStore, RegistryAuthStore, credential_for, default_registry_auth_path,
};
use oci_client::{
    Reference, RegistryOperation,
    client::{ClientConfig, ClientProtocol},
    secrets::RegistryAuth,
};

/// The stored login for `reference`'s registry, or anonymous when none is recorded.
fn registry_auth_for(reference: &Reference) -> RegistryAuth {
    let store = JsonFileRegistryAuthStore::new(default_registry_auth_path());
    let Ok(file) = store.load() else {
        return RegistryAuth::Anonymous;
    };
    match credential_for(&file, reference.registry()) {
        Some(cred) => RegistryAuth::Basic(cred.username.clone(), cred.secret.clone()),
        None => RegistryAuth::Anonymous,
    }
}

/// Upload a built artifact's blobs and then its exact manifest bytes to `target`, reusing the stored `lns login` credential (which must carry push scope).
pub(crate) async fn push_artifact(built: &BuiltArtifact, target: &str) -> Result<()> {
    let reference: Reference = target
        .parse()
        .with_context(|| format!("invalid target ref {target}"))?;
    let protocol = if lns_artifact::is_loopback_registry(reference.registry()) {
        ClientProtocol::Http
    } else {
        ClientProtocol::Https
    };
    let client = oci_client::Client::new(ClientConfig {
        protocol,
        ..Default::default()
    });
    let auth = registry_auth_for(&reference);
    client
        .auth(&reference, &auth, RegistryOperation::Push)
        .await
        .map_err(|e| anyhow::anyhow!("authenticating to {}: {e}", reference.registry()))?;
    for blob in &built.blobs {
        client
            .push_blob(&reference, blob.data.clone(), &blob.digest)
            .await
            .with_context(|| format!("pushing blob {}", blob.digest))?;
    }
    let content_type = HeaderValue::from_str(&built.manifest_media_type)
        .context("building manifest content-type header")?;
    client
        .push_manifest_raw(&reference, built.manifest.clone(), content_type)
        .await
        .with_context(|| format!("pushing manifest to {target}"))?;
    Ok(())
}
