#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::io;
use std::path::Path;

/// Guest writes a mount would have covered: the host stages them here instead, and `land` copies them onto the paths they name once the mounts are up.
pub const STAGED_ROOT: &str = "/.lens/fileset-deferred";

/// `boot_owned` names each mount root whose subtree this boot laid out itself — a volume target, and the target of a bind it split.
pub fn land(newroot: &str, boot_owned: &[String]) -> io::Result<()> {
    let staged = Path::new(newroot).join(&STAGED_ROOT[1..]);
    if !staged.is_dir() {
        return Ok(());
    }
    land_tree(&staged, Path::new(newroot), "", boot_owned)?;
    consume(&staged);
    Ok(())
}

/// Leaving the landed tree behind would show the workload a root-owned duplicate of every file, and it has nothing left to do once the copies are in place.
fn consume(staged: &Path) {
    if let Err(err) = std::fs::remove_dir_all(staged) {
        eprintln!("lns-init: could not remove {}: {err}", staged.display());
    }
}

fn land_tree(from: &Path, onto: &Path, guest: &str, boot_owned: &[String]) -> io::Result<()> {
    for entry in from.read_dir().map_err(|err| blaming(from, err))? {
        let entry = entry.map_err(|err| blaming(from, err))?;
        let staged = entry.path();
        let landing = onto.join(entry.file_name());
        let guest_path = format!("{guest}/{}", entry.file_name().to_string_lossy());
        let staged_is_dir = entry
            .file_type()
            .map_err(|err| blaming(&staged, err))?
            .is_dir();
        if staged_is_dir {
            if inside_what_the_boot_laid_out(&guest_path, boot_owned) {
                unlink_a_symlink_in_the_way(&landing).map_err(|err| blaming(&landing, err))?;
            }
            create_dir_if_absent(&landing).map_err(|err| blaming(&landing, err))?;
            land_tree(&staged, &landing, &guest_path, boot_owned)?;
        } else {
            replace(&staged, &landing).map_err(|err| blaming(&landing, err))?;
        }
    }
    Ok(())
}

/// Inside one of these the layout is this boot's own and what a previous run wrote survives, so a symlink at a directory position is the last workload's doing; above them it is the image's, and the mount stands at the path it resolves to.
fn inside_what_the_boot_laid_out(guest_path: &str, boot_owned: &[String]) -> bool {
    boot_owned
        .iter()
        .any(|root| segments(guest_path).starts_with(&segments(root)))
}

fn segments(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect()
}

/// A symlink the last run planted inside a mount this boot laid out must not redirect where root writes.
fn unlink_a_symlink_in_the_way(path: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(path).is_ok_and(|found| found.file_type().is_symlink()) {
        return std::fs::remove_file(path);
    }
    Ok(())
}

/// A boot refusal has to name the path that refused, or the operator is left with a bare errno.
fn blaming(path: &Path, err: io::Error) -> io::Error {
    io::Error::new(err.kind(), format!("{}: {err}", path.display()))
}

/// A directory a mount already owns keeps the mode and owner it came with, so only one nothing has created yet is made here.
fn create_dir_if_absent(path: &Path) -> io::Result<()> {
    match std::fs::create_dir(path) {
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        other => other,
    }
}

