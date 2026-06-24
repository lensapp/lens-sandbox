#![allow(dead_code)]

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use crate::composefs::Sha256Digest;
use crate::composefs::vendor::generic_tree::{Directory, Inode, LeafContent, Stat};
use crate::composefs::vendor::tree::{FileSystem, RegularFile};
use crate::content_store::ContentStore;

#[cfg(target_os = "linux")]
#[inline]
const fn type_bits(t: libc::mode_t) -> u32 {
    t
}
#[cfg(target_os = "macos")]
#[inline]
const fn type_bits(t: libc::mode_t) -> u32 {
    t as u32
}

#[derive(Debug, Clone)]
pub struct RuntimeFileSpec {
    pub guest_path: String,
    pub mode: u32,
    pub source: RuntimeSource,
}

#[derive(Debug, Clone)]
pub enum RuntimeSource {
    HostFile(PathBuf),
    Bytes(Vec<u8>),
    Symlink(String),
}

#[derive(Debug, Clone)]
pub struct RuntimeLayer {
    pub digest: String,
    pub manifest: Manifest,
    pub entries: Vec<ResolvedEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub schema_version: u32,
    pub label: String,
    pub files: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestEntry {
    pub path: String,
    pub digest: String,
    pub size: u64,
    pub mode: u32,
    pub kind: ManifestKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManifestKind {
    Regular,
    Symlink { target: String },
}

#[derive(Debug, Clone)]
pub struct ResolvedEntry {
    pub path: String,
    pub mode: u32,
    pub kind: ResolvedKind,
}

#[derive(Debug, Clone)]
pub enum ResolvedKind {
    Regular { digest: Sha256Digest, size: u64 },
    Symlink { target: String },
}

#[derive(Debug, Clone)]
pub struct RuntimeLayerBuilder {
    label: String,
}

impl RuntimeLayerBuilder {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }

    pub fn build(
        &self,
        content_store: &ContentStore,
        specs: &[RuntimeFileSpec],
    ) -> Result<RuntimeLayer> {
        let mut manifest_files = Vec::with_capacity(specs.len());
        let mut resolved = Vec::with_capacity(specs.len());

        for spec in specs {
            let path = normalize_guest_path(&spec.guest_path)
                .with_context(|| format!("normalizing runtime guest_path {:?}", spec.guest_path))?;
            let mode = spec.mode & 0o7777;
            match &spec.source {
                RuntimeSource::HostFile(host) => {
                    let bytes = std::fs::read(host)
                        .with_context(|| format!("reading runtime source {}", host.display()))?;
                    let r = content_store
                        .install_from_bytes(&bytes)
                        .with_context(|| format!("installing runtime blob for {path}"))?;
                    manifest_files.push(ManifestEntry {
                        path: path.clone(),
                        digest: r.digest.clone(),
                        size: r.size,
                        mode,
                        kind: ManifestKind::Regular,
                    });
                    resolved.push(ResolvedEntry {
                        path,
                        mode,
                        kind: ResolvedKind::Regular {
                            digest: Sha256Digest::from(r.raw_digest),
                            size: r.size,
                        },
                    });
                }
                RuntimeSource::Bytes(bytes) => {
                    let r = content_store
                        .install_from_bytes(bytes)
                        .with_context(|| format!("installing inline runtime blob for {path}"))?;
                    manifest_files.push(ManifestEntry {
                        path: path.clone(),
                        digest: r.digest.clone(),
                        size: r.size,
                        mode,
                        kind: ManifestKind::Regular,
                    });
                    resolved.push(ResolvedEntry {
                        path,
                        mode,
                        kind: ResolvedKind::Regular {
                            digest: Sha256Digest::from(r.raw_digest),
                            size: r.size,
                        },
                    });
                }
                RuntimeSource::Symlink(target) => {
                    manifest_files.push(ManifestEntry {
                        path: path.clone(),
                        digest: String::new(),
                        size: 0,
                        mode,
                        kind: ManifestKind::Symlink {
                            target: target.clone(),
                        },
                    });
                    resolved.push(ResolvedEntry {
                        path,
                        mode,
                        kind: ResolvedKind::Symlink {
                            target: target.clone(),
                        },
                    });
                }
            }
        }

        manifest_files.sort_by(|a, b| a.path.cmp(&b.path));
        resolved.sort_by(|a, b| a.path.cmp(&b.path));

        let manifest = Manifest {
            schema_version: 1,
            label: self.label.clone(),
            files: manifest_files,
        };
        let manifest_bytes = serde_json::to_vec(&manifest).context("serializing manifest")?;
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(&manifest_bytes)));

