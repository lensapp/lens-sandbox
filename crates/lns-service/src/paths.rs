use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub fn socket_path() -> Result<PathBuf, lns_ipc::SocketPathError> {
    if let Ok(override_path) = std::env::var("LNS_SOCKET_PATH") {
        return Ok(PathBuf::from(override_path));
    }
    lns_ipc::default_socket_path()
}

pub fn ensure_parent_dir(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    create_secure_dir(parent)
}

#[cfg(unix)]
fn create_secure_dir(parent: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn create_secure_dir(parent: &Path) -> io::Result<()> {
    fs::create_dir_all(parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial(env)]
    fn socket_path_returns_absolute_path() {
        let path = socket_path().expect("HOME / data_dir resolves in test env");
        assert!(path.is_absolute());
    }

    #[test]
    #[serial_test::serial(env)]
    fn socket_path_honours_lns_socket_path_override() {
        let _g = crate::test_env::EnvVarGuard::set("LNS_SOCKET_PATH", "/tmp/custom-lns.sock");
        let p = socket_path().expect("override branch");
        assert_eq!(p, PathBuf::from("/tmp/custom-lns.sock"));
    }

    #[test]
    fn ensure_parent_dir_creates_dir() {
        let dir = std::env::temp_dir().join("lns-test-ensure-parent");
        let sock = dir.join("nested/service.sock");
        let _ = fs::remove_dir_all(&dir);

        ensure_parent_dir(&sock).unwrap();
        assert!(sock.parent().unwrap().exists());

        ensure_parent_dir(&sock).unwrap();
        assert!(sock.parent().unwrap().exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_parent_dir_no_parent_is_noop() {
        ensure_parent_dir(Path::new("")).unwrap();
    }
}
