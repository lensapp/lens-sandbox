use std::path::{Path, PathBuf};

pub fn desktop_bundle(executable: &Path) -> Option<PathBuf> {
    executable
        .ancestors()
        .filter(|path| path.file_name() == Some(std::ffi::OsStr::new("Contents")))
        .filter_map(Path::parent)
        .find(|path| path.extension() == Some(std::ffi::OsStr::new("app")))
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_resolve_their_own_app_without_searching_other_installations() {
        assert_eq!(
            desktop_bundle(Path::new(
                "/Users/a/My Apps/LNS.app/Contents/Helpers/lns-service"
            )),
            Some(PathBuf::from("/Users/a/My Apps/LNS.app"))
        );
        assert_eq!(desktop_bundle(Path::new("/work/Some.app/target/lns")), None);
        assert_eq!(desktop_bundle(Path::new("lns")), None);
    }
}
