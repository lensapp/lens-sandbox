use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;

use anyhow::{Result, bail};

use crate::composefs::vendor::generic_tree::{Directory, FileSystem, Inode, LeafContent};

const MAX_SYMLINK_HOPS: u32 = 40;

pub(crate) fn resolve_parent_segments<C>(
    fs: &FileSystem<C>,
    parents: &[OsString],
) -> Result<Vec<OsString>> {
    let mut resolved: Vec<OsString> = Vec::new();
    let mut pending: VecDeque<OsString> = parents.iter().cloned().collect();
    let mut hops = 0u32;

    while let Some(seg) = pending.pop_front() {
        let s = seg.as_os_str();
        if s.is_empty() || s == OsStr::new(".") {
            continue;
        }
        if s == OsStr::new("..") {
            resolved.pop();
            continue;
        }

        let Some(here) = dir_at(&fs.root, &resolved) else {
            resolved.push(seg);
            continue;
        };
        match here.lookup(s) {
            None | Some(Inode::Directory(_)) => resolved.push(seg),
            Some(Inode::Leaf(id, _)) => match &fs.leaves[id.0].content {
                LeafContent::Symlink(target) => {
                    hops += 1;
                    if hops > MAX_SYMLINK_HOPS {
                        bail!("symlink loop resolving runtime injection path at {seg:?}");
                    }
                    splice_symlink_target(target, &mut resolved, &mut pending);
                }
                _ => bail!("descending into {seg:?} (not a directory)"),
            },
        }
    }
    Ok(resolved)
}

fn dir_at<'a, C>(root: &'a Directory<C>, segs: &[OsString]) -> Option<&'a Directory<C>> {
    let mut dir = root;
    for seg in segs {
        match dir.lookup(seg.as_os_str()) {
            Some(Inode::Directory(child)) => dir = child.as_ref(),
            _ => return None,
        }
    }
    Some(dir)
}

