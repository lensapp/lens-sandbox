use anyhow::Result;
use oci_spec::image::{Arch, Os};

use crate::image::PulledImage;
use crate::oci_layer_cache::LayerCache;

#[derive(Debug)]
pub struct IngestedImage {
    pub digests: Vec<String>,
    pub bytes: Vec<Vec<u8>>,
    pub config: Option<oci_client::config::ConfigFile>,
    pub artifact_type: Option<String>,
    pub config_media_type: Option<String>,
}

pub async fn run(
    image: Option<&str>,
    cmd: &[String],
    guest_arch: &Arch,
    layer_cache: &LayerCache,
    pull: impl AsyncFnOnce(&str, &LayerCache) -> Result<PulledImage>,
) -> Result<IngestedImage> {
    match image {
        Some(image) => {
            let pulled = pull(image, layer_cache).await?;
            ensure_runnable_here(&pulled.config, guest_arch)?;
            let bytes: Vec<Vec<u8>> = pulled
                .layers
                .into_iter()
                .map(|layer| layer.data.to_vec())
                .collect();
            Ok(IngestedImage {
                digests: pulled.layer_digests,
                bytes,
                config: Some(pulled.config),
                artifact_type: pulled.artifact_type,
                config_media_type: Some(pulled.config_media_type),
            })
        }
        None => {
            if cmd.is_empty() {
                anyhow::bail!(
                    "imageless mode requires a command after `--`, e.g. `lns run -- echo hello`"
                );
            }
            Ok(IngestedImage {
                digests: Vec::new(),
                bytes: Vec::new(),
                config: None,
                artifact_type: None,
                config_media_type: None,
            })
        }
    }
}

