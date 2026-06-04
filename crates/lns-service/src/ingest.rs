use anyhow::Result;

use crate::image::PulledImage;
use crate::oci_layer_cache::LayerCache;

#[derive(Debug)]
pub struct IngestedImage {
    pub digests: Vec<String>,
    pub bytes: Vec<Vec<u8>>,
    pub config: Option<oci_client::config::ConfigFile>,
}

pub async fn run(
    image: Option<&str>,
    cmd: &[String],
    layer_cache: &LayerCache,
    pull: impl AsyncFnOnce(&str, &LayerCache) -> Result<PulledImage>,
) -> Result<IngestedImage> {
    match image {
        Some(image) => {
            let pulled = pull(image, layer_cache).await?;
            let bytes: Vec<Vec<u8>> = pulled
                .layers
                .into_iter()
                .map(|layer| layer.data.to_vec())
                .collect();
            Ok(IngestedImage {
                digests: pulled.layer_digests,
                bytes,
                config: Some(pulled.config),
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
            })
        }
    }
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
        }
    }

    #[tokio::test]
    async fn imageless_with_empty_cmd_bails_with_actionable_hint() {
        let (_dir, cache) = empty_cache();
        let err = run(None, &[], &cache, failing_puller).await.unwrap_err();
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
        let ingested = run(None, &cmd, &cache, failing_puller).await.unwrap();
        assert!(ingested.digests.is_empty(), "imageless has no digests");
        assert!(ingested.bytes.is_empty(), "imageless has no payloads");
        assert!(
            ingested.config.is_none(),
            "imageless config is None — runtime layer carries everything"
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
        let err = run(Some("alpine:3.20"), &[], &cache, failing_puller)
            .await
            .unwrap_err();
        assert_eq!(format!("{err}"), "registry timeout");
    }
}
