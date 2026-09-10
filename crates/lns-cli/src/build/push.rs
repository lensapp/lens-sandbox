use anyhow::{Context, Result};
use http::HeaderValue;
use lns_artifact::build::BuiltArtifact;
use lns_policy::registry_auth::{JsonFileRegistryAuthStore, RegistryAuthStore};
use oci_client::{Reference, RegistryOperation, secrets::RegistryAuth};

use crate::build::push_auth::{auth_error, names_nothing_yet, push_error, select_auth};

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

/// Upload a built image into `repository` under `tag` — every layer this machine holds on disk, its config, then the manifest that references them (`docs/sandbox-spec.md` §6).
pub(crate) async fn push_image(
    image: &lns_ipc::PushableImage,
    repository: &str,
    tag: &str,
) -> Result<()> {
    let tag = format!("{repository}:{tag}");
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
    Ok(())
}

/// The image manifest media types a repository may answer one architecture's tag with, so a push reads back what another architecture published without a platform resolver picking for it.
const IMAGE_MANIFEST_TYPES: [&str; 2] = [
    oci_client::manifest::OCI_IMAGE_MEDIA_TYPE,
    oci_client::manifest::IMAGE_MANIFEST_MEDIA_TYPE,
];

/// What one architecture's image tag holds, read as the index entry it becomes: the manifest's own digest and size, and the platform its config declares. A tag no push has written yet holds nothing, which is not an error (§6).
pub(crate) async fn image_at(
    repository: &str,
    tag: &str,
) -> Result<Option<lns_artifact::image_index::IndexEntry>> {
    let target = format!("{repository}:{tag}");
    let reference: Reference = target
        .parse()
        .with_context(|| format!("invalid image target {target}"))?;
    let client = oci_client::Client::new(super::push_client_config(reference.registry()));
    let auth = registry_auth_for(&reference)?;
    if let Some(absent) = read_denied(&client, &reference, &auth, &target).await? {
        return Ok(absent);
    }
    let (bytes, digest) = match client
        .pull_manifest_raw(&reference, &auth, &IMAGE_MANIFEST_TYPES)
        .await
    {
        Ok(answer) => answer,
        Err(error) => return nothing_or_error(&target, error),
    };
    let manifest: oci_client::manifest::OciImageManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("reading the manifest at {target}"))?;
    let mut config = Vec::new();
    client
        .pull_blob(&reference, &manifest.config, &mut config)
        .await
        .map_err(|e| anyhow::anyhow!("reading the image config of {target}: {e}"))?;
    let config: serde_json::Value = serde_json::from_slice(&config)
        .with_context(|| format!("parsing the config of {target}"))?;
    let (Some(os), Some(architecture)) = (config["os"].as_str(), config["architecture"].as_str())
    else {
        anyhow::bail!(
            "the image at {target} declares no os and architecture, so no index can hold it"
        );
    };
    Ok(Some(lns_artifact::image_index::IndexEntry {
        digest,
        size: bytes.len() as u64,
        media_type: manifest
            .media_type
            .unwrap_or_else(|| oci_client::manifest::OCI_IMAGE_MEDIA_TYPE.to_string()),
        os: os.to_string(),
        architecture: architecture.to_string(),
    }))
}

/// The digest of the index one reference's document names today, or nothing where no push has written one yet.
pub(crate) async fn index_at(repository: &str, tag: &str) -> Result<Option<String>> {
    let target = format!("{repository}:{tag}");
    let reference: Reference = target
        .parse()
        .with_context(|| format!("invalid image index target {target}"))?;
    let client = oci_client::Client::new(super::push_client_config(reference.registry()));
    let auth = registry_auth_for(&reference)?;
    if let Some(absent) = read_denied(&client, &reference, &auth, &target).await? {
        return Ok(absent);
    }
    match client
        .pull_manifest_raw(
            &reference,
            &auth,
            &[lns_artifact::image_index::INDEX_MEDIA_TYPE],
        )
        .await
    {
        Ok((_, digest)) => Ok(Some(digest)),
        Err(error) => nothing_or_error(&target, error),
    }
}

/// A tag no push has written holds nothing, which is not an error; every other failure stops the push, because an index assembled over a read that failed would drop what another architecture published (§6.2).
fn nothing_or_error<T>(
    target: &str,
    error: oci_client::errors::OciDistributionError,
) -> Result<Option<T>> {
    match names_nothing_yet(&error) {
        true => Ok(None),
        false => {
            Err(anyhow::Error::new(error)).with_context(|| format!("reading what {target} holds"))
        }
    }
}

/// The pull-scope handshake this read needs, answered with `Some(None)` where the registry says the repository holds nothing at all.
async fn read_denied<T>(
    client: &oci_client::Client,
    reference: &Reference,
    auth: &RegistryAuth,
    target: &str,
) -> Result<Option<Option<T>>> {
    match client.auth(reference, auth, RegistryOperation::Pull).await {
        Ok(_) => Ok(None),
        Err(error) => nothing_or_error(target, error).map(Some),
    }
}

/// Upload the assembled image index, which the published document names by digest, under a tag so the manifests it holds stay reachable from one.
pub(crate) async fn push_index(repository: &str, tag: &str, index: &[u8]) -> Result<()> {
    let target = format!("{repository}:{tag}");
    let reference: Reference = target
        .parse()
        .with_context(|| format!("invalid image index target {target}"))?;
    let client = oci_client::Client::new(super::push_client_config(reference.registry()));
    let auth = registry_auth_for(&reference)?;
    client
        .auth(&reference, &auth, RegistryOperation::Push)
        .await
        .map_err(|e| auth_error(&reference, &auth, e))?;
    let content_type = HeaderValue::from_str(lns_artifact::image_index::INDEX_MEDIA_TYPE)
        .context("building image index content-type header")?;
    client
        .push_manifest_raw(&reference, index.to_vec(), content_type)
        .await
        .map_err(|e| {
            push_error(
                &reference,
                &auth,
                e,
                format!("pushing the image index to {target}"),
            )
        })?;
    Ok(())
}
