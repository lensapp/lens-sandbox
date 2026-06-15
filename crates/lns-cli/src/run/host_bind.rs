use std::path::Path;

pub trait DirScan {
    fn entries(&self, dir: &Path) -> Vec<String>;
    fn read_to_string(&self, path: &Path) -> Option<String>;
}

pub struct RealDirScan;

impl DirScan for RealDirScan {
    fn entries(&self, dir: &Path) -> Vec<String> {
        let Ok(read) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        read.flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect()
    }

    fn read_to_string(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
}

const EXACT_SECRET_NAMES: &[&str] = &[
    ".npmrc",
    ".netrc",
    ".git-credentials",
    ".pgpass",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "credentials",
    ".ssh",
    ".aws",
    ".gnupg",
];

pub fn looks_like_secret(name: &str) -> bool {
    name.starts_with(".env")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || EXACT_SECRET_NAMES.contains(&name)
}

pub fn scan_secrets(scan: &dyn DirScan, root: &Path) -> Vec<String> {
    let mut found: Vec<String> = scan
        .entries(root)
        .into_iter()
        .filter(|name| looks_like_secret(name))
        .collect();
    found.sort();
    found
}

pub fn lensignore_patterns(scan: &dyn DirScan, root: &Path) -> Vec<String> {
    let Some(content) = scan.read_to_string(&root.join(".lensignore")) else {
        return Vec::new();
    };
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

pub fn is_ignored(name: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| p == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeDir {
        entries: Vec<String>,
        files: HashMap<String, String>,
    }
    impl DirScan for FakeDir {
        fn entries(&self, _dir: &Path) -> Vec<String> {
            self.entries.clone()
        }
        fn read_to_string(&self, path: &Path) -> Option<String> {
            self.files
                .get(&path.to_string_lossy().into_owned())
                .cloned()
        }
    }

    #[test]
    fn looks_like_secret_matches_env_keys_and_known_credential_files() {
        for s in [
            ".env",
            ".env.local",
            "server.pem",
            "tls.key",
            ".npmrc",
            ".netrc",
            "id_rsa",
            "id_ed25519",
            ".ssh",
            ".aws",
        ] {
            assert!(looks_like_secret(s), "{s} should look like a secret");
        }
    }

    #[test]
    fn looks_like_secret_passes_ordinary_files() {
        for s in ["main.rs", "README.md", "Cargo.toml", "src", "package.json"] {
            assert!(!looks_like_secret(s), "{s} should not look like a secret");
        }
    }

    #[test]
    fn scan_secrets_returns_sorted_matching_top_level_entries() {
        let dir = FakeDir {
            entries: vec![
                "src".into(),
                ".npmrc".into(),
                "Cargo.toml".into(),
                ".env".into(),
            ],
            ..Default::default()
        };
        let found = scan_secrets(&dir, Path::new("/proj"));
        assert_eq!(found, vec![".env".to_string(), ".npmrc".to_string()]);
    }

    #[test]
    fn scan_secrets_is_empty_for_a_clean_directory() {
        let dir = FakeDir {
            entries: vec!["src".into(), "Cargo.toml".into()],
            ..Default::default()
        };
        assert!(scan_secrets(&dir, Path::new("/proj")).is_empty());
    }

    #[test]
    fn lensignore_patterns_skips_blanks_and_comments() {
        let mut files = HashMap::new();
        files.insert(
            "/proj/.lensignore".to_string(),
            "# secrets\n.env\n\n  .npmrc  \n".to_string(),
        );
        let dir = FakeDir {
            files,
            ..Default::default()
        };
        let patterns = lensignore_patterns(&dir, Path::new("/proj"));
        assert_eq!(patterns, vec![".env".to_string(), ".npmrc".to_string()]);
    }

    #[test]
    fn lensignore_patterns_empty_when_file_absent() {
        let dir = FakeDir::default();
        assert!(lensignore_patterns(&dir, Path::new("/proj")).is_empty());
    }

    #[test]
    fn is_ignored_matches_exact_names_only() {
        let patterns = vec![".env".to_string()];
        assert!(is_ignored(".env", &patterns));
        assert!(!is_ignored(".env.local", &patterns));
    }

    #[test]
    fn real_dir_scan_reads_entries_and_files_from_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(".env"), "SECRET=1").unwrap();
        std::fs::write(dir.path().join(".lensignore"), ".env\n").unwrap();
        let scan = RealDirScan;
        assert!(scan.entries(dir.path()).contains(&".env".to_string()));
        assert_eq!(
            scan.read_to_string(&dir.path().join(".lensignore")),
            Some(".env\n".to_string())
        );
        assert_eq!(scan.read_to_string(&dir.path().join("absent")), None);
        assert!(scan.entries(Path::new("/no/such/dir")).is_empty());
    }
}
