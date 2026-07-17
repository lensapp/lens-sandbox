use std::path::Path;

use anyhow::Result;

use crate::image_store::Fs;

/// Remove every file in the flat producer build cache, returning the bytes reclaimed; a cache that was never populated is not an error.
pub async fn sweep_with<F: Fs>(fs: &F, builds_root: &Path) -> Result<u64> {
    let entries = match fs.read_dir(builds_root).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let mut reclaimed = 0u64;
    for path in &entries {
        reclaimed += fs.read(path).await?.len() as u64;
        fs.remove_file(path).await?;
    }
    Ok(reclaimed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io;
    use std::path::PathBuf;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeFs {
        files: Mutex<HashMap<PathBuf, Vec<u8>>>,
        read_dir_error: Option<io::ErrorKind>,
    }

    impl Fs for FakeFs {
        async fn read_dir(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
            if let Some(kind) = self.read_dir_error {
                return Err(io::Error::from(kind));
            }
            Ok(self
                .files
                .lock()
                .unwrap()
                .keys()
                .filter(|p| p.parent() == Some(dir))
                .cloned()
                .collect())
        }
        async fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
            self.files
                .lock()
                .unwrap()
                .get(p)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
        async fn write(&self, p: &Path, bytes: &[u8]) -> io::Result<()> {
            self.files
                .lock()
                .unwrap()
                .insert(p.to_path_buf(), bytes.to_vec());
            Ok(())
        }
        async fn remove_file(&self, p: &Path) -> io::Result<()> {
            self.files.lock().unwrap().remove(p);
            Ok(())
        }
    }

    const ROOT: &str = "/cache/lns/builds";

    #[tokio::test]
    async fn sweeping_a_cache_that_was_never_populated_reclaims_nothing() {
        let fs = FakeFs {
            read_dir_error: Some(io::ErrorKind::NotFound),
            ..Default::default()
        };
        assert_eq!(sweep_with(&fs, Path::new(ROOT)).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn sweeping_an_empty_cache_reclaims_nothing() {
        let fs = FakeFs::default();
        assert_eq!(sweep_with(&fs, Path::new(ROOT)).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn sweeping_removes_every_file_and_sums_reclaimed_bytes() {
        let fs = FakeFs::default();
        fs.write(&PathBuf::from(format!("{ROOT}/blobone")), b"aaaa")
            .await
            .unwrap();
        fs.write(&PathBuf::from(format!("{ROOT}/rec.json")), b"bb")
            .await
            .unwrap();
        assert_eq!(sweep_with(&fs, Path::new(ROOT)).await.unwrap(), 6);
        assert!(
            fs.files.lock().unwrap().is_empty(),
            "the whole build cache must be cleared"
        );
    }

    #[tokio::test]
    async fn a_read_dir_failure_other_than_not_found_propagates() {
        let fs = FakeFs {
            read_dir_error: Some(io::ErrorKind::PermissionDenied),
            ..Default::default()
        };
        assert!(sweep_with(&fs, Path::new(ROOT)).await.is_err());
    }
}
