use anyhow::{Context, Result};
use http::HeaderValue;
use lns_artifact::build::BuiltArtifact;
use lns_policy::registry_auth::{
    JsonFileRegistryAuthStore, RegistryAuthStore, default_registry_auth_path,
};
use oci_client::{Reference, RegistryOperation, client::ClientConfig, secrets::RegistryAuth};

use crate::build::push_auth::{auth_error, push_error, select_auth};

/// The stored login for `reference`'s registry, or anonymous when none is recorded.
fn registry_auth_for(reference: &Reference) -> RegistryAuth {
    let loaded = JsonFileRegistryAuthStore::new(default_registry_auth_path()).load();
    select_auth(loaded, reference.registry())
}

/// Upload a built artifact's blobs and then its exact manifest bytes to `target`, reusing the stored `lns login` credential (which must carry push scope).
pub(crate) async fn push_artifact(built: &BuiltArtifact, target: &str) -> Result<()> {
    let reference: Reference = target
        .parse()
        .with_context(|| format!("invalid target ref {target}"))?;
    let client = oci_client::Client::new(ClientConfig {
        protocol: lns_artifact::client_protocol_for(reference.registry()),
        ..Default::default()
    });
    let auth = registry_auth_for(&reference);
    client
        .auth(&reference, &auth, RegistryOperation::Push)
        .await
        .map_err(|e| auth_error(&reference, &auth, e))?;
    for blob in &built.blobs {
        client
            .push_blob(&reference, blob.data.clone(), &blob.digest)
            .await
            .map_err(|e| {
                push_error(
                    &reference,
                    &auth,
                    e,
                    format!("pushing blob {}", blob.digest),
                )
            })?;
    }
    let content_type = HeaderValue::from_str(&built.manifest_media_type)
        .context("building manifest content-type header")?;
    client
        .push_manifest_raw(&reference, built.manifest.clone(), content_type)
        .await
        .map_err(|e| {
            push_error(
                &reference,
                &auth,
                e,
                format!("pushing manifest to {target}"),
            )
        })?;
    Ok(())
}
