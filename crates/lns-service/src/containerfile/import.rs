use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::image::manifest_cache::{CachedManifest, ManifestCache};
use crate::image_store::{Fs, ImageRecord, LayerRef, RecordKind, record_with};
use crate::oci_layer_cache::LayerCache;

use super::image::BuiltImage;
use super::tar_layer::LayerBlob;

/// Where a built image lands so the ordinary pull path finds every part of it without a registry.
pub(crate) struct LocalStore<'a> {
    pub layers: &'a LayerCache,
    pub manifests: PathBuf,
    pub images: PathBuf,
}

pub(crate) async fn import<F: Fs>(
    fs: &F,
    store: &LocalStore<'_>,
    reference: &str,
    built: &BuiltImage,
    layers: &[LayerBlob],
    pulled_unix_secs: u64,
) -> Result<String> {
    let pinned = pinned_reference(reference, &built.manifest_digest)?;

    for layer in layers {
        store
            .layers
            .install_from_bytes(&layer.digest, &layer.bytes)
            .with_context(|| format!("installing the built layer {}", layer.digest))?;
    }

    ManifestCache::new(&store.manifests)
        .put(
            &pinned,
            &CachedManifest {
                manifest: built.manifest.clone(),
                manifest_digest: built.manifest_digest.clone(),
                config: built.config.clone(),
            },
        )
        .with_context(|| format!("caching the built manifest for {pinned}"))?;

    let record = ImageRecord {
        reference: pinned.clone(),
        digest: built.manifest_digest.clone(),
        kind: RecordKind::Built,
        dependencies: Vec::new(),
        layers: built
            .manifest
            .layers
            .iter()
            .map(|descriptor| LayerRef {
                digest: descriptor.digest.clone(),
                size_bytes: descriptor.size.max(0) as u64,
            })
            .collect(),
        pulled_unix_secs,
    };
    record_with(fs, &store.images, &record)
        .await
        .with_context(|| format!("recording the built image {pinned}"))?;

    Ok(pinned)
}

fn pinned_reference(reference: &str, digest: &str) -> Result<String> {
    let parsed: oci_client::Reference = reference
        .parse()
        .with_context(|| format!("invalid built-image reference: {reference}"))?;
    Ok(parsed.clone_with_digest(digest.to_string()).whole())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::containerfile::image::assemble;
    use crate::containerfile::image::tests::{layer, parent};
    use crate::image_store::RealFs;

    pub(crate) struct Fixture {
        pub dir: tempfile::TempDir,
        pub layers: LayerCache,
    }

    pub(crate) fn fixture() -> Fixture {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let layers = LayerCache::new(dir.path().join("layers"));
        Fixture { dir, layers }
    }

    impl Fixture {
        pub(crate) fn store(&self) -> LocalStore<'_> {
            LocalStore {
                layers: &self.layers,
                manifests: self.dir.path().join("manifests"),
                images: self.dir.path().join("images"),
            }
        }
    }

    fn built() -> BuiltImage {
        assemble(
            &parent(),
            Some(&layer()),
            &crate::containerfile::executor::ConfigDraft::default(),
            "RUN spike",
        )
        .unwrap()
    }

    fn recorded(f: &Fixture, pinned: &str) -> ImageRecord {
        std::fs::read_dir(f.dir.path().join("images"))
            .expect("the index directory must exist")
            .map(|entry| entry.unwrap().path())
            .filter_map(|path| std::fs::read(path).ok())
            .filter_map(|bytes| serde_json::from_slice::<ImageRecord>(&bytes).ok())
            .find(|record| record.reference == pinned)
            .expect("the built image must be in the index")
    }

    async fn imported(f: &Fixture) -> String {
        import(
            &RealFs,
            &f.store(),
            "lns-build.local/spike",
            &built(),
            std::slice::from_ref(&layer()),
            1_757_000_000,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn the_built_image_is_named_by_the_digest_of_its_own_manifest() {
        let f = fixture();
        assert_eq!(
            imported(&f).await,
            format!("lns-build.local/spike@{}", built().manifest_digest),
        );
    }

    #[tokio::test]
    async fn the_layer_lands_in_the_layer_cache_under_its_digest() {
        let f = fixture();
        imported(&f).await;
        assert_eq!(f.layers.read(&layer().digest).unwrap(), layer().bytes);
    }

    #[tokio::test]
    async fn the_manifest_cache_serves_the_built_manifest_config_and_digest_back() {
        let f = fixture();
        let pinned = imported(&f).await;
        let cached = ManifestCache::new(f.dir.path().join("manifests"))
            .get(&pinned)
            .expect("the built manifest must be cached under its pinned reference");
        assert_eq!(cached.manifest_digest, built().manifest_digest);
        assert_eq!(cached.config, built().config);
        assert_eq!(cached.manifest.layers.len(), 2);
    }

    #[tokio::test]
    async fn the_image_record_lists_every_layer_so_a_prune_cannot_sweep_them() {
        let f = fixture();
        let pinned = imported(&f).await;
        let record = recorded(&f, &pinned);
        assert_eq!(record.kind, RecordKind::Built);
        assert_eq!(record.pulled_unix_secs, 1_757_000_000);
        assert_eq!(
            record.layers,
            vec![
                LayerRef {
                    digest: "sha256:baseblob".into(),
                    size_bytes: 42,
                },
                LayerRef {
                    digest: layer().digest,
                    size_bytes: layer().size(),
                },
            ],
        );
    }

    #[tokio::test]
    async fn importing_the_same_build_twice_is_benign() {
        let f = fixture();
        let first = imported(&f).await;
        let second = imported(&f).await;
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn a_reference_no_registry_could_name_is_refused() {
        let f = fixture();
        let err = import(
            &RealFs,
            &f.store(),
            "NOT A REFERENCE",
            &built(),
            std::slice::from_ref(&layer()),
            0,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid built-image reference"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn a_layer_cache_that_cannot_be_written_names_the_layer() {
        let f = fixture();
        let blocked = f.dir.path().join("layers");
        std::fs::create_dir_all(&blocked).unwrap();
        std::fs::write(blocked.join("sha256"), b"").unwrap();

        let err = import(
            &RealFs,
            &f.store(),
            "lns-build.local/spike",
            &built(),
            std::slice::from_ref(&layer()),
            0,
        )
        .await
        .unwrap_err();

        assert!(
            format!("{err:#}").contains(&format!("installing the built layer {}", layer().digest)),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn a_manifest_cache_that_cannot_be_written_names_the_reference() {
        let f = fixture();
        let manifests = f.dir.path().join("manifests");
        std::fs::write(&manifests, b"a file where the cache dir goes").unwrap();

        let err = import(
            &RealFs,
            &f.store(),
            "lns-build.local/spike",
            &built(),
            std::slice::from_ref(&layer()),
            0,
        )
        .await
        .unwrap_err();

        assert!(
            format!("{err:#}").contains("caching the built manifest for lns-build.local/spike@"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn an_index_that_cannot_be_written_names_the_built_image() {
        let f = fixture();
        let images = f.dir.path().join("images");
        std::fs::write(&images, b"a file where the index dir goes").unwrap();

        let err = import(
            &RealFs,
            &f.store(),
            "lns-build.local/spike",
            &built(),
            std::slice::from_ref(&layer()),
            0,
        )
        .await
        .unwrap_err();

        assert!(
            format!("{err:#}").contains("recording the built image"),
            "{err:#}"
        );
    }
}
