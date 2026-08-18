use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};

use crate::archive_limits::{ArchiveLimits, LimitedReader};
use crate::composefs::Sha256Digest;
use crate::composefs::changeset::{PathChange, classify_path};
use crate::composefs::vendor::generic_tree::{Directory, Inode, LeafContent, Stat};
use crate::composefs::vendor::tree::{FileSystem, RegularFile};
use crate::content_store::ContentStore;

#[cfg(target_os = "linux")]
#[inline]
pub const fn type_bits(t: libc::mode_t) -> u32 {
    t
}
#[cfg(target_os = "macos")]
#[inline]
pub const fn type_bits(t: libc::mode_t) -> u32 {
    t as u32
}

pub fn build_filesystem_from_layer_bytes(
    content_store: &ContentStore,
    layers: &[Vec<u8>],
    progress: &dyn Fn(u64, u64),
) -> Result<FileSystem<Sha256Digest>> {
    let total: u64 = layers.iter().map(|l| l.len() as u64).sum();
    progress(0, total);
    let mut fs = FileSystem::<Sha256Digest>::new(root_stat());
    let mut applied: u64 = 0;
    for (idx, layer_bytes) in layers.iter().enumerate() {
        apply_layer_bytes(&mut fs, content_store, layer_bytes, Limits::PRODUCTION)
            .with_context(|| format!("applying layer {idx}"))?;
        applied += layer_bytes.len() as u64;
        progress(applied, total);
    }
    Ok(fs)
}

pub fn build_filesystem_from_layer_paths(
    content_store: &ContentStore,
    layer_paths: &[PathBuf],
) -> Result<FileSystem<Sha256Digest>> {
    let mut fs = FileSystem::<Sha256Digest>::new(root_stat());
    for path in layer_paths {
        let mut bytes = Vec::new();
        File::open(path)
            .with_context(|| format!("opening {}", path.display()))?
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading {}", path.display()))?;
        apply_layer_bytes(&mut fs, content_store, &bytes, Limits::PRODUCTION)
            .with_context(|| format!("applying {}", path.display()))?;
    }
    Ok(fs)
}

#[cfg(test)]
fn build_filesystem_from_layer_bytes_with_limits(
    content_store: &ContentStore,
    layers: &[Vec<u8>],
    limits: Limits,
) -> Result<FileSystem<Sha256Digest>> {
    let mut fs = FileSystem::<Sha256Digest>::new(root_stat());
    for (idx, layer_bytes) in layers.iter().enumerate() {
        apply_layer_bytes(&mut fs, content_store, layer_bytes, limits)
            .with_context(|| format!("applying layer {idx}"))?;
    }
    Ok(fs)
}

fn root_stat() -> Stat {
    Stat {
        st_mode: type_bits(libc::S_IFDIR) | 0o755,
        st_uid: 0,
        st_gid: 0,
        st_mtim_sec: 0,
        xattrs: std::collections::BTreeMap::new(),
    }
}

fn apply_layer_bytes(
    fs: &mut FileSystem<Sha256Digest>,
    content_store: &ContentStore,
    bytes: &[u8],
    limits: Limits,
) -> Result<()> {
    if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        let decoder = GzDecoder::new(Cursor::new(bytes));
        let limited = LimitedReader::new(decoder, limits.archive.bytes, "decompressed layer");
        apply_tar(fs, content_store, tar::Archive::new(limited), limits)
    } else {
        let limited = LimitedReader::new(
            Cursor::new(bytes),
            limits.archive.bytes,
            "decompressed layer",
        );
        apply_tar(fs, content_store, tar::Archive::new(limited), limits)
    }
}

type Content = LeafContent<RegularFile<Sha256Digest>>;

enum Op {
    ClearDir(Vec<OsString>),
    Remove(Vec<OsString>, OsString),
    EnsureDir {
        path: PathBuf,
        stat: Stat,
    },
    Leaf {
        parents: Vec<OsString>,
        name: OsString,
        stat: Stat,
        content: Content,
    },
    RegularLeaf {
        parents: Vec<OsString>,
        name: OsString,
        stat: Stat,
        seq: usize,
    },
}

enum LeafKind {
    Inline(Content),
    Regular,
    Skip,
}

fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().min(8))
        .unwrap_or(4)
}

// Files above this stream straight to the content store rather than buffering in RAM, so a layer of multi-GB blobs can't balloon the in-flight queue.
const MAX_BUFFERED_FILE_BYTES: u64 = 8 * 1024 * 1024;

const MAX_BUFFERED_LAYER_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Copy)]
struct Limits {
    archive: ArchiveLimits,
    buffered_bytes: u64,
}

impl Limits {
    const PRODUCTION: Self = Self {
        archive: ArchiveLimits::PRODUCTION,
        buffered_bytes: MAX_BUFFERED_LAYER_BYTES,
    };
}

