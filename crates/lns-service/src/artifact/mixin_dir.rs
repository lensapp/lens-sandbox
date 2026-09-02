use std::path::Path;

use anyhow::{Context, Result};

use crate::artifact::mixin::FetchedMixin;

/// The document a directory means, the same one `lns run ./dir` reads.
const DOCUMENT: &str = "lns.yaml";

/// Reads a mixin document off this machine, so the walk can merge a local artifact without owning any filesystem access itself.
pub trait MixinDir: Send + Sync {
    fn read(&self, path: &Path) -> std::io::Result<String>;
    fn is_dir(&self, path: &Path) -> bool;
}

/// Read the document a local path names, as the JSON every other source is parsed from: a directory holds its `lns.yaml`, and a path naming the document is the document — the two spellings `lns run` takes. It answers under the document's own folded path, which is what makes the two spellings one source.
pub fn read_path_mixin<D: MixinDir>(dir: &D, path: &Path) -> Result<FetchedMixin> {
    let document = if dir.is_dir(path) {
        path.join(DOCUMENT)
    } else {
        path.to_path_buf()
    };
    let yaml = dir
        .read(&document)
        .with_context(|| format!("reading {}", document.display()))?;
    let document = lns_artifact::sandbox::fold_path(&document);
    let root = document.parent().unwrap_or(Path::new(".")).to_path_buf();
    parse_document(&yaml, &document, &root, document.display().to_string())
}

/// Read a run's own decisions, which that run resolves without being named (`docs/sandbox-spec.md` §8.1); a run that has been asked nothing has none to read.
pub fn read_local_mixin<D: MixinDir>(dir: &D, file: &Path) -> Result<Option<FetchedMixin>> {
    let yaml = match dir.read(file) {
        Ok(yaml) => yaml,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {}", file.display()))),
    };
    if yaml.trim().is_empty() {
        return Ok(None);
    }
    let label = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.display().to_string());
    parse_document(&yaml, file, file.parent().unwrap_or(Path::new(".")), label).map(Some)
}

/// Parse one document off this machine, rooting the paths it writes against the directory it was written in.
fn parse_document(
    yaml: &str,
    document: &Path,
    root: &Path,
    pinned: String,
) -> Result<FetchedMixin> {
    let mut parsed: serde_json::Value =
        serde_yaml::from_str(yaml).with_context(|| format!("parsing {}", document.display()))?;
    root_relative_paths(&mut parsed, root);
    Ok(FetchedMixin {
        pinned,
        document: serde_json::to_string(&parsed).context("serializing the mixin")?,
        layers: Vec::new(),
    })
}

/// A path a mixin writes is written against the mixin's own directory, so it has to be absolute before it travels into a document that will be read somewhere else.
fn root_relative_paths(document: &mut serde_json::Value, dir: &Path) {
    root_each(document, "filesets", "path", dir, |_| true);
    root_each(document, "volumes", "source", dir, |entry| {
        entry.get("type").and_then(serde_json::Value::as_str) == Some("bind")
    });
}

