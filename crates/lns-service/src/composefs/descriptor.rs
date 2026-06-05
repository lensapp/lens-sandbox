use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::composefs::oci;
use crate::content_store::ContentStore;
use crate::runtime_layer::RuntimeLayer;

pub const ENCODER_VERSION: &str = "composefs-rs-0.4.0-vendored-1";

pub struct DescriptorRequest<'a> {
    pub layer_digests: &'a [String],
    pub layers: &'a [Vec<u8>],
    pub runtime_layer: Option<&'a RuntimeLayer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltDescriptor {
    pub merge_hash: String,
    pub path: PathBuf,
    pub descriptor_sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct DescriptorBuilder {
    root: PathBuf,
}

impl DescriptorBuilder {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn build(
        &self,
        content_store: &ContentStore,
        req: &DescriptorRequest<'_>,
    ) -> Result<BuiltDescriptor> {
        let runtime_digest = req.runtime_layer.map(|r| r.digest());
        let merge_hash = compute_merge_hash(req.layer_digests, runtime_digest);
        let dir = self.root.join("composefs").join(&merge_hash);
        let path = dir.join("descriptor.erofs");

        if let Ok(meta) = std::fs::metadata(&path)
            && meta.is_file()
        {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("re-reading cached {}", path.display()))?;
            return Ok(BuiltDescriptor {
                merge_hash,
                path,
                descriptor_sha256: format!("sha256:{}", hex::encode(Sha256::digest(&bytes))),
                size: meta.len(),
            });
        }

        let mut fs = oci::build_filesystem_from_layer_bytes(content_store, req.layers)
            .context("expanding OCI layers into composefs tree")?;

        if fs.root.lookup(std::ffi::OsStr::new("usr")).is_some() {
            fs.transform_for_oci()
                .map_err(|e| anyhow::anyhow!("transform_for_oci: {e:?}"))?;
        }

        if let Some(rt) = req.runtime_layer {
            rt.inject_into_filesystem(&mut fs)
                .context("injecting runtime layer into composefs tree")?;
        }

        let bytes: Box<[u8]> = super::mkfs_erofs(&fs);
        let descriptor_sha256 = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));

        atomic_install(&path, &bytes)
            .with_context(|| format!("installing composefs descriptor at {}", path.display()))?;

        Ok(BuiltDescriptor {
            merge_hash,
            path,
            descriptor_sha256,
            size: bytes.len() as u64,
        })
    }
}

fn compute_merge_hash(layer_digests: &[String], runtime_digest: Option<&str>) -> String {
    let mut h = Sha256::new();
    for d in layer_digests {
        h.update(d.as_bytes());
        h.update(b"\n");
    }
    h.update(b"runtime=");
    h.update(runtime_digest.unwrap_or("none").as_bytes());
    h.update(b"\n");
    h.update(b"encoder=");
    h.update(ENCODER_VERSION.as_bytes());
    hex::encode(h.finalize())
}