        Ok(RuntimeLayer {
            digest,
            manifest,
            entries: resolved,
        })
    }
}

impl RuntimeLayer {
    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn inject_into_filesystem(&self, fs: &mut FileSystem<Sha256Digest>) -> Result<()> {
        for entry in &self.entries {
            inject_entry(fs, entry)?;
        }
        Ok(())
    }
}

fn inject_entry(fs: &mut FileSystem<Sha256Digest>, entry: &ResolvedEntry) -> Result<()> {
    let segments = split_path(&entry.path);
    if segments.is_empty() {
        bail!("runtime entry has empty path: {:?}", entry.path);
    }
    let (parents, name) = segments.split_at(segments.len() - 1);
    let name = OsStr::new(&name[0]);

    let parent_os: Vec<OsString> = parents.iter().map(|s| OsString::from(s.as_str())).collect();
    let resolved = crate::composefs::inject_path::resolve_parent_segments(fs, &parent_os)?;

    let mut dir = &mut fs.root;
    for seg in &resolved {
        let seg_os = seg.as_os_str();
        if dir.lookup(seg_os).is_none() {
            dir.insert(
                seg_os,
                Inode::Directory(Box::new(Directory::new(default_dir_stat()))),
            );
        }
        dir = dir
            .get_directory_mut(seg_os)
            .with_context(|| format!("descending into {:?} (not a directory)", seg))?;
    }
    dir.remove(name);
    let content = match &entry.kind {
        ResolvedKind::Regular { digest, size } => {
            LeafContent::Regular(RegularFile::External(digest.clone(), *size))
        }
        ResolvedKind::Symlink { target } => {
            let target_os: std::ffi::OsString = target.clone().into();
            LeafContent::Symlink(target_os.into_boxed_os_str())
        }
    };
    let stat = stat_for_entry(entry);
    let leaf_id = fs.push_leaf(stat, content);
    let mut dir = &mut fs.root;
    for seg in &resolved {
        dir = dir.get_directory_mut(seg.as_os_str()).expect("just walked");
    }
    dir.insert(name, Inode::leaf(leaf_id));
    Ok(())
}

fn stat_for_entry(entry: &ResolvedEntry) -> Stat {
    let type_bits = match &entry.kind {
        ResolvedKind::Regular { .. } => type_bits(libc::S_IFREG),
        ResolvedKind::Symlink { .. } => type_bits(libc::S_IFLNK),
    };
    Stat {
        st_mode: type_bits | (entry.mode & 0o7777),
        st_uid: 0,
        st_gid: 0,
        st_mtim_sec: 0,
        xattrs: std::collections::BTreeMap::new(),
    }
}

fn default_dir_stat() -> Stat {
    Stat {
        st_mode: type_bits(libc::S_IFDIR) | 0o755,
        st_uid: 0,
        st_gid: 0,
        st_mtim_sec: 0,
        xattrs: std::collections::BTreeMap::new(),
    }
}

fn normalize_guest_path(input: &str) -> Result<String> {
    let p = Path::new(input);
    let mut segs: Vec<&str> = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(seg) => {
                let s = seg
                    .to_str()
                    .with_context(|| format!("non-utf8 path segment in {input:?}"))?;
                segs.push(s);
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => continue,
            Component::ParentDir => bail!("runtime path {input:?} contains `..`"),
        }
    }
    if segs.is_empty() {
        bail!("runtime path is empty: {input:?}");
    }
    Ok(segs.join("/"))
}