fn ensure_runnable_here(config: &oci_client::config::ConfigFile, guest_arch: &Arch) -> Result<()> {
    if config.architecture == *guest_arch && config.os == Os::Linux {
        return Ok(());
    }
    anyhow::bail!(
        "image is {}/{} but this sandbox runs linux/{guest_arch}; \
         use a multi-arch image or one built for linux/{guest_arch}",
        config.os,
        config.architecture,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use oci_client::Reference;
    use std::sync::Mutex;
    use tempfile::tempdir;

    fn empty_cache() -> (tempfile::TempDir, LayerCache) {
        let dir = tempdir().unwrap();
        let cache = LayerCache::new(dir.path().to_path_buf());
        (dir, cache)
    }

    async fn failing_puller(_image: &str, _cache: &LayerCache) -> Result<PulledImage> {
        Err(anyhow::anyhow!("registry timeout"))
    }

    fn sample_pulled() -> PulledImage {
        let reference: Reference = "alpine:3.20".parse().unwrap();
        let config = oci_client::config::ConfigFile {
            architecture: "arm64".into(),
            os: "linux".into(),
            ..Default::default()
        };
        PulledImage {
            reference,
            digest: "sha256:deadbeef".to_string(),
            layers: vec![
                oci_client::client::ImageLayer::new(
                    b"layer-zero".to_vec(),
                    "application/vnd.oci.image.layer.v1.tar+gzip".to_string(),
                    None,
                ),
                oci_client::client::ImageLayer::new(
                    b"layer-one".to_vec(),
                    "application/vnd.oci.image.layer.v1.tar+gzip".to_string(),
                    None,
                ),
            ],
            config,
            layer_digests: vec!["sha256:aaaa".to_string(), "sha256:bbbb".to_string()],
            artifact_type: None,
            config_media_type: "application/vnd.oci.image.config.v1+json".to_string(),
        }
    }

    #[tokio::test]
    async fn imageless_with_empty_cmd_bails_with_actionable_hint() {
        let (_dir, cache) = empty_cache();
        let err = run(None, &[], &Arch::ARM64, &cache, failing_puller)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("imageless mode requires a command"),
            "expected actionable hint, got: {msg}"
        );
        assert!(
            msg.contains("lns run -- echo hello"),
            "expected example invocation in error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn imageless_with_non_empty_cmd_returns_empty_ingest() {
        let (_dir, cache) = empty_cache();
        let cmd = vec!["echo".to_string(), "hello".to_string()];
        let ingested = run(None, &cmd, &Arch::ARM64, &cache, failing_puller)
            .await
            .unwrap();
        assert!(ingested.digests.is_empty(), "imageless has no digests");
        assert!(ingested.bytes.is_empty(), "imageless has no payloads");
        assert!(
            ingested.config.is_none(),
            "imageless config is None — runtime layer carries everything"
        );
        assert!(
            ingested.artifact_type.is_none() && ingested.config_media_type.is_none(),
            "imageless has no pulled manifest, so no artifact type to dispatch on"
        );
    }

    #[tokio::test]
    async fn oci_branch_threads_the_artifact_type_for_dispatch() {
        let (_dir, cache) = empty_cache();
        let mut pulled = sample_pulled();
        pulled.artifact_type = Some("application/vnd.lens.sandbox.v1+json".into());
        pulled.config_media_type = "application/vnd.lens.sandbox.config.v1+json".into();
        let cell: Mutex<Option<PulledImage>> = Mutex::new(Some(pulled));
        let ingested = run(
            Some("reg/sandbox:1"),
            &[],
            &Arch::ARM64,
            &cache,
            async |_img: &str, _c: &LayerCache| Ok(cell.lock().unwrap().take().unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(
            ingested.artifact_type.as_deref(),
            Some("application/vnd.lens.sandbox.v1+json"),
            "the pulled manifest's artifactType must reach the ingest result so run can dispatch",
        );
        assert_eq!(
            ingested.config_media_type.as_deref(),
            Some("application/vnd.lens.sandbox.config.v1+json"),
        );
    }

    #[tokio::test]
    async fn oci_branch_maps_pulled_image_to_ingested_in_manifest_order() {
        let (_dir, cache) = empty_cache();
        let captured: Mutex<Option<String>> = Mutex::new(None);
        let pulled_cell: Mutex<Option<PulledImage>> = Mutex::new(Some(sample_pulled()));

        async fn capturing_puller(
            captured: &Mutex<Option<String>>,
            pulled_cell: &Mutex<Option<PulledImage>>,
            image: &str,
            _cache: &LayerCache,
        ) -> Result<PulledImage> {
            *captured.lock().unwrap() = Some(image.to_string());
            Ok(pulled_cell.lock().unwrap().take().unwrap())
        }

        let ingested = run(
            Some("alpine:3.20"),
            &[],
            &Arch::ARM64,
            &cache,
            async |img: &str, c: &LayerCache| {
                capturing_puller(&captured, &pulled_cell, img, c).await
            },
        )
        .await
        .unwrap();
        assert_eq!(
            captured.lock().unwrap().as_deref(),
            Some("alpine:3.20"),
            "image reference forwarded verbatim"
        );
        assert_eq!(
            ingested.digests,
            vec!["sha256:aaaa".to_string(), "sha256:bbbb".to_string()],
            "digests preserved in manifest order"
        );
        assert_eq!(
            ingested.bytes,
            vec![b"layer-zero".to_vec(), b"layer-one".to_vec()],
            "layer bytes preserved in manifest order"
        );
        assert!(
            ingested.config.is_some(),
            "OCI path carries the parsed image config through"
        );
    }

    #[tokio::test]
    async fn oci_branch_propagates_puller_error_unchanged() {
        let (_dir, cache) = empty_cache();
        let err = run(
            Some("alpine:3.20"),
            &[],
            &Arch::ARM64,
            &cache,
            failing_puller,
        )
        .await
        .unwrap_err();
        assert_eq!(format!("{err}"), "registry timeout");
    }

    #[tokio::test]
    async fn oci_branch_bails_when_image_arch_does_not_match_the_guest() {
        let (_dir, cache) = empty_cache();
        let pulled_cell: Mutex<Option<PulledImage>> = Mutex::new(Some(sample_pulled()));
        // sample_pulled() is linux/arm64; ask for an amd64 sandbox so the architectures disagree.
        let err = run(
            Some("alpine:3.20"),
            &[],
            &Arch::Amd64,
            &cache,
            async |_img: &str, _c: &LayerCache| Ok(pulled_cell.lock().unwrap().take().unwrap()),
        )
        .await
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("image is linux/arm64") && msg.contains("sandbox runs linux/amd64"),
            "mismatch error must name both architectures, got: {msg}"
        );
        assert!(
            msg.contains("multi-arch"),
            "error should point at the multi-arch fix, got: {msg}"
        );
    }

    fn cfg(arch: &str, os: &str) -> oci_client::config::ConfigFile {
        oci_client::config::ConfigFile {
            architecture: arch.into(),
            os: os.into(),
            ..Default::default()
        }
    }

    #[test]
    fn ensure_runnable_here_accepts_a_matching_linux_arch() {
        ensure_runnable_here(&cfg("arm64", "linux"), &Arch::ARM64).expect("exact match runs");
        ensure_runnable_here(&cfg("amd64", "linux"), &Arch::Amd64).expect("exact match runs");
    }

    #[test]
    fn ensure_runnable_here_rejects_a_foreign_arch() {
        let err = format!(
            "{:#}",
            ensure_runnable_here(&cfg("amd64", "linux"), &Arch::ARM64).unwrap_err()
        );
        assert!(
            err.contains("image is linux/amd64") && err.contains("linux/arm64"),
            "mismatch must name both architectures: {err}"
        );
    }

    #[test]
    fn ensure_runnable_here_rejects_a_non_linux_os() {
        let err = format!(
            "{:#}",
            ensure_runnable_here(&cfg("arm64", "windows"), &Arch::ARM64).unwrap_err()
        );
        assert!(
            err.contains("windows/arm64"),
            "a non-linux os must surface in the message: {err}"
        );
    }
}
