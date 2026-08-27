use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use lns_artifact::sandbox::FilesetOwner;
use oci_client::manifest::OciImageManifest;

use crate::artifact::assembly::{HostFileset, InlineFileset, LocalFileset};
use crate::content_store::ContentStore;
use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};

pub const OWNED_MANIFEST_PATH: &str = "/.lens/fileset-owned";

/// Where a guest write a volume mount would cover is staged instead, for lns-init to copy onto its final path once the volume is mounted.
pub const DEFERRED_ROOT: &str = "/.lens/fileset-deferred";

/// A volume mounts after the runtime layer is in place, so a write under its target is staged here for lns-init to copy in once the volume is mounted.
pub fn stage_what_a_volume_would_hide(
    specs: &mut [RuntimeFileSpec],
    volumes: &[lns_ipc::VolumeMount],
) {
    for spec in specs {
        if volumes
            .iter()
            .filter(|volume| !volume.read_only)
            .any(|volume| lns_artifact::sandbox::encloses(&volume.target, &spec.guest_path))
        {
            spec.guest_path = format!("{DEFERRED_ROOT}{}", spec.guest_path);
        }
    }
}

/// A read-only volume takes no write and a bind would leave the file in the host directory it shares, so a run whose write lands under either is refused rather than started without it.
pub fn refuse_writes_a_mount_would_hide(
    specs: &[RuntimeFileSpec],
    volumes: &[lns_ipc::VolumeMount],
    binds: &[lns_ipc::BindMount],
) -> Result<()> {
    let covering = volumes
        .iter()
        .filter(|volume| volume.read_only)
        .map(|volume| ("read-only volume", volume.target.as_str()))
        .chain(binds.iter().map(|bind| ("bind", bind.target.as_str())));
    for (kind, target) in covering {
        let hidden = specs
            .iter()
            .find(|spec| lns_artifact::sandbox::encloses(target, &spec.guest_path));
        if let Some(hidden) = hidden {
            bail!(
                "the fileset at {} lands under the {kind} mounted at {target}, which takes no write from a fileset; the workload would never see the file",
                hidden.guest_path
            );
        }
    }
    Ok(())
}

pub(crate) struct FilesetBudget {
    bytes: u64,
    entries: usize,
    max_bytes: u64,
    max_entries: usize,
}

impl FilesetBudget {
    pub(crate) fn new() -> Self {
        Self::with_limits(
            lns_artifact::build::MAX_FILESET_BYTES,
            lns_artifact::build::MAX_FILESET_ENTRIES,
        )
    }

    fn with_limits(max_bytes: u64, max_entries: usize) -> Self {
        Self {
            bytes: 0,
            entries: 0,
            max_bytes,
            max_entries,
        }
    }

    fn charge(&mut self, size: u64) -> Result<()> {
        if self.entries >= self.max_entries {
            bail!("fileset contains more than {} entries", self.max_entries);
        }
        self.entries += 1;
        if size > self.max_bytes.saturating_sub(self.bytes) {
            bail!("fileset content exceeds the {}-byte limit", self.max_bytes);
        }
        self.bytes += size;
        Ok(())
    }
}

/// The fileset specs for a run plus the guest paths lns-init must chown to the workload user.
#[derive(Default)]
pub struct MaterializedFilesets {
    pub specs: Vec<RuntimeFileSpec>,
    pub owned_paths: BTreeSet<String>,
}

impl MaterializedFilesets {
    pub fn absorb(&mut self, owner: FilesetOwner, guest_path: &str, specs: Vec<RuntimeFileSpec>) {
        if owner == FilesetOwner::Workload {
            self.owned_paths.extend(owned_paths_for(guest_path, &specs));
        }
        self.specs.extend(specs);
    }

    /// One layer entry naming every workload-owned path, so the guest can chown them after it resolves the run-as ids; no workload-owned filesets, no entry.
    pub fn into_specs(mut self) -> Vec<RuntimeFileSpec> {
        if !self.owned_paths.is_empty() {
            let body: String = self.owned_paths.iter().map(|p| format!("{p}\n")).collect();
            self.specs.push(RuntimeFileSpec {
                guest_path: OWNED_MANIFEST_PATH.into(),
                mode: 0o444,
                source: RuntimeSource::Bytes(body.into_bytes()),
            });
        }
        self.specs
    }
}

/// The guest path, every directory the fileset introduces beneath it, and each shipped file.
fn owned_paths_for(guest_path: &str, specs: &[RuntimeFileSpec]) -> BTreeSet<String> {
    let root = guest_path.trim_end_matches('/');
    let mut owned = BTreeSet::new();
    owned.insert(root.to_string());
    for spec in specs {
        let mut path = spec.guest_path.as_str();
        owned.insert(path.to_string());
        while let Some((parent, _)) = path.rsplit_once('/') {
            if parent.len() <= root.len() {
                break;
            }
            owned.insert(parent.to_string());
            path = parent;
        }
    }
    owned
}

