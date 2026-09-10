//! Where a key is remembered, and what still answers for a built image.
//!
//! One entry per key: the image the key produced, and the Containerfile it was produced from. The
//! second half is what makes a sweep possible — an image nothing on this machine names any more,
//! neither a document nor a run, is an image `lns sandbox prune` drops.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// This cache's own directory, and not `lns_ipc::build_cache_root()`, whose sweeper owns everything under it.
pub(crate) const CACHE_DIR: &str = "containerfile-builds";

/// Which of the two keys an entry answers: the whole image, or one instruction over the image before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Image,
    Step,
}

impl Kind {
    fn dir(self) -> &'static str {
        match self {
            Self::Image => "images",
            Self::Step => "steps",
        }
    }
}

/// One remembered build: what it produced, and every document that has answered for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub reference: String,
    /// Every Containerfile absolute path this key has answered for, so one copy leaving the machine does not drop the image the others need.
    pub sources: Vec<String>,
    /// Whether the host Docker daemon built this image, because both engines take the key over the same four inputs and only the entry says which one answered (§3.1.1).
    pub built_outside_the_gate: bool,
}

impl Entry {
    pub(crate) fn built_from(reference: &str, source: &str, built_outside_the_gate: bool) -> Self {
        Self {
            reference: reference.to_string(),
            sources: vec![source.to_string()],
            built_outside_the_gate,
        }
    }
}

/// The cache directory as the host holds it: a key nothing answers for is absent, not an error.
pub(crate) trait CacheFs {
    fn read(&self, path: &Path) -> Option<Vec<u8>>;
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<()>;
    fn remove(&self, path: &Path) -> Result<()>;
    fn list(&self, dir: &Path) -> Vec<PathBuf>;
    fn exists(&self, path: &Path) -> bool;
}

pub(crate) struct BuildCache<'a, F: CacheFs> {
    fs: &'a F,
    root: PathBuf,
}

/// What a sweep decided: the built images something still names, and the entries it dropped.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Swept {
    pub kept: BTreeSet<String>,
    pub dropped: usize,
}

impl<'a, F: CacheFs> BuildCache<'a, F> {
    pub(crate) fn new(fs: &'a F, cache_root: &Path) -> Self {
        Self {
            fs,
            root: cache_root.join(CACHE_DIR),
        }
    }

    /// What this machine built for the key, while it still holds the image the entry names; the
    /// asking document is recorded, so the entry outlives whichever copy of it goes first.
    pub(crate) fn get(
        &self,
        kind: Kind,
        key: &str,
        source: &str,
        holds: &dyn Fn(&str) -> bool,
    ) -> Option<Entry> {
        let entry = self.entry_at(&self.path(kind, key))?;
        if !holds(&entry.reference) {
            return None;
        }
        Some(self.also_answering_for(kind, key, entry, source))
    }

    pub(crate) fn remember(&self, kind: Kind, key: &str, entry: &Entry) -> Result<()> {
        let path = self.path(kind, key);
        let merged = match self.entry_at(&path) {
            Some(held) if held.reference == entry.reference => {
                let mut merged = held;
                merged.built_outside_the_gate = entry.built_outside_the_gate;
                let added: Vec<String> = entry
                    .sources
                    .iter()
                    .filter(|source| !merged.sources.contains(source))
                    .cloned()
                    .collect();
                merged.sources.extend(added);
                merged
            }
            _ => entry.clone(),
        };
        let bytes = serde_json::to_vec(&merged).context("writing a build cache entry")?;
        self.fs
            .write(&path, &bytes)
            .with_context(|| format!("remembering this build at {}", path.display()))
    }

    fn also_answering_for(&self, kind: Kind, key: &str, entry: Entry, source: &str) -> Entry {
        if entry.sources.iter().any(|held| held == source) {
            return entry;
        }
        let mut refreshed = entry;
        refreshed.sources.push(source.to_string());
        if let Err(e) = self.remember(kind, key, &refreshed) {
            crate::log::warn!("this build answered for a second document unrecorded: {e:#}");
        }
        refreshed
    }

