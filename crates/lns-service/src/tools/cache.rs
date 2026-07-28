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
    pub resolved: lns_artifact::tools::SafeVersion,
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_host: Option<String>,
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

pub fn guest_root(tool: &str, resolved: &lns_artifact::tools::SafeVersion) -> String {
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
                let file = open_staged_tar(path)?;
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
}

/// The provisioner guest runs as root on a writable bind of this directory, so between the driver's `tar -cf` and this read it can replace the tar with a link to any host path — or a fifo that would block the service. Only a regular file opened without following links is the tar we asked for.
fn open_staged_tar(path: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("opening staged tool tar {}", path.display()))?;
    let kind = file
        .metadata()
        .with_context(|| format!("inspecting staged tool tar {}", path.display()))?
        .file_type();
    anyhow::ensure!(
        kind.is_file(),
        "staged tool tar {} is not a regular file",
        path.display()
    );
    Ok(file)
}

/// Expand a staged tool tar into content-store entries. Fail-closed like fileset ingestion — escaping paths and exotic entry types are refused — but tool trees legitimately carry symlinks (`bin/node -> ../lib/...`) and hardlinks (JDK layouts), so those are preserved as manifest entries.
fn ingest_tar_entries<R: Read>(tar: R, store: &ContentStore) -> Result<Vec<ManifestEntry>> {
    let mut entries = Vec::new();
    let mut by_path: std::collections::HashMap<String, EntryKind> =
        std::collections::HashMap::new();
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
            if !target_stays_in_tree(&rel, &target) {
                bail!(
                    "tool symlink {} -> {} escapes the tool tree",
                    path.display(),
                    target.display()
                );
            }
            entries.push(ManifestEntry {
                path: rel,
                mode,
                kind: EntryKind::Symlink {
                    target: target.to_string_lossy().into_owned(),
                },
            });
            continue;
        }
        // tar names the second and later copies of a multiply-linked file as a link to the first; the content store already holds those bytes, so the entry resolves to the same digest.
        if entry_type.is_hard_link() {
            let target = entry
                .link_name()
                .context("reading hardlink target")?
                .with_context(|| format!("tool hardlink {} has no target", path.display()))?;
            let kind = by_path
                .get(&normalized(&target))
                .cloned()
                .with_context(|| {
                    format!(
                        "tool hardlink {} points at {}, which is not in the tree",
                        path.display(),
                        target.display()
                    )
                })?;
            entries.push(ManifestEntry {
                path: rel,
                mode,
                kind,
            });
            continue;
        }
        if !entry_type.is_file() {
            bail!(
                "tool entry {} is neither a regular file, a directory, a symlink, nor a hardlink",
                path.display()
            );
        }
        let installed = store
            .install_from_reader(&mut entry)
            .with_context(|| format!("installing tool entry {}", path.display()))?;
        let kind = EntryKind::Regular {
            digest: installed.digest,
            size: installed.size,
        };
        by_path.insert(rel.clone(), kind.clone());
        entries.push(ManifestEntry {
            path: rel,
            mode,
            kind,
        });
    }
    Ok(entries)
}