pub struct SnapshotEntry {
    pub name: String,
    pub dir: bool,
    pub mode: u32,
}

/// Directory listing seam for local path-fileset snapshots; `RealSnapshotDir` in `real.rs` is the std::fs leaf.
pub trait SnapshotDir {
    fn entries(&self, dir: &Path) -> std::io::Result<Vec<SnapshotEntry>>;
}

/// Snapshot each local path fileset into host-file guest-write specs, so a local definition's files land in the guest exactly like a published fileset's.
pub fn local_fileset_specs<D: SnapshotDir + ?Sized>(
    dir: &D,
    locals: &[LocalFileset],
    out: &mut MaterializedFilesets,
) -> Result<()> {
    for local in locals {
        let root = local.guest_path.trim_end_matches('/');
        let mut specs = Vec::new();
        snapshot_into(dir, Path::new(&local.source), root, &mut specs)
            .with_context(|| format!("snapshotting fileset {}", local.source))?;
        out.absorb(local.owner, &local.guest_path, specs);
    }
    Ok(())
}

/// What a host path resolves to, symlinks followed, so the seam reports facts and this module decides what they mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostFileFacts {
    pub mode: u32,
    pub is_regular_file: bool,
}

/// Host-file seam for hostPath filesets; `RealSnapshotDir` in `real.rs` is the std::fs leaf.
pub trait HostFileProbe {
    fn home(&self) -> Option<PathBuf>;
    /// `Ok(None)` when nothing resolves at the path, which includes a symlink whose target is gone.
    fn stat(&self, path: &Path) -> std::io::Result<Option<HostFileFacts>>;
}

/// Snapshot each declared host file into one guest-write spec at its guestPath, so a definition can seed the guest from the machine that runs it; an absent path refuses the plan unless the author marked it optional, and a path this machine refused a pulled sandbox is never read at all.
pub fn host_fileset_specs<P: HostFileProbe + ?Sized>(
    probe: &P,
    hosts: &[HostFileset],
    denied: &[String],
    out: &mut MaterializedFilesets,
) -> Result<()> {
    for host in hosts {
        if denied.iter().any(|source| source == &host.source) {
            crate::log::info!(
                "fileset",
                "skipping hostPath {} — this machine did not grant it to this sandbox",
                host.source
            );
            continue;
        }
        let resolved = resolve_host_source(probe, &host.source)?;
        let facts = probe
            .stat(&resolved)
            .with_context(|| format!("hostPath {} ({})", host.source, resolved.display()))?;
        let Some(facts) = host_file_facts(host, &resolved, facts)? else {
            continue;
        };
        let specs = vec![RuntimeFileSpec {
            guest_path: host.guest_path.clone(),
            mode: facts.mode,
            source: RuntimeSource::HostFile(resolved),
        }];
        out.absorb(host.owner, &host.guest_path, specs);
    }
    Ok(())
}

/// `Ok(None)` is the one benign miss: an optional path nothing resolves at, announced on the run's own log so the operator sees why the guest is unseeded.
fn host_file_facts(
    host: &HostFileset,
    resolved: &Path,
    facts: Option<HostFileFacts>,
) -> Result<Option<HostFileFacts>> {
    let Some(facts) = facts else {
        if !host.optional {
            bail!(
                "hostPath {} is not present on this host ({}); create it or declare the fileset optional",
                host.source,
                resolved.display()
            );
        }
        crate::log::info!(
            "fileset",
            "skipping optional hostPath {} — not present on this host",
            host.source
        );
        return Ok(None);
    };
    if !facts.is_regular_file {
        bail!(
            "hostPath {} ({}) is not a regular file; a hostPath fileset seeds the guest with one file",
            host.source,
            resolved.display()
        );
    }
    Ok(Some(facts))
}

fn resolve_host_source<P: HostFileProbe + ?Sized>(probe: &P, source: &str) -> Result<PathBuf> {
    let Some(under_home) = source.strip_prefix("~/") else {
        return Ok(PathBuf::from(source));
    };
    let home = probe.home().with_context(|| {
        format!("cannot resolve hostPath {source}: this machine has no home directory")
    })?;
    Ok(home.join(under_home))
}

pub fn inline_fileset_specs(inline_filesets: &[InlineFileset], out: &mut MaterializedFilesets) {
    for inline in inline_filesets {
        let root = inline.guest_path.trim_end_matches('/');
        let specs = inline
            .files
            .iter()
            .map(|(path, content)| RuntimeFileSpec {
                guest_path: format!("{root}/{path}"),
                mode: 0o644,
                source: RuntimeSource::Bytes(content.as_bytes().to_vec()),
            })
            .collect();
        out.absorb(inline.owner, &inline.guest_path, specs);
    }
}

