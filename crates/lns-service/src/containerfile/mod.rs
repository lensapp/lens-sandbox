//! Slice 1 of the Containerfile executor (lensapp/lens-sandbox#393): one filesystem-changing
//! instruction captured out of a guest as one OCI layer, assembled onto its base image, and
//! imported where the ordinary boot path finds it. No parser, one hard-coded command.

pub(crate) mod ext4_upper;
pub(crate) mod image;
pub(crate) mod import;
pub mod real;
pub(crate) mod tar_layer;
pub(crate) mod upper;

use anyhow::{Context, Result};

use crate::image_store::Fs;

use image::ParentImage;
use import::LocalStore;
use tar_layer::tar_layer;
use upper::ChangeSet;

/// The repository a built image is named under: a host no registry resolves, so a built digest can only ever be used locally.
pub(crate) const BUILT_IMAGE_REPOSITORY: &str = "lns-build.local/built";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltLayer {
    pub reference: String,
    pub layer_digest: String,
    pub layer_bytes: u64,
    pub entries: usize,
}

pub(crate) async fn import_captured<F: Fs>(
    fs: &F,
    changes: &ChangeSet,
    parent: &ParentImage,
    store: &LocalStore<'_>,
    created_by: &str,
    created: &str,
    now_unix_secs: u64,
) -> Result<BuiltLayer> {
    let layer = tar_layer(changes).context("writing the captured change set as an OCI layer")?;
    let built = image::assemble(parent, &layer, created_by, created)
        .context("assembling the built image's config and manifest")?;
    let reference = import::import(
        fs,
        store,
        BUILT_IMAGE_REPOSITORY,
        &built,
        &layer,
        now_unix_secs,
    )
    .await?;
    Ok(BuiltLayer {
        reference,
        layer_bytes: layer.size(),
        layer_digest: layer.digest,
        entries: layer.entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composefs::oci::build_filesystem_from_layer_bytes;
    use crate::containerfile::image::tests::{parent, parent_manifest};
    use crate::containerfile::import::tests::{Fixture, fixture};
    use crate::containerfile::upper::tests::FakeUpper;
    use crate::containerfile::upper::{Change, capture};
    use crate::image::manifest_cache::ManifestCache;
    use crate::image_store::RealFs;
    use std::ffi::OsStr;
    use std::io::Cursor;

    /// What the spike's one guest command leaves in the upper: one file created, one the base shipped deleted.
    fn one_run_upper() -> FakeUpper {
        let mut upper = FakeUpper::new();
        upper
            .file("spike-created", 0o644, b"built-by-lns\n")
            .dir("etc", 0o755)
            .whiteout("etc/alpine-release");
        upper
    }

    async fn built_from(f: &Fixture, upper: &FakeUpper) -> BuiltLayer {
        let changes = capture(upper).unwrap();
        import_captured(
            &RealFs,
            &changes,
            &parent(),
            &f.store(),
            "RUN sh -c 'echo built-by-lns > /spike-created; rm /etc/alpine-release'",
            "2026-09-09T00:00:00Z",
            1_757_000_000,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn the_built_image_is_the_base_plus_exactly_one_layer() {
        let f = fixture();
        let built = built_from(&f, &one_run_upper()).await;

        let cached = ManifestCache::new(f.dir.path().join("manifests"))
            .get(&built.reference)
            .expect("the built manifest is cached under the reference the build returned");
        assert_eq!(
            cached.manifest.layers.len(),
            parent_manifest().layers.len() + 1
        );
        assert_eq!(
            cached.manifest.layers.last().unwrap().digest,
            built.layer_digest,
        );
        assert_eq!(built.entries, 3);
        assert_eq!(
            built.layer_bytes,
            f.layers.read(&built.layer_digest).unwrap().len() as u64
        );
    }

    /// The round trip's other half, host-side: the layer this build produced goes back in through the
    /// unchanged OCI-layer-to-composefs path, and the tree a guest would boot has the created file and
    /// not the deleted one.
    #[tokio::test]
    async fn the_layer_reimports_through_the_composefs_path_and_the_deletion_holds() {
        let f = fixture();
        let built = built_from(&f, &one_run_upper()).await;
        let base = base_layer_tar();
        let captured = f.layers.read(&built.layer_digest).unwrap();

        let store = crate::content_store::ContentStore::new(f.dir.path().join("content"));
        let mut tree =
            build_filesystem_from_layer_bytes(&store, &[base, captured], &|_, _| {}).unwrap();

        assert!(
            tree.root.leaf_id(OsStr::new("spike-created")).is_ok(),
            "the second guest must see the file the first one created",
        );
        let etc = tree.root.get_directory_mut(OsStr::new("etc")).unwrap();
        assert!(
            etc.leaf_id(OsStr::new("alpine-release")).is_err(),
            "the whiteout must hide the file the base image shipped",
        );
        assert!(
            etc.leaf_id(OsStr::new("passwd")).is_ok(),
            "nothing else the base shipped may disappear",
        );
    }

    fn base_layer_tar() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(Cursor::new(&mut bytes));
            let mut dir = tar::Header::new_gnu();
            dir.set_path("etc/").unwrap();
            dir.set_size(0);
            dir.set_mode(0o755);
            dir.set_uid(0);
            dir.set_gid(0);
            dir.set_mtime(0);
            dir.set_entry_type(tar::EntryType::Directory);
            dir.set_cksum();
            builder.append(&dir, std::io::empty()).unwrap();
            for (path, body) in [
                ("etc/alpine-release", &b"3.20.3\n"[..]),
                ("etc/passwd", &b"root:x:0:0:root:/root:/bin/sh\n"[..]),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_path(path).unwrap();
                header.set_size(body.len() as u64);
                header.set_mode(0o644);
                header.set_uid(0);
                header.set_gid(0);
                header.set_mtime(0);
                header.set_entry_type(tar::EntryType::Regular);
                header.set_cksum();
                builder.append(&header, Cursor::new(body)).unwrap();
            }
            builder.finish().unwrap();
        }
        bytes
    }

    #[tokio::test]
    async fn a_base_image_the_build_cannot_stand_on_is_refused_before_anything_is_imported() {
        let f = fixture();
        let mut parent = parent();
        parent.config = "{}".into();
        let changes = capture(&one_run_upper()).unwrap();

        let err = import_captured(
            &RealFs,
            &changes,
            &parent,
            &f.store(),
            "RUN spike",
            "now",
            0,
        )
        .await
        .unwrap_err();

        assert!(
            format!("{err:#}").contains("assembling the built image's config and manifest"),
            "{err:#}"
        );
        assert!(
            !f.dir.path().join("images").exists(),
            "a refused build must import nothing",
        );
    }

    #[tokio::test]
    async fn a_change_set_no_tar_can_hold_names_the_layer_step() {
        let f = fixture();
        let changes = ChangeSet {
            changes: vec![Change::Regular {
                path: "bad\0path".into(),
                mode: 0o644,
                uid: 0,
                gid: 0,
                bytes: Vec::new(),
            }],
        };

        let err = import_captured(
            &RealFs,
            &changes,
            &parent(),
            &f.store(),
            "RUN spike",
            "now",
            0,
        )
        .await
        .unwrap_err();

        assert!(
            format!("{err:#}").contains("writing the captured change set as an OCI layer"),
            "{err:#}"
        );
    }

    #[test]
    fn a_built_image_is_named_under_a_repository_no_registry_resolves() {
        assert_eq!(BUILT_IMAGE_REPOSITORY, "lns-build.local/built");
    }
}