fn apply_tar<R: Read>(
    fs: &mut FileSystem<Sha256Digest>,
    content_store: &ContentStore,
    archive: tar::Archive<R>,
    limits: Limits,
) -> Result<()> {
    let workers = worker_count();
    let (work_tx, work_rx) = std::sync::mpsc::sync_channel::<(usize, Vec<u8>)>(workers * 4);
    let work_rx = Arc::new(Mutex::new(work_rx));
    let (res_tx, res_rx) = std::sync::mpsc::channel::<(usize, Result<RegularFile<Sha256Digest>>)>();

    std::thread::scope(|scope| -> Result<()> {
        for _ in 0..workers {
            let work_rx = Arc::clone(&work_rx);
            let res_tx = res_tx.clone();
            scope.spawn(move || install_worker(content_store, &work_rx, &res_tx));
        }
        drop(res_tx);

        let read = read_layer_ops(content_store, archive, &work_tx, limits);
        drop(work_tx);
        let (ops, regular_count) = read?;

        let mut digests: Vec<Option<RegularFile<Sha256Digest>>> = std::iter::repeat_with(|| None)
            .take(regular_count)
            .collect();
        for (seq, installed) in res_rx {
            digests[seq] =
                Some(installed.with_context(|| format!("installing regular file #{seq}"))?);
        }

        apply_ops(fs, ops, &mut digests)
    })
}

fn install_worker(
    content_store: &ContentStore,
    work_rx: &Mutex<Receiver<(usize, Vec<u8>)>>,
    res_tx: &Sender<(usize, Result<RegularFile<Sha256Digest>>)>,
) {
    loop {
        let job = work_rx
            .lock()
            .expect("ingest work queue mutex poisoned")
            .recv();
        let Ok((seq, bytes)) = job else { return };
        let installed = content_store
            .install_from_bytes(&bytes)
            .map(|r| RegularFile::External(Sha256Digest::from(r.raw_digest), r.size));
        let _ = res_tx.send((seq, installed));
    }
}

fn read_layer_ops<R: Read>(
    content_store: &ContentStore,
    mut archive: tar::Archive<R>,
    work_tx: &SyncSender<(usize, Vec<u8>)>,
    limits: Limits,
) -> Result<(Vec<Op>, usize)> {
    let mut removals = Vec::new();
    let mut additions = Vec::new();
    let mut seq = 0usize;
    let mut entry_count = 0usize;
    let mut buffered_bytes = 0u64;
    for entry in archive.entries().context("opening tar")? {
        entry_count += 1;
        if entry_count > limits.archive.entries {
            bail!("layer exceeds the {}-entry cap", limits.archive.entries);
        }
        let mut entry = entry.context("reading tar entry")?;
        let header_path: PathBuf = entry.path().context("entry path")?.into_owned();
        let Some(change) = classify_path(&header_path)? else {
            continue;
        };
        let normalized = match change {
            PathChange::ClearDirectory(path) => {
                removals.push(Op::ClearDir(path_segments(&path)));
                continue;
            }
            PathChange::Remove(path) => {
                let (parents, name) = split_path(&path);
                removals.push(Op::Remove(parents, name));
                continue;
            }
            PathChange::Entry(path) => path,
        };
        let (parents, name) = split_path(&normalized);
        let entry_type = entry.header().entry_type();
        let stat = stat_from_entry(&entry, entry_type.is_dir())?;
        if entry_type.is_dir() {
            additions.push(Op::EnsureDir {
                path: normalized,
                stat,
            });
            continue;
        }
        match leaf_kind(&entry, entry_type, &normalized)? {
            LeafKind::Inline(content) => additions.push(Op::Leaf {
                parents,
                name,
                stat,
                content,
            }),
            LeafKind::Regular if entry.size() > MAX_BUFFERED_FILE_BYTES => {
                let installed = content_store
                    .install_from_reader(&mut entry)
                    .with_context(|| format!("installing {}", normalized.display()))?;
                let file =
                    RegularFile::External(Sha256Digest::from(installed.raw_digest), installed.size);
                additions.push(Op::Leaf {
                    parents,
                    name,
                    stat,
                    content: LeafContent::Regular(file),
                });
            }
            LeafKind::Regular => {
                buffered_bytes = buffered_bytes.saturating_add(entry.size());
                if buffered_bytes > limits.buffered_bytes {
                    bail!(
                        "layer's RAM-buffered content exceeds the {}-byte cap",
                        limits.buffered_bytes
                    );
                }
                let prealloc = entry.size().min(MAX_BUFFERED_FILE_BYTES) as usize;
                let mut bytes = Vec::with_capacity(prealloc);
                entry
                    .read_to_end(&mut bytes)
                    .with_context(|| format!("reading content for {}", normalized.display()))?;
                work_tx
                    .send((seq, bytes))
                    .expect("ingest workers stopped before the tar was fully read");
                additions.push(Op::RegularLeaf {
                    parents,
                    name,
                    stat,
                    seq,
                });
                seq += 1;
            }
            LeafKind::Skip => {}
        }
    }
    removals.append(&mut additions);
    Ok((removals, seq))
}

fn apply_ops(
    fs: &mut FileSystem<Sha256Digest>,
    ops: Vec<Op>,
    digests: &mut [Option<RegularFile<Sha256Digest>>],
) -> Result<()> {
    for op in ops {
        match op {
            Op::ClearDir(parents) => get_or_create_dir(fs, &parents)?.clear(),
            Op::Remove(parents, target) => {
                get_or_create_dir(fs, &parents)?.remove(target.as_os_str())
            }
            Op::EnsureDir { path, stat } => ensure_directory(fs, &path, stat)?,
            Op::Leaf {
                parents,
                name,
                stat,
                content,
            } => insert_leaf(fs, &parents, name.as_os_str(), stat, content)?,
            Op::RegularLeaf {
                parents,
                name,
                stat,
                seq,
            } => {
                let file = digests
                    .get_mut(seq)
                    .and_then(Option::take)
                    .with_context(|| format!("no installed content for regular file #{seq}"))?;
                insert_leaf(
                    fs,
                    &parents,
                    name.as_os_str(),
                    stat,
                    LeafContent::Regular(file),
                )?;
            }
        }
    }
    Ok(())
}