fn snapshot_into<D: SnapshotDir + ?Sized>(
    dir: &D,
    host_dir: &Path,
    guest_dir: &str,
    out: &mut Vec<RuntimeFileSpec>,
) -> Result<()> {
    let listed = dir
        .entries(host_dir)
        .with_context(|| format!("reading {}", host_dir.display()))?;
    for entry in listed {
        let host_path = host_dir.join(&entry.name);
        let guest_path = format!("{guest_dir}/{}", entry.name);
        if entry.dir {
            snapshot_into(dir, &host_path, &guest_path, out)?;
        } else {
            out.push(RuntimeFileSpec {
                guest_path,
                mode: entry.mode,
                source: RuntimeSource::HostFile(host_path),
            });
        }
    }
    Ok(())
}

/// Expand a packed fileset's tar into guest-write specs rooted at `guest_path`, so the files the artifact shipped land in the guest at boot. Fail-closed: an entry whose path escapes the mount (absolute or `..`) or isn't a regular file is refused, so a hand-built or tampered layer can't write outside its declared mount.
pub fn fileset_runtime_specs<R: Read>(
    guest_path: &str,
    layer_tar: R,
    content_store: &ContentStore,
) -> Result<Vec<RuntimeFileSpec>> {
    fileset_runtime_specs_with_budget(
        guest_path,
        layer_tar,
        content_store,
        &mut FilesetBudget::new(),
    )
}

pub(crate) fn fileset_runtime_specs_with_budget<R: Read>(
    guest_path: &str,
    layer_tar: R,
    content_store: &ContentStore,
    budget: &mut FilesetBudget,
) -> Result<Vec<RuntimeFileSpec>> {
    let root = guest_path.trim_end_matches('/');
    let mut specs = Vec::new();
    let mut archive = tar::Archive::new(layer_tar);
    for entry in archive.entries().context("reading fileset layer")? {
        let mut entry = entry.context("reading fileset entry")?;
        let entry_type = entry.header().entry_type();
        let size = entry
            .header()
            .size()
            .context("reading fileset entry size")?;
        budget.charge(size)?;
        if entry_type.is_dir() {
            continue;
        }
        let path = entry
            .path()
            .context("reading fileset entry path")?
            .into_owned();
        if path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
        {
            bail!("fileset entry {} escapes its guest path", path.display());
        }
        if path.to_string_lossy().chars().any(char::is_control) {
            bail!("fileset entry {path:?} must not contain control characters");
        }
        if !entry_type.is_file() {
            bail!(
                "fileset entry {} is not a regular file (filesets carry only regular files)",
                path.display()
            );
        }
        let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
        let installed = content_store
            .install_from_reader(&mut entry)
            .with_context(|| format!("installing fileset entry {}", path.display()))?;
        specs.push(RuntimeFileSpec {
            guest_path: format!("{root}/{}", path.to_string_lossy()),
            mode,
            source: RuntimeSource::Content {
                digest: installed.digest,
                raw_digest: installed.raw_digest,
                size: installed.size,
            },
        });
    }
    Ok(specs)
}

/// The packed fileset layers a pulled manifest carries, in manifest order — the only layer media type an artifact's fileset travels in, so anything else is not one and is left out rather than guessed at.
pub(crate) fn packed_layers(manifest: &OciImageManifest) -> Vec<crate::artifact::PackedLayer> {
    manifest
        .layers
        .iter()
        .filter(|layer| layer.media_type == lns_artifact::build::FILESET_LAYER_MEDIA_TYPE)
        .filter_map(|layer| {
            Some(crate::artifact::PackedLayer {
                digest: layer.digest.clone(),
                size: u64::try_from(layer.size).ok()?,
            })
        })
        .collect()
}

