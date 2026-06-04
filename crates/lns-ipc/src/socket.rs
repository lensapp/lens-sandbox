use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SocketPathError {
    #[error("cannot locate Lens Sandbox service socket: HOME is not set")]
    HomeNotSet,
    #[error("unsupported platform: only macOS and Linux are supported")]
    UnsupportedPlatform,
}

#[cfg(target_os = "macos")]
pub fn default_socket_path() -> Result<PathBuf, SocketPathError> {
    build_socket_path_macos(dirs::data_dir())
}

#[cfg(target_os = "linux")]
pub fn default_socket_path() -> Result<PathBuf, SocketPathError> {
    build_socket_path_linux(
        std::env::var("XDG_RUNTIME_DIR").ok().as_deref(),
        dirs::home_dir(),
    )
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn default_socket_path() -> Result<PathBuf, SocketPathError> {
    Err(SocketPathError::UnsupportedPlatform)
}

#[cfg(any(target_os = "macos", test))]
fn build_socket_path_macos(data_dir: Option<PathBuf>) -> Result<PathBuf, SocketPathError> {
    let data_dir = data_dir.ok_or(SocketPathError::HomeNotSet)?;
    Ok(data_dir.join("com.lensapp.sandbox/service.sock"))
}

#[cfg(any(target_os = "linux", test))]
fn build_socket_path_linux(
    runtime_dir: Option<&str>,
    home: Option<PathBuf>,
) -> Result<PathBuf, SocketPathError> {
    if let Some(rd) = runtime_dir {
        return Ok(PathBuf::from(rd).join("lns/service.sock"));
    }
    let home = home.ok_or(SocketPathError::HomeNotSet)?;
    Ok(home.join(".lns/service.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_is_absolute() {
        let path = default_socket_path().expect("HOME / data_dir resolves in test env");
        assert!(path.is_absolute());
    }

    #[test]
    fn socket_path_ends_with_sock() {
        let path = default_socket_path().expect("HOME / data_dir resolves in test env");
        assert_eq!(path.file_name().unwrap(), "service.sock");
    }

    #[test]
    fn macos_returns_home_not_set_when_data_dir_missing() {
        let err = build_socket_path_macos(None).unwrap_err();
        assert!(matches!(err, SocketPathError::HomeNotSet));
    }

    #[test]
    fn macos_joins_data_dir_with_app_subpath() {
        let path = build_socket_path_macos(Some(PathBuf::from("/tmp/data"))).unwrap();
        assert_eq!(
            path,
            PathBuf::from("/tmp/data/com.lensapp.sandbox/service.sock")
        );
    }

    #[test]
    fn linux_prefers_runtime_dir() {
        let path = build_socket_path_linux(Some("/run/user/1000"), Some(PathBuf::from("/home/u")))
            .unwrap();
        assert_eq!(path, PathBuf::from("/run/user/1000/lns/service.sock"));
    }

    #[test]
    fn linux_falls_back_to_home_when_runtime_dir_missing() {
        let path = build_socket_path_linux(None, Some(PathBuf::from("/home/u"))).unwrap();
        assert_eq!(path, PathBuf::from("/home/u/.lns/service.sock"));
    }

    #[test]
    fn linux_returns_home_not_set_when_neither_available() {
        let err = build_socket_path_linux(None, None).unwrap_err();
        assert!(matches!(err, SocketPathError::HomeNotSet));
    }

    #[test]
    fn default_socket_path_returns_non_empty_absolute_path() {
        let path = default_socket_path().expect("resolves in test env");
        assert!(!path.as_os_str().is_empty());
        assert!(path.is_absolute());
        assert_eq!(path.file_name().unwrap(), "service.sock");
    }

    #[test]
    fn default_socket_path_matches_platform_helper() {
        let path = default_socket_path().expect("resolves in test env");
        #[cfg(target_os = "macos")]
        {
            let expected = build_socket_path_macos(dirs::data_dir()).unwrap();
            assert_eq!(path, expected);
        }
        #[cfg(target_os = "linux")]
        {
            let expected = build_socket_path_linux(
                std::env::var("XDG_RUNTIME_DIR").ok().as_deref(),
                dirs::home_dir(),
            )
            .unwrap();
            assert_eq!(path, expected);
        }
    }
}