fn insert_leaf(
    fs: &mut FileSystem<Sha256Digest>,
    parents: &[OsString],
    name: &OsStr,
    stat: Stat,
    content: Content,
) -> Result<()> {
    let leaf_id = fs.push_leaf(stat, content);
    get_or_create_dir(fs, parents)?.insert(name, Inode::leaf(leaf_id));
    Ok(())
}

fn leaf_kind<R: Read>(
    entry: &tar::Entry<R>,
    entry_type: tar::EntryType,
    path: &Path,
) -> Result<LeafKind> {
    match entry_type {
        tar::EntryType::Regular | tar::EntryType::Continuous => Ok(LeafKind::Regular),
        tar::EntryType::Symlink => {
            let target = entry
                .header()
                .link_name()
                .context("symlink: reading link_name")?
                .with_context(|| format!("symlink without link_name at {}", path.display()))?
                .into_owned();
            let target_os = OsString::from(target.as_os_str());
            Ok(LeafKind::Inline(LeafContent::Symlink(
                target_os.into_boxed_os_str(),
            )))
        }
        tar::EntryType::Fifo => Ok(LeafKind::Inline(LeafContent::Fifo)),
        _ => Ok(LeafKind::Skip),
    }
}

fn get_or_create_dir<'a>(
    fs: &'a mut FileSystem<Sha256Digest>,
    segments: &[OsString],
) -> Result<&'a mut Directory<RegularFile<Sha256Digest>>> {
    let resolved = crate::composefs::inject_path::resolve_parent_segments(fs, segments)?;
    let mut dir = &mut fs.root;
    for seg in &resolved {
        let exists = dir.lookup(seg).is_some();
        if !exists {
            let inode = Inode::Directory(Box::new(Directory::new(default_dir_stat())));
            dir.insert(seg, inode);
        }
        dir = dir
            .get_directory_mut(seg)
            .with_context(|| format!("descending into {:?} (not a directory)", seg))?;
    }
    Ok(dir)
}