/// Refuse a declared layer bigger than the byte ceiling before it is downloaded.
pub(crate) fn validate_packed_layer_size(
    layer: &crate::artifact::PackedLayer,
    max_bytes: u64,
) -> Result<()> {
    if layer.size > max_bytes {
        bail!(
            "fileset layer {} exceeds the {max_bytes}-byte limit",
            layer.digest
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oci_client::manifest::OciDescriptor;
    use sha2::{Digest, Sha256};

    fn spec_at(guest_path: &str) -> RuntimeFileSpec {
        RuntimeFileSpec {
            guest_path: guest_path.to_string(),
            mode: 0o644,
            source: RuntimeSource::Bytes(b"x".to_vec()),
        }
    }

    fn volume_at(target: &str, read_only: bool) -> lns_ipc::VolumeMount {
        lns_ipc::VolumeMount {
            name: "home".to_string(),
            target: target.to_string(),
            read_only,
            size_bytes: None,
        }
    }

    #[test]
    fn staging_leaves_a_write_no_volume_target_encloses_where_it_was_written() {
        let mut specs = vec![
            spec_at("/home/node/.config/tool.md"),
            spec_at("/home/nodejs/tool.md"),
            spec_at(OWNED_MANIFEST_PATH),
        ];
        stage_what_a_volume_would_hide(&mut specs, &[volume_at("/home/node", false)]);
        let paths: Vec<&str> = specs.iter().map(|s| s.guest_path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "/.lens/fileset-deferred/home/node/.config/tool.md",
                "/home/nodejs/tool.md",
                OWNED_MANIFEST_PATH,
            ]
        );
    }

    #[test]
    fn a_write_at_the_volume_target_itself_is_the_mount_point_and_is_left_alone() {
        let mut specs = vec![spec_at("/home/node")];
        stage_what_a_volume_would_hide(&mut specs, &[volume_at("/home/node", false)]);
        assert_eq!(specs[0].guest_path, "/home/node");
    }

    #[test]
    fn a_write_under_a_read_only_volume_is_left_where_it_was_written_because_no_copy_can_land() {
        let mut specs = vec![spec_at("/home/node/tool.md")];
        stage_what_a_volume_would_hide(&mut specs, &[volume_at("/home/node", true)]);
        assert_eq!(specs[0].guest_path, "/home/node/tool.md");
    }

    fn bind_at(target: &str) -> lns_ipc::BindMount {
        lns_ipc::BindMount {
            host_source: "/Users/dev/project".to_string(),
            target: target.to_string(),
            read_only: false,
            kept_paths: Vec::new(),
            dropped_paths: Vec::new(),
        }
    }

    #[test]
    fn a_read_only_mount_the_flags_added_refuses_the_run_the_document_could_not_refuse() {
        let specs = [spec_at("/home/node/.config/tool.md")];
        let err = refuse_writes_a_mount_would_hide(&specs, &[volume_at("/home/node", true)], &[])
            .unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("/home/node/.config/tool.md") && message.contains("/home/node"),
            "the refusal has to name the write and the mount: {message}"
        );
        assert!(message.contains("read-only volume"), "got: {message}");
    }

    #[test]
    fn a_bind_the_flags_added_refuses_the_run_rather_than_seeding_the_host_directory() {
        let specs = [spec_at("/work/.agent/settings.json")];
        let err = refuse_writes_a_mount_would_hide(&specs, &[], &[bind_at("/work")]).unwrap_err();
        assert!(format!("{err:#}").contains("bind"), "got: {err:#}");
    }

    #[test]
    fn a_writable_named_volume_refuses_nothing_because_the_staged_copy_lands_in_it() {
        let specs = [spec_at("/home/node/.config/tool.md")];
        refuse_writes_a_mount_would_hide(&specs, &[volume_at("/home/node", false)], &[])
            .expect("a writable named volume takes the copy lns-init makes once it is mounted");
    }

    #[test]
    fn a_mount_beside_every_write_refuses_nothing() {
        let specs = [spec_at("/home/node/tool.md")];
        refuse_writes_a_mount_would_hide(&specs, &[volume_at("/data", true)], &[bind_at("/work")])
            .expect("neither mount covers where the write lands");
    }

    fn tar_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *data).unwrap();
        }
        builder.into_inner().unwrap()
    }

    fn expand(guest_path: &str, layer_tar: &[u8]) -> (tempfile::TempDir, Vec<RuntimeFileSpec>) {
        let dir = tempfile::tempdir().unwrap();
        let store = ContentStore::new(dir.path());
        let specs = fileset_runtime_specs(guest_path, layer_tar, &store).unwrap();
        (dir, specs)
    }

    #[test]
    fn a_fileset_entry_name_with_a_newline_is_refused() {
        let tar = raw_tar("seed\n/etc/shadow", b"x");
        let dir = tempfile::tempdir().unwrap();
        let result =
            fileset_runtime_specs("/root/skills", &tar[..], &ContentStore::new(dir.path()));
        assert!(
            result.is_err(),
            "a newline in a fileset entry name splits into an extra line of the /.lens/fileset-owned chown manifest, letting lns-init lchown an arbitrary absolute path (/etc/shadow); it must be refused, but was accepted: {result:?}"
        );
    }

    #[test]
    fn expansion_strips_the_setuid_bit_from_a_fileset_file() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(1);
        header.set_mode(0o4755);
        header.set_cksum();
        builder
            .append_data(&mut header, "helper", &b"x"[..])
            .unwrap();
        let tar = builder.into_inner().unwrap();
        let (_dir, specs) = expand("/opt/tools", &tar);
        assert_eq!(specs.len(), 1);
        assert_eq!(
            specs[0].mode & 0o7000,
            0,
            "a pulled fileset must not carry setuid/setgid/sticky bits — push strips them with & 0o777 and pull must match, else a ref fileset with owner: root ships a setuid-root binary"
        );
    }

    #[test]
    fn expands_files_under_the_guest_path_as_streamed_content_specs() {
        let tar = tar_with(&[("deep.md", b"research"), ("nested/tools.md", b"tools")]);
        let (dir, specs) = expand("/root/.some-agent/skills", &tar);
        let mut paths: Vec<&str> = specs.iter().map(|s| s.guest_path.as_str()).collect();
        paths.sort();
        assert_eq!(
            paths,
            [
                "/root/.some-agent/skills/deep.md",
                "/root/.some-agent/skills/nested/tools.md"
            ]
        );
        let deep = specs
            .iter()
            .find(|s| s.guest_path.ends_with("deep.md"))
            .unwrap();
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(b"research")));
        assert!(
            matches!(&deep.source, RuntimeSource::Content { digest: actual, size: 8, .. } if actual == &digest),
            "fileset entries must reference streamed content, got {:?}",
            deep.source
        );
        assert_eq!(
            std::fs::read(ContentStore::new(dir.path()).path_for(&digest).unwrap()).unwrap(),
            b"research"
        );
    }

    #[test]
    fn a_trailing_slash_on_the_guest_path_does_not_double_up() {
        let tar = tar_with(&[("a.txt", b"x")]);
        let (_dir, specs) = expand("/mount/", &tar);
        assert_eq!(specs[0].guest_path, "/mount/a.txt");
    }

    #[test]
    fn a_directory_entry_is_skipped_not_materialized() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut dir = tar::Header::new_gnu();
        dir.set_entry_type(tar::EntryType::Directory);
        dir.set_size(0);
        dir.set_mode(0o755);
        dir.set_cksum();
        builder.append_data(&mut dir, "sub/", &b""[..]).unwrap();
        let mut file = tar::Header::new_gnu();
        file.set_entry_type(tar::EntryType::Regular);
        file.set_size(1);
        file.set_mode(0o644);
        file.set_cksum();
        builder.append_data(&mut file, "sub/f", &b"y"[..]).unwrap();
        let tar = builder.into_inner().unwrap();
        let (_dir, specs) = expand("/m", &tar);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].guest_path, "/m/sub/f");
    }

    #[test]
    fn expansion_refuses_content_beyond_the_aggregate_limit() {
        let tar = tar_with(&[("a", b"123"), ("b", b"456")]);
        let dir = tempfile::tempdir().unwrap();
        let mut budget = FilesetBudget::with_limits(5, 10);
        let err = fileset_runtime_specs_with_budget(
            "/mount",
            &tar[..],
            &ContentStore::new(dir.path()),
            &mut budget,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("5-byte limit"));
    }

    #[test]
    fn expansion_refuses_more_than_the_entry_limit() {
        let tar = tar_with(&[("a", b"1"), ("b", b"2")]);
        let dir = tempfile::tempdir().unwrap();
        let mut budget = FilesetBudget::with_limits(10, 1);
        let err = fileset_runtime_specs_with_budget(
            "/mount",
            &tar[..],
            &ContentStore::new(dir.path()),
            &mut budget,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("more than 1 entries"));
    }

    /// A raw single-file tar with an arbitrary `name` — `tar::Builder` refuses to write `..`, but an untrusted registry blob can still contain it, which is exactly what the guard defends against.
    fn raw_tar(name: &str, data: &[u8]) -> Vec<u8> {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        header[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        let mut out = header.to_vec();
        out.extend_from_slice(data);
        out.resize(out.len() + (512 - data.len() % 512) % 512, 0);
        out.resize(out.len() + 1024, 0);
        out
    }

    #[test]
    fn a_traversing_entry_is_refused() {
        let tar = raw_tar("../escape", b"x");
        let dir = tempfile::tempdir().unwrap();
        let err =
            fileset_runtime_specs("/mount", &tar[..], &ContentStore::new(dir.path())).unwrap_err();
        assert!(
            format!("{err:#}").contains("escapes its guest path"),
            "got: {err:#}"
        );
    }

    #[test]
    fn an_absolute_entry_is_refused() {
        let tar = raw_tar("/etc/evil", b"x");
        let dir = tempfile::tempdir().unwrap();
        let err =
            fileset_runtime_specs("/mount", &tar[..], &ContentStore::new(dir.path())).unwrap_err();
        assert!(
            format!("{err:#}").contains("escapes its guest path"),
            "got: {err:#}"
        );
    }

    struct TwoLevel;
    impl SnapshotDir for TwoLevel {
        fn entries(&self, dir: &Path) -> std::io::Result<Vec<SnapshotEntry>> {
            if dir == Path::new("/work/skills") {
                Ok(vec![
                    SnapshotEntry {
                        name: "deep".into(),
                        dir: true,
                        mode: 0o755,
                    },
                    SnapshotEntry {
                        name: "prompts.md".into(),
                        dir: false,
                        mode: 0o644,
                    },
                ])
            } else if dir == Path::new("/work/skills/deep") {
                Ok(vec![SnapshotEntry {
                    name: "run.sh".into(),
                    dir: false,
                    mode: 0o755,
                }])
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no such directory",
                ))
            }
        }
    }

    fn local(source: &str, guest_path: &str, owner: FilesetOwner) -> LocalFileset {
        LocalFileset {
            source: source.into(),
            guest_path: guest_path.into(),
            owner,
        }
    }

    #[test]
    fn local_specs_snapshot_nested_directories_as_host_file_specs() {
        let mut out = MaterializedFilesets::default();
        local_fileset_specs(
            &TwoLevel,
            &[local(
                "/work/skills",
                "/root/.agent/skills/",
                FilesetOwner::Root,
            )],
            &mut out,
        )
        .unwrap();
        let rendered: Vec<(String, u32)> = out
            .specs
            .iter()
            .map(|spec| (spec.guest_path.clone(), spec.mode))
            .collect();
        assert_eq!(
            rendered,
            [
                ("/root/.agent/skills/deep/run.sh".to_string(), 0o755),
                ("/root/.agent/skills/prompts.md".to_string(), 0o644),
            ]
        );
        assert!(out.specs.iter().all(|spec| matches!(
            &spec.source,
            RuntimeSource::HostFile(path) if path.starts_with("/work/skills")
        )));
    }

    #[test]
    fn local_specs_surface_a_missing_directory_naming_the_fileset() {
        let err = local_fileset_specs(
            &TwoLevel,
            &[local("/work/missing", "/s", FilesetOwner::Workload)],
            &mut MaterializedFilesets::default(),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("snapshotting fileset /work/missing"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_workload_owned_fileset_lists_its_mount_dirs_and_files_for_the_guest_chown() {
        let mut out = MaterializedFilesets::default();
        local_fileset_specs(
            &TwoLevel,
            &[local(
                "/work/skills",
                "/root/.agent/skills/",
                FilesetOwner::Workload,
            )],
            &mut out,
        )
        .unwrap();
        let owned: Vec<&str> = out.owned_paths.iter().map(String::as_str).collect();
        assert_eq!(
            owned,
            [
                "/root/.agent/skills",
                "/root/.agent/skills/deep",
                "/root/.agent/skills/deep/run.sh",
                "/root/.agent/skills/prompts.md",
            ],
            "the guest path, introduced dirs, and files must all transfer to the workload"
        );
        let manifest = out
            .into_specs()
            .into_iter()
            .find(|s| s.guest_path == OWNED_MANIFEST_PATH)
            .expect("owned manifest spec");
        assert_eq!(manifest.mode, 0o444);
        assert!(matches!(
            &manifest.source,
            RuntimeSource::Bytes(body) if body == b"/root/.agent/skills\n/root/.agent/skills/deep\n/root/.agent/skills/deep/run.sh\n/root/.agent/skills/prompts.md\n"
        ));
    }

    #[test]
    fn a_root_owned_fileset_ships_no_chown_manifest() {
        let mut out = MaterializedFilesets::default();
        local_fileset_specs(
            &TwoLevel,
            &[local("/work/skills", "/opt/skills", FilesetOwner::Root)],
            &mut out,
        )
        .unwrap();
        assert!(out.owned_paths.is_empty());
        assert!(
            !out.into_specs()
                .iter()
                .any(|s| s.guest_path == OWNED_MANIFEST_PATH),
            "pinned inputs must not be transferred to the workload"
        );
    }

    struct StagedHost {
        facts: Option<HostFileFacts>,
        unreadable: bool,
        home: Option<PathBuf>,
    }

    impl StagedHost {
        fn present() -> Self {
            Self {
                facts: Some(HostFileFacts {
                    mode: 0o644,
                    is_regular_file: true,
                }),
                unreadable: false,
                home: Some(PathBuf::from("/home/some-user")),
            }
        }
    }

    impl HostFileProbe for StagedHost {
        fn home(&self) -> Option<PathBuf> {
            self.home.clone()
        }

        fn stat(&self, _path: &Path) -> std::io::Result<Option<HostFileFacts>> {
            if self.unreadable {
                return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
            }
            Ok(self.facts)
        }
    }

    fn host(source: &str, guest_path: &str, owner: FilesetOwner, optional: bool) -> HostFileset {
        HostFileset {
            source: source.into(),
            guest_path: guest_path.into(),
            owner,
            optional,
        }
    }

    #[test]
    fn a_denied_host_file_is_never_read() {
        let mut out = MaterializedFilesets::default();
        host_fileset_specs(
            &StagedHost::present(),
            &[host(
                "~/.gitconfig",
                "/home/agent/.gitconfig",
                FilesetOwner::Workload,
                false,
            )],
            &["~/.gitconfig".to_string()],
            &mut out,
        )
        .unwrap();
        assert!(
            out.into_specs().is_empty(),
            "a path this machine refused the sandbox must reach the guest by no route, and a required one must not refuse the run either — the CLI already settled that"
        );
    }

    #[test]
    fn denying_one_host_file_leaves_the_others_readable() {
        let mut out = MaterializedFilesets::default();
        host_fileset_specs(
            &StagedHost::present(),
            &[
                host(
                    "~/.gitconfig",
                    "/home/agent/.gitconfig",
                    FilesetOwner::Root,
                    false,
                ),
                host("~/.vimrc", "/home/agent/.vimrc", FilesetOwner::Root, false),
            ],
            &["~/.gitconfig".to_string()],
            &mut out,
        )
        .unwrap();
        let mounted: Vec<String> = out
            .into_specs()
            .iter()
            .map(|spec| spec.guest_path.clone())
            .collect();
        assert_eq!(mounted, ["/home/agent/.vimrc"]);
    }

    #[test]
    fn a_workload_owned_host_file_joins_the_chown_manifest() {
        let mut out = MaterializedFilesets::default();
        host_fileset_specs(
            &StagedHost::present(),
            &[host(
                "~/.gitconfig",
                "/home/agent/.gitconfig",
                FilesetOwner::Workload,
                false,
            )],
            &[],
            &mut out,
        )
        .unwrap();
        assert_eq!(
            out.owned_paths
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["/home/agent/.gitconfig"],
            "without the chown the workload cannot rewrite the file it was seeded with"
        );
        assert!(
            out.into_specs()
                .iter()
                .any(|spec| spec.guest_path == OWNED_MANIFEST_PATH)
        );
    }

    #[test]
    fn a_root_owned_host_file_does_not() {
        let mut out = MaterializedFilesets::default();
        host_fileset_specs(
            &StagedHost::present(),
            &[host(
                "/etc/gitconfig",
                "/etc/gitconfig",
                FilesetOwner::Root,
                false,
            )],
            &[],
            &mut out,
        )
        .unwrap();
        assert!(out.owned_paths.is_empty());
        assert!(
            !out.into_specs()
                .iter()
                .any(|spec| spec.guest_path == OWNED_MANIFEST_PATH)
        );
    }

    #[test]
    fn a_host_file_keeps_the_mode_the_host_reports() {
        let mut out = MaterializedFilesets::default();
        host_fileset_specs(
            &StagedHost {
                facts: Some(HostFileFacts {
                    mode: 0o600,
                    is_regular_file: true,
                }),
                ..StagedHost::present()
            },
            &[host(
                "~/.gitconfig",
                "/home/agent/.gitconfig",
                FilesetOwner::Workload,
                false,
            )],
            &[],
            &mut out,
        )
        .unwrap();
        assert_eq!(out.specs[0].mode, 0o600);
    }

    #[test]
    fn a_host_path_that_resolves_to_something_other_than_a_regular_file_is_refused() {
        let err = host_fileset_specs(
            &StagedHost {
                facts: Some(HostFileFacts {
                    mode: 0o755,
                    is_regular_file: false,
                }),
                ..StagedHost::present()
            },
            &[host("/etc", "/etc/gitconfig", FilesetOwner::Root, true)],
            &[],
            &mut MaterializedFilesets::default(),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("/etc") && format!("{err:#}").contains("regular file"),
            "a directory or device is not a file the guest can be seeded with, and `optional` must not swallow it: {err:#}"
        );
    }

    #[test]
    fn a_host_path_the_probe_cannot_read_names_the_path_it_failed_on() {
        let err = host_fileset_specs(
            &StagedHost {
                unreadable: true,
                ..StagedHost::present()
            },
            &[host(
                "~/.gitconfig",
                "/home/agent/.gitconfig",
                FilesetOwner::Workload,
                true,
            )],
            &[],
            &mut MaterializedFilesets::default(),
        )
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("~/.gitconfig") && message.contains("/home/some-user/.gitconfig"),
            "a denied stat is not an absent file, so `optional` must not skip it silently: {message}"
        );
    }

    #[test]
    fn an_absent_optional_host_path_is_skipped_and_tells_the_operator_one_line() {
        let mut out = MaterializedFilesets::default();
        let frames = crate::log::testing::capture_run_frames(|| {
            host_fileset_specs(
                &StagedHost {
                    facts: None,
                    ..StagedHost::present()
                },
                &[host(
                    "~/.gitconfig",
                    "/home/agent/.gitconfig",
                    FilesetOwner::Workload,
                    true,
                )],
                &[],
                &mut out,
            )
            .unwrap();
        });
        assert!(
            out.specs.is_empty(),
            "an absent optional host file must reach nothing downstream: {:?}",
            out.specs
        );
        assert_eq!(
            frames,
            vec![lns_ipc::WireFrame::Json(lns_ipc::Response::RunLog {
                level: lns_ipc::LogLevel::Info,
                verb: Some("fileset".to_string()),
                message: "skipping optional hostPath ~/.gitconfig — not present on this host"
                    .to_string(),
            })],
            "the run span carries this line to the CLI, which prints it as one status line; drop it and the guest is silently unseeded"
        );
    }

    #[test]
    fn a_home_rooted_host_path_without_a_known_home_is_refused() {
        let err = host_fileset_specs(
            &StagedHost {
                home: None,
                ..StagedHost::present()
            },
            &[host(
                "~/.gitconfig",
                "/home/agent/.gitconfig",
                FilesetOwner::Workload,
                true,
            )],
            &[],
            &mut MaterializedFilesets::default(),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("~/.gitconfig"),
            "an unresolvable home must name the path rather than seed a literal `~` file: {err:#}"
        );
    }

    #[test]
    fn an_absent_required_host_path_names_both_the_declared_and_the_resolved_path() {
        let err = host_fileset_specs(
            &StagedHost {
                facts: None,
                ..StagedHost::present()
            },
            &[host(
                "~/.gitconfig",
                "/home/agent/.gitconfig",
                FilesetOwner::Workload,
                false,
            )],
            &[],
            &mut MaterializedFilesets::default(),
        )
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("~/.gitconfig"), "got: {message}");
        assert!(
            message.contains("/home/some-user/.gitconfig"),
            "the resolved path is what the operator has to create — including when a symlink is what points nowhere, which the probe reports as this same absence: {message}"
        );
    }

    #[test]
    fn owned_paths_never_climb_above_the_guest_path() {
        let specs = [RuntimeFileSpec {
            guest_path: "/home/sandbox/.claude/settings.json".into(),
            mode: 0o644,
            source: RuntimeSource::Bytes(b"{}".to_vec()),
        }];
        let owned = owned_paths_for("/home/sandbox", &specs);
        assert_eq!(
            owned.iter().map(String::as_str).collect::<Vec<_>>(),
            [
                "/home/sandbox",
                "/home/sandbox/.claude",
                "/home/sandbox/.claude/settings.json",
            ],
            "/home must stay untouched"
        );
    }

    #[test]
    fn a_symlink_entry_is_refused() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        builder
            .append_link(&mut header, "link", "/etc/passwd")
            .unwrap();
        let tar = builder.into_inner().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err =
            fileset_runtime_specs("/mount", &tar[..], &ContentStore::new(dir.path())).unwrap_err();
        assert!(
            format!("{err:#}").contains("not a regular file"),
            "got: {err:#}"
        );
    }

    fn descriptor(media_type: &str, digest: &str, size: i64) -> OciDescriptor {
        OciDescriptor {
            media_type: media_type.into(),
            digest: digest.into(),
            size,
            ..Default::default()
        }
    }

    #[test]
    fn a_manifests_packed_layers_are_read_in_manifest_order() {
        let manifest = OciImageManifest {
            layers: vec![
                descriptor(lns_artifact::build::FILESET_LAYER_MEDIA_TYPE, "sha256:a", 3),
                descriptor(lns_artifact::build::FILESET_LAYER_MEDIA_TYPE, "sha256:b", 4),
            ],
            ..Default::default()
        };
        assert_eq!(
            packed_layers(&manifest),
            [
                crate::artifact::PackedLayer {
                    digest: "sha256:a".into(),
                    size: 3
                },
                crate::artifact::PackedLayer {
                    digest: "sha256:b".into(),
                    size: 4
                },
            ],
            "the i-th path fileset owns the i-th layer, so manifest order is the correlation"
        );
    }

    #[test]
    fn a_layer_of_another_media_type_is_not_a_packed_fileset() {
        let manifest = OciImageManifest {
            layers: vec![
                descriptor("application/vnd.oci.empty.v1+json", "sha256:e", 2),
                descriptor("application/vnd.oci.image.layer.v1.tar", "sha256:t", 9),
                descriptor(lns_artifact::build::README_LAYER_MEDIA_TYPE, "sha256:r", 5),
            ],
            ..Default::default()
        };
        assert!(
            packed_layers(&manifest).is_empty(),
            "a document declaring no path fileset still carries the OCI empty descriptor, and its README never enters the guest — counting any of these as content would misalign every index after it"
        );
    }

    #[test]
    fn a_packed_layer_with_a_negative_declared_size_is_not_countable() {
        let manifest = OciImageManifest {
            layers: vec![descriptor(
                lns_artifact::build::FILESET_LAYER_MEDIA_TYPE,
                "sha256:a",
                -1,
            )],
            ..Default::default()
        };
        assert!(
            packed_layers(&manifest).is_empty(),
            "a size no byte count can be is not a layer to download, and the layer-count cross-check then refuses the run"
        );
    }

    #[test]
    fn a_declared_layer_beyond_the_byte_ceiling_is_refused_before_it_downloads() {
        let layer = crate::artifact::PackedLayer {
            digest: "sha256:a".into(),
            size: 6,
        };
        let err = validate_packed_layer_size(&layer, 5).unwrap_err();
        assert!(format!("{err:#}").contains("5-byte limit"), "got: {err:#}");
        validate_packed_layer_size(&layer, 6).expect("a layer at the ceiling still pulls");
    }
}
