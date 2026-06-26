use anyhow::{Context, Result};
use lns_policy::artifact::Family;
use lns_policy::registry_auth::{
    JsonFileRegistryAuthStore, RegistryAuthFile, RegistryAuthStore, credential_for,
    default_registry_auth_path,
};
use oci_client::Reference;
use oci_client::secrets::RegistryAuth;

use crate::image::RealRegistry;

pub struct ManifestHead {
    pub config_media_type: String,
    pub artifact_type: Option<String>,
    pub config_blob: Vec<u8>,
    pub digest: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Pulled {
    Artifact {
        artifact_type: String,
        config_blob: Vec<u8>,
        digest: String,
    },
    Image {
        digest: String,
    },
}

pub(crate) trait ArtifactRegistry: Send + Sync {
    fn push_artifact(
        &self,
        reference: &Reference,
        artifact_type: &str,
        config_media_type: &str,
        config_blob: &[u8],
        layers: &[Vec<u8>],
        auth: &RegistryAuth,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    fn pull_head(
        &self,
        reference: &Reference,
        auth: &RegistryAuth,
    ) -> impl std::future::Future<Output = Result<ManifestHead>> + Send;

    fn pull_image_to_cache(
        &self,
        reference: &str,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    fn push_image_from_cache(
        &self,
        source_reference: &str,
        target: &Reference,
        auth: &RegistryAuth,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    fn pull_artifact_layers(
        &self,
        reference: &Reference,
        auth: &RegistryAuth,
    ) -> impl std::future::Future<Output = Result<Vec<Vec<u8>>>> + Send;
}

fn auth_for(registry: &str, file: &RegistryAuthFile) -> RegistryAuth {
    match credential_for(file, registry) {
        Some(cred) => RegistryAuth::Basic(cred.username.clone(), cred.secret.clone()),
        None => RegistryAuth::Anonymous,
    }
}

fn resolve(reference: &str, store: &dyn RegistryAuthStore) -> Result<(Reference, RegistryAuth)> {
    let reference: Reference = reference
        .parse()
        .with_context(|| format!("invalid registry reference: {reference}"))?;
    let file = store.load().context("loading registry credentials")?;
    let auth = auth_for(reference.resolve_registry(), &file);
    Ok((reference, auth))
}

/// Best-effort auth for an image pull: resolves the stored credential for the reference's registry, falling back to anonymous on any parse/load failure (the pull surfaces its own error if the reference is bad).
pub(crate) fn resolve_auth(reference: &str) -> RegistryAuth {
    match reference.parse::<Reference>() {
        Ok(parsed) => auth_for(
            parsed.resolve_registry(),
            &store().load().unwrap_or_default(),
        ),
        Err(_) => RegistryAuth::Anonymous,
    }
}

async fn push_artifact_with<R: ArtifactRegistry>(
    client: &R,
    store: &dyn RegistryAuthStore,
    reference: &str,
    artifact_type: &str,
    config_media_type: &str,
    config_blob: &[u8],
    layers: &[Vec<u8>],
) -> Result<String> {
    let (reference, auth) = resolve(reference, store)?;
    client
        .push_artifact(
            &reference,
            artifact_type,
            config_media_type,
            config_blob,
            layers,
            &auth,
        )
        .await
}

async fn pull_with<R: ArtifactRegistry>(
    client: &R,
    store: &dyn RegistryAuthStore,
    reference: &str,
) -> Result<Pulled> {
    let (parsed, auth) = resolve(reference, store)?;
    let head = client.pull_head(&parsed, &auth).await?;
    if Family::from_config_media_type(&head.config_media_type).is_some() {
        Ok(Pulled::Artifact {
            artifact_type: head
                .artifact_type
                .unwrap_or_else(|| head.config_media_type.clone()),
            config_blob: head.config_blob,
            digest: head.digest,
        })
    } else {
        let digest = client.pull_image_to_cache(reference).await?;
        Ok(Pulled::Image { digest })
    }
}

async fn push_image_with<R: ArtifactRegistry>(
    client: &R,
    store: &dyn RegistryAuthStore,
    source_reference: &str,
    target_reference: &str,
) -> Result<String> {
    let (target, auth) = resolve(target_reference, store)?;
    client
        .push_image_from_cache(source_reference, &target, &auth)
        .await
}

pub(crate) fn registry_for(reference: &str) -> RealRegistry {
    let target = reference
        .parse::<Reference>()
        .ok()
        .map(|r| r.resolve_registry().to_string());
    let protocol = crate::image::registry_protocol(
        std::env::var("LNS_REGISTRY_PLAIN_HTTP").ok().as_deref(),
        target.as_deref(),
    );
    RealRegistry::with_protocol(protocol)
}

pub(crate) fn store() -> JsonFileRegistryAuthStore {
    JsonFileRegistryAuthStore::new(default_registry_auth_path())
}

pub async fn push_artifact(
    reference: &str,
    artifact_type: &str,
    config_media_type: &str,
    config_blob: &[u8],
    layers: &[Vec<u8>],
) -> Result<String> {
    push_artifact_with(
        &registry_for(reference),
        &store(),
        reference,
        artifact_type,
        config_media_type,
        config_blob,
        layers,
    )
    .await
}

pub async fn push_image(source_reference: &str, target_reference: &str) -> Result<String> {
    push_image_with(
        &registry_for(target_reference),
        &store(),
        source_reference,
        target_reference,
    )
    .await
}

pub async fn pull(reference: &str) -> Result<Pulled> {
    pull_with(&registry_for(reference), &store(), reference).await
}

async fn materialize_mounts_with<R: ArtifactRegistry>(
    client: &R,
    store: &dyn RegistryAuthStore,
    mounts: &[lns_ipc::ArtifactMount],
) -> Result<Vec<crate::runtime_layer::RuntimeFileSpec>> {
    use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};
    let mut specs = Vec::with_capacity(mounts.len());
    let mut seen = std::collections::HashSet::new();
    for mount in mounts {
        if !seen.insert(mount.path.as_str()) {
            anyhow::bail!("duplicate mount path {}", mount.path);
        }
        // mount.read_only is advisory today: the seed lands in the read-only composefs base, but per-file RO mode and overlay-CoW-shadow enforcement are future work.
        let (reference, auth) = resolve(&mount.reference, store)?;
        let head = client.pull_head(&reference, &auth).await?;
        let family = Family::from_config_media_type(&head.config_media_type).ok_or_else(|| {
            anyhow::anyhow!(
                "{} is not a typed artifact (media type {})",
                mount.reference,
                head.config_media_type
            )
        })?;
        match family {
            Family::Model => specs.push(RuntimeFileSpec {
                guest_path: mount.path.clone(),
                mode: 0o644,
                source: RuntimeSource::Bytes(model_spec_bytes(&head.config_blob)),
            }),
            Family::Tool | Family::Knowledge | Family::Fileset => {
                let layers = client.pull_artifact_layers(&reference, &auth).await?;
                if layers.is_empty() {
                    crate::log::warn!("mount {} has no layer content", mount.reference);
                }
                for layer in &layers {
                    specs.extend(expand_layer_to_specs(layer, &mount.path)?);
                }
            }
            other => anyhow::bail!(
                "{} is a {} artifact and cannot be mounted",
                mount.reference,
                other.slug()
            ),
        }
    }
    Ok(specs)
}

/// Unwraps the envelope `spec` so a model mount delivers `{provider, model, parameters?}` directly (mirrors the policy unwrap), falling back to the full blob when there is no `spec`.
fn model_spec_bytes(config_blob: &[u8]) -> Vec<u8> {
    serde_json::from_slice::<serde_json::Value>(config_blob)
        .ok()
        .and_then(|v| v.get("spec").map(serde_json::to_vec).and_then(Result::ok))
        .unwrap_or_else(|| config_blob.to_vec())
}

/// Expands one OCI layer tarball (optionally gzip-compressed) into runtime file specs rooted at `mount_root`; rejects path traversal before the runtime layer's second gate.
fn expand_layer_to_specs(
    layer_bytes: &[u8],
    mount_root: &str,
) -> Result<Vec<crate::runtime_layer::RuntimeFileSpec>> {
    use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};
    use std::io::Read;

    let reader: Box<dyn Read> = if layer_bytes.starts_with(&[0x1f, 0x8b]) {
        Box::new(flate2::read::GzDecoder::new(layer_bytes))
    } else {
        Box::new(layer_bytes)
    };
    let mut archive = tar::Archive::new(reader);
    let root = mount_root.trim_end_matches('/');
    let mut specs = Vec::new();
    for entry in archive.entries().context("reading layer tar")? {
        let mut entry = entry.context("reading layer tar entry")?;
        let etype = entry.header().entry_type();
        let mode = entry.header().mode().unwrap_or(0o644) & 0o7777;
        let rel = safe_rel_path(&entry.path().context("layer entry path")?)?;
        if rel.is_empty() {
            continue;
        }
        let guest_path = format!("{root}/{rel}");
        if etype.is_dir() {
            continue;
        } else if etype.is_symlink() {
            // The target is left verbatim: it resolves only inside the guest FS at runtime (never on the host), and the entry path itself is already safe_rel_path-guarded.
            let target = entry
                .link_name()
                .context("layer symlink target")?
                .ok_or_else(|| anyhow::anyhow!("symlink {guest_path} has no target"))?
                .to_string_lossy()
                .into_owned();
            specs.push(RuntimeFileSpec {
                guest_path,
                mode,
                source: RuntimeSource::Symlink(target),
            });
        } else if etype.is_file() {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .context("reading layer file")?;
            specs.push(RuntimeFileSpec {
                guest_path,
                mode,
                source: RuntimeSource::Bytes(bytes),
            });
        }
    }
    Ok(specs)
}

/// Joins a tar entry path's normal components, rejecting any `..`, absolute, or non-UTF8 component (a tar-slip guard).
fn safe_rel_path(path: &std::path::Path) -> Result<String> {
    use std::path::Component;
    let mut out = String::new();
    for comp in path.components() {
        match comp {
            Component::Normal(s) => {
                let s = s
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("non-utf8 path in layer"))?;
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str(s);
            }
            Component::CurDir => {}
            _ => anyhow::bail!("unsafe path component in layer entry: {}", path.display()),
        }
    }
    Ok(out)
}

/// Materializes application-layer artifact mounts into runtime files injected at boot; model writes its config blob at the mount path (layer-content families land in a later stage).
pub async fn materialize_mounts(
    mounts: &[lns_ipc::ArtifactMount],
) -> Result<Vec<crate::runtime_layer::RuntimeFileSpec>> {
    if mounts.is_empty() {
        return Ok(Vec::new());
    }
    materialize_mounts_with(&registry_for(&mounts[0].reference), &store(), mounts).await
}

/// Runs a command inside the live guest and returns its stdout; the production impl wraps the vsock session capture, fakes drive the unit tests.
pub(crate) trait GuestCapture: Send + Sync {
    fn capture(
        &self,
        argv: Vec<String>,
    ) -> impl std::future::Future<Output = Result<String>> + Send;
}

/// Picks the single application-layer mount to commit; `selector` matches by reference or guest path and is required to disambiguate when a run has more than one.
pub(crate) fn select_commit_mount<'a>(
    mounts: &'a [lns_ipc::ArtifactMount],
    selector: Option<&str>,
) -> Result<&'a lns_ipc::ArtifactMount> {
    let app: Vec<&lns_ipc::ArtifactMount> = mounts
        .iter()
        .filter(|m| {
            Family::infer_from_reference(&m.reference)
                .map(Family::is_application_layer)
                .unwrap_or(false)
        })
        .collect();
    if let Some(sel) = selector {
        return app
            .into_iter()
            .find(|m| m.reference == sel || m.path == sel)
            .ok_or_else(|| anyhow::anyhow!("no application-layer mount matching {sel:?}"));
    }
    match app.as_slice() {
        [] => anyhow::bail!("this run has no application-layer mounts to commit"),
        [one] => Ok(one),
        many => {
            let refs: Vec<&str> = many.iter().map(|m| m.reference.as_str()).collect();
            anyhow::bail!(
                "this run has several application-layer mounts; pass --mount to choose one of: {}",
                refs.join(", ")
            )
        }
    }
}