fn ensure_directory(fs: &mut FileSystem<Sha256Digest>, path: &Path, stat: Stat) -> Result<()> {
    let (parents, name) = split_path(path);
    let parent = get_or_create_dir(fs, &parents)?;
    match parent.lookup(name.as_os_str()) {
        Some(Inode::Directory(_)) => {
            if let Some(Inode::Directory(d)) = parent.pop(name.as_os_str()) {
                let mut fresh = Directory::new(stat);
                fresh.entries = d.entries;
                parent.insert(name.as_os_str(), Inode::Directory(Box::new(fresh)));
            }
        }
        Some(_) => {
            parent.remove(name.as_os_str());
            parent.insert(
                name.as_os_str(),
                Inode::Directory(Box::new(Directory::new(stat))),
            );
        }
        None => {
            parent.insert(
                name.as_os_str(),
                Inode::Directory(Box::new(Directory::new(stat))),
            );
        }
    }
    Ok(())
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

fn stat_from_entry<R: Read>(entry: &tar::Entry<R>, is_dir: bool) -> Result<Stat> {
    let hdr = entry.header();
    let perms = hdr.mode().context("entry mode")? & 0o7777;
    let type_bits: u32 = if is_dir {
        type_bits(libc::S_IFDIR)
    } else {
        match hdr.entry_type() {
            tar::EntryType::Regular | tar::EntryType::Continuous => type_bits(libc::S_IFREG),
            tar::EntryType::Symlink => type_bits(libc::S_IFLNK),
            tar::EntryType::Fifo => type_bits(libc::S_IFIFO),
            tar::EntryType::Char => type_bits(libc::S_IFCHR),
            tar::EntryType::Block => type_bits(libc::S_IFBLK),
            _ => type_bits(libc::S_IFREG),
        }
    };
    let uid = hdr.uid().context("entry uid")? as u32;
    let gid = hdr.gid().context("entry gid")? as u32;
    let mtime = hdr.mtime().context("entry mtime")? as i64;
    Ok(Stat {
        st_mode: type_bits | perms,
        st_uid: uid,
        st_gid: gid,
        st_mtim_sec: mtime,
        xattrs: std::collections::BTreeMap::new(),
    })
}

fn path_segments(path: &Path) -> Vec<OsString> {
    path.iter().map(OsStr::to_os_string).collect()
}

fn split_path(path: &Path) -> (Vec<OsString>, OsString) {
    let mut segs: Vec<OsString> = path
        .components()
        .map(|c| c.as_os_str().to_os_string())
        .collect();
    let name = segs.pop().unwrap_or_default();
    (segs, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composefs::changeset::{MAX_NAME_BYTES, normalize_path, strip_wh_prefix};
    use std::io::Cursor;
    use tar::{Builder, EntryType, Header};

    fn tempdir() -> tempfile::TempDir {
        tempfile::TempDir::new().expect("tempdir")
    }

    fn write_file_entry(
        builder: &mut Builder<Cursor<&mut Vec<u8>>>,
        path: &str,
        mode: u32,
        contents: &[u8],
    ) {
        let mut h = Header::new_gnu();
        h.set_path(path).unwrap();
        h.set_size(contents.len() as u64);
        h.set_mode(mode);
        h.set_uid(0);
        h.set_gid(0);
        h.set_mtime(0);
        h.set_entry_type(EntryType::Regular);
        h.set_cksum();
        builder.append(&h, Cursor::new(contents)).unwrap();
    }

    fn write_dir_entry(builder: &mut Builder<Cursor<&mut Vec<u8>>>, path: &str, mode: u32) {
        let mut h = Header::new_gnu();
        h.set_path(path).unwrap();
        h.set_size(0);
        h.set_mode(mode);
        h.set_uid(0);
        h.set_gid(0);
        h.set_mtime(0);
        h.set_entry_type(EntryType::Directory);
        h.set_cksum();
        builder.append(&h, Cursor::new(&[][..])).unwrap();
    }

    fn write_symlink_entry(builder: &mut Builder<Cursor<&mut Vec<u8>>>, path: &str, target: &str) {
        let mut h = Header::new_gnu();
        h.set_path(path).unwrap();
        h.set_size(0);
        h.set_mode(0o777);
        h.set_uid(0);
        h.set_gid(0);
        h.set_mtime(0);
        h.set_entry_type(EntryType::Symlink);
        h.set_link_name(target).unwrap();
        h.set_cksum();
        builder.append(&h, Cursor::new(&[][..])).unwrap();
    }

    fn write_whiteout(builder: &mut Builder<Cursor<&mut Vec<u8>>>, path: &str) {
        let mut h = Header::new_gnu();
        h.set_path(path).unwrap();
        h.set_size(0);
        h.set_mode(0o644);
        h.set_uid(0);
        h.set_gid(0);
        h.set_mtime(0);
        h.set_entry_type(EntryType::Regular);
        h.set_cksum();
        builder.append(&h, Cursor::new(&[][..])).unwrap();
    }

    fn build_tar<F>(f: F) -> Vec<u8>
    where
        F: FnOnce(&mut Builder<Cursor<&mut Vec<u8>>>),
    {
        let mut bytes = Vec::new();
        {
            let mut builder = Builder::new(Cursor::new(&mut bytes));
            f(&mut builder);
            builder.finish().unwrap();
        }
        bytes
    }

    fn store(tmp: &tempfile::TempDir) -> ContentStore {
        ContentStore::new(tmp.path().join("content"))
    }

    #[test]
    fn build_from_single_layer_with_one_file() {
        let d = tempdir();
        let s = store(&d);
        let tar = build_tar(|b| {
            write_dir_entry(b, "etc/", 0o755);
            write_file_entry(b, "etc/hostname", 0o644, b"sandbox\n");
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[tar], &|_, _| {}).unwrap();
        let etc = fs.root.get_directory_mut(OsStr::new("etc")).unwrap();
        let leaf_id = etc.leaf_id(OsStr::new("hostname")).unwrap();
        assert!(matches!(
            &fs.leaves[leaf_id.0].content,
            LeafContent::Regular(RegularFile::External(_, 8))
        ));
        let entries = std::fs::read_dir(d.path().join("content").join("sha256"))
            .unwrap()
            .count();
        assert_eq!(entries, 1);
    }

    #[test]
    fn second_layer_overrides_file() {
        let d = tempdir();
        let s = store(&d);
        let layer1 = build_tar(|b| {
            write_dir_entry(b, "etc/", 0o755);
            write_file_entry(b, "etc/version", 0o644, b"v1\n");
        });
        let layer2 = build_tar(|b| {
            write_file_entry(b, "etc/version", 0o644, b"v2\n");
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[layer1, layer2], &|_, _| {}).unwrap();
        let etc = fs.root.get_directory_mut(OsStr::new("etc")).unwrap();
        let leaf_id = etc.leaf_id(OsStr::new("version")).unwrap();
        assert!(matches!(
            &fs.leaves[leaf_id.0].content,
            LeafContent::Regular(RegularFile::External(_, 3))
        ));
        let entries = std::fs::read_dir(d.path().join("content").join("sha256"))
            .unwrap()
            .count();
        assert_eq!(entries, 2);
    }

    #[test]
    fn whiteout_removes_file_from_cumulative_state() {
        let d = tempdir();
        let s = store(&d);
        let layer1 = build_tar(|b| {
            write_dir_entry(b, "etc/", 0o755);
            write_file_entry(b, "etc/secret", 0o600, b"hush");
        });
        let layer2 = build_tar(|b| write_whiteout(b, "etc/.wh.secret"));
        let mut fs = build_filesystem_from_layer_bytes(&s, &[layer1, layer2], &|_, _| {}).unwrap();
        let etc = fs.root.get_directory_mut(OsStr::new("etc")).unwrap();
        assert!(etc.leaf_id(OsStr::new("secret")).is_err());
    }

    #[test]
    fn whiteout_only_removes_the_lower_layer_version_of_a_same_layer_file() {
        let d = tempdir();
        let s = store(&d);
        let layer1 = build_tar(|b| {
            write_dir_entry(b, "etc/", 0o755);
            write_file_entry(b, "etc/version", 0o644, b"old");
        });
        let layer2 = build_tar(|b| {
            write_file_entry(b, "etc/version", 0o644, b"newer");
            write_whiteout(b, "etc/.wh.version");
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[layer1, layer2], &|_, _| {}).unwrap();
        let etc = fs.root.get_directory_mut(OsStr::new("etc")).unwrap();
        let leaf_id = etc.leaf_id(OsStr::new("version")).unwrap();
        assert!(matches!(
            &fs.leaves[leaf_id.0].content,
            LeafContent::Regular(RegularFile::External(_, 5))
        ));
    }

    #[test]
    fn opaque_whiteout_clears_directory_contents() {
        let d = tempdir();
        let s = store(&d);
        let layer1 = build_tar(|b| {
            write_dir_entry(b, "var/", 0o755);
            write_file_entry(b, "var/a", 0o644, b"a");
            write_file_entry(b, "var/b", 0o644, b"b");
        });
        let layer2 = build_tar(|b| {
            write_whiteout(b, "var/.wh..wh..opq");
            write_file_entry(b, "var/c", 0o644, b"c");
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[layer1, layer2], &|_, _| {}).unwrap();
        let var = fs.root.get_directory_mut(OsStr::new("var")).unwrap();
        assert!(var.leaf_id(OsStr::new("a")).is_err());
        assert!(var.leaf_id(OsStr::new("b")).is_err());
        assert!(var.leaf_id(OsStr::new("c")).is_ok());
    }

    #[test]
    fn opaque_whiteout_preserves_same_layer_entries_that_precede_the_marker() {
        let d = tempdir();
        let s = store(&d);
        let layer1 = build_tar(|b| {
            write_dir_entry(b, "var/", 0o755);
            write_file_entry(b, "var/old", 0o644, b"old");
        });
        let layer2 = build_tar(|b| {
            write_file_entry(b, "var/new", 0o644, b"new");
            write_whiteout(b, "var/.wh..wh..opq");
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[layer1, layer2], &|_, _| {}).unwrap();
        let var = fs.root.get_directory_mut(OsStr::new("var")).unwrap();
        assert!(var.leaf_id(OsStr::new("old")).is_err());
        assert!(var.leaf_id(OsStr::new("new")).is_ok());
    }

    #[test]
    fn symlink_preserves_target() {
        let d = tempdir();
        let s = store(&d);
        let tar = build_tar(|b| {
            write_dir_entry(b, "bin/", 0o755);
            write_symlink_entry(b, "bin/sh", "busybox");
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[tar], &|_, _| {}).unwrap();
        let bin = fs.root.get_directory_mut(OsStr::new("bin")).unwrap();
        let leaf_id = bin.leaf_id(OsStr::new("sh")).unwrap();
        let content = &fs.leaves[leaf_id.0].content;
        assert!(matches!(content, LeafContent::Symlink(_)));
        if let LeafContent::Symlink(target) = content {
            assert_eq!(&**target, OsStr::new("busybox"));
        }
    }

    #[test]
    fn directories_created_implicitly_for_deep_paths() {
        let d = tempdir();
        let s = store(&d);
        let tar = build_tar(|b| {
            write_file_entry(b, "deep/nested/file", 0o644, b"x");
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[tar], &|_, _| {}).unwrap();
        let nested = fs
            .root
            .get_directory_mut(OsStr::new("deep"))
            .unwrap()
            .get_directory_mut(OsStr::new("nested"))
            .unwrap();
        assert!(nested.leaf_id(OsStr::new("file")).is_ok());
    }

    #[test]
    fn dotslash_prefix_is_stripped() {
        let d = tempdir();
        let s = store(&d);
        let tar = build_tar(|b| {
            write_file_entry(b, "./etc/hostname", 0o644, b"sandbox");
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[tar], &|_, _| {}).unwrap();
        let etc = fs.root.get_directory_mut(OsStr::new("etc")).unwrap();
        assert!(etc.leaf_id(OsStr::new("hostname")).is_ok());
    }

    #[test]
    fn normalize_path_rejects_dotdot() {
        let err = normalize_path(Path::new("foo/../escape")).unwrap_err();
        assert!(format!("{err:#}").contains(".."));
    }

    #[test]
    fn normalize_path_rejects_an_overlong_name_component() {
        let long = "a".repeat(MAX_NAME_BYTES + 1);
        let err = normalize_path(Path::new(&long)).unwrap_err();
        assert!(format!("{err:#}").contains("name limit"), "{err:#}");
    }

    #[test]
    fn normalize_path_accepts_a_name_component_at_the_limit() {
        let ok = "a".repeat(MAX_NAME_BYTES);
        assert_eq!(
            normalize_path(Path::new(&ok)).unwrap(),
            Some(PathBuf::from(&ok))
        );
    }

    #[test]
    fn normalize_path_strips_root_and_curdir() {
        assert_eq!(
            normalize_path(Path::new("/etc/./hostname")).unwrap(),
            Some(PathBuf::from("etc/hostname"))
        );
    }

    #[test]
    fn normalize_path_empty_becomes_none() {
        assert_eq!(normalize_path(Path::new("./")).unwrap(), None);
    }

    fn write_typed_entry(
        builder: &mut Builder<Cursor<&mut Vec<u8>>>,
        path: &str,
        entry_type: EntryType,
    ) {
        let mut h = Header::new_gnu();
        h.set_path(path).unwrap();
        h.set_size(0);
        h.set_mode(0o644);
        h.set_uid(0);
        h.set_gid(0);
        h.set_mtime(0);
        h.set_entry_type(entry_type);
        if entry_type == EntryType::Link {
            h.set_link_name("etc/hostname").unwrap();
        }
        h.set_cksum();
        builder.append(&h, Cursor::new(&[][..])).unwrap();
    }

    #[test]
    fn build_filesystem_from_layer_paths_reads_each_layer_from_disk() {
        let d = tempdir();
        let s = store(&d);
        let layer1_bytes = build_tar(|b| write_file_entry(b, "a", 0o644, b"x"));
        let layer2_bytes = build_tar(|b| write_file_entry(b, "b", 0o644, b"y"));
        let l1 = d.path().join("layer1.tar");
        let l2 = d.path().join("layer2.tar");
        std::fs::write(&l1, &layer1_bytes).unwrap();
        std::fs::write(&l2, &layer2_bytes).unwrap();
        let fs = build_filesystem_from_layer_paths(&s, &[l1, l2]).unwrap();
        assert!(fs.root.leaf_id(OsStr::new("a")).is_ok());
        assert!(fs.root.leaf_id(OsStr::new("b")).is_ok());
    }

    #[test]
    fn build_filesystem_from_layer_paths_propagates_missing_file_with_context() {
        let d = tempdir();
        let s = store(&d);
        let missing = d.path().join("does-not-exist.tar");
        let err = build_filesystem_from_layer_paths(&s, &[missing]).unwrap_err();
        assert!(format!("{err:#}").contains("does-not-exist.tar"));
    }

    #[test]
    fn apply_layer_bytes_auto_detects_gzip_magic() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        let tar = build_tar(|b| write_file_entry(b, "etc/hostname", 0o644, b"sandbox"));
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&tar).unwrap();
        let gzipped = gz.finish().unwrap();
        let d = tempdir();
        let s = store(&d);
        let mut fs = build_filesystem_from_layer_bytes(&s, &[gzipped], &|_, _| {}).unwrap();
        let etc = fs.root.get_directory_mut(OsStr::new("etc")).unwrap();
        assert!(etc.leaf_id(OsStr::new("hostname")).is_ok());
    }

    #[test]
    fn strip_wh_prefix_returns_none_for_double_wh_markers() {
        assert_eq!(strip_wh_prefix(b".wh..wh..opq"), None);
        assert_eq!(strip_wh_prefix(b".wh..wh..future"), None);
        assert_eq!(strip_wh_prefix(b".wh.secret"), Some(OsStr::new("secret")));
        assert_eq!(strip_wh_prefix(b"normal"), None);
    }

    #[test]
    fn a_whiteout_without_a_target_is_rejected() {
        let d = tempdir();
        let s = store(&d);
        let layer = build_tar(|b| write_whiteout(b, "etc/.wh."));
        let err = build_filesystem_from_layer_bytes(&s, &[layer], &|_, _| {}).unwrap_err();
        assert!(format!("{err:#}").contains("whiteout path has no target"));
    }

    #[test]
    fn fifo_entries_materialize_as_fifo_leaves_with_ifo_type_bits() {
        let d = tempdir();
        let s = store(&d);
        let tar = build_tar(|b| {
            write_typed_entry(b, "dev/myfifo", EntryType::Fifo);
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[tar], &|_, _| {}).unwrap();
        let dev = fs.root.get_directory_mut(OsStr::new("dev")).unwrap();
        let leaf_id = dev.leaf_id(OsStr::new("myfifo")).unwrap();
        let leaf = &fs.leaves[leaf_id.0];
        assert!(
            matches!(leaf.content, LeafContent::Fifo),
            "got {:?}",
            leaf.content
        );
        assert_eq!(
            leaf.stat.st_mode & 0o170000,
            type_bits(libc::S_IFIFO),
            "fifo type bits in stat",
        );
    }

    #[test]
    fn hardlink_entries_are_silently_dropped() {
        let d = tempdir();
        let s = store(&d);
        let tar = build_tar(|b| {
            write_file_entry(b, "etc/hostname", 0o644, b"sandbox");
            write_typed_entry(b, "etc/host", EntryType::Link);
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[tar], &|_, _| {}).unwrap();
        let etc = fs.root.get_directory_mut(OsStr::new("etc")).unwrap();
        assert!(etc.leaf_id(OsStr::new("hostname")).is_ok());
        assert!(
            etc.leaf_id(OsStr::new("host")).is_err(),
            "hardlink must not be added as a leaf"
        );
    }

    #[test]
    fn char_and_block_devnodes_are_silently_dropped() {
        let d = tempdir();
        let s = store(&d);
        let tar = build_tar(|b| {
            write_dir_entry(b, "dev/", 0o755);
            write_typed_entry(b, "dev/null", EntryType::Char);
            write_typed_entry(b, "dev/sda", EntryType::Block);
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[tar], &|_, _| {}).unwrap();
        let dev = fs.root.get_directory_mut(OsStr::new("dev")).unwrap();
        assert!(
            dev.leaf_id(OsStr::new("null")).is_err(),
            "char devnodes must not materialize as leaves"
        );
        assert!(
            dev.leaf_id(OsStr::new("sda")).is_err(),
            "block devnodes must not materialize as leaves"
        );
    }

    #[test]
    fn empty_path_tar_entries_are_skipped_without_error() {
        let d = tempdir();
        let s = store(&d);
        let tar = build_tar(|b| {
            write_dir_entry(b, "./", 0o755);
            write_file_entry(b, "etc/hostname", 0o644, b"sandbox");
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[tar], &|_, _| {}).unwrap();
        let etc = fs.root.get_directory_mut(OsStr::new("etc")).unwrap();
        assert!(etc.leaf_id(OsStr::new("hostname")).is_ok());
    }

    #[test]
    fn unknown_tar_entry_types_are_silently_dropped() {
        let d = tempdir();
        let s = store(&d);
        let tar = build_tar(|b| {
            write_typed_entry(b, "globalmeta", EntryType::XGlobalHeader);
            write_file_entry(b, "etc/hostname", 0o644, b"sandbox");
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[tar], &|_, _| {}).unwrap();
        let etc = fs.root.get_directory_mut(OsStr::new("etc")).unwrap();
        assert!(etc.leaf_id(OsStr::new("hostname")).is_ok());
        assert!(
            fs.root.leaf_id(OsStr::new("globalmeta")).is_err(),
            "unknown tar entry types must not materialize as leaves"
        );
    }

    #[test]
    fn ensure_directory_overwrites_existing_directory_stat_preserving_children() {
        let d = tempdir();
        let s = store(&d);
        let layer1 = build_tar(|b| {
            write_dir_entry(b, "etc/", 0o700);
            write_file_entry(b, "etc/keep", 0o644, b"keep");
        });
        let layer2 = build_tar(|b| {
            write_dir_entry(b, "etc/", 0o755);
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[layer1, layer2], &|_, _| {}).unwrap();
        let etc = fs.root.get_directory_mut(OsStr::new("etc")).unwrap();
        assert!(
            etc.leaf_id(OsStr::new("keep")).is_ok(),
            "children must survive a layer-2 dir stat overwrite"
        );
    }

    #[test]
    fn ensure_directory_replaces_a_non_directory_at_the_same_path() {
        let d = tempdir();
        let s = store(&d);
        let layer1 = build_tar(|b| write_file_entry(b, "etc", 0o644, b"file-not-dir"));
        let layer2 = build_tar(|b| {
            write_dir_entry(b, "etc/", 0o755);
            write_file_entry(b, "etc/hostname", 0o644, b"sandbox");
        });
        let mut fs = build_filesystem_from_layer_bytes(&s, &[layer1, layer2], &|_, _| {}).unwrap();
        let etc = fs.root.get_directory_mut(OsStr::new("etc")).unwrap();
        assert!(etc.leaf_id(OsStr::new("hostname")).is_ok());
    }

    #[test]
    fn cross_layer_content_dedup_via_digest() {
        let d = tempdir();
        let s = store(&d);
        let layer1 = build_tar(|b| write_file_entry(b, "a", 0o644, b"identical"));
        let layer2 = build_tar(|b| write_file_entry(b, "b", 0o644, b"identical"));
        let _fs = build_filesystem_from_layer_bytes(&s, &[layer1, layer2], &|_, _| {}).unwrap();
        let count = std::fs::read_dir(d.path().join("content").join("sha256"))
            .unwrap()
            .count();
        assert_eq!(count, 1, "identical bytes should dedup in content store");
    }

    #[test]
    fn a_content_store_install_failure_aborts_the_layer_with_context() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempdir();
        let s = store(&d);
        let sha_dir = d.path().join("content").join("sha256");
        std::fs::create_dir_all(&sha_dir).unwrap();
        std::fs::set_permissions(&sha_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let tar = build_tar(|b| write_file_entry(b, "etc/hostname", 0o644, b"sandbox\n"));
        let err = build_filesystem_from_layer_bytes(&s, &[tar], &|_, _| {}).unwrap_err();

        std::fs::set_permissions(&sha_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            format!("{err:#}").contains("installing regular file"),
            "expected install context, got: {err:#}"
        );
    }

    #[test]
    fn a_file_above_the_buffer_cap_is_streamed_and_installed_correctly() {
        let d = tempdir();
        let s = store(&d);
        let big = vec![0xabu8; (MAX_BUFFERED_FILE_BYTES + 1) as usize];
        let tar = build_tar(|b| write_file_entry(b, "opt/model.bin", 0o644, &big));
        let mut fs = build_filesystem_from_layer_bytes(&s, &[tar], &|_, _| {}).unwrap();
        let opt = fs.root.get_directory_mut(OsStr::new("opt")).unwrap();
        let leaf_id = opt.leaf_id(OsStr::new("model.bin")).unwrap();
        assert!(
            matches!(
                &fs.leaves[leaf_id.0].content,
                LeafContent::Regular(RegularFile::External(_, size)) if *size == big.len() as u64
            ),
            "a streamed large file must land as an External blob with its full size"
        );
        let count = std::fs::read_dir(d.path().join("content").join("sha256"))
            .unwrap()
            .count();
        assert_eq!(
            count, 1,
            "the streamed blob must be written to the content store"
        );
    }

    #[test]
    fn a_large_file_install_failure_aborts_the_layer_with_path_context() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempdir();
        let s = store(&d);
        let sha_dir = d.path().join("content").join("sha256");
        std::fs::create_dir_all(&sha_dir).unwrap();
        std::fs::set_permissions(&sha_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let big = vec![0x7eu8; (MAX_BUFFERED_FILE_BYTES + 1) as usize];
        let tar = build_tar(|b| write_file_entry(b, "opt/model.bin", 0o644, &big));
        let err = build_filesystem_from_layer_bytes(&s, &[tar], &|_, _| {}).unwrap_err();

        std::fs::set_permissions(&sha_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            format!("{err:#}").contains("installing opt/model.bin"),
            "expected the streamed-file path in the context, got: {err:#}"
        );
    }

    const ROOMY: u64 = 100 * 1024 * 1024;

    #[test]
    fn a_layer_decompressing_past_the_cap_errors_before_the_store_is_written() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        let big = vec![0u8; (MAX_BUFFERED_FILE_BYTES + 50_000) as usize];
        let tar = build_tar(|b| write_file_entry(b, "opt/bomb.bin", 0o644, &big));
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&tar).unwrap();
        let gzipped = gz.finish().unwrap();
        let d = tempdir();
        let s = store(&d);
        let limits = Limits {
            archive: ArchiveLimits {
                bytes: MAX_BUFFERED_FILE_BYTES + 1024,
                entries: 1000,
            },
            buffered_bytes: ROOMY,
        };
        let err =
            build_filesystem_from_layer_bytes_with_limits(&s, &[gzipped], limits).unwrap_err();
        assert!(
            format!("{err:#}").contains("decompressed layer exceeds"),
            "got: {err:#}"
        );
        let sha_dir = d.path().join("content").join("sha256");
        let landed = std::fs::read_dir(&sha_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| !e.file_name().to_string_lossy().contains(".tmp."))
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(landed, 0, "no full blob may land once the cap is hit");
    }

    #[test]
    fn limited_reader_never_returns_more_than_the_budget_in_one_read() {
        let source = vec![0xaau8; 10_000];
        let mut reader = LimitedReader::new(Cursor::new(source), 100, "decompressed layer");
        let mut buf = [0u8; 4096];
        let n = reader.read(&mut buf).unwrap();
        assert!(
            n <= 100,
            "a single read decompressed {n} bytes past the 100-byte budget"
        );
        assert_eq!(reader.read, n as u64);
    }

    #[test]
    fn limited_reader_errors_once_the_budget_is_exhausted_and_bytes_remain() {
        let source = vec![0xbbu8; 200];
        let mut reader = LimitedReader::new(Cursor::new(source), 100, "decompressed layer");
        let mut buf = [0u8; 100];
        assert_eq!(reader.read(&mut buf).unwrap(), 100);
        let err = reader.read(&mut buf).unwrap_err();
        assert!(
            err.to_string().contains("exceeds the 100-byte cap"),
            "got: {err}"
        );
    }

    #[test]
    fn limited_reader_reports_clean_eof_when_the_source_exactly_fills_the_budget() {
        let source = vec![0xccu8; 100];
        let mut reader = LimitedReader::new(Cursor::new(source), 100, "decompressed layer");
        let mut buf = [0u8; 100];
        assert_eq!(reader.read(&mut buf).unwrap(), 100);
        assert_eq!(
            reader.read(&mut buf).unwrap(),
            0,
            "exact-fit source must EOF, not error"
        );
    }

    #[test]
    fn entry_count_over_the_cap_errors() {
        let d = tempdir();
        let s = store(&d);
        let tar = build_tar(|b| {
            for i in 0..20 {
                write_file_entry(b, &format!("f{i}"), 0o644, b"x");
            }
        });
        let limits = Limits {
            archive: ArchiveLimits {
                bytes: ROOMY,
                entries: 8,
            },
            buffered_bytes: ROOMY,
        };
        let err = build_filesystem_from_layer_bytes_with_limits(&s, &[tar], limits).unwrap_err();
        assert!(format!("{err:#}").contains("8-entry cap"), "got: {err:#}");
    }

    #[test]
    fn cumulative_buffered_bytes_over_the_cap_errors() {
        let d = tempdir();
        let s = store(&d);
        let chunk = vec![0xcdu8; 2000];
        let tar = build_tar(|b| {
            write_file_entry(b, "a", 0o644, &chunk);
            write_file_entry(b, "b", 0o644, &chunk);
            write_file_entry(b, "c", 0o644, &chunk);
        });
        let limits = Limits {
            archive: ArchiveLimits {
                bytes: ROOMY,
                entries: 1000,
            },
            buffered_bytes: 4096,
        };
        let err = build_filesystem_from_layer_bytes_with_limits(&s, &[tar], limits).unwrap_err();
        assert!(
            format!("{err:#}").contains("RAM-buffered content exceeds"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_layer_within_all_caps_builds_successfully() {
        let d = tempdir();
        let s = store(&d);
        let tar = build_tar(|b| write_file_entry(b, "etc/ok", 0o644, b"fits"));
        let limits = Limits {
            archive: ArchiveLimits {
                bytes: ROOMY,
                entries: 1000,
            },
            buffered_bytes: ROOMY,
        };
        let mut fs = build_filesystem_from_layer_bytes_with_limits(&s, &[tar], limits).unwrap();
        let etc = fs.root.get_directory_mut(OsStr::new("etc")).unwrap();
        assert!(etc.leaf_id(OsStr::new("ok")).is_ok());
    }

    #[test]
    fn production_limits_are_the_named_consts() {
        assert_eq!(Limits::PRODUCTION.archive.bytes, 16 * 1024 * 1024 * 1024);
        assert_eq!(Limits::PRODUCTION.archive.entries, 4_000_000);
        assert_eq!(Limits::PRODUCTION.buffered_bytes, MAX_BUFFERED_LAYER_BYTES);
    }

    #[test]
    fn apply_ops_errors_when_a_regular_leaf_has_no_installed_digest() {
        let mut fs = FileSystem::<Sha256Digest>::new(root_stat());
        let ops = vec![Op::RegularLeaf {
            parents: vec![],
            name: OsString::from("orphan"),
            stat: default_dir_stat(),
            seq: 0,
        }];
        let err = apply_ops(&mut fs, ops, &mut []).unwrap_err();
        assert!(
            format!("{err:#}").contains("no installed content for regular file #0"),
            "got: {err:#}"
        );
    }
}