fn atomic_install(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    let parent = path
        .parent()
        .with_context(|| format!("path {} has no parent", path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create_dir_all {}", parent.display()))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = path.with_extension(format!("erofs.tmp.{}.{}", std::process::id(), nanos));

    {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(bytes)
            .with_context(|| format!("writing {} bytes to {}", bytes.len(), tmp.display()))?;
        f.sync_all().context("fsync descriptor")?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tar::{Builder, EntryType, Header};

    fn tempdir() -> tempfile::TempDir {
        tempfile::TempDir::new().expect("tempdir")
    }

    fn tiny_tar(file_name: &str, body: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut b = Builder::new(Cursor::new(&mut bytes));
            let mut h = Header::new_gnu();
            h.set_path(file_name).unwrap();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_uid(0);
            h.set_gid(0);
            h.set_mtime(0);
            h.set_entry_type(EntryType::Regular);
            h.set_cksum();
            b.append(&h, Cursor::new(body)).unwrap();
            b.finish().unwrap();
        }
        bytes
    }

    fn usr_tree_tar(bin_name: &str, body: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut b = Builder::new(Cursor::new(&mut bytes));
            for dir in ["usr/", "usr/bin/"] {
                let mut h = Header::new_gnu();
                h.set_path(dir).unwrap();
                h.set_size(0);
                h.set_mode(0o755);
                h.set_uid(0);
                h.set_gid(0);
                h.set_mtime(0);
                h.set_entry_type(EntryType::Directory);
                h.set_cksum();
                b.append(&h, std::io::empty()).unwrap();
            }
            let path = format!("usr/bin/{bin_name}");
            let mut h = Header::new_gnu();
            h.set_path(&path).unwrap();
            h.set_size(body.len() as u64);
            h.set_mode(0o755);
            h.set_uid(0);
            h.set_gid(0);
            h.set_mtime(0);
            h.set_entry_type(EntryType::Regular);
            h.set_cksum();
            b.append(&h, Cursor::new(body)).unwrap();
            b.finish().unwrap();
        }
        bytes
    }

    #[test]
    fn build_writes_descriptor_and_returns_paths() {
        let d = tempdir();
        let store = ContentStore::new(d.path().join("content"));
        let builder = DescriptorBuilder::new(d.path());
        let tar = tiny_tar("hello", b"world");
        let req = DescriptorRequest {
            layer_digests: &["sha256:0011".repeat(8).to_string()],
            layers: &[tar],
            runtime_layer: None,
        };
        let r = builder.build(&store, &req).unwrap();

        assert!(r.path.is_file());
        assert_eq!(
            r.path.parent().unwrap().file_name().unwrap(),
            &*r.merge_hash
        );
        assert!(r.path.starts_with(d.path().join("composefs")));
        assert_eq!(r.size, r.path.metadata().unwrap().len());

        let bytes = std::fs::read(&r.path).unwrap();
        assert!(bytes.len() > 1024);
        assert_eq!(&bytes[1024..1028], &[0xe2, 0xe1, 0xf5, 0xe0]);
    }

    #[test]
    fn merge_hash_is_deterministic_for_same_inputs() {
        let a = compute_merge_hash(&["sha256:aa".repeat(32)], Some("sha256:rr"));
        let b = compute_merge_hash(&["sha256:aa".repeat(32)], Some("sha256:rr"));
        assert_eq!(a, b);
    }

    #[test]
    fn merge_hash_changes_with_runtime_digest() {
        let a = compute_merge_hash(&["sha256:aa".repeat(32)], None);
        let b = compute_merge_hash(&["sha256:aa".repeat(32)], Some("sha256:rr"));
        assert_ne!(a, b, "runtime layer must re-key the merge hash");
    }

    #[test]
    fn merge_hash_changes_with_layer_order() {
        let a = compute_merge_hash(&["sha256:01".to_string(), "sha256:02".to_string()], None);
        let b = compute_merge_hash(&["sha256:02".to_string(), "sha256:01".to_string()], None);
        assert_ne!(a, b, "layer order is part of the merge identity");
    }

    #[test]
    fn merge_hash_includes_encoder_version() {
        let h = compute_merge_hash(&["sha256:00".repeat(32)], None);
        let mut expected = Sha256::new();
        expected.update("sha256:00".repeat(32).as_bytes());
        expected.update(b"\n");
        expected.update(b"runtime=");
        expected.update(b"none");
        expected.update(b"\n");
        expected.update(b"encoder=");
        expected.update(ENCODER_VERSION.as_bytes());
        assert_eq!(h, hex::encode(expected.finalize()));
    }

    #[test]
    fn build_is_idempotent_and_hits_cache() {
        let d = tempdir();
        let store = ContentStore::new(d.path().join("content"));
        let builder = DescriptorBuilder::new(d.path());
        let tar = tiny_tar("x", b"y");
        let req = DescriptorRequest {
            layer_digests: &["sha256:0a".repeat(32)],
            layers: &[tar],
            runtime_layer: None,
        };
        let a = builder.build(&store, &req).unwrap();
        let mtime_a = std::fs::metadata(&a.path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = builder.build(&store, &req).unwrap();
        assert_eq!(a, b);
        let mtime_b = std::fs::metadata(&b.path).unwrap().modified().unwrap();
        assert_eq!(mtime_a, mtime_b, "cached build must not rewrite the file");
    }

    #[test]
    fn build_for_different_runtime_digests_creates_separate_descriptors() {
        use crate::runtime_layer::{RuntimeFileSpec, RuntimeLayerBuilder, RuntimeSource};
        let d = tempdir();
        let store = ContentStore::new(d.path().join("content"));
        let builder = DescriptorBuilder::new(d.path());
        let tar = tiny_tar("file", b"contents");
        let layer_digests = vec!["sha256:01".repeat(32)];

        let rt_a = RuntimeLayerBuilder::new("v-aaa")
            .build(
                &store,
                &[RuntimeFileSpec {
                    guest_path: "/.lens/bin/super".into(),
                    mode: 0o755,
                    source: RuntimeSource::Bytes(b"super-a".to_vec()),
                }],
            )
            .unwrap();
        let rt_b = RuntimeLayerBuilder::new("v-bbb")
            .build(
                &store,
                &[RuntimeFileSpec {
                    guest_path: "/.lens/bin/super".into(),
                    mode: 0o755,
                    source: RuntimeSource::Bytes(b"super-b".to_vec()),
                }],
            )
            .unwrap();
        assert_ne!(rt_a.digest, rt_b.digest);

        let r1 = builder
            .build(
                &store,
                &DescriptorRequest {
                    layer_digests: &layer_digests,
                    layers: std::slice::from_ref(&tar),
                    runtime_layer: Some(&rt_a),
                },
            )
            .unwrap();
        let r2 = builder
            .build(
                &store,
                &DescriptorRequest {
                    layer_digests: &layer_digests,
                    layers: std::slice::from_ref(&tar),
                    runtime_layer: Some(&rt_b),
                },
            )
            .unwrap();
        assert_ne!(r1.merge_hash, r2.merge_hash);
        assert_ne!(r1.path, r2.path);
        assert!(r1.path.is_file());
        assert!(r2.path.is_file());
    }

    #[test]
    fn build_runs_oci_transform_for_usr_shaped_trees() {
        let d = tempdir();
        let store = ContentStore::new(d.path().join("content"));
        let builder = DescriptorBuilder::new(d.path());
        let tar = usr_tree_tar("hello", b"#!/bin/sh\n");
        let req = DescriptorRequest {
            layer_digests: &["sha256:0c".repeat(32)],
            layers: &[tar],
            runtime_layer: None,
        };
        let r = builder.build(&store, &req).unwrap();
        assert!(r.path.is_file());
        let bytes = std::fs::read(&r.path).unwrap();
        assert!(bytes.len() > 1024);
        assert_eq!(&bytes[1024..1028], &[0xe2, 0xe1, 0xf5, 0xe0]);
    }

    #[test]
    fn descriptor_sha256_matches_file_contents() {
        let d = tempdir();
        let store = ContentStore::new(d.path().join("content"));
        let builder = DescriptorBuilder::new(d.path());
        let tar = tiny_tar("a", b"b");
        let r = builder
            .build(
                &store,
                &DescriptorRequest {
                    layer_digests: &["sha256:99".repeat(32)],
                    layers: &[tar],
                    runtime_layer: None,
                },
            )
            .unwrap();
        let bytes = std::fs::read(&r.path).unwrap();
        let want = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
        assert_eq!(r.descriptor_sha256, want);
    }
}