/// The relative file entries a layer set ships — used to re-capture exactly the artifact's own files (not the agent's runtime writes under the same path).
fn list_layer_entries(layers: &[Vec<u8>]) -> Result<Vec<String>> {
    use std::io::Read;
    let mut entries = Vec::new();
    for layer in layers {
        let reader: Box<dyn Read> = if layer.starts_with(&[0x1f, 0x8b]) {
            Box::new(flate2::read::GzDecoder::new(&layer[..]))
        } else {
            Box::new(&layer[..])
        };
        let mut archive = tar::Archive::new(reader);
        for entry in archive.entries().context("reading source layer tar")? {
            let entry = entry.context("reading source layer entry")?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let rel = safe_rel_path(&entry.path().context("source layer entry path")?)?;
            if !rel.is_empty() {
                entries.push(rel);
            }
        }
    }
    Ok(entries)
}

/// Single-quotes a value for safe interpolation into the `sh -c` capture command.
fn shquote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

async fn capture_layer<C: GuestCapture>(
    capture: &C,
    path: &str,
    entries: &[String],
) -> Result<Vec<u8>> {
    use base64::Engine;
    let joined = entries
        .iter()
        .map(|e| shquote(e))
        .collect::<Vec<_>>()
        .join(" ");
    // --mode=a+rX forces captured files world-readable (and dirs traversable): the agent may have written the file 0600, but on re-mount it lands root-owned in the composefs base, so a non-root sandbox user must still be able to read it.
    let cmd = format!(
        "tar -C {} --mode=a+rX -czf - {joined} | base64 -w0",
        shquote(path)
    );
    let b64 = capture.capture(vec!["sh".into(), "-c".into(), cmd]).await?;
    base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .context("decoding the captured layer")
}

