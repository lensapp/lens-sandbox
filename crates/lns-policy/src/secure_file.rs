use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Atomically installs `contents` at a `.json` `path` with mode 0600 via a NOFOLLOW create-new tmp + rename, so a stale tmp or planted symlink cannot leak the secret through broader perms.
pub fn write_json_secret_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    // Drop any leftover tmp so the next open is create_new — `.mode(0o600)` is ignored on an existing file, so a stale tmp with broader perms (or a planted symlink) would otherwise carry through the rename and leave the secret world-readable.
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)?;
    f.write_all(contents)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)?;
    Ok(())
}

pub struct SidecarLock {
    _file: fs::File,
}

/// Takes the exclusive advisory lock that serializes a sidecar's read-modify-write across processes, on a companion file (never the sidecar itself, whose inode a rename-install replaces); the lock is the kernel's, so it releases when the guard drops and a crashed holder can never wedge later writers.
pub fn lock_sidecar_exclusive(path: &Path) -> io::Result<SidecarLock> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.lock()?;
    Ok(SidecarLock { _file: file })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn writes_contents_readable_back() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("secret.json");
        write_json_secret_atomic(&path, b"payload").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"payload");
    }

    #[test]
    fn creates_parent_directory_when_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let nested = dir.path().join("a/b/c/secret.json");
        write_json_secret_atomic(&nested, b"{}").unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn uses_tmp_rename_pattern_leaving_no_stale_tmp() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("secret.json");
        write_json_secret_atomic(&path, b"{}").unwrap();
        assert!(
            !path.with_extension("json.tmp").exists(),
            "tmp must be renamed away, not left in place"
        );
    }

    #[test]
    fn writes_file_with_mode_0600_so_a_secret_is_not_world_readable() {
        // Must be 0600 regardless of umask (default 022 would yield 0644, readable by other local users) since the file holds a plaintext credential.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("secret.json");
        write_json_secret_atomic(&path, b"{}").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "got 0o{mode:o}, want 0o600");
    }

    #[test]
    fn pre_existing_loose_perm_tmp_is_replaced_at_mode_0600() {
        // A leftover `*.json.tmp` with default-umask 0o644 must not be reused: `create_new` would fail or `.mode()` would be ignored, so the rename could otherwise produce a world-readable secret file.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("secret.json");
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, b"stale leftover from a prior crash").unwrap();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644)).unwrap();

        write_json_secret_atomic(&path, b"{}").unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "stale tmp must not carry its 0o644 perms through the rename, got 0o{mode:o}"
        );
    }

    #[test]
    fn propagates_non_not_found_error_when_clearing_leftover_tmp() {
        // If the tmp path is occupied by a directory (e.g. a stray `*.json.tmp/` planted by another process), `fs::remove_file` returns a non-NotFound error and the write must surface it rather than fall through and clobber.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("secret.json");
        fs::create_dir(path.with_extension("json.tmp")).unwrap();

        let err = write_json_secret_atomic(&path, b"{}").unwrap_err();

        assert_ne!(
            err.kind(),
            io::ErrorKind::NotFound,
            "must propagate a non-NotFound error, got {err:?}"
        );
        assert!(
            !path.exists(),
            "the target file must not appear when the tmp clear failed"
        );
    }

    #[test]
    fn does_not_follow_a_symlink_planted_at_the_tmp_path() {
        // Before the NOFOLLOW guard: open(tmp) followed a symlink and wrote the secret into whatever the symlink pointed at (attacker_target).
        let dir = tempfile::TempDir::new().unwrap();
        let attacker_target = dir.path().join("attacker-target");
        let attacker_contents = b"victim-data-must-survive";
        fs::write(&attacker_target, attacker_contents).unwrap();
        let path = dir.path().join("secret.json");
        std::os::unix::fs::symlink(&attacker_target, path.with_extension("json.tmp")).unwrap();

        let _ = write_json_secret_atomic(&path, b"{}");

        assert_eq!(
            fs::read(&attacker_target).unwrap(),
            attacker_contents,
            "a symlink at the tmp path must not redirect the secret write"
        );
    }

    #[test]
    fn overwrites_an_existing_file_atomically() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("secret.json");
        write_json_secret_atomic(&path, b"first").unwrap();
        write_json_secret_atomic(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
    }

    #[test]
    fn bare_filename_path_writes_into_cwd_without_panicking() {
        // Uses a tempdir rather than `set_current_dir`, which would race with sibling tests.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("secret.json");
        write_json_secret_atomic(&path, b"a").unwrap();
        write_json_secret_atomic(&path, b"b").unwrap();
        assert!(path.exists());
    }

    #[test]
    fn lock_excludes_an_independent_open_file_description_until_the_guard_drops() {
        // A second open of the same path is a distinct open-file-description, which flock treats exactly as another process would — so this pins the cross-process exclusion without spawning one.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sidecar.json.lock");
        let guard = lock_sidecar_exclusive(&path).unwrap();

        let contender = fs::File::open(&path).unwrap();
        assert!(
            matches!(contender.try_lock(), Err(fs::TryLockError::WouldBlock)),
            "a second holder must be excluded while the guard lives"
        );
        drop(contender);

        drop(guard);
        let after = fs::File::open(&path).unwrap();
        assert!(
            after.try_lock().is_ok(),
            "dropping the guard must release the lock, so a crashed holder can never wedge later writers"
        );
    }

    #[test]
    fn lock_creates_the_lockfile_and_missing_parents_at_mode_0600() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("a/b/sidecar.json.lock");
        let _guard = lock_sidecar_exclusive(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "got 0o{mode:o}, want 0o600");
    }

    #[test]
    fn lock_fails_when_the_lock_path_is_occupied_by_a_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sidecar.json.lock");
        fs::create_dir(&path).unwrap();
        assert!(
            lock_sidecar_exclusive(&path).is_err(),
            "an unopenable lock path must fail closed rather than yield an unlocked guard"
        );
    }

    #[test]
    fn lock_does_not_follow_a_symlink_planted_at_the_lock_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let attacker_target = dir.path().join("attacker-target");
        let path = dir.path().join("sidecar.json.lock");
        std::os::unix::fs::symlink(&attacker_target, &path).unwrap();

        assert!(lock_sidecar_exclusive(&path).is_err());
        assert!(
            !attacker_target.exists(),
            "a symlink at the lock path must not redirect the create"
        );
    }
}
