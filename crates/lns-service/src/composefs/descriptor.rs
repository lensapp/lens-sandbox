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

    pub fn cached(&self, req: &DescriptorRequest<'_>) -> Result<Option<BuiltDescriptor>> {
        let runtime_digest = req.runtime_layer.map(|r| r.digest());
        let merge_hash = compute_merge_hash(req.layer_digests, runtime_digest);
        self.try_cached(&merge_hash)
    }

    pub fn build(
        &self,
        content_store: &ContentStore,
        req: &DescriptorRequest<'_>,
    ) -> Result<BuiltDescriptor> {
        let runtime_digest = req.runtime_layer.map(|r| r.digest());
        let merge_hash = compute_merge_hash(req.layer_digests, runtime_digest);
        if let Some(hit) = self.try_cached(&merge_hash)? {
            return Ok(hit);
        }
        let path = self.path_for(&merge_hash);

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
        write_sha_sidecar(&path, &descriptor_sha256);

        Ok(BuiltDescriptor {
            merge_hash,
            path,
            descriptor_sha256,
            size: bytes.len() as u64,
        })
    }

    fn path_for(&self, merge_hash: &str) -> PathBuf {
        self.root
            .join("composefs")
            .join(merge_hash)
            .join("descriptor.erofs")
    }

    fn try_cached(&self, merge_hash: &str) -> Result<Option<BuiltDescriptor>> {
        let path = self.path_for(merge_hash);
        let Ok(meta) = std::fs::metadata(&path) else {
            return Ok(None);
        };
        if !meta.is_file() {
            return Ok(None);
        }
        let descriptor_sha256 = match read_sha_sidecar(&path) {
            Some(sha) => sha,
            None => {
                let bytes = std::fs::read(&path)
                    .with_context(|| format!("re-reading cached {}", path.display()))?;
                let sha = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
                write_sha_sidecar(&path, &sha);
                sha
            }
        };
        Ok(Some(BuiltDescriptor {
            merge_hash: merge_hash.to_string(),
            path,
            descriptor_sha256,
            size: meta.len(),
        }))
    }
}

fn sidecar_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".sha256");
    PathBuf::from(s)
}

fn read_sha_sidecar(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(sidecar_path(path)).ok()?;
    let text = text.trim();
    let hex_part = text.strip_prefix("sha256:")?;
    let valid = hex_part.len() == 64 && hex_part.chars().all(|c| c.is_ascii_hexdigit());
    valid.then(|| text.to_string())
}

fn write_sha_sidecar(path: &Path, sha: &str) {
    let sidecar = sidecar_path(path);
    if let Err(e) = std::fs::write(&sidecar, sha) {
        let sidecar_str = sidecar.display();
        crate::log::warn!(
            "descriptor sha sidecar write failed at {sidecar_str} ({e}); next run will re-hash"
        );
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

    struct Fixture {
        _dir: tempfile::TempDir,
        store: ContentStore,
        builder: DescriptorBuilder,
        digests: Vec<String>,
        layers: Vec<Vec<u8>>,
    }

    fn fixture(layer_digest: &str) -> Fixture {
        let dir = tempdir();
        let store = ContentStore::new(dir.path().join("content"));
        let builder = DescriptorBuilder::new(dir.path());
        Fixture {
            _dir: dir,
            store,
            builder,
            digests: vec![layer_digest.repeat(32)],
            layers: vec![tiny_tar("hello", b"world")],
        }
    }

    impl Fixture {
        fn request(&self) -> DescriptorRequest<'_> {
            DescriptorRequest {
                layer_digests: &self.digests,
                layers: &self.layers,
                runtime_layer: None,
            }
        }
    }

    #[test]
    fn build_writes_a_sha_sidecar_next_to_the_descriptor() {
        let f = fixture("sha256:1a");
        let built = f.builder.build(&f.store, &f.request()).unwrap();
        let sidecar = sidecar_path(&built.path);
        assert_eq!(
            std::fs::read_to_string(sidecar).unwrap(),
            built.descriptor_sha256,
        );
    }

    #[test]
    fn cached_returns_none_before_build_and_the_built_descriptor_after() {
        let f = fixture("sha256:2b");
        assert!(f.builder.cached(&f.request()).unwrap().is_none());
        let built = f.builder.build(&f.store, &f.request()).unwrap();
        assert_eq!(f.builder.cached(&f.request()).unwrap(), Some(built));
    }

    #[test]
    fn cached_hit_serves_sha_from_sidecar_without_rehashing_the_descriptor() {
        let f = fixture("sha256:3c");
        let built = f.builder.build(&f.store, &f.request()).unwrap();
        std::fs::write(&built.path, b"corrupted on disk after install").unwrap();
        let hit = f.builder.cached(&f.request()).unwrap().expect("cache hit");
        assert_eq!(
            hit.descriptor_sha256, built.descriptor_sha256,
            "a warm hit must trust the sidecar instead of re-reading the descriptor",
        );
    }

    #[test]
    fn cached_falls_back_to_hashing_and_self_heals_when_sidecar_is_missing() {
        let f = fixture("sha256:4d");
        let built = f.builder.build(&f.store, &f.request()).unwrap();
        let sidecar = sidecar_path(&built.path);
        std::fs::remove_file(&sidecar).unwrap();
        let hit = f.builder.cached(&f.request()).unwrap().expect("cache hit");
        assert_eq!(hit.descriptor_sha256, built.descriptor_sha256);
        assert_eq!(
            std::fs::read_to_string(&sidecar).unwrap(),
            built.descriptor_sha256,
            "the fallback hash must be persisted so the next run skips it",
        );
    }

    #[test]
    fn cached_rejects_a_corrupt_sidecar_and_rehashes() {
        let f = fixture("sha256:5e");
        let built = f.builder.build(&f.store, &f.request()).unwrap();
        let sidecar = sidecar_path(&built.path);
        for garbage in ["not a digest", "sha256:zz", "sha256:abc"] {
            std::fs::write(&sidecar, garbage).unwrap();
            let hit = f.builder.cached(&f.request()).unwrap().expect("cache hit");
            assert_eq!(
                hit.descriptor_sha256, built.descriptor_sha256,
                "garbage sidecar {garbage:?} must be ignored and re-derived",
            );
        }
    }

    #[test]
    fn a_directory_squatting_on_the_descriptor_path_reads_as_a_cache_miss() {
        let f = fixture("sha256:7a");
        let path = f.builder.path_for(&compute_merge_hash(&f.digests, None));
        std::fs::create_dir_all(&path).unwrap();
        assert_eq!(
            f.builder.cached(&f.request()).unwrap(),
            None,
            "a non-file at the descriptor path must not be served as a hit",
        );
    }

    #[test]
    fn a_directory_squatting_on_the_sidecar_path_does_not_fail_the_build() {
        let f = fixture("sha256:6f");
        assert!(f.builder.cached(&f.request()).unwrap().is_none());
        let path = f.builder.path_for(&compute_merge_hash(&f.digests, None));
        std::fs::create_dir_all(sidecar_path(&path)).unwrap();
        let built = f.builder.build(&f.store, &f.request()).unwrap();
        assert!(
            built.path.is_file(),
            "descriptor must land despite the sidecar failure",
        );
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