    /// Every entry whose document has left this machine, or whose image has, goes; what the rest name stays.
    pub(crate) fn sweep(&self, holds: &dyn Fn(&str) -> bool) -> Swept {
        let (kept, doomed) = self.examine(holds);
        let mut swept = Swept { kept, dropped: 0 };
        for path in doomed {
            if self.fs.remove(&path).is_ok() {
                swept.dropped += 1;
            }
        }
        swept
    }

    /// The same reading, taken without removing an entry, so a prune can say what it would drop.
    pub(crate) fn survivors(&self, holds: &dyn Fn(&str) -> bool) -> BTreeSet<String> {
        self.examine(holds).0
    }

    fn examine(&self, holds: &dyn Fn(&str) -> bool) -> (BTreeSet<String>, Vec<PathBuf>) {
        let mut kept = BTreeSet::new();
        let mut doomed = Vec::new();
        for kind in [Kind::Image, Kind::Step] {
            for path in self.fs.list(&self.root.join(kind.dir())) {
                match self.entry_at(&path) {
                    Some(entry)
                        if holds(&entry.reference)
                            && entry
                                .sources
                                .iter()
                                .any(|source| self.fs.exists(Path::new(source))) =>
                    {
                        kept.insert(entry.reference);
                    }
                    _ => doomed.push(path),
                }
            }
        }
        (kept, doomed)
    }

    fn entry_at(&self, path: &Path) -> Option<Entry> {
        serde_json::from_slice(&self.fs.read(path)?).ok()
    }

    fn path(&self, kind: Kind, key: &str) -> PathBuf {
        self.root.join(kind.dir()).join(key.replace(':', "-"))
    }
}

/// Every built image something still answers for: a document this machine holds a build of, and a run booted from one.
pub(crate) fn still_referenced<F: CacheFs>(
    fs: &F,
    cache_root: &Path,
    runs: &[String],
    holds: &dyn Fn(&str) -> bool,
) -> Swept {
    let mut swept = BuildCache::new(fs, cache_root).sweep(holds);
    name_what_the_runs_booted(fs, cache_root, runs, &mut swept.kept);
    swept
}

/// The same reading as `still_referenced`, taken without dropping an entry.
pub(crate) fn would_be_referenced<F: CacheFs>(
    fs: &F,
    cache_root: &Path,
    runs: &[String],
    holds: &dyn Fn(&str) -> bool,
) -> BTreeSet<String> {
    let mut kept = BuildCache::new(fs, cache_root).survivors(holds);
    name_what_the_runs_booted(fs, cache_root, runs, &mut kept);
    kept
}

