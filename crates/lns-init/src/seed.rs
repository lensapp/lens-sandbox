#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::io;
use std::path::Path;

pub fn is_pristine(path: &Path) -> io::Result<bool> {
    for entry in path.read_dir()? {
        if entry?.file_name() != "lost+found" {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn prepare_pristine_volume(
    vol_root: &Path,
    image_seed: &Path,
    uid: u32,
    gid: u32,
) -> io::Result<bool> {
    if !is_pristine(vol_root)? {
        return Ok(false);
    }
    if image_seed.is_dir() {
        copy_dir_recursive(image_seed, vol_root)?;
    }
    chown_recursive(vol_root, uid, gid)?;
    Ok(true)
}

pub fn chown_recursive(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    for entry in path.read_dir()? {
        let entry = entry?;
        if is_fs_reserved(&entry.file_name()) {
            continue;
        }
        if entry.file_type()?.is_dir() {
            chown_recursive(&entry.path(), uid, gid)?;
        } else {
            std::os::unix::fs::lchown(entry.path(), Some(uid), Some(gid))?;
        }
    }
    std::os::unix::fs::lchown(path, Some(uid), Some(gid))
}

fn is_fs_reserved(name: &std::ffi::OsStr) -> bool {
    name == "lost+found"
}

pub fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in src.read_dir()? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_symlink() {
            let target = std::fs::read_link(&from)?;
            std::os::unix::fs::symlink(&target, &to)?;
            mirror_symlink_owner(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
            mirror_owner_and_mode(&from, &to)?;
        }
    }
    mirror_owner_and_mode(src, dst)?;
    Ok(())
}

fn mirror_owner_and_mode(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let md = std::fs::metadata(src)?;
    std::os::unix::fs::chown(dst, Some(md.uid()), Some(md.gid()))?;
    std::fs::set_permissions(dst, std::fs::Permissions::from_mode(md.mode()))
}

fn mirror_symlink_owner(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::symlink_metadata(src)?;
    std::os::unix::fs::lchown(dst, Some(md.uid()), Some(md.gid()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn current_ids(probe_dir: &Path) -> (u32, u32) {
        let probe = probe_dir.join(".probe");
        std::fs::write(&probe, b"").unwrap();
        let md = std::fs::metadata(&probe).unwrap();
        (md.uid(), md.gid())
    }

    #[test]
    fn prepare_pristine_volume_chowns_an_empty_volume_so_the_workload_can_write() {
        let vol = tempfile::tempdir().unwrap();
        std::fs::create_dir(vol.path().join("lost+found")).unwrap();
        let probe = tempfile::tempdir().unwrap();
        let (uid, gid) = current_ids(probe.path());

        let seeded =
            prepare_pristine_volume(vol.path(), &probe.path().join("absent"), uid, gid).unwrap();

        assert!(seeded);
        let md = std::fs::metadata(vol.path()).unwrap();
        assert_eq!((md.uid(), md.gid()), (uid, gid));
    }

    #[test]
    fn prepare_pristine_volume_seeds_then_chowns_when_the_image_ships_the_target() {
        let img = tempfile::tempdir().unwrap();
        std::fs::write(img.path().join("seed.txt"), b"x").unwrap();
        let vol = tempfile::tempdir().unwrap();
        let (uid, gid) = current_ids(img.path());

        let seeded = prepare_pristine_volume(vol.path(), img.path(), uid, gid).unwrap();

        assert!(seeded);
        assert_eq!(std::fs::read(vol.path().join("seed.txt")).unwrap(), b"x");
        let md = std::fs::metadata(vol.path().join("seed.txt")).unwrap();
        assert_eq!((md.uid(), md.gid()), (uid, gid));
    }

    #[test]
    fn prepare_pristine_volume_leaves_a_populated_volume_untouched() {
        let img = tempfile::tempdir().unwrap();
        std::fs::write(img.path().join("from-image"), b"x").unwrap();
        let vol = tempfile::tempdir().unwrap();
        std::fs::write(vol.path().join("existing.db"), b"data").unwrap();
        let (uid, gid) = current_ids(img.path());

        let seeded = prepare_pristine_volume(vol.path(), img.path(), uid, gid).unwrap();

        assert!(!seeded);
        assert!(!vol.path().join("from-image").exists());
        assert_eq!(
            std::fs::read(vol.path().join("existing.db")).unwrap(),
            b"data"
        );
    }

    #[test]
    fn chown_recursive_descends_into_nested_dirs_files_and_symlinks() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("sub")).unwrap();
        std::fs::write(root.path().join("sub/inner.txt"), b"i").unwrap();
        std::os::unix::fs::symlink("inner.txt", root.path().join("sub/link")).unwrap();
        let (uid, gid) = current_ids(root.path());

        chown_recursive(root.path(), uid, gid).unwrap();

        for p in ["sub", "sub/inner.txt", "sub/link"] {
            let md = std::fs::symlink_metadata(root.path().join(p)).unwrap();
            assert_eq!((md.uid(), md.gid()), (uid, gid), "{p}");
        }
    }

    #[test]
    fn chown_recursive_does_not_descend_into_the_ext4_reserved_lost_found() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let lf = root.path().join("lost+found");
        std::fs::create_dir(&lf).unwrap();
        let canary = lf.join("0000001");
        std::fs::write(&canary, b"x").unwrap();
        std::fs::set_permissions(&lf, std::fs::Permissions::from_mode(0o000)).unwrap();
        let (uid, gid) = current_ids(root.path());

        chown_recursive(root.path(), uid, gid).unwrap();

        std::fs::set_permissions(&lf, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(canary.exists());
    }

    #[test]
    fn is_fs_reserved_matches_only_lost_found() {
        use std::ffi::OsStr;
        assert!(is_fs_reserved(OsStr::new("lost+found")));
        assert!(!is_fs_reserved(OsStr::new("data")));
        assert!(!is_fs_reserved(OsStr::new("lost+found.bak")));
    }

    #[test]
    fn pristine_true_for_empty_dir() {
        let d = tempfile::tempdir().unwrap();
        assert!(is_pristine(d.path()).unwrap());
    }

    #[test]
    fn pristine_true_when_only_lost_found_present() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("lost+found")).unwrap();
        assert!(is_pristine(d.path()).unwrap());
    }

    #[test]
    fn pristine_false_when_real_content_exists_alongside_lost_found() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("lost+found")).unwrap();
        std::fs::write(d.path().join("db.sqlite"), b"x").unwrap();
        assert!(!is_pristine(d.path()).unwrap());
    }

    #[test]
    fn pristine_errors_on_missing_path() {
        let d = tempfile::tempdir().unwrap();
        is_pristine(&d.path().join("nope")).unwrap_err();
    }

    #[test]
    fn copy_dir_recursive_copies_nested_files_dirs_and_symlinks() {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("top.txt"), b"top").unwrap();
        std::fs::create_dir(src.path().join("sub")).unwrap();
        std::fs::write(src.path().join("sub/inner.txt"), b"inner").unwrap();
        std::os::unix::fs::symlink("top.txt", src.path().join("link")).unwrap();

        let dst = tempfile::tempdir().unwrap();
        let into = dst.path().join("seeded");
        copy_dir_recursive(src.path(), &into).unwrap();

        assert_eq!(std::fs::read(into.join("top.txt")).unwrap(), b"top");
        assert_eq!(std::fs::read(into.join("sub/inner.txt")).unwrap(), b"inner");
        let link = std::fs::read_link(into.join("link")).unwrap();
        assert_eq!(link, Path::new("top.txt"));
    }

    #[test]
    fn copy_dir_recursive_creates_destination_when_absent() {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a"), b"1").unwrap();
        let dst = tempfile::tempdir().unwrap();
        let into = dst.path().join("a/b/c");
        copy_dir_recursive(src.path(), &into).unwrap();
        assert!(into.join("a").exists());
    }

    #[test]
    fn copy_dir_recursive_preserves_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("secret");
        std::fs::write(&f, b"x").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        let dst = tempfile::tempdir().unwrap();
        let into = dst.path().join("seeded");
        copy_dir_recursive(src.path(), &into).unwrap();
        let mode = std::fs::metadata(into.join("secret"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn copy_dir_recursive_preserves_dir_mode_after_populating_it() {
        use std::os::unix::fs::PermissionsExt;
        let src = tempfile::tempdir().unwrap();
        std::fs::create_dir(src.path().join("d")).unwrap();
        std::fs::write(src.path().join("d/inner"), b"y").unwrap();
        std::fs::set_permissions(src.path().join("d"), std::fs::Permissions::from_mode(0o750))
            .unwrap();
        let dst = tempfile::tempdir().unwrap();
        let into = dst.path().join("seeded");
        copy_dir_recursive(src.path(), &into).unwrap();
        assert_eq!(std::fs::read(into.join("d/inner")).unwrap(), b"y");
        let mode = std::fs::metadata(into.join("d"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o750);
    }

    #[test]
    fn copy_dir_recursive_mirrors_source_ownership() {
        use std::os::unix::fs::MetadataExt;
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a"), b"1").unwrap();
        std::os::unix::fs::symlink("a", src.path().join("l")).unwrap();
        let dst = tempfile::tempdir().unwrap();
        let into = dst.path().join("seeded");
        copy_dir_recursive(src.path(), &into).unwrap();
        let s = std::fs::metadata(src.path().join("a")).unwrap();
        let d = std::fs::metadata(into.join("a")).unwrap();
        assert_eq!((d.uid(), d.gid()), (s.uid(), s.gid()));
        let sl = std::fs::symlink_metadata(src.path().join("l")).unwrap();
        let dl = std::fs::symlink_metadata(into.join("l")).unwrap();
        assert_eq!((dl.uid(), dl.gid()), (sl.uid(), sl.gid()));
    }
}