/// Folds a committed `spec` (the raw `/etc/agent/model` bytes) back into the source artifact's envelope, inverting the boot-time spec unwrap.
fn fold_spec_into_envelope(config_blob: &[u8], spec_bytes: &[u8]) -> Result<Vec<u8>> {
    let mut envelope: serde_json::Value =
        serde_json::from_slice(config_blob).context("parsing the source model envelope")?;
    let spec: serde_json::Value =
        serde_json::from_slice(spec_bytes).context("parsing the committed model spec")?;
    let obj = envelope
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("model envelope is not a JSON object"))?;
    obj.insert("spec".to_string(), spec);
    serde_json::to_vec(&envelope).context("serializing the updated model envelope")
}

/// Captures a sandbox mount's current content and pushes it back as an updated artifact: layer-based families (fileset/tool/knowledge) re-tar their original files into a fresh layer; model re-reads its mounted spec into the envelope.
pub(crate) async fn commit_mount_with<R: ArtifactRegistry, C: GuestCapture>(
    client: &R,
    store: &dyn RegistryAuthStore,
    capture: &C,
    mount: &lns_ipc::ArtifactMount,
    to: Option<&str>,
) -> Result<(String, String)> {
    use base64::Engine;
    let family = Family::infer_from_reference(&mount.reference)
        .ok_or_else(|| anyhow::anyhow!("cannot infer a family from {}", mount.reference))?;
    let target = to.unwrap_or(&mount.reference);
    let (source, source_auth) = resolve(&mount.reference, store)?;
    let head = client.pull_head(&source, &source_auth).await?;
    let artifact_type = head
        .artifact_type
        .clone()
        .unwrap_or_else(|| head.config_media_type.clone());
    let (target_ref, target_auth) = resolve(target, store)?;

    let digest = if matches!(family, Family::Model) {
        let b64 = capture
            .capture(vec![
                "sh".into(),
                "-c".into(),
                format!("base64 -w0 {}", shquote(&mount.path)),
            ])
            .await?;
        let spec = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .context("decoding the captured model config")?;
        let blob = fold_spec_into_envelope(&head.config_blob, &spec)?;
        client
            .push_artifact(
                &target_ref,
                &artifact_type,
                &Family::Model.config_media_type(),
                &blob,
                &[],
                &target_auth,
            )
            .await?
    } else {
        let layers = client.pull_artifact_layers(&source, &source_auth).await?;
        let entries = list_layer_entries(&layers)?;
        if entries.is_empty() {
            anyhow::bail!("{} ships no files to commit", mount.reference);
        }
        let layer = capture_layer(capture, &mount.path, &entries).await?;
        client
            .push_artifact(
                &target_ref,
                &artifact_type,
                &family.config_media_type(),
                &head.config_blob,
                &[layer],
                &target_auth,
            )
            .await?
    };
    Ok((target.to_string(), digest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::registry_auth::RegistryCredential;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRegistry {
        digest: String,
        head: Option<ManifestHead>,
        image_digest: String,
        fail: bool,
        image_fail: bool,
        layers: Vec<Vec<u8>>,
        layers_fail: bool,
        seen_auth: Mutex<Option<RegistryAuth>>,
        pushed_layers: Mutex<Vec<Vec<u8>>>,
        pushed_config: Mutex<Option<Vec<u8>>>,
        image_pulled: Mutex<Option<String>>,
        image_pushed: Mutex<Option<(String, String)>>,
    }

    impl ArtifactRegistry for FakeRegistry {
        async fn push_artifact(
            &self,
            _reference: &Reference,
            _artifact_type: &str,
            _config_media_type: &str,
            config_blob: &[u8],
            layers: &[Vec<u8>],
            auth: &RegistryAuth,
        ) -> Result<String> {
            *self.seen_auth.lock().unwrap() = Some(auth.clone());
            *self.pushed_layers.lock().unwrap() = layers.to_vec();
            *self.pushed_config.lock().unwrap() = Some(config_blob.to_vec());
            if self.fail {
                anyhow::bail!("registry refused the push");
            }
            Ok(self.digest.clone())
        }

        async fn pull_head(
            &self,
            _reference: &Reference,
            auth: &RegistryAuth,
        ) -> Result<ManifestHead> {
            *self.seen_auth.lock().unwrap() = Some(auth.clone());
            if self.fail {
                anyhow::bail!("registry refused the pull");
            }
            let head = self.head.as_ref().expect("canned head");
            Ok(ManifestHead {
                config_media_type: head.config_media_type.clone(),
                artifact_type: head.artifact_type.clone(),
                config_blob: head.config_blob.clone(),
                digest: head.digest.clone(),
            })
        }

        async fn pull_image_to_cache(&self, reference: &str) -> Result<String> {
            *self.image_pulled.lock().unwrap() = Some(reference.to_string());
            if self.image_fail {
                anyhow::bail!("image pull failed");
            }
            Ok(self.image_digest.clone())
        }

        async fn pull_artifact_layers(
            &self,
            _reference: &Reference,
            _auth: &RegistryAuth,
        ) -> Result<Vec<Vec<u8>>> {
            if self.layers_fail {
                anyhow::bail!("layer fetch failed");
            }
            Ok(self.layers.clone())
        }

        async fn push_image_from_cache(
            &self,
            source_reference: &str,
            target: &Reference,
            auth: &RegistryAuth,
        ) -> Result<String> {
            *self.seen_auth.lock().unwrap() = Some(auth.clone());
            *self.image_pushed.lock().unwrap() =
                Some((source_reference.into(), target.to_string()));
            if self.fail {
                anyhow::bail!("registry refused the image push");
            }
            Ok(self.image_digest.clone())
        }
    }

    fn head(config_media_type: &str, artifact_type: Option<&str>) -> ManifestHead {
        ManifestHead {
            config_media_type: config_media_type.into(),
            artifact_type: artifact_type.map(str::to_string),
            config_blob: br#"{"network":{}}"#.to_vec(),
            digest: "sha256:abc".into(),
        }
    }

    /// A path that never exists, so the real store's `load()` returns an empty map (→ anonymous).
    fn empty_store() -> JsonFileRegistryAuthStore {
        JsonFileRegistryAuthStore::new(std::env::temp_dir().join("lns-artifact-cov-absent.json"))
    }

    /// Pointing the real store at a directory makes `load()` surface a non-NotFound IO error.
    fn failing_store() -> JsonFileRegistryAuthStore {
        JsonFileRegistryAuthStore::new(std::env::temp_dir())
    }

    const REF: &str = "registry.example.test/org/acme/policies/pii:v1";
    const POLICY_CMT: &str = "application/vnd.lens.policy.config.v1+json";
    const POLICY_AT: &str = "application/vnd.lens.policy.v1+json";
    const OCI_CMT: &str = "application/vnd.oci.image.config.v1+json";

    #[test]
    fn auth_for_builds_basic_from_a_stored_credential_else_anonymous() {
        let mut f = RegistryAuthFile::new();
        f.insert(
            "reg.example".into(),
            RegistryCredential {
                username: "ci-bot".into(),
                secret: "lns_tok".into(),
            },
        );
        assert_eq!(
            auth_for("reg.example", &f),
            RegistryAuth::Basic("ci-bot".into(), "lns_tok".into())
        );
        assert_eq!(auth_for("absent", &f), RegistryAuth::Anonymous);
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn resolve_auth_returns_basic_for_a_stored_credential() {
        let dir = tempfile::tempdir().unwrap();
        let _g = crate::test_env::EnvVarGuard::set(
            "LNS_REGISTRY_AUTH_PATH",
            dir.path().join("auth.json"),
        );
        let store = JsonFileRegistryAuthStore::new(dir.path().join("auth.json"));
        let mut file = RegistryAuthFile::new();
        file.insert(
            "registry.example.test".into(),
            RegistryCredential {
                username: "any".into(),
                secret: "lns_tok".into(),
            },
        );
        store.save(&file).unwrap();
        assert_eq!(
            resolve_auth("registry.example.test/org/x/agents/a:v1"),
            RegistryAuth::Basic("any".into(), "lns_tok".into())
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn resolve_auth_is_anonymous_without_a_credential_or_for_a_bad_reference() {
        let dir = tempfile::tempdir().unwrap();
        let _g = crate::test_env::EnvVarGuard::set(
            "LNS_REGISTRY_AUTH_PATH",
            dir.path().join("absent.json"),
        );
        assert_eq!(
            resolve_auth("registry.example.test/org/x/agents/a:v1"),
            RegistryAuth::Anonymous
        );
        assert_eq!(resolve_auth("::bad::"), RegistryAuth::Anonymous);
    }

    #[test]
    fn auth_for_keeps_an_explicit_username() {
        let mut f = RegistryAuthFile::new();
        f.insert(
            "reg.example".into(),
            RegistryCredential {
                username: "ci-bot".into(),
                secret: "lns_tok".into(),
            },
        );
        assert!(matches!(auth_for("reg.example", &f), RegistryAuth::Basic(u, _) if u == "ci-bot"));
    }

    #[tokio::test]
    async fn push_sends_basic_auth_from_the_store_and_returns_the_digest() {
        let client = FakeRegistry {
            digest: "sha256:abc".into(),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileRegistryAuthStore::new(dir.path().join("auth.json"));
        let mut file = RegistryAuthFile::new();
        file.insert(
            "registry.example.test".into(),
            RegistryCredential {
                username: "any".into(),
                secret: "lns_secret".into(),
            },
        );
        store.save(&file).unwrap();

        let digest = push_artifact_with(&client, &store, REF, POLICY_AT, POLICY_CMT, b"{}", &[])
            .await
            .unwrap();
        assert_eq!(digest, "sha256:abc");
        assert_eq!(
            *client.seen_auth.lock().unwrap(),
            Some(RegistryAuth::Basic("any".into(), "lns_secret".into()))
        );
    }

    #[tokio::test]
    async fn push_falls_back_to_anonymous_without_a_stored_credential() {
        let client = FakeRegistry::default();
        push_artifact_with(
            &client,
            &empty_store(),
            REF,
            POLICY_AT,
            POLICY_CMT,
            b"{}",
            &[],
        )
        .await
        .unwrap();
        assert_eq!(
            *client.seen_auth.lock().unwrap(),
            Some(RegistryAuth::Anonymous)
        );
    }

    #[tokio::test]
    async fn push_forwards_content_layers_to_the_registry() {
        let client = FakeRegistry::default();
        let layer = b"tar-gz-bytes".to_vec();
        push_artifact_with(
            &client,
            &empty_store(),
            REF,
            POLICY_AT,
            POLICY_CMT,
            b"{}",
            std::slice::from_ref(&layer),
        )
        .await
        .unwrap();
        assert_eq!(*client.pushed_layers.lock().unwrap(), vec![layer]);
    }

    #[tokio::test]
    async fn push_rejects_an_invalid_reference_before_touching_the_registry() {
        let client = FakeRegistry::default();
        let err = push_artifact_with(
            &client,
            &empty_store(),
            "::bad::",
            POLICY_AT,
            POLICY_CMT,
            b"{}",
            &[],
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid registry reference"),
            "got: {err:#}"
        );
        assert!(client.seen_auth.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn push_surfaces_a_credential_store_load_error() {
        let client = FakeRegistry::default();
        let err = push_artifact_with(
            &client,
            &failing_store(),
            REF,
            POLICY_AT,
            POLICY_CMT,
            b"{}",
            &[],
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("loading registry credentials"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_propagates_a_registry_failure() {
        let client = FakeRegistry {
            fail: true,
            ..Default::default()
        };
        let err = push_artifact_with(
            &client,
            &empty_store(),
            REF,
            POLICY_AT,
            POLICY_CMT,
            b"{}",
            &[],
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("refused the push"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_image_resolves_target_auth_and_returns_the_digest() {
        let client = FakeRegistry {
            image_digest: "sha256:img".into(),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileRegistryAuthStore::new(dir.path().join("auth.json"));
        let mut file = RegistryAuthFile::new();
        file.insert(
            "registry.example.test".into(),
            RegistryCredential {
                username: "any".into(),
                secret: "lns_tok".into(),
            },
        );
        store.save(&file).unwrap();

        let digest = push_image_with(&client, &store, "docker.io/library/alpine:3.20", REF)
            .await
            .unwrap();
        assert_eq!(digest, "sha256:img");
        assert_eq!(
            client.image_pushed.lock().unwrap().as_ref().unwrap().0,
            "docker.io/library/alpine:3.20"
        );
        assert_eq!(
            *client.seen_auth.lock().unwrap(),
            Some(RegistryAuth::Basic("any".into(), "lns_tok".into()))
        );
    }

    #[tokio::test]
    async fn push_image_rejects_an_invalid_target_reference() {
        let client = FakeRegistry::default();
        let err = push_image_with(&client, &empty_store(), "alpine:3.20", "::bad::")
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid registry reference"),
            "got: {err:#}"
        );
        assert!(client.image_pushed.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn push_image_propagates_a_registry_failure() {
        let client = FakeRegistry {
            fail: true,
            ..Default::default()
        };
        let err = push_image_with(&client, &empty_store(), "alpine:3.20", REF)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("refused the image push"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_returns_an_artifact_when_the_config_media_type_is_a_lens_family() {
        let client = FakeRegistry {
            head: Some(head(POLICY_CMT, Some(POLICY_AT))),
            ..Default::default()
        };
        let pulled = pull_with(&client, &empty_store(), REF).await.unwrap();
        assert_eq!(
            pulled,
            Pulled::Artifact {
                artifact_type: POLICY_AT.into(),
                config_blob: br#"{"network":{}}"#.to_vec(),
                digest: "sha256:abc".into(),
            }
        );
        assert!(client.image_pulled.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn pull_falls_back_to_the_config_media_type_when_artifact_type_is_absent() {
        let client = FakeRegistry {
            head: Some(head(POLICY_CMT, None)),
            ..Default::default()
        };
        let pulled = pull_with(&client, &empty_store(), REF).await.unwrap();
        assert!(
            matches!(pulled, Pulled::Artifact { artifact_type, .. } if artifact_type == POLICY_CMT)
        );
    }

    #[tokio::test]
    async fn pull_pulls_an_image_into_the_cache_when_the_config_is_an_oci_image() {
        let client = FakeRegistry {
            head: Some(head(OCI_CMT, None)),
            image_digest: "sha256:img".into(),
            ..Default::default()
        };
        let pulled = pull_with(&client, &empty_store(), REF).await.unwrap();
        assert!(matches!(pulled, Pulled::Image { digest } if digest == "sha256:img"));
        assert_eq!(client.image_pulled.lock().unwrap().as_deref(), Some(REF));
    }

    #[tokio::test]
    async fn pull_propagates_a_head_fetch_error() {
        let client = FakeRegistry {
            fail: true,
            head: Some(head(OCI_CMT, None)),
            ..Default::default()
        };
        let err = pull_with(&client, &empty_store(), REF).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("refused the pull"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_propagates_an_image_pull_failure() {
        let client = FakeRegistry {
            head: Some(head(OCI_CMT, None)),
            image_fail: true,
            ..Default::default()
        };
        let err = pull_with(&client, &empty_store(), REF).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("image pull failed"),
            "got: {err:#}"
        );
    }

    const MODEL_CMT: &str = "application/vnd.lens.model.config.v1+json";
    const TOOL_CMT: &str = "application/vnd.lens.tool.config.v1+json";
    const MODEL_REF: &str = "registry.example.test/org/acme/models/m:v1";

    fn model_mount() -> lns_ipc::ArtifactMount {
        lns_ipc::ArtifactMount {
            reference: MODEL_REF.into(),
            path: "/etc/agent/model".into(),
            read_only: true,
        }
    }

    #[tokio::test]
    async fn materialize_mounts_writes_a_model_spec_at_its_path() {
        let mut h = head(MODEL_CMT, Some("application/vnd.lens.model.v1+json"));
        h.config_blob = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Model","metadata":{"name":"m"},"spec":{"provider":"some-provider","model":"some-model"}}"#.to_vec();
        let fake = FakeRegistry {
            head: Some(h),
            ..Default::default()
        };
        let specs = materialize_mounts_with(&fake, &empty_store(), &[model_mount()])
            .await
            .unwrap();
        use crate::runtime_layer::RuntimeSource;
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].guest_path, "/etc/agent/model");
        let is_bytes = matches!(&specs[0].source, RuntimeSource::Bytes(_));
        assert!(is_bytes, "expected model spec bytes");
        if let RuntimeSource::Bytes(b) = &specs[0].source {
            let written: serde_json::Value = serde_json::from_slice(b).unwrap();
            assert_eq!(
                written,
                serde_json::json!({"provider": "some-provider", "model": "some-model"})
            );
        }
    }

    #[test]
    fn model_spec_bytes_unwraps_the_envelope_spec() {
        let envelope = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Model","metadata":{"name":"m"},"spec":{"provider":"some-provider","model":"some-model"}}"#;
        let out: serde_json::Value = serde_json::from_slice(&model_spec_bytes(envelope)).unwrap();
        assert_eq!(
            out,
            serde_json::json!({"provider": "some-provider", "model": "some-model"})
        );
    }

    #[test]
    fn model_spec_bytes_falls_back_when_there_is_no_spec() {
        let flat = br#"{"provider":"some-provider"}"#;
        assert_eq!(model_spec_bytes(flat), flat.to_vec());
        let garbage = b"not json";
        assert_eq!(model_spec_bytes(garbage), garbage.to_vec());
    }

    #[tokio::test]
    async fn materialize_mounts_rejects_a_duplicate_mount_path() {
        let fake = FakeRegistry {
            head: Some(head(MODEL_CMT, None)),
            ..Default::default()
        };
        let err = materialize_mounts_with(&fake, &empty_store(), &[model_mount(), model_mount()])
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate mount path"),
            "got: {err:#}"
        );
    }

    fn tar_layer(entries: &[(&str, u32, &[u8])]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut b = tar::Builder::new(std::io::Cursor::new(&mut bytes));
            for (name, mode, content) in entries {
                let mut h = tar::Header::new_gnu();
                h.set_path(name).unwrap();
                h.set_mode(*mode);
                h.set_size(content.len() as u64);
                h.set_entry_type(tar::EntryType::Regular);
                h.set_cksum();
                b.append(&h, &content[..]).unwrap();
            }
            b.finish().unwrap();
        }
        bytes
    }

    fn layered_mount(path: &str) -> lns_ipc::ArtifactMount {
        lns_ipc::ArtifactMount {
            reference: "registry.example.test/org/acme/tools/t:v1".into(),
            path: path.into(),
            read_only: true,
        }
    }

    #[tokio::test]
    async fn materialize_mounts_expands_a_tool_layer_under_its_path() {
        let fake = FakeRegistry {
            head: Some(head(TOOL_CMT, None)),
            layers: vec![tar_layer(&[("bin/run", 0o755, b"#!/bin/sh\n")])],
            ..Default::default()
        };
        let specs = materialize_mounts_with(
            &fake,
            &empty_store(),
            &[layered_mount("/etc/agent/tools/t")],
        )
        .await
        .unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].guest_path, "/etc/agent/tools/t/bin/run");
        assert_eq!(specs[0].mode, 0o755);
    }

    #[tokio::test]
    async fn materialize_mounts_warns_but_succeeds_on_an_empty_layer_set() {
        let fake = FakeRegistry {
            head: Some(head(TOOL_CMT, None)),
            layers: vec![],
            ..Default::default()
        };
        let specs = materialize_mounts_with(
            &fake,
            &empty_store(),
            &[layered_mount("/etc/agent/tools/t")],
        )
        .await
        .unwrap();
        assert!(specs.is_empty());
    }

    #[tokio::test]
    async fn materialize_mounts_surfaces_a_layer_fetch_error() {
        let fake = FakeRegistry {
            head: Some(head(TOOL_CMT, None)),
            layers_fail: true,
            ..Default::default()
        };
        let err = materialize_mounts_with(
            &fake,
            &empty_store(),
            &[layered_mount("/etc/agent/tools/t")],
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("layer fetch failed"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn materialize_mounts_rejects_a_runtime_layer_family() {
        let fake = FakeRegistry {
            head: Some(head(POLICY_CMT, None)),
            ..Default::default()
        };
        let err = materialize_mounts_with(&fake, &empty_store(), &[model_mount()])
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("cannot be mounted"),
            "got: {err:#}"
        );
    }

    #[test]
    fn expand_layer_to_specs_handles_files_and_gzip() {
        let plain = tar_layer(&[("config.yaml", 0o644, b"provider: anthropic\n")]);
        let specs = expand_layer_to_specs(&plain, "/opt/data").unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].guest_path, "/opt/data/config.yaml");
        assert!(matches!(
            &specs[0].source,
            crate::runtime_layer::RuntimeSource::Bytes(_)
        ));

        let mut gz = Vec::new();
        {
            use std::io::Write;
            let mut enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::fast());
            enc.write_all(&plain).unwrap();
            enc.finish().unwrap();
        }
        let from_gz = expand_layer_to_specs(&gz, "/opt/data").unwrap();
        assert_eq!(from_gz[0].guest_path, "/opt/data/config.yaml");
    }

    #[test]
    fn expand_layer_to_specs_expands_a_symlink_and_skips_dirs() {
        let mut bytes = Vec::new();
        {
            let mut b = tar::Builder::new(std::io::Cursor::new(&mut bytes));
            let mut root = tar::Header::new_gnu();
            root.set_path("./").unwrap();
            root.set_entry_type(tar::EntryType::Directory);
            root.set_size(0);
            root.set_mode(0o755);
            root.set_cksum();
            b.append(&root, std::io::empty()).unwrap();
            let mut d = tar::Header::new_gnu();
            d.set_path("sub/").unwrap();
            d.set_entry_type(tar::EntryType::Directory);
            d.set_size(0);
            d.set_mode(0o755);
            d.set_cksum();
            b.append(&d, std::io::empty()).unwrap();
            let mut s = tar::Header::new_gnu();
            s.set_entry_type(tar::EntryType::Symlink);
            s.set_size(0);
            s.set_mode(0o777);
            s.set_link_name("real").unwrap();
            s.set_path("link").unwrap();
            s.set_cksum();
            b.append(&s, std::io::empty()).unwrap();
            b.finish().unwrap();
        }
        let specs = expand_layer_to_specs(&bytes, "/opt/data").unwrap();
        assert_eq!(specs.len(), 1, "dir skipped, symlink kept");
        assert!(matches!(
            &specs[0].source,
            crate::runtime_layer::RuntimeSource::Symlink(t) if t == "real"
        ));
    }

    #[test]
    fn safe_rel_path_joins_normal_skips_curdir_and_rejects_escapes() {
        use std::path::Path;
        assert_eq!(safe_rel_path(Path::new("./a/b")).unwrap(), "a/b");
        assert!(
            format!("{:#}", safe_rel_path(Path::new("../escape")).unwrap_err())
                .contains("unsafe path")
        );
        assert!(
            format!("{:#}", safe_rel_path(Path::new("/etc/passwd")).unwrap_err())
                .contains("unsafe path")
        );
    }

    #[cfg(unix)]
    #[test]
    fn safe_rel_path_rejects_non_utf8_components() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let p = std::path::Path::new(OsStr::from_bytes(b"a/\xff"));
        assert!(format!("{:#}", safe_rel_path(p).unwrap_err()).contains("non-utf8"));
    }

    #[tokio::test]
    async fn materialize_mounts_rejects_a_non_typed_artifact() {
        let fake = FakeRegistry {
            head: Some(head(OCI_CMT, None)),
            ..Default::default()
        };
        let err = materialize_mounts_with(&fake, &empty_store(), &[model_mount()])
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("not a typed artifact"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn materialize_mounts_surfaces_a_pull_error() {
        let fake = FakeRegistry {
            fail: true,
            ..Default::default()
        };
        let err = materialize_mounts_with(&fake, &empty_store(), &[model_mount()])
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("refused the pull"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn materialize_mounts_is_empty_for_no_mounts() {
        assert!(materialize_mounts(&[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn public_materialize_mounts_wires_real_registry_and_fails_fast_on_a_bad_reference() {
        let dir = tempfile::tempdir().unwrap();
        let _g = crate::test_env::EnvVarGuard::set(
            "LNS_REGISTRY_AUTH_PATH",
            dir.path().join("auth.json"),
        );
        let err = materialize_mounts(&[lns_ipc::ArtifactMount {
            reference: "::bad::".into(),
            path: "/etc/agent/model".into(),
            read_only: true,
        }])
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid registry reference"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn public_push_wires_real_registry_and_fails_fast_on_a_bad_reference() {
        let dir = tempfile::tempdir().unwrap();
        let _g = crate::test_env::EnvVarGuard::set(
            "LNS_REGISTRY_AUTH_PATH",
            dir.path().join("auth.json"),
        );
        let err = push_artifact("::bad::", POLICY_AT, POLICY_CMT, b"{}", &[])
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid registry reference"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn public_push_image_wires_real_registry_and_fails_fast_on_a_bad_reference() {
        let dir = tempfile::tempdir().unwrap();
        let _g = crate::test_env::EnvVarGuard::set(
            "LNS_REGISTRY_AUTH_PATH",
            dir.path().join("auth.json"),
        );
        let err = push_image("alpine:3.20", "::bad::").await.unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid registry reference"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn public_pull_wires_real_registry_and_fails_fast_on_a_bad_reference() {
        let dir = tempfile::tempdir().unwrap();
        let _g = crate::test_env::EnvVarGuard::set(
            "LNS_REGISTRY_AUTH_PATH",
            dir.path().join("auth.json"),
        );
        let err = pull("::bad::").await.unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid registry reference"),
            "got: {err:#}"
        );
    }

    use base64::Engine as _;

    #[derive(Default)]
    struct FakeCapture {
        out: String,
        argv: Mutex<Option<Vec<String>>>,
    }

    impl GuestCapture for FakeCapture {
        async fn capture(&self, argv: Vec<String>) -> Result<String> {
            *self.argv.lock().unwrap() = Some(argv);
            Ok(self.out.clone())
        }
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    const FILESET_CMT: &str = "application/vnd.lens.fileset.config.v1+json";
    const FILESET_REF: &str = "registry.example.test/org/acme/filesets/cfg:v1";

    fn fileset_mount() -> lns_ipc::ArtifactMount {
        lns_ipc::ArtifactMount {
            reference: FILESET_REF.into(),
            path: "/opt/data".into(),
            read_only: false,
        }
    }

    fn gz_tar_one(name: &str, body: &[u8]) -> Vec<u8> {
        use flate2::{Compression, write::GzEncoder};
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut b = tar::Builder::new(&mut gz);
            let mut h = tar::Header::new_gnu();
            h.set_path(name).unwrap();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append(&h, body).unwrap();
            b.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    #[test]
    fn select_commit_mount_picks_the_lone_application_layer_mount() {
        let mounts = vec![fileset_mount()];
        assert_eq!(
            select_commit_mount(&mounts, None).unwrap().reference,
            FILESET_REF
        );
    }

    #[test]
    fn select_commit_mount_matches_a_selector_by_ref_or_path() {
        let mounts = vec![fileset_mount(), model_mount()];
        assert_eq!(
            select_commit_mount(&mounts, Some(MODEL_REF))
                .unwrap()
                .reference,
            MODEL_REF
        );
        assert_eq!(
            select_commit_mount(&mounts, Some("/opt/data"))
                .unwrap()
                .reference,
            FILESET_REF
        );
        let err = select_commit_mount(&mounts, Some("nope")).unwrap_err();
        assert!(format!("{err:#}").contains("no application-layer mount"));
    }

    #[test]
    fn select_commit_mount_errors_on_none_or_ambiguous() {
        let none = select_commit_mount(&[], None).unwrap_err();
        assert!(format!("{none:#}").contains("no application-layer mounts"));
        let many = vec![fileset_mount(), model_mount()];
        let err = select_commit_mount(&many, None).unwrap_err();
        assert!(format!("{err:#}").contains("pass --mount"), "got: {err:#}");
    }

    #[test]
    fn list_layer_entries_collects_files_and_skips_dirs() {
        let layer = gz_tar_one("config.yaml", b"x: 1\n");
        assert_eq!(list_layer_entries(&[layer]).unwrap(), vec!["config.yaml"]);
    }

    #[test]
    fn fold_spec_into_envelope_replaces_the_spec() {
        let envelope = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Model","metadata":{"name":"m"},"spec":{"provider":"old","model":"m1"}}"#;
        let out = fold_spec_into_envelope(envelope, br#"{"provider":"new","model":"m2"}"#).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["spec"]["provider"], "new");
        assert_eq!(v["metadata"]["name"], "m");
    }

    #[test]
    fn fold_spec_into_envelope_rejects_bad_json() {
        assert!(fold_spec_into_envelope(b"not json", b"{}").is_err());
        assert!(fold_spec_into_envelope(b"{}", b"not json").is_err());
        assert!(fold_spec_into_envelope(b"[]", b"{}").is_err());
    }

    #[tokio::test]
    async fn commit_mount_with_repacks_a_fileset_layer_and_pushes_to_the_source_ref() {
        let fake = FakeRegistry {
            digest: "sha256:new".into(),
            head: Some(head(
                FILESET_CMT,
                Some("application/vnd.lens.fileset.v1+json"),
            )),
            layers: vec![gz_tar_one("config.yaml", b"old\n")],
            ..Default::default()
        };
        let capture = FakeCapture {
            out: b64(b"new-layer-gz"),
            ..Default::default()
        };
        let (reference, digest) =
            commit_mount_with(&fake, &empty_store(), &capture, &fileset_mount(), None)
                .await
                .unwrap();
        assert_eq!(reference, FILESET_REF);
        assert_eq!(digest, "sha256:new");
        assert_eq!(fake.pushed_layers.lock().unwrap()[0], b"new-layer-gz");
        // the capture tars the original entry under the mount path
        let argv = capture.argv.lock().unwrap().clone().unwrap();
        assert!(argv[2].contains("tar -C '/opt/data'") && argv[2].contains("'config.yaml'"));
        assert!(
            argv[2].contains("--mode=a+rX"),
            "captured files must be forced readable: {}",
            argv[2]
        );
    }

    #[tokio::test]
    async fn commit_mount_with_honors_a_to_override() {
        let fake = FakeRegistry {
            digest: "sha256:new".into(),
            head: Some(head(FILESET_CMT, None)),
            layers: vec![gz_tar_one("config.yaml", b"old\n")],
            ..Default::default()
        };
        let capture = FakeCapture {
            out: b64(b"layer"),
            ..Default::default()
        };
        let (reference, _) = commit_mount_with(
            &fake,
            &empty_store(),
            &capture,
            &fileset_mount(),
            Some("registry.example.test/org/acme/filesets/cfg:v2"),
        )
        .await
        .unwrap();
        assert_eq!(reference, "registry.example.test/org/acme/filesets/cfg:v2");
    }

    #[tokio::test]
    async fn commit_mount_with_bails_when_the_source_layer_has_no_files() {
        let fake = FakeRegistry {
            head: Some(head(FILESET_CMT, None)),
            layers: vec![],
            ..Default::default()
        };
        let capture = FakeCapture::default();
        let err = commit_mount_with(&fake, &empty_store(), &capture, &fileset_mount(), None)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("ships no files"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn commit_mount_with_folds_a_model_spec_and_pushes_no_layer() {
        let fake = FakeRegistry {
            digest: "sha256:m".into(),
            head: Some(ManifestHead {
                config_media_type: MODEL_CMT.into(),
                artifact_type: Some("application/vnd.lens.model.v1+json".into()),
                config_blob: br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Model","metadata":{"name":"m"},"spec":{"provider":"old"}}"#.to_vec(),
                digest: "sha256:src".into(),
            }),
            ..Default::default()
        };
        let capture = FakeCapture {
            out: b64(br#"{"provider":"new","model":"m2"}"#),
            ..Default::default()
        };
        let (reference, digest) =
            commit_mount_with(&fake, &empty_store(), &capture, &model_mount(), None)
                .await
                .unwrap();
        assert_eq!(reference, MODEL_REF);
        assert_eq!(digest, "sha256:m");
        assert!(fake.pushed_layers.lock().unwrap().is_empty());
        let pushed = fake.pushed_config.lock().unwrap().clone().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&pushed).unwrap();
        assert_eq!(v["spec"]["provider"], "new");
        let argv = capture.argv.lock().unwrap().clone().unwrap();
        assert!(argv[2].contains("base64 -w0 '/etc/agent/model'"));
    }

    #[tokio::test]
    async fn commit_mount_with_surfaces_a_bad_base64_capture() {
        let fake = FakeRegistry {
            head: Some(head(FILESET_CMT, None)),
            layers: vec![gz_tar_one("config.yaml", b"old\n")],
            ..Default::default()
        };
        let capture = FakeCapture {
            out: "not~valid~base64".into(),
            ..Default::default()
        };
        let err = commit_mount_with(&fake, &empty_store(), &capture, &fileset_mount(), None)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("captured layer"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn commit_mount_with_rejects_a_reference_without_a_family() {
        let fake = FakeRegistry::default();
        let capture = FakeCapture::default();
        let mount = lns_ipc::ArtifactMount {
            reference: "registry.example.test/nofamily:v1".into(),
            path: "/x".into(),
            read_only: false,
        };
        let err = commit_mount_with(&fake, &empty_store(), &capture, &mount, None)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("infer a family"),
            "got: {err:#}"
        );
    }

    #[test]
    fn shquote_escapes_embedded_single_quotes() {
        assert_eq!(shquote("a'b"), "'a'\\''b'");
    }
}
