use std::io::Read;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::{StagedTar, StagedTool, ToolCacheKey};
use crate::content_store::ContentStore;
use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// A provisioned tool tree, ingested into the content store and described entry-by-entry so injection composes specs without re-reading any file body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolManifest {
    pub schema_version: u32,
    pub tool: String,
    pub resolved: String,
    pub backend: String,
    pub source_host: String,
    /// Debugging metadata only — never part of the cache key.
    pub engine_version: String,
    pub entries: Vec<ManifestEntry>,
    pub bin_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub mode: u32,
    #[serde(flatten)]
    pub kind: EntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EntryKind {
    Regular { digest: String, size: u64 },
    Symlink { target: String },
}

pub fn guest_root(tool: &str, resolved: &str) -> String {
    format!("/.lens/tools/{tool}/{resolved}")
}

impl ToolManifest {
    pub fn runtime_specs(&self) -> Result<Vec<RuntimeFileSpec>> {
        let root = guest_root(&self.tool, &self.resolved);
        self.entries
            .iter()
            .map(|entry| {
                let source = match &entry.kind {
                    EntryKind::Regular { digest, size } => RuntimeSource::Content {
                        digest: digest.clone(),
                        raw_digest: decode_sha256(digest)?,
                        size: *size,
                    },
                    EntryKind::Symlink { target } => RuntimeSource::Symlink(target.clone()),
                };
                Ok(RuntimeFileSpec {
                    guest_path: format!("{root}/{}", entry.path),
                    mode: entry.mode,
                    source,
                })
            })
            .collect()
    }

    pub fn guest_bin_paths(&self) -> Vec<String> {
        let root = guest_root(&self.tool, &self.resolved);
        self.bin_paths
            .iter()
            .map(|bin| {
                if bin.is_empty() || bin == "." {
                    root.clone()
                } else {
                    format!("{root}/{bin}")
                }
            })
            .collect()
    }
}

fn decode_sha256(digest: &str) -> Result<[u8; 32]> {
    let hex = digest
        .strip_prefix("sha256:")
        .with_context(|| format!("digest {digest} is not sha256-prefixed"))?;
    if hex.len() != 64 {
        bail!("digest {digest} is not 64 hex characters");
    }
    let mut raw = [0u8; 32];
    for (i, byte) in raw.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .with_context(|| format!("digest {digest} carries a non-hex character"))?;
    }
    Ok(raw)
}

pub trait ToolCache {
    fn lookup(&self, key: &ToolCacheKey) -> Result<Option<ToolManifest>>;
    fn ingest(&self, key: &ToolCacheKey, staged: &StagedTool) -> Result<ToolManifest>;
    fn evict(&self, key: &ToolCacheKey) -> Result<()>;
}

pub struct RealToolCache {
    trees_root: PathBuf,
    store: ContentStore,
    engine_version: String,
}

impl RealToolCache {
    pub fn new(tools_root: &Path, store: ContentStore, engine_version: &str) -> Self {
        Self {
            trees_root: tools_root.join("trees"),
            store,
            engine_version: engine_version.to_string(),
        }
    }

    fn key_dir(&self, key: &ToolCacheKey) -> PathBuf {
        self.trees_root
            .join(&key.name)
            .join(&key.resolved)
            .join(format!("{}-{}", key.arch, key.libc))
    }
}

impl ToolCache for RealToolCache {
    fn lookup(&self, key: &ToolCacheKey) -> Result<Option<ToolManifest>> {
        let dir = self.key_dir(key);
        if !dir.join(".ready").exists() {
            return Ok(None);
        }
        let bytes = match std::fs::read(dir.join("manifest.json")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        let manifest: ToolManifest = match serde_json::from_slice(&bytes) {
            Ok(manifest) => manifest,
            Err(_) => return Ok(None),
        };
        // A tree written under different manifest semantics is a miss, not something to reinterpret under today's.
        if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
            return Ok(None);
        }
        for entry in &manifest.entries {
            if let EntryKind::Regular { digest, .. } = &entry.kind
                && !self.store.contains(digest)?
            {
                return Ok(None);
            }
        }
        Ok(Some(manifest))
    }

