use std::io;
use std::path::Path;

use anyhow::{Context, Result};
use lns_policy::Policy;

pub(crate) trait Fs: Send + Sync {
    fn is_file(&self, p: &Path) -> bool;
    fn create_dir_all(&self, p: &Path) -> io::Result<()>;
    fn write(&self, p: &Path, bytes: &[u8]) -> io::Result<()>;
}

pub(crate) struct RealFs;

impl Fs for RealFs {
    fn is_file(&self, p: &Path) -> bool {
        p.is_file()
    }
    fn create_dir_all(&self, p: &Path) -> io::Result<()> {
        std::fs::create_dir_all(p)
    }
    /// The same atomic write every later decision goes through, so an interrupted first boot never leaves a half-written document the next start has to parse.
    fn write(&self, p: &Path, bytes: &[u8]) -> io::Result<()> {
        lns_policy::secure_file::write_yaml_document_atomic(p, bytes)
    }
}

pub(crate) fn ensure(path: &Path) -> Result<()> {
    ensure_with(&RealFs, path)
}

/// A restart rejoins the run that already answered, so an existing file is left exactly as the developer left it.
pub(crate) fn ensure_with<F: Fs + ?Sized>(fs: &F, path: &Path) -> Result<()> {
    if fs.is_file(path) {
        return Ok(());
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    fs.create_dir_all(parent)
        .with_context(|| format!("creating the run directory {}", parent.display()))?;
    let bytes = Policy::default()
        .document_bytes(path)
        .context("rendering the empty decisions document")?;
    fs.write(path, &bytes)
        .with_context(|| format!("creating the decisions file {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeFs {
        files: Mutex<HashMap<PathBuf, Vec<u8>>>,
        dirs: Mutex<Vec<PathBuf>>,
        mkdir_fails: bool,
        write_fails: bool,
    }

    impl Fs for FakeFs {
        fn is_file(&self, p: &Path) -> bool {
            self.files.lock().expect("lock").contains_key(p)
        }
        fn create_dir_all(&self, p: &Path) -> io::Result<()> {
            if self.mkdir_fails {
                return Err(io::Error::other("no space"));
            }
            self.dirs.lock().expect("lock").push(p.to_path_buf());
            Ok(())
        }
        fn write(&self, p: &Path, bytes: &[u8]) -> io::Result<()> {
            if self.write_fails {
                return Err(io::Error::other("read-only"));
            }
            self.files
                .lock()
                .expect("lock")
                .insert(p.to_path_buf(), bytes.to_vec());
            Ok(())
        }
    }

    fn path() -> PathBuf {
        PathBuf::from("/h/runs/aa01/decisions.yaml")
    }

    #[test]
    fn a_run_without_a_decisions_file_gets_an_empty_one_named_for_its_stem() {
        let fs = FakeFs::default();
        ensure_with(&fs, &path())
            .expect("§8.1 has the file exist whether or not anything was asked");
        let files = fs.files.lock().expect("lock");
        let text = String::from_utf8(files[&path()].clone()).expect("utf-8");
        assert!(
            text.contains("kind: mixin"),
            "§8.1 records a decision in mixin grammar; got: {text}"
        );
        assert!(
            text.contains("name: decisions"),
            "§8.3 names the document for the file's own stem; got: {text}"
        );
        assert!(
            !text.contains("match:"),
            "a run that has been asked nothing has decided nothing; got: {text}"
        );
    }

    /// The production entry, against a real filesystem: the fake above proves the decision, and this proves the wiring that carries it out.
    #[test]
    fn the_production_entry_lands_an_empty_decisions_file_in_a_fresh_run_directory() {
        let home = tempfile::tempdir().expect("tempdir");
        let path = crate::cache::decisions_path(home.path(), "aa01");
        ensure(&path).expect("a first run has neither the directory nor the file");
        let text = std::fs::read_to_string(&path).expect("the run can read back what it wrote");
        assert!(
            text.contains("kind: mixin") && text.contains("name: decisions"),
            "§8.3 puts a run's decisions in its own directory, in mixin grammar; got: {text}"
        );
        ensure(&path).expect("a restart is not an error");
    }

    #[test]
    fn the_run_directory_is_created_before_the_file_lands_in_it() {
        let fs = FakeFs::default();
        ensure_with(&fs, &path()).expect("a first run has no directory yet");
        assert_eq!(
            fs.dirs.lock().expect("lock").as_slice(),
            [PathBuf::from("/h/runs/aa01")],
            "the decisions file is the first thing written into a fresh run's directory"
        );
    }

    #[test]
    fn a_restart_keeps_what_the_run_already_decided() {
        let fs = FakeFs::default();
        fs.files
            .lock()
            .expect("lock")
            .insert(path(), b"apiVersion: lns.run/v1\nkind: mixin\nname: decisions\nspec:\n  egress:\n    http:\n      - match: git.example.test\n        verdict: allow\n".to_vec());
        ensure_with(&fs, &path()).expect("an existing file is not an error");
        let files = fs.files.lock().expect("lock");
        let text = String::from_utf8(files[&path()].clone()).expect("utf-8");
        assert!(
            text.contains("git.example.test"),
            "a start rejoins the run that answered, so overwriting the file would drop its answers; got: {text}"
        );
    }

    #[test]
    fn a_run_directory_that_cannot_be_created_names_itself_in_the_error() {
        let fs = FakeFs {
            mkdir_fails: true,
            ..FakeFs::default()
        };
        let err = ensure_with(&fs, &path())
            .expect_err("a run with nowhere to record decisions must not boot");
        assert!(
            format!("{err:#}").contains("/h/runs/aa01"),
            "the directory that failed is what the operator has to fix; got: {err:#}"
        );
    }

    #[test]
    fn a_decisions_file_that_cannot_be_written_names_itself_in_the_error() {
        let fs = FakeFs {
            write_fails: true,
            ..FakeFs::default()
        };
        let err = ensure_with(&fs, &path())
            .expect_err("a run whose answers cannot be recorded must not boot");
        assert!(
            format!("{err:#}").contains("decisions.yaml"),
            "the file that failed is what the operator has to fix; got: {err:#}"
        );
    }
}
