use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Atomically installs `contents` at a `.json` `path` with mode 0600 via a NOFOLLOW create-new tmp + rename, so a stale tmp or planted symlink cannot leak the secret through broader perms.
pub fn write_json_secret_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_atomic_at(path, &path.with_extension("json.tmp"), contents, 0o600)
}

/// The same install for a `.yaml` document the developer commits, whose mode umask decides rather than the 0600 a secret takes.
pub fn write_yaml_document_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_atomic_at(path, &path.with_extension("yaml.tmp"), contents, 0o666)
}

fn write_atomic_at(path: &Path, tmp: &Path, contents: &[u8], mode: u32) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    // Drop any leftover tmp so the next open is create_new — `.mode()` is ignored on an existing file, so a stale tmp with broader perms (or a planted symlink) would otherwise carry through the rename.
    match fs::remove_file(tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(libc::O_NOFOLLOW)
        .open(tmp)?;
    match fill_and_rename(f, tmp, path, contents) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(tmp);
            Err(e)
        }
    }
}

fn fill_and_rename(mut f: fs::File, tmp: &Path, path: &Path, contents: &[u8]) -> io::Result<()> {
    f.write_all(contents)?;
    f.sync_all()?;
    drop(f);
    fs::rename(tmp, path)
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
    fn an_install_that_cannot_land_leaves_no_tmp_behind() {
        // A tmp the writer created and could not install is untracked noise the developer then finds beside their own file, and only the next write of the same path clears it.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("secret.json");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("occupant"), b"x").unwrap();

        write_json_secret_atomic(&path, b"{}").unwrap_err();

        assert!(
            !path.with_extension("json.tmp").exists(),
            "a write that cannot reach its target must take its tmp with it"
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
}