    fn ingest(&self, key: &ToolCacheKey, staged: &StagedTool) -> Result<ToolManifest> {
        let entries = match &staged.tar {
            StagedTar::File(path) => {
                let file = std::fs::File::open(path)
                    .with_context(|| format!("opening staged tool tar {}", path.display()))?;
                ingest_tar_entries(file, &self.store)?
            }
            StagedTar::Bytes(bytes) => ingest_tar_entries(&bytes[..], &self.store)?,
        };
        let manifest = ToolManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            tool: key.name.clone(),
            resolved: key.resolved.clone(),
            backend: staged.backend.clone(),
            source_host: staged.source_host.clone(),
            engine_version: self.engine_version.clone(),
            entries,
            bin_paths: staged.bin_paths.clone(),
        };
        let dir = self.key_dir(key);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let tmp = dir.join("manifest.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&manifest)?)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, dir.join("manifest.json"))
            .with_context(|| format!("installing the manifest into {}", dir.display()))?;
        std::fs::write(dir.join(".ready"), b"")
            .with_context(|| format!("marking {} ready", dir.display()))?;
        Ok(manifest)
    }

    fn evict(&self, key: &ToolCacheKey) -> Result<()> {
        match std::fs::remove_dir_all(self.key_dir(key)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("evicting {:?}", key.name)),
        }
    }
}