fn splice_symlink_target(
    target: &OsStr,
    resolved: &mut Vec<OsString>,
    pending: &mut VecDeque<OsString>,
) {
    let bytes = target.as_bytes();
    if bytes.first() == Some(&b'/') {
        resolved.clear();
    }
    let parts: Vec<OsString> = bytes
        .split(|&b| b == b'/')
        .map(|p| OsStr::from_bytes(p).to_os_string())
        .collect();
    for part in parts.into_iter().rev() {
        pending.push_front(part);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composefs::Sha256Digest;
    use crate::composefs::vendor::generic_tree::Stat;
    use crate::composefs::vendor::tree::{
        Directory as TreeDir, FileSystem as TreeFs, Inode as TreeInode, RegularFile,
    };
    use std::collections::BTreeMap;

    fn stat() -> Stat {
        Stat {
            st_mode: 0,
            st_uid: 0,
            st_gid: 0,
            st_mtim_sec: 0,
            xattrs: BTreeMap::new(),
        }
    }

    fn fs() -> TreeFs<Sha256Digest> {
        TreeFs::<Sha256Digest>::new(stat())
    }

    fn segs(parts: &[&str]) -> Vec<OsString> {
        parts.iter().map(OsString::from).collect()
    }

    fn mkdir(parent: &mut TreeDir<Sha256Digest>, name: &str) {
        parent.insert(
            OsStr::new(name),
            TreeInode::Directory(Box::new(TreeDir::new(stat()))),
        );
    }

    fn mksymlink(fs: &mut TreeFs<Sha256Digest>, parent_dir: &str, name: &str, target: &str) {
        let leaf = fs.push_leaf(stat(), LeafContent::Symlink(OsStr::new(target).into()));
        let parent = if parent_dir.is_empty() {
            &mut fs.root
        } else {
            fs.root
                .get_directory_mut(OsStr::new(parent_dir))
                .expect("symlink parent dir exists")
        };
        parent.insert(OsStr::new(name), TreeInode::leaf(leaf));
    }

    fn resolve(fs: &TreeFs<Sha256Digest>, parts: &[&str]) -> Result<Vec<String>> {
        resolve_parent_segments(fs, &segs(parts))
            .map(|v| v.iter().map(|s| s.to_string_lossy().into_owned()).collect())
    }

    #[test]
    fn real_directory_chain_resolves_unchanged() {
        let mut fs = fs();
        mkdir(&mut fs.root, "usr");
        let usr = fs.root.get_directory_mut(OsStr::new("usr")).unwrap();
        mkdir(usr, "lib");
        assert_eq!(resolve(&fs, &["usr", "lib"]).unwrap(), vec!["usr", "lib"]);
    }

    #[test]
    fn nonexistent_parents_pass_through_verbatim() {
        let fs = fs();
        assert_eq!(
            resolve(&fs, &["opt", "vendor"]).unwrap(),
            vec!["opt", "vendor"]
        );
    }

    #[test]
    fn relative_parent_symlink_is_followed() {
        let mut fs = fs();
        mkdir(&mut fs.root, "usr");
        let usr = fs.root.get_directory_mut(OsStr::new("usr")).unwrap();
        mkdir(usr, "lib");
        mksymlink(&mut fs, "", "lib", "usr/lib");
        assert_eq!(resolve(&fs, &["lib"]).unwrap(), vec!["usr", "lib"]);
    }

    #[test]
    fn absolute_parent_symlink_resets_to_root() {
        let mut fs = fs();
        mkdir(&mut fs.root, "usr");
        let usr = fs.root.get_directory_mut(OsStr::new("usr")).unwrap();
        mkdir(usr, "lib");
        mksymlink(&mut fs, "", "lib", "/usr/lib");
        assert_eq!(resolve(&fs, &["lib"]).unwrap(), vec!["usr", "lib"]);
    }

    #[test]
    fn symlink_target_directory_need_not_exist_yet() {
        let mut fs = fs();
        mksymlink(&mut fs, "", "lib", "usr/lib");
        assert_eq!(resolve(&fs, &["lib"]).unwrap(), vec!["usr", "lib"]);
    }

    #[test]
    fn chained_symlinks_resolve_to_final_directory() {
        let mut fs = fs();
        mkdir(&mut fs.root, "c");
        mksymlink(&mut fs, "", "b", "c");
        mksymlink(&mut fs, "", "a", "b");
        assert_eq!(resolve(&fs, &["a"]).unwrap(), vec!["c"]);
    }

    #[test]
    fn dotdot_in_symlink_target_is_normalized() {
        let mut fs = fs();
        mkdir(&mut fs.root, "usr");
        let usr = fs.root.get_directory_mut(OsStr::new("usr")).unwrap();
        mkdir(usr, "lib");
        mkdir(&mut fs.root, "top");
        mksymlink(&mut fs, "top", "lib", "../usr/lib");
        assert_eq!(resolve(&fs, &["top", "lib"]).unwrap(), vec!["usr", "lib"]);
    }

    #[test]
    fn dot_in_symlink_target_is_skipped() {
        let mut fs = fs();
        mkdir(&mut fs.root, "usr");
        let usr = fs.root.get_directory_mut(OsStr::new("usr")).unwrap();
        mkdir(usr, "lib");
        mksymlink(&mut fs, "", "lib", "usr/./lib");
        assert_eq!(resolve(&fs, &["lib"]).unwrap(), vec!["usr", "lib"]);
    }

    #[test]
    fn symlink_loop_is_rejected() {
        let mut fs = fs();
        mksymlink(&mut fs, "", "a", "b");
        mksymlink(&mut fs, "", "b", "a");
        let err = resolve(&fs, &["a"]).unwrap_err();
        assert!(format!("{err:#}").contains("symlink loop"));
    }

    #[test]
    fn regular_file_parent_is_not_a_directory() {
        let mut fs = fs();
        let leaf = fs.push_leaf(
            stat(),
            LeafContent::Regular(RegularFile::External(Sha256Digest::from([0; 32]), 0)),
        );
        fs.root.insert(OsStr::new("lib"), TreeInode::leaf(leaf));
        let err = resolve(&fs, &["lib", "x"]).unwrap_err();
        assert!(format!("{err:#}").contains("not a directory"));
    }
}
