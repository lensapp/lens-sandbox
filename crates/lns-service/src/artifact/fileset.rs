use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path};

use anyhow::{Context, Result, bail};
use lns_artifact::sandbox::FilesetOwner;

use crate::artifact::assembly::LocalFileset;
use crate::content_store::ContentStore;
use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};

pub const OWNED_MANIFEST_PATH: &str = "/.lens/fileset-owned";

/// The fileset specs for a run plus the guest paths lns-init must chown to the workload user.
#[derive(Default)]
pub struct MaterializedFilesets {
    pub specs: Vec<RuntimeFileSpec>,
    pub owned_paths: BTreeSet<String>,
}

impl MaterializedFilesets {
    pub fn absorb(&mut self, owner: FilesetOwner, mount_path: &str, specs: Vec<RuntimeFileSpec>) {
        if owner == FilesetOwner::Workload {
            self.owned_paths.extend(owned_paths_for(mount_path, &specs));
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

/// The mount path, every directory the fileset introduces beneath it, and each shipped file.
fn owned_paths_for(mount_path: &str, specs: &[RuntimeFileSpec]) -> BTreeSet<String> {
    let root = mount_path.trim_end_matches('/');
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
        let root = local.mount_path.trim_end_matches('/');
        let mut specs = Vec::new();
        snapshot_into(dir, Path::new(&local.source), root, &mut specs)
            .with_context(|| format!("snapshotting fileset {}", local.source))?;
        out.absorb(local.owner, &local.mount_path, specs);
    }
    Ok(())
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

/// Expand a FileSet's tar layer into guest-write specs rooted at `mount_path`, so the fileset's files land in the guest at boot. Fail-closed: an entry whose path escapes the mount (absolute or `..`) or isn't a regular file is refused, so a hand-built or tampered fileset can't write outside its declared mount.
pub fn fileset_runtime_specs<R: Read>(
    mount_path: &str,
    layer_tar: R,
    content_store: &ContentStore,
) -> Result<Vec<RuntimeFileSpec>> {
    let root = mount_path.trim_end_matches('/');
    let mut specs = Vec::new();
    let mut archive = tar::Archive::new(layer_tar);
    for entry in archive.entries().context("reading fileset layer")? {
        let mut entry = entry.context("reading fileset entry")?;
        let entry_type = entry.header().entry_type();
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
            bail!("fileset entry {} escapes its mount path", path.display());
        }
        if !entry_type.is_file() {
            bail!(
                "fileset entry {} is not a regular file (filesets carry only regular files)",
                path.display()
            );
        }
        let mode = entry.header().mode().unwrap_or(0o644);
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

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

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

    fn expand(mount_path: &str, layer_tar: &[u8]) -> (tempfile::TempDir, Vec<RuntimeFileSpec>) {
        let dir = tempfile::tempdir().unwrap();
        let store = ContentStore::new(dir.path());
        let specs = fileset_runtime_specs(mount_path, layer_tar, &store).unwrap();
        (dir, specs)
    }

    #[test]
    fn expands_files_under_the_mount_path_as_streamed_content_specs() {
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
    fn a_trailing_slash_on_the_mount_path_does_not_double_up() {
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
            format!("{err:#}").contains("escapes its mount"),
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
            format!("{err:#}").contains("escapes its mount"),
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

    fn local(source: &str, mount_path: &str, owner: FilesetOwner) -> LocalFileset {
        LocalFileset {
            source: source.into(),
            mount_path: mount_path.into(),
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
            "the mount path, introduced dirs, and files must all transfer to the workload"
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

    #[test]
    fn owned_paths_never_climb_above_the_mount_path() {
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
}