/// Expand a staged tool tar into content-store entries. Fail-closed like fileset ingestion — escaping paths and exotic entry types are refused — but tool trees legitimately carry symlinks (`bin/node -> ../lib/...`), so those are preserved as manifest entries.
fn ingest_tar_entries<R: Read>(tar: R, store: &ContentStore) -> Result<Vec<ManifestEntry>> {
    let mut entries = Vec::new();
    let mut archive = tar::Archive::new(tar);
    for entry in archive.entries().context("reading staged tool tar")? {
        let mut entry = entry.context("reading staged tool entry")?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            continue;
        }
        let path = entry
            .path()
            .context("reading staged tool entry path")?
            .into_owned();
        if path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
        {
            bail!("tool entry {} escapes the tool tree", path.display());
        }
        if path.to_string_lossy().chars().any(char::is_control) {
            bail!("tool entry {path:?} must not contain control characters");
        }
        let rel = normalized(&path);
        let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
        if entry_type.is_symlink() {
            let target = entry
                .link_name()
                .context("reading symlink target")?
                .with_context(|| format!("tool symlink {} has no target", path.display()))?;
            entries.push(ManifestEntry {
                path: rel,
                mode,
                kind: EntryKind::Symlink {
                    target: target.to_string_lossy().into_owned(),
                },
            });
            continue;
        }
        if !entry_type.is_file() {
            bail!(
                "tool entry {} is neither a regular file, a directory, nor a symlink",
                path.display()
            );
        }
        let installed = store
            .install_from_reader(&mut entry)
            .with_context(|| format!("installing tool entry {}", path.display()))?;
        entries.push(ManifestEntry {
            path: rel,
            mode,
            kind: EntryKind::Regular {
                digest: installed.digest,
                size: installed.size,
            },
        });
    }
    Ok(entries)
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy().trim_start_matches("./").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Arch, Libc, StagedTar, StagedTool};

    fn key() -> ToolCacheKey {
        ToolCacheKey {
            name: "some-tool".into(),
            resolved: "1.2.3".into(),
            arch: Arch::Aarch64,
            libc: Libc::Musl,
        }
    }

    fn staged(tar: Vec<u8>) -> StagedTool {
        StagedTool {
            name: "some-tool".into(),
            resolved: "1.2.3".into(),
            backend: "core:some-tool".into(),
            source_host: "upstream.example.test".into(),
            tar: StagedTar::Bytes(tar),
            bin_paths: vec!["bin".into()],
        }
    }

    fn tool_tar() -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_path("bin/alias").unwrap();
        link.set_link_name("some-tool").unwrap();
        link.set_size(0);
        link.set_mode(0o777);
        link.set_cksum();
        builder.append(&link, std::io::empty()).unwrap();
        let body = b"#!binary";
        let mut header = tar::Header::new_gnu();
        header.set_path("./bin/some-tool").unwrap();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append(&header, &body[..]).unwrap();
        let mut dir = tar::Header::new_gnu();
        dir.set_entry_type(tar::EntryType::Directory);
        dir.set_path("lib/").unwrap();
        dir.set_size(0);
        dir.set_mode(0o755);
        dir.set_cksum();
        builder.append(&dir, std::io::empty()).unwrap();
        builder.into_inner().unwrap()
    }

    fn cache(dir: &Path) -> RealToolCache {
        RealToolCache::new(dir, ContentStore::new(dir.join("content")), "2026.7.14")
    }

    #[test]
    fn ingest_then_lookup_round_trips_the_manifest() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = cache(dir.path());
        let manifest = cache.ingest(&key(), &staged(tool_tar())).unwrap();
        assert_eq!(cache.lookup(&key()).unwrap(), Some(manifest.clone()));
        assert_eq!(manifest.entries.len(), 2, "dirs are skipped");
        assert_eq!(manifest.engine_version, "2026.7.14");
    }

    #[test]
    fn lookup_misses_before_ingest_and_after_evict() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = cache(dir.path());
        assert_eq!(cache.lookup(&key()).unwrap(), None);
        cache.ingest(&key(), &staged(tool_tar())).unwrap();
        cache.evict(&key()).unwrap();
        assert_eq!(cache.lookup(&key()).unwrap(), None);
        cache.evict(&key()).unwrap();
    }

    #[test]
    fn a_ready_marker_without_a_readable_manifest_is_a_miss_or_an_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = cache(dir.path());
        let key_dir = dir.path().join("trees/some-tool/1.2.3").join(format!(
            "{}-{}",
            Arch::Aarch64,
            Libc::Musl
        ));
        std::fs::create_dir_all(&key_dir).unwrap();
        std::fs::write(key_dir.join(".ready"), b"").unwrap();
        assert_eq!(cache.lookup(&key()).unwrap(), None, "missing manifest");
        std::fs::write(key_dir.join("manifest.json"), b"not json").unwrap();
        assert_eq!(cache.lookup(&key()).unwrap(), None, "corrupt manifest");
        std::fs::remove_file(key_dir.join("manifest.json")).unwrap();
        std::fs::create_dir_all(key_dir.join("manifest.json")).unwrap();
        assert!(cache.lookup(&key()).is_err(), "unreadable manifest");
    }

    #[test]
    fn a_staged_tar_file_on_disk_ingests_like_bytes() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = cache(dir.path());
        let tar_path = dir.path().join("staged.tar");
        std::fs::write(&tar_path, tool_tar()).unwrap();
        let mut from_file = staged(Vec::new());
        from_file.tar = StagedTar::File(tar_path);
        let manifest = cache.ingest(&key(), &from_file).unwrap();
        assert_eq!(manifest.entries.len(), 2);
    }

    #[test]
    fn evicting_an_unremovable_key_surfaces_the_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = cache(dir.path());
        let key_dir = dir.path().join("trees/some-tool/1.2.3").join(format!(
            "{}-{}",
            Arch::Aarch64,
            Libc::Musl
        ));
        std::fs::create_dir_all(key_dir.parent().unwrap()).unwrap();
        std::fs::write(&key_dir, b"a file where the tree dir belongs").unwrap();
        assert!(cache.evict(&key()).is_err());
    }

    #[test]
    fn a_control_character_in_a_tar_path_is_refused() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_path("a").unwrap();
        {
            let name = header.as_gnu_mut().unwrap();
            name.name[..4].copy_from_slice(b"a\x07bc");
            name.name[4] = 0;
        }
        header.set_size(1);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &b"x"[..]).unwrap();
        let tar = builder.into_inner().unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let err = cache(dir.path()).ingest(&key(), &staged(tar)).unwrap_err();
        assert!(
            format!("{err:#}").contains("control characters"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_tree_written_under_another_manifest_schema_is_a_miss() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = cache(dir.path());
        let mut manifest = cache.ingest(&key(), &staged(tool_tar())).unwrap();
        manifest.schema_version = MANIFEST_SCHEMA_VERSION + 1;
        let path = dir
            .path()
            .join("trees")
            .join(&key().name)
            .join(&key().resolved)
            .join(format!("{}-{}", key().arch, key().libc))
            .join("manifest.json");
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert_eq!(
            cache.lookup(&key()).unwrap(),
            None,
            "the tree is re-provisioned rather than read under today's semantics"
        );
    }

    #[test]
    fn a_swept_content_blob_turns_the_lookup_into_a_miss() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = cache(dir.path());
        let manifest = cache.ingest(&key(), &staged(tool_tar())).unwrap();
        let digest = manifest
            .entries
            .iter()
            .find_map(|entry| match &entry.kind {
                EntryKind::Regular { digest, .. } => Some(digest.clone()),
                EntryKind::Symlink { .. } => None,
            })
            .unwrap();
        std::fs::remove_file(cache.store.path_for(&digest).unwrap()).unwrap();
        assert_eq!(cache.lookup(&key()).unwrap(), None);
    }

    #[test]
    fn an_escaping_tar_path_is_refused() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_path("ok").unwrap();
        // no direct API writes `..`; craft the header path bytes by hand
        let mut escaping = tar::Header::new_gnu();
        escaping.set_path("a").unwrap();
        {
            let name = escaping.as_gnu_mut().unwrap();
            name.name[..9].copy_from_slice(b"../escape");
            name.name[9] = 0;
        }
        escaping.set_size(1);
        escaping.set_mode(0o644);
        escaping.set_cksum();
        builder.append(&escaping, &b"x"[..]).unwrap();
        let _ = header;
        let tar = builder.into_inner().unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let err = cache(dir.path()).ingest(&key(), &staged(tar)).unwrap_err();
        assert!(
            format!("{err:#}").contains("escapes the tool tree"),
            "got: {err:#}"
        );
    }

    #[test]
    fn an_exotic_entry_type_is_refused() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut fifo = tar::Header::new_gnu();
        fifo.set_entry_type(tar::EntryType::Fifo);
        fifo.set_path("pipe").unwrap();
        fifo.set_size(0);
        fifo.set_mode(0o644);
        fifo.set_cksum();
        builder.append(&fifo, std::io::empty()).unwrap();
        let tar = builder.into_inner().unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let err = cache(dir.path()).ingest(&key(), &staged(tar)).unwrap_err();
        assert!(
            format!("{err:#}").contains("neither a regular file"),
            "got: {err:#}"
        );
    }

    #[test]
    fn runtime_specs_land_under_the_reserved_tool_root_with_modes_and_symlinks() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = cache(dir.path());
        let manifest = cache.ingest(&key(), &staged(tool_tar())).unwrap();
        let specs = manifest.runtime_specs().unwrap();
        let file = specs
            .iter()
            .find(|spec| spec.guest_path == "/.lens/tools/some-tool/1.2.3/bin/some-tool")
            .expect("the regular file spec");
        assert_eq!(file.mode, 0o755);
        assert!(matches!(&file.source, RuntimeSource::Content { size, .. } if *size == 8));
        let link = specs
            .iter()
            .find(|spec| spec.guest_path == "/.lens/tools/some-tool/1.2.3/bin/alias")
            .expect("the symlink spec");
        assert!(matches!(&link.source, RuntimeSource::Symlink(target) if target == "some-tool"));
        assert_eq!(
            manifest.guest_bin_paths(),
            vec!["/.lens/tools/some-tool/1.2.3/bin".to_string()]
        );
    }

    #[test]
    fn a_rootless_bin_path_maps_to_the_tool_root_itself() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = cache(dir.path());
        let mut bare = staged(tool_tar());
        bare.bin_paths = vec![".".into()];
        let manifest = cache.ingest(&key(), &bare).unwrap();
        assert_eq!(
            manifest.guest_bin_paths(),
            vec!["/.lens/tools/some-tool/1.2.3".to_string()]
        );
    }

    #[test]
    fn decode_sha256_rejects_malformed_digests() {
        assert!(decode_sha256("md5:abc").is_err());
        assert!(decode_sha256(&format!("sha256:{}", "a".repeat(63))).is_err());
        assert!(decode_sha256(&format!("sha256:{}z", "a".repeat(63))).is_err());
        assert_eq!(
            decode_sha256(&format!("sha256:{}", "0".repeat(64))).unwrap(),
            [0u8; 32]
        );
    }
}