fn name_what_the_runs_booted<F: CacheFs>(
    fs: &F,
    cache_root: &Path,
    runs: &[String],
    kept: &mut BTreeSet<String>,
) {
    for run in runs {
        let path = crate::cache::run_dir(cache_root, run).join(super::real::BUILT_REFERENCE_FILE);
        if let Some(bytes) = fs.read(&path)
            && let Ok(reference) = String::from_utf8(bytes)
        {
            kept.insert(reference.trim().to_string());
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    /// One cache directory in memory, plus the one way each write can fail.
    #[derive(Default)]
    pub(crate) struct FakeCacheFs {
        files: RefCell<BTreeMap<PathBuf, Vec<u8>>>,
        unwritable: bool,
        unremovable: bool,
    }

    impl FakeCacheFs {
        fn with(files: &[(&str, &str)]) -> Self {
            let fs = Self::default();
            for (path, body) in files {
                fs.files
                    .borrow_mut()
                    .insert(PathBuf::from(path), body.as_bytes().to_vec());
            }
            fs
        }

        fn paths(&self) -> Vec<String> {
            self.files
                .borrow()
                .keys()
                .map(|path| path.display().to_string())
                .collect()
        }
    }

    impl CacheFs for FakeCacheFs {
        fn read(&self, path: &Path) -> Option<Vec<u8>> {
            self.files.borrow().get(path).cloned()
        }

        fn write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
            if self.unwritable {
                anyhow::bail!("read-only file system");
            }
            self.files
                .borrow_mut()
                .insert(path.to_path_buf(), bytes.to_vec());
            Ok(())
        }

        fn remove(&self, path: &Path) -> Result<()> {
            if self.unremovable {
                anyhow::bail!("permission denied");
            }
            self.files.borrow_mut().remove(path);
            Ok(())
        }

        fn list(&self, dir: &Path) -> Vec<PathBuf> {
            self.files
                .borrow()
                .keys()
                .filter(|path| path.parent() == Some(dir))
                .cloned()
                .collect()
        }

        fn exists(&self, path: &Path) -> bool {
            self.files.borrow().contains_key(path)
        }
    }

    fn entry(reference: &str, source: &str) -> Entry {
        Entry::built_from(reference, source, false)
    }

    /// The key both engines take over the same four inputs, so what the key answers with says which engine filled it (§3.1.1).
    #[test]
    fn an_entry_remembers_the_engine_that_built_it() {
        let fs = FakeCacheFs::default();
        let cache = BuildCache::new(&fs, Path::new("/cache"));
        cache
            .remember(
                Kind::Image,
                "sha256:aa",
                &Entry::built_from("built@sha256:one", "/work/Containerfile", true),
            )
            .unwrap();

        let hit = cache
            .get(Kind::Image, "sha256:aa", "/work/Containerfile", &everything)
            .expect("the key answers");

        assert!(
            hit.built_outside_the_gate,
            "a reuse of a daemon build has to say the gate did not apply",
        );
    }

    /// A rebuild through the other engine writes the same key, and the record follows the build that wrote it last.
    #[test]
    fn a_rebuild_in_a_guest_takes_the_gate_record_back_from_the_daemon() {
        let fs = FakeCacheFs::default();
        let cache = BuildCache::new(&fs, Path::new("/cache"));
        cache
            .remember(
                Kind::Image,
                "sha256:aa",
                &Entry::built_from("built@sha256:one", "/work/Containerfile", true),
            )
            .unwrap();

        cache
            .remember(
                Kind::Image,
                "sha256:aa",
                &Entry::built_from("built@sha256:one", "/work/Containerfile", false),
            )
            .unwrap();

        let hit = cache
            .get(Kind::Image, "sha256:aa", "/work/Containerfile", &everything)
            .expect("the key answers");

        assert!(!hit.built_outside_the_gate);
    }

    fn everything(_: &str) -> bool {
        true
    }

    fn nothing(_: &str) -> bool {
        false
    }

    #[test]
    fn a_key_is_answered_by_what_was_remembered_for_it_and_by_nothing_else() {
        let fs = FakeCacheFs::default();
        let cache = BuildCache::new(&fs, Path::new("/cache"));
        let remembered = entry("built@sha256:one", "/work/image/Containerfile");

        cache
            .remember(Kind::Image, "sha256:aa", &remembered)
            .expect("the cache is writable");

        assert_eq!(
            cache.get(
                Kind::Image,
                "sha256:aa",
                "/work/image/Containerfile",
                &everything
            ),
            Some(remembered)
        );
        assert_eq!(
            cache.get(
                Kind::Image,
                "sha256:bb",
                "/work/image/Containerfile",
                &everything
            ),
            None
        );
        assert_eq!(
            cache.get(
                Kind::Step,
                "sha256:aa",
                "/work/image/Containerfile",
                &everything
            ),
            None,
            "an instruction key and an image key are two keys, not one",
        );
    }

    /// The manifest cache is the image; a reference it no longer holds is a key that answers for nothing.
    #[test]
    fn a_key_naming_an_image_this_machine_no_longer_holds_answers_nothing() {
        let fs = FakeCacheFs::default();
        let cache = BuildCache::new(&fs, Path::new("/cache"));
        cache
            .remember(
                Kind::Step,
                "sha256:aa",
                &entry("built@sha256:gone", "/work/Containerfile"),
            )
            .unwrap();

        assert_eq!(
            cache.get(Kind::Step, "sha256:aa", "/work/Containerfile", &nothing),
            None
        );
    }

    #[test]
    fn an_entry_that_does_not_parse_answers_nothing_and_is_swept() {
        let fs = FakeCacheFs::with(&[("/cache/containerfile-builds/images/sha256-aa", "{")]);
        let cache = BuildCache::new(&fs, Path::new("/cache"));

        assert_eq!(
            cache.get(Kind::Image, "sha256:aa", "/work/Containerfile", &everything),
            None
        );
        assert_eq!(cache.sweep(&everything).dropped, 1);
        assert!(fs.paths().is_empty());
    }

    /// The key excludes the path by decision, so two copies of one document share an entry; the one
    /// that goes first must not take the other's image with it.
    #[test]
    fn a_second_document_that_hits_a_key_is_recorded_as_answering_for_it() {
        let fs = FakeCacheFs::default();
        let cache = BuildCache::new(&fs, Path::new("/cache"));
        cache
            .remember(
                Kind::Image,
                "sha256:aa",
                &entry("built@sha256:one", "/work/a/Containerfile"),
            )
            .unwrap();
        fs.write(Path::new("/work/b/Containerfile"), b"FROM base\n")
            .unwrap();

        let hit = cache
            .get(
                Kind::Image,
                "sha256:aa",
                "/work/b/Containerfile",
                &everything,
            )
            .expect("the key answers");

        assert_eq!(
            hit.sources,
            ["/work/a/Containerfile", "/work/b/Containerfile"]
        );
        assert_eq!(
            cache.sweep(&everything).kept,
            BTreeSet::from(["built@sha256:one".to_string()]),
            "the copy that is still here keeps the image the other one built",
        );
    }

    /// One document asking twice is one source, not a list that grows with every build.
    #[test]
    fn the_document_that_built_a_key_is_recorded_once() {
        let fs = FakeCacheFs::default();
        let cache = BuildCache::new(&fs, Path::new("/cache"));
        cache
            .remember(
                Kind::Image,
                "sha256:aa",
                &entry("built@sha256:one", "/work/a/Containerfile"),
            )
            .unwrap();

        cache
            .get(
                Kind::Image,
                "sha256:aa",
                "/work/a/Containerfile",
                &everything,
            )
            .expect("the key answers");
        cache
            .remember(
                Kind::Image,
                "sha256:aa",
                &entry("built@sha256:one", "/work/a/Containerfile"),
            )
            .unwrap();

        let held = cache
            .get(
                Kind::Image,
                "sha256:aa",
                "/work/a/Containerfile",
                &everything,
            )
            .expect("the key answers");
        assert_eq!(held.sources, ["/work/a/Containerfile"]);
    }

    /// A key that now names a different image is that build's entry, not an amended one.
    #[test]
    fn a_key_rebuilt_into_another_image_forgets_the_documents_of_the_old_one() {
        let fs = FakeCacheFs::default();
        let cache = BuildCache::new(&fs, Path::new("/cache"));
        cache
            .remember(
                Kind::Image,
                "sha256:aa",
                &entry("built@sha256:one", "/work/a/Containerfile"),
            )
            .unwrap();

        cache
            .remember(
                Kind::Image,
                "sha256:aa",
                &entry("built@sha256:two", "/work/b/Containerfile"),
            )
            .unwrap();

        let held = cache
            .get(
                Kind::Image,
                "sha256:aa",
                "/work/b/Containerfile",
                &everything,
            )
            .expect("the key answers");
        assert_eq!(held.reference, "built@sha256:two");
        assert_eq!(held.sources, ["/work/b/Containerfile"]);
    }

    /// A hit the cache cannot write back is still a hit: the build is reused, and the operator is told once.
    #[test]
    fn a_hit_an_unwritable_cache_cannot_record_still_answers() {
        let fs = FakeCacheFs {
            unwritable: true,
            ..FakeCacheFs::with(&[(
                "/cache/containerfile-builds/images/sha256-aa",
                r#"{"reference":"built@sha256:one","sources":["/work/a/Containerfile"],"built_outside_the_gate":false}"#,
            )])
        };
        let mut hit = None;
        let messages = crate::test_env::captured_messages(|| {
            hit = BuildCache::new(&fs, Path::new("/cache")).get(
                Kind::Image,
                "sha256:aa",
                "/work/b/Containerfile",
                &everything,
            );
        });

        assert_eq!(hit.expect("the key answers").reference, "built@sha256:one");
        assert!(
            messages
                .iter()
                .any(|m| m.contains("answered for a second document")),
            "{messages:?}"
        );
    }

    #[test]
    fn a_cache_that_cannot_be_written_names_the_entry_it_could_not_write() {
        let fs = FakeCacheFs {
            unwritable: true,
            ..FakeCacheFs::default()
        };
        let refusal = format!(
            "{:#}",
            BuildCache::new(&fs, Path::new("/cache"))
                .remember(
                    Kind::Image,
                    "sha256:aa",
                    &entry("built@sha256:one", "/work/Containerfile")
                )
                .expect_err("an unwritable cache is reported")
        );

        assert!(
            refusal.contains("containerfile-builds/images/sha256-aa"),
            "{refusal}"
        );
        assert!(refusal.contains("read-only file system"), "{refusal}");
    }

    #[test]
    fn an_entry_the_sweep_cannot_remove_is_not_counted_as_dropped() {
        let fs = FakeCacheFs {
            unremovable: true,
            ..FakeCacheFs::with(&[("/cache/containerfile-builds/images/sha256-aa", "{")])
        };

        assert_eq!(
            BuildCache::new(&fs, Path::new("/cache"))
                .sweep(&everything)
                .dropped,
            0
        );
    }

    fn populated() -> FakeCacheFs {
        let fs = FakeCacheFs::default();
        let cache = BuildCache::new(&fs, Path::new("/cache"));
        cache
            .remember(
                Kind::Image,
                "sha256:kept",
                &entry("built@sha256:image", "/work/image/Containerfile"),
            )
            .unwrap();
        cache
            .remember(
                Kind::Step,
                "sha256:step",
                &entry("built@sha256:step", "/work/image/Containerfile"),
            )
            .unwrap();
        cache
            .remember(
                Kind::Image,
                "sha256:orphan",
                &entry("built@sha256:orphan", "/gone/image/Containerfile"),
            )
            .unwrap();
        fs.write(Path::new("/work/image/Containerfile"), b"FROM base\n")
            .unwrap();
        fs
    }

    #[test]
    fn a_build_whose_document_left_this_machine_is_dropped_and_its_image_is_named_by_nothing() {
        let fs = populated();

        let swept = BuildCache::new(&fs, Path::new("/cache")).sweep(&everything);

        assert_eq!(swept.dropped, 1);
        assert_eq!(
            swept.kept,
            BTreeSet::from([
                "built@sha256:image".to_string(),
                "built@sha256:step".to_string(),
            ]),
            "the intermediates of a document that is still here are still named by it",
        );
        let left = fs.paths();
        assert!(
            !left
                .iter()
                .any(|path| path.ends_with("images/sha256-orphan")),
            "{left:?}"
        );
    }

    #[test]
    fn a_run_booted_from_a_built_image_names_it_even_when_no_document_does() {
        let fs = FakeCacheFs::with(&[("/cache/runs/run-1/built-image", "built@sha256:booted\n")]);

        let swept = still_referenced(&fs, Path::new("/cache"), &["run-1".into()], &everything);

        assert_eq!(
            swept.kept,
            BTreeSet::from(["built@sha256:booted".to_string()])
        );
    }

    /// A prune says what it would drop before it asks, and asking must not drop an entry.
    #[test]
    fn reading_what_is_referenced_drops_no_entry() {
        let fs = populated();
        let before = fs.paths();

        let named = would_be_referenced(&fs, Path::new("/cache"), &["run-1".into()], &everything);

        assert!(
            named.contains("built@sha256:image"),
            "a document that is still here still names what it built: {named:?}"
        );
        assert_eq!(fs.paths(), before, "a reading removes nothing");
    }

    #[test]
    fn a_run_that_booted_no_built_image_names_none() {
        let fs = populated();

        let swept = still_referenced(
            &fs,
            Path::new("/cache"),
            &["run-1".into(), "run-2".into()],
            &everything,
        );

        assert_eq!(swept.kept.len(), 2);
    }

    #[test]
    fn nothing_survives_a_sweep_once_the_manifests_are_gone() {
        let fs = populated();

        let swept = BuildCache::new(&fs, Path::new("/cache")).sweep(&nothing);

        assert!(swept.kept.is_empty());
        assert_eq!(swept.dropped, 3);
    }
}