fn replace(staged: &Path, landing: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(landing).is_ok() {
        std::fs::remove_file(landing)?;
    }
    if staged.symlink_metadata()?.file_type().is_symlink() {
        return std::os::unix::fs::symlink(std::fs::read_link(staged)?, landing);
    }
    std::fs::copy(staged, landing).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn stage(newroot: &Path, guest_path: &str, body: &[u8], mode: u32) {
        let staged = newroot.join(&STAGED_ROOT[1..]).join(&guest_path[1..]);
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::fs::write(&staged, body).unwrap();
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_staged_write_lands_on_the_path_it_names_with_its_mode() {
        let root = tempfile::tempdir().unwrap();
        stage(root.path(), "/home/node/.config/tool.md", b"read me", 0o600);

        land(root.path().to_str().unwrap(), &[]).unwrap();

        let landed = root.path().join("home/node/.config/tool.md");
        assert_eq!(std::fs::read(&landed).unwrap(), b"read me");
        assert_eq!(
            std::fs::metadata(&landed).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_staged_write_replaces_what_the_last_boot_left_in_the_volume() {
        let root = tempfile::tempdir().unwrap();
        let volume = root.path().join("home/node");
        std::fs::create_dir_all(&volume).unwrap();
        std::fs::write(volume.join("tool.md"), b"stale").unwrap();
        stage(root.path(), "/home/node/tool.md", b"fresh", 0o644);

        land(root.path().to_str().unwrap(), &[]).unwrap();

        assert_eq!(
            std::fs::read(volume.join("tool.md")).unwrap(),
            b"fresh",
            "the host file is what this boot declares, so it wins over what the volume kept"
        );
    }

    #[test]
    fn a_directory_the_mount_already_owns_keeps_its_own_mode() {
        let root = tempfile::tempdir().unwrap();
        let volume = root.path().join("home/node");
        std::fs::create_dir_all(&volume).unwrap();
        std::fs::set_permissions(&volume, std::fs::Permissions::from_mode(0o700)).unwrap();
        stage(root.path(), "/home/node/tool.md", b"x", 0o644);

        land(root.path().to_str().unwrap(), &[]).unwrap();

        assert_eq!(
            std::fs::metadata(&volume).unwrap().permissions().mode() & 0o777,
            0o700,
            "landing a file must not restate the volume root's own mode"
        );
    }

    #[test]
    fn a_staged_symlink_lands_as_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let staged = root.path().join(&STAGED_ROOT[1..]).join("home/node");
        std::fs::create_dir_all(&staged).unwrap();
        std::os::unix::fs::symlink("/.lens/bin/agent", staged.join("agent")).unwrap();

        land(root.path().to_str().unwrap(), &[]).unwrap();

        let landed = root.path().join("home/node/agent");
        assert_eq!(
            std::fs::read_link(&landed).unwrap(),
            Path::new("/.lens/bin/agent")
        );
    }

    #[test]
    fn a_symlink_the_last_run_left_at_a_directory_position_does_not_redirect_the_landing() {
        let root = tempfile::tempdir().unwrap();
        let volume = root.path().join("home/node");
        std::fs::create_dir_all(&volume).unwrap();
        let elsewhere = root.path().join("etc");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, volume.join(".config")).unwrap();
        stage(root.path(), "/home/node/.config/tool.md", b"x", 0o644);

        land(root.path().to_str().unwrap(), &["/home/node".to_string()]).unwrap();

        assert!(
            !elsewhere.join("tool.md").exists(),
            "a root-driven write must land where the document says, not where the workload pointed"
        );
        assert_eq!(std::fs::read(volume.join(".config/tool.md")).unwrap(), b"x");
    }

    /// A split bind's target is guest-local and preserved across a restart, exactly as a volume is, so the last workload could have left a symlink at the mask a fileset writes into.
    #[test]
    fn a_symlink_the_last_run_left_in_a_split_binds_mask_does_not_redirect_the_landing() {
        let root = tempfile::tempdir().unwrap();
        let bind = root.path().join("root/.agent");
        std::fs::create_dir_all(&bind).unwrap();
        let elsewhere = root.path().join("work");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, bind.join("session")).unwrap();
        stage(root.path(), "/root/.agent/session/state.json", b"x", 0o644);

        land(root.path().to_str().unwrap(), &["/root/.agent".to_string()]).unwrap();

        assert!(
            !elsewhere.join("state.json").exists(),
            "the link's target is a bound host directory, so following it writes the file to the host"
        );
        assert_eq!(
            std::fs::read(bind.join("session/state.json")).unwrap(),
            b"x"
        );
    }

    #[test]
    fn a_directory_symlink_the_image_ships_above_the_volume_survives_and_is_followed() {
        let root = tempfile::tempdir().unwrap();
        let volume = root.path().join("var/home/node");
        std::fs::create_dir_all(&volume).unwrap();
        std::os::unix::fs::symlink("var/home", root.path().join("home")).unwrap();
        stage(root.path(), "/home/node/tool.md", b"x", 0o644);

        land(root.path().to_str().unwrap(), &["/home/node".to_string()]).unwrap();

        assert!(
            std::fs::symlink_metadata(root.path().join("home"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the volume is mounted at the path this symlink resolves to, so replacing it strands the mount"
        );
        assert_eq!(std::fs::read(volume.join("tool.md")).unwrap(), b"x");
    }

    #[test]
    fn the_staged_tree_is_gone_once_landed_so_the_workload_sees_no_duplicate() {
        let root = tempfile::tempdir().unwrap();
        stage(root.path(), "/home/node/tool.md", b"x", 0o644);

        land(root.path().to_str().unwrap(), &[]).unwrap();

        assert!(!root.path().join(&STAGED_ROOT[1..]).exists());
        assert_eq!(
            std::fs::read(root.path().join("home/node/tool.md")).unwrap(),
            b"x"
        );
    }

    #[test]
    fn a_staged_tree_that_cannot_be_removed_does_not_fail_the_boot() {
        let root = tempfile::tempdir().unwrap();
        stage(root.path(), "/home/node/tool.md", b"x", 0o644);
        let lens = root.path().join(".lens");
        std::fs::set_permissions(&lens, std::fs::Permissions::from_mode(0o500)).unwrap();

        let landed = land(root.path().to_str().unwrap(), &[]);

        std::fs::set_permissions(&lens, std::fs::Permissions::from_mode(0o700)).unwrap();
        landed.expect("the copies are in place, so a leftover staging tree is not worth a refusal");
        assert_eq!(
            std::fs::read(root.path().join("home/node/tool.md")).unwrap(),
            b"x"
        );
    }

    #[test]
    fn a_run_that_staged_nothing_lands_nothing() {
        let root = tempfile::tempdir().unwrap();
        land(root.path().to_str().unwrap(), &[]).unwrap();
        assert!(!root.path().join("home").exists());
    }
}