fn split_path(normalized: &str) -> Vec<String> {
    normalized
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

pub fn for_run(
    imageless: bool,
    content_store: &crate::content_store::ContentStore,
    guest_tools: &crate::guest_tools::GuestTools,
    assets: Option<&crate::supervisor::SupervisorAssets>,
    extra_specs: Vec<RuntimeFileSpec>,
) -> Result<Option<RuntimeLayer>> {
    let runtime_label = format!("lns-runtime-v1+{}", env!("CARGO_PKG_VERSION"));
    let (mut specs, label) = if let Some(a) = assets {
        let mut specs = crate::supervisor::runtime_specs(a)?;
        if imageless {
            specs.extend(crate::supervisor::imageless_runtime_extras());
        }
        let label = if imageless {
            format!("{runtime_label}+imageless")
        } else {
            runtime_label.clone()
        };
        (specs, label)
    } else if imageless {
        (
            crate::supervisor::guest_tools_only_specs(&guest_tools.root)?,
            format!("{runtime_label}+imageless"),
        )
    } else {
        (
            crate::supervisor::guest_tools_tree_specs(&guest_tools.root)?,
            runtime_label.clone(),
        )
    };
    specs.extend(extra_specs);
    Ok(Some(
        RuntimeLayerBuilder::new(&label).build(content_store, &specs)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composefs::fsverity::FsVerityHashValue;

    fn tempdir() -> tempfile::TempDir {
        tempfile::TempDir::new().expect("tempdir")
    }

    fn store(d: &tempfile::TempDir) -> ContentStore {
        ContentStore::new(d.path().join("content"))
    }

    #[test]
    fn build_with_one_inline_file_produces_manifest_and_blob() {
        let d = tempdir();
        let s = store(&d);
        let layer = RuntimeLayerBuilder::new("test-label")
            .build(
                &s,
                &[RuntimeFileSpec {
                    guest_path: "/.lens/bin/lns-supervisor".into(),
                    mode: 0o755,
                    source: RuntimeSource::Bytes(b"#!/bin/sh\necho fake supervisor".to_vec()),
                }],
            )
            .unwrap();
        assert_eq!(layer.manifest.schema_version, 1);
        assert_eq!(layer.manifest.label, "test-label");
        assert_eq!(layer.manifest.files.len(), 1);
        let e = &layer.manifest.files[0];
        assert_eq!(e.path, ".lens/bin/lns-supervisor");
        assert_eq!(e.mode, 0o755);
        assert!(matches!(e.kind, ManifestKind::Regular));
        assert!(e.digest.starts_with("sha256:"));
        assert!(s.contains(&e.digest).unwrap());
    }

    #[test]
    fn digest_is_deterministic_for_same_inputs() {
        let d = tempdir();
        let s = store(&d);
        let builder = RuntimeLayerBuilder::new("v1");
        let spec = vec![RuntimeFileSpec {
            guest_path: "/usr/bin/x".into(),
            mode: 0o755,
            source: RuntimeSource::Bytes(b"x".to_vec()),
        }];
        let a = builder.build(&s, &spec).unwrap();
        let b = builder.build(&s, &spec).unwrap();
        assert_eq!(a.digest, b.digest);
    }

    #[test]
    fn digest_changes_with_label() {
        let d = tempdir();
        let s = store(&d);
        let spec = vec![RuntimeFileSpec {
            guest_path: "/x".into(),
            mode: 0o755,
            source: RuntimeSource::Bytes(b"x".to_vec()),
        }];
        let a = RuntimeLayerBuilder::new("v1").build(&s, &spec).unwrap();
        let b = RuntimeLayerBuilder::new("v2").build(&s, &spec).unwrap();
        assert_ne!(a.digest, b.digest);
    }

    #[test]
    fn digest_changes_with_content() {
        let d = tempdir();
        let s = store(&d);
        let builder = RuntimeLayerBuilder::new("v1");
        let a = builder
            .build(
                &s,
                &[RuntimeFileSpec {
                    guest_path: "/x".into(),
                    mode: 0o755,
                    source: RuntimeSource::Bytes(b"alpha".to_vec()),
                }],
            )
            .unwrap();
        let b = builder
            .build(
                &s,
                &[RuntimeFileSpec {
                    guest_path: "/x".into(),
                    mode: 0o755,
                    source: RuntimeSource::Bytes(b"beta".to_vec()),
                }],
            )
            .unwrap();
        assert_ne!(a.digest, b.digest);
    }

    #[test]
    fn build_is_order_independent_for_same_set() {
        let d = tempdir();
        let s = store(&d);
        let specs_forward = vec![
            RuntimeFileSpec {
                guest_path: "/a".into(),
                mode: 0o644,
                source: RuntimeSource::Bytes(b"alpha".to_vec()),
            },
            RuntimeFileSpec {
                guest_path: "/b".into(),
                mode: 0o644,
                source: RuntimeSource::Bytes(b"beta".to_vec()),
            },
        ];
        let specs_reverse: Vec<_> = specs_forward.iter().rev().cloned().collect();
        let a = RuntimeLayerBuilder::new("v")
            .build(&s, &specs_forward)
            .unwrap();
        let b = RuntimeLayerBuilder::new("v")
            .build(&s, &specs_reverse)
            .unwrap();
        assert_eq!(a.digest, b.digest);
    }

    #[test]
    fn build_handles_symlink_entries() {
        let d = tempdir();
        let s = store(&d);
        let layer = RuntimeLayerBuilder::new("v1")
            .build(
                &s,
                &[RuntimeFileSpec {
                    guest_path: "/bin/sh".into(),
                    mode: 0o777,
                    source: RuntimeSource::Symlink("busybox".into()),
                }],
            )
            .unwrap();
        let e = &layer.manifest.files[0];
        assert!(matches!(&e.kind, ManifestKind::Symlink { target } if target == "busybox"));
        assert_eq!(e.digest, "");
    }

    #[test]
    fn build_reads_host_file() {
        let d = tempdir();
        let s = store(&d);
        let source = d.path().join("super");
        std::fs::write(&source, b"binary bytes here").unwrap();
        let layer = RuntimeLayerBuilder::new("v1")
            .build(
                &s,
                &[RuntimeFileSpec {
                    guest_path: "/.lens/bin/super".into(),
                    mode: 0o755,
                    source: RuntimeSource::HostFile(source),
                }],
            )
            .unwrap();
        let e = &layer.manifest.files[0];
        assert!(matches!(e.kind, ManifestKind::Regular));
        assert!(s.contains(&e.digest).unwrap());
    }

    #[test]
    fn normalize_guest_path_strips_leading_slash() {
        assert_eq!(normalize_guest_path("/.lens/bin/x").unwrap(), ".lens/bin/x");
    }

    #[test]
    fn normalize_guest_path_rejects_dotdot() {
        let err = normalize_guest_path("/run/../etc").unwrap_err();
        assert!(format!("{err:#}").contains(".."));
    }

    #[test]
    fn normalize_guest_path_collapses_curdir() {
        assert_eq!(normalize_guest_path("/./run/./x").unwrap(), "run/x");
    }

    #[test]
    fn normalize_guest_path_rejects_empty() {
        let err = normalize_guest_path("/").unwrap_err();
        assert!(format!("{err:#}").contains("empty"));
    }

    #[test]
    fn inject_overlays_runtime_files_onto_existing_tree() {
        let d = tempdir();
        let s = store(&d);

        let mut fs = FileSystem::<Sha256Digest>::new(Stat {
            st_mode: type_bits(libc::S_IFDIR) | 0o755,
            st_uid: 0,
            st_gid: 0,
            st_mtim_sec: 0,
            xattrs: Default::default(),
        });

        let layer = RuntimeLayerBuilder::new("v1")
            .build(
                &s,
                &[RuntimeFileSpec {
                    guest_path: "/.lens/bin/foo".into(),
                    mode: 0o755,
                    source: RuntimeSource::Bytes(b"foo bytes".to_vec()),
                }],
            )
            .unwrap();
        layer.inject_into_filesystem(&mut fs).unwrap();

        let lens = fs.root.get_directory_mut(OsStr::new(".lens")).unwrap();
        let bin = lens.get_directory_mut(OsStr::new("bin")).unwrap();
        let leaf_id = bin.leaf_id(OsStr::new("foo")).unwrap();
        assert!(matches!(
            &fs.leaves[leaf_id.0].content,
            LeafContent::Regular(RegularFile::External(_, size)) if *size == 9
        ));
    }

    #[test]
    fn inject_overrides_existing_image_file() {
        let d = tempdir();
        let s = store(&d);

        let mut fs = FileSystem::<Sha256Digest>::new(default_dir_stat());

        let image_blob = s.install_from_bytes(b"image-resolv").unwrap();
        let etc_id = fs.push_leaf(
            Stat {
                st_mode: type_bits(libc::S_IFDIR) | 0o755,
                st_uid: 0,
                st_gid: 0,
                st_mtim_sec: 0,
                xattrs: Default::default(),
            },
            LeafContent::Regular(RegularFile::Inline(Box::new([]))),
        );
        let _ = etc_id;
        fs.root.insert(
            OsStr::new("etc"),
            Inode::Directory(Box::new(Directory::new(default_dir_stat()))),
        );
        let resolv_id = fs.push_leaf(
            Stat {
                st_mode: type_bits(libc::S_IFREG) | 0o644,
                st_uid: 0,
                st_gid: 0,
                st_mtim_sec: 0,
                xattrs: Default::default(),
            },
            LeafContent::Regular(RegularFile::External(
                Sha256Digest::from(image_blob.raw_digest),
                image_blob.size,
            )),
        );
        let etc_dir = fs.root.get_directory_mut(OsStr::new("etc")).unwrap();
        etc_dir.insert(OsStr::new("resolv.conf"), Inode::leaf(resolv_id));

        let layer = RuntimeLayerBuilder::new("v1")
            .build(
                &s,
                &[RuntimeFileSpec {
                    guest_path: "/etc/resolv.conf".into(),
                    mode: 0o644,
                    source: RuntimeSource::Bytes(b"runtime-resolv".to_vec()),
                }],
            )
            .unwrap();
        layer.inject_into_filesystem(&mut fs).unwrap();

        let etc_dir = fs.root.get_directory_mut(OsStr::new("etc")).unwrap();
        let id = etc_dir.leaf_id(OsStr::new("resolv.conf")).unwrap();
        let runtime_blob_digest = layer.manifest.files[0]
            .digest
            .strip_prefix("sha256:")
            .unwrap();
        assert!(matches!(
            &fs.leaves[id.0].content,
            LeafContent::Regular(RegularFile::External(digest, _))
                if digest.to_hex() == runtime_blob_digest
        ));
    }

    #[test]
    fn inject_follows_usrmerge_lib_symlink() {
        let d = tempdir();
        let s = store(&d);

        let mut fs = FileSystem::<Sha256Digest>::new(default_dir_stat());
        fs.root.insert(
            OsStr::new("usr"),
            Inode::Directory(Box::new(Directory::new(default_dir_stat()))),
        );
        fs.root
            .get_directory_mut(OsStr::new("usr"))
            .unwrap()
            .insert(
                OsStr::new("lib"),
                Inode::Directory(Box::new(Directory::new(default_dir_stat()))),
            );
        let lib_link = fs.push_leaf(
            Stat {
                st_mode: type_bits(libc::S_IFLNK) | 0o777,
                st_uid: 0,
                st_gid: 0,
                st_mtim_sec: 0,
                xattrs: Default::default(),
            },
            LeafContent::Symlink(OsStr::new("usr/lib").into()),
        );
        fs.root.insert(OsStr::new("lib"), Inode::leaf(lib_link));

        let layer = RuntimeLayerBuilder::new("v1")
            .build(
                &s,
                &[RuntimeFileSpec {
                    guest_path: "/lib/ld-musl-test.so.1".into(),
                    mode: 0o755,
                    source: RuntimeSource::Bytes(b"loader".to_vec()),
                }],
            )
            .unwrap();
        layer.inject_into_filesystem(&mut fs).unwrap();

        let usr_lib = fs.root.get_directory(OsStr::new("usr/lib")).unwrap();
        assert!(usr_lib.leaf_id(OsStr::new("ld-musl-test.so.1")).is_ok());
        assert!(matches!(
            fs.root.lookup(OsStr::new("lib")),
            Some(Inode::Leaf(..))
        ));
    }

    #[test]
    fn imageless_runtime_layer_carries_bin_sh_symlink() {
        let d = tempdir();
        let s = store(&d);

        let specs = crate::supervisor::guest_tools_only_specs(d.path()).unwrap();
        let layer = RuntimeLayerBuilder::new("v1+imageless")
            .build(&s, &specs)
            .unwrap();
        let bin_sh = layer
            .manifest
            .files
            .iter()
            .find(|e| e.path == "bin/sh")
            .expect("/bin/sh entry in imageless runtime manifest");
        assert!(matches!(
            &bin_sh.kind,
            ManifestKind::Symlink { target }
                if target == "/.lens/guest-tools/bin/busybox"
        ));
    }

    #[test]
    fn inject_places_symlink_entries() {
        let d = tempdir();
        let s = store(&d);

        let mut fs = FileSystem::<Sha256Digest>::new(default_dir_stat());

        let layer = RuntimeLayerBuilder::new("v1")
            .build(
                &s,
                &[RuntimeFileSpec {
                    guest_path: "/bin/sh".into(),
                    mode: 0o777,
                    source: RuntimeSource::Symlink("/.lens/guest-tools/bin/busybox".into()),
                }],
            )
            .unwrap();
        layer.inject_into_filesystem(&mut fs).unwrap();

        let bin_dir = fs.root.get_directory_mut(OsStr::new("bin")).unwrap();
        let leaf_id = bin_dir.leaf_id(OsStr::new("sh")).unwrap();
        let leaf = &fs.leaves[leaf_id.0];
        assert!(matches!(
            &leaf.content,
            LeafContent::Symlink(target)
                if target.as_ref() == OsStr::new("/.lens/guest-tools/bin/busybox")
        ));
        assert_eq!(
            leaf.stat.st_mode & type_bits(libc::S_IFMT),
            type_bits(libc::S_IFLNK)
        );
    }

    #[test]
    fn inject_into_filesystem_rejects_empty_path() {
        let mut fs = FileSystem::<Sha256Digest>::new(default_dir_stat());
        let layer = RuntimeLayer {
            digest: "sha256:0".into(),
            manifest: Manifest {
                schema_version: 1,
                label: "v1".into(),
                files: Vec::new(),
            },
            entries: vec![ResolvedEntry {
                path: String::new(),
                mode: 0o755,
                kind: ResolvedKind::Symlink { target: "x".into() },
            }],
        };
        let err = layer.inject_into_filesystem(&mut fs).unwrap_err();
        assert!(format!("{err:#}").contains("empty path"));
    }

    fn fake_assets(d: &tempfile::TempDir) -> crate::supervisor::SupervisorAssets {
        let supervisor_bin = d.path().join("supervisor.real");
        std::fs::write(&supervisor_bin, b"fake-supervisor-bytes").unwrap();
        let guest_tools_root = d.path().join("supervisor-guest-tools");
        std::fs::create_dir_all(&guest_tools_root).unwrap();
        crate::supervisor::SupervisorAssets {
            supervisor_bin,
            guest_tools_root,
        }
    }

    fn fake_guest_tools(d: &tempfile::TempDir) -> crate::guest_tools::GuestTools {
        let root = d.path().join("guest-tools");
        std::fs::create_dir_all(&root).unwrap();
        crate::guest_tools::GuestTools { root }
    }

    fn paths(layer: &RuntimeLayer) -> Vec<&str> {
        layer
            .manifest
            .files
            .iter()
            .map(|e| e.path.as_str())
            .collect()
    }

    struct NftEnvOverride {
        _env: crate::test_env::EnvVarGuard,
        _file: tempfile::NamedTempFile,
    }

    impl NftEnvOverride {
        fn install() -> Self {
            let file = tempfile::NamedTempFile::new().expect("tempfile for nft override");
            let env = crate::test_env::EnvVarGuard::set("LNS_NFT_BIN", file.path());
            Self {
                _env: env,
                _file: file,
            }
        }
    }

    #[test]
    #[serial_test::serial(env)]
    fn nft_env_override_restores_previous_value() {
        let _harness = crate::test_env::EnvVarGuard::set("LNS_NFT_BIN", "sentinel-prev");
        {
            let _g = NftEnvOverride::install();
        }
        assert_eq!(
            std::env::var_os("LNS_NFT_BIN").as_deref(),
            Some(std::ffi::OsStr::new("sentinel-prev")),
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn nft_env_override_unsets_when_no_previous_value() {
        let _harness = crate::test_env::EnvVarGuard::unset("LNS_NFT_BIN");
        {
            let _g = NftEnvOverride::install();
        }
        assert!(std::env::var_os("LNS_NFT_BIN").is_none());
    }

    #[test]
    #[serial_test::serial(env)]
    fn for_run_oci_supervised_carries_supervisor_and_nft_and_no_bin_sh() {
        let _env = NftEnvOverride::install();
        let d = tempdir();
        let s = store(&d);
        let gt = fake_guest_tools(&d);
        let assets = fake_assets(&d);
        let layer = for_run(false, &s, &gt, Some(&assets), Vec::new())
            .unwrap()
            .unwrap();
        let p = paths(&layer);
        assert!(p.contains(&".lens/bin/lns-supervisor"));
        assert!(!p.contains(&".lens/bin/supervisor.real"));
        assert!(p.contains(&".lens/nft"));
        assert!(!p.contains(&"bin/sh"));
        assert!(!layer.manifest.label.contains("imageless"));
    }

    #[test]
    #[serial_test::serial(env)]
    fn for_run_imageless_supervised_adds_bin_sh_and_suffixes_label() {
        let _env = NftEnvOverride::install();
        let d = tempdir();
        let s = store(&d);
        let gt = fake_guest_tools(&d);
        let assets = fake_assets(&d);
        let layer = for_run(true, &s, &gt, Some(&assets), Vec::new())
            .unwrap()
            .unwrap();
        let p = paths(&layer);
        assert!(p.contains(&".lens/bin/lns-supervisor"));
        assert!(
            p.contains(&"bin/sh"),
            "imageless+supervised must carry /bin/sh"
        );
        assert!(layer.manifest.label.ends_with("+imageless"));
    }

    #[test]
    fn for_run_oci_unsupervised_ships_guest_tools_tree_without_bin_sh() {
        let d = tempdir();
        let s = store(&d);
        let gt = fake_guest_tools(&d);
        std::fs::write(gt.root.join("marker"), b"x").unwrap();
        let layer = for_run(false, &s, &gt, None, Vec::new()).unwrap().unwrap();
        let p = paths(&layer);
        assert!(p.contains(&"marker"));
        assert!(!p.contains(&"bin/sh"));
        assert!(!p.contains(&".lens/bin/supervisor.real"));
        assert!(!layer.manifest.label.contains("imageless"));
    }

    #[test]
    fn for_run_imageless_unsupervised_adds_bin_sh_and_suffixes_label() {
        let d = tempdir();
        let s = store(&d);
        let gt = fake_guest_tools(&d);
        let layer = for_run(true, &s, &gt, None, Vec::new()).unwrap().unwrap();
        let p = paths(&layer);
        assert!(p.contains(&"bin/sh"));
        assert!(!p.contains(&".lens/bin/supervisor.real"));
        assert!(layer.manifest.label.ends_with("+imageless"));
    }

    #[test]
    fn for_run_injects_extra_mount_specs_into_the_runtime_layer() {
        let d = tempdir();
        let s = store(&d);
        let gt = fake_guest_tools(&d);
        let extra = vec![RuntimeFileSpec {
            guest_path: "/etc/agent/model".into(),
            mode: 0o644,
            source: RuntimeSource::Bytes(b"{\"provider\":\"anthropic\"}".to_vec()),
        }];
        let layer = for_run(false, &s, &gt, None, extra).unwrap().unwrap();
        assert!(paths(&layer).contains(&"etc/agent/model"));
    }
}