/// Root one block's paths, leaving alone what already names one file: an absolute path, and `~/`, which names the machine that runs it rather than the one that wrote it.
fn root_each(
    document: &mut serde_json::Value,
    block: &str,
    field: &str,
    dir: &Path,
    is_a_path: impl Fn(&serde_json::Value) -> bool,
) {
    let Some(entries) = document
        .get_mut("spec")
        .and_then(|spec| spec.get_mut(block))
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for entry in entries {
        if !is_a_path(entry) {
            continue;
        }
        let Some(path) = entry.get(field).and_then(serde_json::Value::as_str) else {
            continue;
        };
        if Path::new(path).is_absolute() || path.starts_with('~') {
            continue;
        }
        let rooted = lns_artifact::sandbox::fold_path(&dir.join(path));
        entry[field] = serde_json::Value::String(rooted.display().to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    struct Fake {
        files: BTreeMap<PathBuf, String>,
        dirs: std::collections::BTreeSet<PathBuf>,
    }

    impl Fake {
        fn new(files: BTreeMap<PathBuf, String>) -> Self {
            let mut dirs = std::collections::BTreeSet::new();
            for file in files.keys() {
                let mut parent = file.parent();
                while let Some(dir) = parent {
                    dirs.insert(dir.to_path_buf());
                    parent = dir.parent();
                }
            }
            Self { files, dirs }
        }

        /// A directory that exists and holds nothing, which no map of files can express.
        fn empty_dir(path: &str) -> Self {
            Self {
                files: BTreeMap::new(),
                dirs: std::collections::BTreeSet::from([PathBuf::from(path)]),
            }
        }
    }

    impl MixinDir for Fake {
        fn read(&self, path: &Path) -> std::io::Result<String> {
            self.files.get(path).cloned().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, format!("{}", path.display()))
            })
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.dirs.contains(path)
        }
    }

    fn holding(path: &str, yaml: &str) -> Fake {
        Fake::new(BTreeMap::from([(PathBuf::from(path), yaml.to_string())]))
    }

    #[test]
    fn a_directory_answers_with_the_document_it_holds_under_its_own_path() {
        let dir = holding(
            "/work/mixins/pg/lns.yaml",
            "apiVersion: lns.run/v1\nkind: mixin\nname: pg\nspec:\n  tools:\n    - python@3.12\n",
        );
        let fetched = read_path_mixin(&dir, Path::new("/work/mixins/pg")).unwrap();
        assert_eq!(
            fetched.pinned, "/work/mixins/pg/lns.yaml",
            "a local source has no digest, so the document it resolved to is its identity — which is what makes a directory and that document one source rather than two"
        );
        assert!(
            fetched.document.contains(r#""python@3.12""#),
            "got: {}",
            fetched.document
        );
    }

    #[test]
    fn a_fileset_a_mixin_declares_is_rooted_where_that_mixin_lives() {
        let dir = holding(
            "/work/mixins/pg/lns.yaml",
            "apiVersion: lns.run/v1\nkind: mixin\nname: pg\nspec:\n  filesets:\n    - path: ./skills\n      guestPath: /root/.agent/skills\n",
        );
        let fetched = read_path_mixin(&dir, Path::new("/work/mixins/pg")).unwrap();
        assert!(
            fetched.document.contains(r#""/work/mixins/pg/skills""#),
            "a relative path that travelled unrooted would re-read against whichever project merged it; got: {}",
            fetched.document
        );
    }

    #[test]
    fn a_fileset_that_names_no_path_is_left_alone() {
        let dir = holding(
            "/work/mixins/pg/lns.yaml",
            "apiVersion: lns.run/v1\nkind: mixin\nname: pg\nspec:\n  filesets:\n    - inline:\n        notes.md: hello\n      guestPath: /root/.agent/skills\n",
        );
        let fetched = read_path_mixin(&dir, Path::new("/work/mixins/pg")).unwrap();
        assert!(
            fetched.document.contains(r#""notes.md""#),
            "a fileset carrying its content has no path to root; got: {}",
            fetched.document
        );
    }

    #[test]
    fn a_bind_a_mixin_declares_is_rooted_where_that_mixin_lives() {
        let dir = holding(
            "/work/mixins/pg/lns.yaml",
            "apiVersion: lns.run/v1\nkind: mixin\nname: pg\nspec:\n  volumes:\n    - type: bind\n      source: ./data\n      target: /data\n",
        );
        let fetched = read_path_mixin(&dir, Path::new("/work/mixins/pg")).unwrap();
        assert!(
            fetched.document.contains(r#""/work/mixins/pg/data""#),
            "a relative bind that travelled unrooted would mount the consumer's directory of that name instead of the mixin's own; got: {}",
            fetched.document
        );
    }

    #[test]
    fn a_named_volume_keeps_the_name_it_was_given() {
        let dir = holding(
            "/work/mixins/pg/lns.yaml",
            "apiVersion: lns.run/v1\nkind: mixin\nname: pg\nspec:\n  volumes:\n    - type: volume\n      source: pg-data\n      target: /data\n",
        );
        let fetched = read_path_mixin(&dir, Path::new("/work/mixins/pg")).unwrap();
        assert!(
            fetched.document.contains(r#""pg-data""#),
            "a named volume's source is its name, not a path, and rooting it would ask for a volume nobody created; got: {}",
            fetched.document
        );
    }

    #[test]
    fn a_home_anchored_bind_stays_the_consumers_own_home() {
        let dir = holding(
            "/work/mixins/pg/lns.yaml",
            "apiVersion: lns.run/v1\nkind: mixin\nname: pg\nspec:\n  volumes:\n    - type: bind\n      source: ~/.config/pg\n      target: /config\n",
        );
        let fetched = read_path_mixin(&dir, Path::new("/work/mixins/pg")).unwrap();
        assert!(
            fetched.document.contains(r#""~/.config/pg""#),
            "`~/` already names the machine that runs it, so rooting it at the mixin would point somewhere nobody asked for; got: {}",
            fetched.document
        );
    }

    #[test]
    fn an_absolute_fileset_path_is_left_as_the_author_wrote_it() {
        let dir = holding(
            "/work/mixins/pg/lns.yaml",
            "apiVersion: lns.run/v1\nkind: mixin\nname: pg\nspec:\n  filesets:\n    - path: /etc/agent/skills\n      guestPath: /root/.agent/skills\n",
        );
        let fetched = read_path_mixin(&dir, Path::new("/work/mixins/pg")).unwrap();
        assert!(
            fetched.document.contains(r#""/etc/agent/skills""#),
            "an absolute path already names one file, so rooting it again would move it; got: {}",
            fetched.document
        );
    }

    #[test]
    fn a_directory_holding_no_document_says_which_file_is_missing() {
        let err = read_path_mixin(
            &Fake::empty_dir("/work/mixins/pg"),
            Path::new("/work/mixins/pg"),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("/work/mixins/pg/lns.yaml"),
            "a typo in a directory name is only correctable if the refusal names the file it looked for; got: {err:#}"
        );
    }

    #[test]
    fn a_document_that_is_not_yaml_names_the_file_it_could_not_parse() {
        let dir = holding("/work/mixins/pg/lns.yaml", "\tnot: [valid");
        let err = read_path_mixin(&dir, Path::new("/work/mixins/pg")).unwrap_err();
        assert!(
            format!("{err:#}").contains("parsing /work/mixins/pg/lns.yaml"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_directory_nobody_has_decided_anything_in_contributes_nothing() {
        assert!(
            read_local_mixin(
                &Fake::new(BTreeMap::new()),
                Path::new("/work/decisions.yaml")
            )
            .unwrap()
            .is_none(),
            "§8.1 has the mixin exist whether or not anyone created it, so an absent file is not an error"
        );
    }

    #[test]
    fn a_decisions_file_an_editor_has_truncated_contributes_nothing() {
        let dir = holding("/work/decisions.yaml", "  \n");
        assert!(
            read_local_mixin(&dir, Path::new("/work/decisions.yaml"))
                .unwrap()
                .is_none(),
            "an editor truncates a file before it writes one, and a run that caught it mid-write must not refuse"
        );
    }

    #[test]
    fn the_decisions_file_is_named_by_the_file_it_is() {
        let dir = holding(
            "/work/decisions.yaml",
            "apiVersion: lns.run/v1\nkind: mixin\nname: decisions\nspec:\n  tools:\n    - ripgrep@14\n",
        );
        let fetched = read_local_mixin(&dir, Path::new("/work/decisions.yaml"))
            .unwrap()
            .expect("a written file contributes");
        assert_eq!(
            fetched.pinned, "decisions.yaml",
            "nothing names this source, so a disclosure attributes it by the file it is"
        );
        assert!(
            fetched.document.contains(r#""ripgrep@14""#),
            "got: {}",
            fetched.document
        );
    }

    #[test]
    fn a_path_the_decisions_file_writes_is_rooted_in_its_own_directory() {
        let dir = holding(
            "/work/decisions.yaml",
            "apiVersion: lns.run/v1\nkind: mixin\nname: decisions\nspec:\n  filesets:\n    - path: ./notes\n      guestPath: /home/agent/notes\n",
        );
        let fetched = read_local_mixin(&dir, Path::new("/work/decisions.yaml"))
            .unwrap()
            .expect("a written file contributes");
        assert!(
            fetched.document.contains(r#""/work/notes""#),
            "a relative path is written against the directory the file sits in; got: {}",
            fetched.document
        );
    }

    #[test]
    fn a_decisions_file_that_cannot_be_read_says_so_rather_than_deciding_nothing() {
        struct Denied;
        impl MixinDir for Denied {
            fn read(&self, _: &Path) -> std::io::Result<String> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "denied",
                ))
            }

            fn is_dir(&self, _: &Path) -> bool {
                false
            }
        }
        let refused = read_path_mixin(&Denied, Path::new("/work/mixins/pg")).unwrap_err();
        assert!(
            format!("{refused:#}").contains("/work/mixins/pg"),
            "a path this machine cannot read is not a path that decided nothing; got: {refused:#}"
        );
        let err = read_local_mixin(&Denied, Path::new("/work/decisions.yaml")).unwrap_err();
        assert!(
            format!("{err:#}").contains("/work/decisions.yaml"),
            "a file that exists and cannot be read is not a directory that decided nothing; got: {err:#}"
        );
    }

    #[test]
    fn a_decisions_file_that_is_not_yaml_names_the_file_it_could_not_parse() {
        let dir = holding("/work/decisions.yaml", "\tnot: [valid");
        let err = read_local_mixin(&dir, Path::new("/work/decisions.yaml")).unwrap_err();
        assert!(
            format!("{err:#}").contains("parsing /work/decisions.yaml"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_path_that_names_the_document_is_read_as_the_document() {
        let dir = holding(
            "/work/mixins/pg/lns.yaml",
            "apiVersion: lns.run/v1\nkind: mixin\nname: pg\nspec:\n  tools:\n    - python@3.12\n",
        );
        let fetched = read_path_mixin(&dir, Path::new("/work/mixins/pg/lns.yaml")).unwrap();
        assert!(
            fetched.document.contains(r#""python@3.12""#),
            "`lns run` takes a directory or the document inside it, and one reference grammar means one rule; got: {}",
            fetched.document
        );
    }

    #[test]
    fn a_path_that_names_the_document_roots_beside_that_document() {
        let dir = holding(
            "/work/mixins/pg/lns.yaml",
            "apiVersion: lns.run/v1\nkind: mixin\nname: pg\nspec:\n  filesets:\n    - path: ./skills\n      guestPath: /root/.agent/skills\n",
        );
        let fetched = read_path_mixin(&dir, Path::new("/work/mixins/pg/lns.yaml")).unwrap();
        assert!(
            fetched.document.contains(r#""/work/mixins/pg/skills""#),
            "a path the document writes is written against the directory it sits in, however the reference spelled it; got: {}",
            fetched.document
        );
    }
}
