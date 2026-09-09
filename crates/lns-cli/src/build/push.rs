use anyhow::{Context, Result};
use http::HeaderValue;
use lns_artifact::build::BuiltArtifact;
use lns_policy::registry_auth::{JsonFileRegistryAuthStore, RegistryAuthStore};
use oci_client::{Reference, RegistryOperation, secrets::RegistryAuth};

use crate::build::push_auth::{auth_error, push_error, select_auth};

/// The stored login for `reference`'s registry, or anonymous when none is recorded.
fn registry_auth_for(reference: &Reference) -> Result<RegistryAuth> {
    let loaded = JsonFileRegistryAuthStore::new(lns_ipc::registry_auth_path()?).load();
    Ok(select_auth(loaded, reference.registry()))
}

/// Upload a built artifact's blobs and then its exact manifest bytes to `target`, reusing the stored `lns login` credential (which must carry push scope).
pub(crate) async fn push_artifact(built: &BuiltArtifact, target: &str) -> Result<()> {
    let reference: Reference = target
        .parse()
        .with_context(|| format!("invalid target ref {target}"))?;
    let client = oci_client::Client::new(super::push_client_config(reference.registry()));
    let auth = registry_auth_for(&reference)?;
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

/// Upload a built image into `repository` — every layer this machine holds on disk, its config, then the manifest that references them — and answer with the digest reference the published document names (`docs/sandbox-spec.md` §6).
pub(crate) async fn push_image(image: &lns_ipc::PushableImage, repository: &str) -> Result<String> {
    let tag = format!("{repository}:{}", image.digest.replace(':', "-"));
    let reference: Reference = tag
        .parse()
        .with_context(|| format!("invalid image target {tag}"))?;
    let client = oci_client::Client::new(super::push_client_config(reference.registry()));
    let auth = registry_auth_for(&reference)?;
    client
        .auth(&reference, &auth, RegistryOperation::Push)
        .await
        .map_err(|e| auth_error(&reference, &auth, e))?;
    for layer in &image.layers {
        let bytes = std::fs::read(&layer.path).with_context(|| {
            format!("reading the built layer {} at {}", layer.digest, layer.path)
        })?;
        client
            .push_blob(&reference, bytes, &layer.digest)
            .await
            .map_err(|e| {
                push_error(
                    &reference,
                    &auth,
                    e,
                    format!("pushing image layer {}", layer.digest),
                )
            })?;
    }
    client
        .push_blob(
            &reference,
            image.config.clone().into_bytes(),
            &image.config_digest,
        )
        .await
        .map_err(|e| {
            push_error(
                &reference,
                &auth,
                e,
                format!("pushing image config {}", image.config_digest),
            )
        })?;
    let content_type = HeaderValue::from_str(&image.manifest_media_type)
        .context("building image manifest content-type header")?;
    client
        .push_manifest_raw(
            &reference,
            image.manifest.clone().into_bytes(),
            content_type,
        )
        .await
        .map_err(|e| {
            push_error(
                &reference,
                &auth,
                e,
                format!("pushing image manifest to {tag}"),
            )
        })?;
    Ok(format!("{repository}@{}", image.digest))
}