/// Injection resolves a path's parent segments *through* symlinks, so a link out of the tree makes every entry "under" it land wherever the link points — over the supervisor's own files, which are injected first. Only a target that still resolves inside the tool's own root may be preserved.
fn target_stays_in_tree(link_rel: &str, target: &Path) -> bool {
    let mut resolved: Vec<&str> = match link_rel.rsplit_once('/') {
        Some((parent, _)) => parent.split('/').collect(),
        None => Vec::new(),
    };
    for component in target.components() {
        match component {
            Component::Normal(seg) => resolved.push(seg.to_str().unwrap_or_default()),
            Component::CurDir => {}
            Component::ParentDir => {
                if resolved.pop().is_none() {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy().trim_start_matches("./").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Arch, Libc, StagedTar, StagedTool};

    fn version(literal: &str) -> lns_artifact::tools::SafeVersion {
        literal.parse().expect("a usable version")
    }

    fn key() -> ToolCacheKey {
        ToolCacheKey {
            name: "some-tool".into(),
            resolved: version("1.2.3"),
            arch: Arch::Aarch64,
            libc: Libc::Musl,
        }
    }

    fn staged(tar: Vec<u8>) -> StagedTool {
        StagedTool {
            name: "some-tool".into(),
            resolved: version("1.2.3"),
            backend: "core:some-tool".into(),
            source_host: Some("upstream.example.test".into()),
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

    fn tree_dir(root: &Path) -> PathBuf {
        root.join("trees")
            .join(&key().name)
            .join(key().resolved.as_str())
            .join(format!("{}-{}", key().arch, key().libc))
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
    fn lookup_misses_before_ingest_and_after_the_tree_is_removed() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = cache(dir.path());
        assert_eq!(cache.lookup(&key()).unwrap(), None);
        cache.ingest(&key(), &staged(tool_tar())).unwrap();
        std::fs::remove_dir_all(tree_dir(dir.path())).unwrap();
        assert_eq!(cache.lookup(&key()).unwrap(), None);
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
    fn a_symlink_that_leaves_the_tool_tree_is_refused() {
        // A link out plus a regular file "under" it lands wherever the link points: injection resolves parent segments through symlinks, and tool specs are appended after the supervisor's.
        for (link, target) in [
            ("esc", "/.lens/bin"),
            ("esc", ".."),
            ("esc", "../../../../bin"),
            ("bin/esc", "../../../.."),
            // tar spells paths relative as `./x`, and a `.` segment must not buy a level back.
            ("./bin/esc", "./../.."),
        ] {
            let mut builder = tar::Builder::new(Vec::new());
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_path(link).unwrap();
            header.set_link_name(target).unwrap();
            header.set_size(0);
            header.set_mode(0o777);
            header.set_cksum();
            builder.append(&header, std::io::empty()).unwrap();
            let tar = builder.into_inner().unwrap();

            let dir = tempfile::TempDir::new().unwrap();
            let err = cache(dir.path())
                .ingest(&key(), &staged(tar))
                .expect_err("a tool tree must not reach outside itself");
            assert!(
                format!("{err:#}").contains("escapes the tool tree"),
                "{link} -> {target}: got {err:#}"
            );
        }
    }

    #[test]
    fn a_symlink_inside_the_tool_tree_is_preserved() {
        // Real trees rely on these: node ships bin/node -> ../lib/node_modules/... and friends.
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_path("./bin/node").unwrap();
        header.set_link_name("./../lib/node").unwrap();
        header.set_size(0);
        header.set_mode(0o777);
        header.set_cksum();
        builder.append(&header, std::io::empty()).unwrap();
        let tar = builder.into_inner().unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let manifest = cache(dir.path()).ingest(&key(), &staged(tar)).unwrap();
        assert_eq!(
            manifest.entries[0].kind,
            EntryKind::Symlink {
                target: "./../lib/node".into()
            }
        );
    }

    #[test]
    fn a_hardlinked_file_resolves_to_the_digest_already_ingested() {
        // GNU and busybox tar both name the second copy of a multiply-linked file as a type-1 link; JDK trees ship them.
        let mut builder = tar::Builder::new(Vec::new());
        let payload = b"elf";
        let mut first = tar::Header::new_gnu();
        first.set_path("bin/java").unwrap();
        first.set_size(payload.len() as u64);
        first.set_mode(0o755);
        first.set_cksum();
        builder.append(&first, &payload[..]).unwrap();
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Link);
        link.set_path("bin/javac").unwrap();
        link.set_link_name("bin/java").unwrap();
        link.set_size(0);
        link.set_mode(0o755);
        link.set_cksum();
        builder.append(&link, std::io::empty()).unwrap();
        let tar = builder.into_inner().unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let cache = cache(dir.path());
        let manifest = cache.ingest(&key(), &staged(tar)).unwrap();
        assert_eq!(manifest.entries.len(), 2);
        assert_eq!(
            manifest.entries[0].kind, manifest.entries[1].kind,
            "the link carries the same content as the file it points at"
        );
        assert_eq!(manifest.entries[1].path, "bin/javac");
    }

    #[test]
    fn a_hardlink_to_something_outside_the_tree_is_refused_by_name() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Link);
        link.set_path("bin/javac").unwrap();
        link.set_link_name("/etc/shadow").unwrap();
        link.set_size(0);
        link.set_mode(0o755);
        link.set_cksum();
        builder.append(&link, std::io::empty()).unwrap();
        let tar = builder.into_inner().unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let err = cache(dir.path()).ingest(&key(), &staged(tar)).unwrap_err();
        assert!(
            format!("{err:#}").contains("not in the tree"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_staged_tar_swapped_for_a_symlink_is_refused_instead_of_read() {
        // The provisioner guest is root on a writable bind of the staging dir and can win the race between its tar and this read.
        let dir = tempfile::TempDir::new().unwrap();
        let secret = dir.path().join("id_ed25519");
        std::fs::write(&secret, b"PRIVATE KEY").unwrap();
        let staged_path = dir.path().join("node.tar");
        std::os::unix::fs::symlink(&secret, &staged_path).unwrap();

        let err = cache(dir.path())
            .ingest(
                &key(),
                &StagedTool {
                    name: "some-tool".into(),
                    resolved: version("1.2.3"),
                    backend: "core:some-tool".into(),
                    source_host: Some("upstream.example.test".into()),
                    tar: StagedTar::File(staged_path),
                    bin_paths: vec!["bin".into()],
                },
            )
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("opening staged tool tar"),
            "the host never follows the link: {err:#}"
        );
    }

    #[test]
    fn a_staged_tar_swapped_for_a_fifo_is_refused_instead_of_blocking_the_service() {
        // O_NONBLOCK means the open succeeds on a fifo, so the file type is what has to refuse it.
        let dir = tempfile::TempDir::new().unwrap();
        let staged_path = dir.path().join("node.tar");
        let c_path = std::ffi::CString::new(staged_path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: c_path is a valid NUL-terminated path that lives across the call.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o644) }, 0);

        let err = cache(dir.path())
            .ingest(
                &key(),
                &StagedTool {
                    name: "some-tool".into(),
                    resolved: version("1.2.3"),
                    backend: "core:some-tool".into(),
                    source_host: Some("upstream.example.test".into()),
                    tar: StagedTar::File(staged_path),
                    bin_paths: vec!["bin".into()],
                },
            )
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("is not a regular file"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_manifest_naming_an_unusable_version_is_a_miss() {
        // manifest.resolved becomes the guest path the tool is injected at, so an edited file must not be readable.
        let dir = tempfile::TempDir::new().unwrap();
        let cache = cache(dir.path());
        cache.ingest(&key(), &staged(tool_tar())).unwrap();
        let path = dir
            .path()
            .join("trees")
            .join(&key().name)
            .join(key().resolved.as_str())
            .join(format!("{}-{}", key().arch, key().libc))
            .join("manifest.json");
        let mut raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        raw["resolved"] = serde_json::Value::String("../../..".into());
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        assert_eq!(cache.lookup(&key()).unwrap(), None);
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
