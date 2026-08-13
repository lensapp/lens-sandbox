use std::path::Path;

use anyhow::{Context, Result};

use crate::artifact::mixin::FetchedMixin;

/// The file a directory means, the same one `lns run ./dir` reads.
const DOCUMENT: &str = "lns.yaml";

/// Reads a mixin document off this machine, so the walk can merge a directory without owning any filesystem access itself.
pub trait MixinDir: Send + Sync {
    fn read(&self, path: &Path) -> std::io::Result<String>;
}

/// Read the document a directory holds, as the JSON every other source is parsed from.
pub fn read_directory_mixin<D: MixinDir>(dir: &D, path: &Path) -> Result<FetchedMixin> {
    let document = path.join(DOCUMENT);
    let yaml = dir
        .read(&document)
        .with_context(|| format!("reading {}", document.display()))?;
    let mut parsed: serde_json::Value =
        serde_yaml::from_str(&yaml).with_context(|| format!("parsing {}", document.display()))?;
    root_relative_paths(&mut parsed, path);
    Ok(FetchedMixin {
        pinned: path.display().to_string(),
        document: serde_json::to_string(&parsed).context("serializing the mixin")?,
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

    struct Fake(BTreeMap<PathBuf, String>);

    impl MixinDir for Fake {
        fn read(&self, path: &Path) -> std::io::Result<String> {
            self.0.get(path).cloned().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, format!("{}", path.display()))
            })
        }
    }

    fn holding(path: &str, yaml: &str) -> Fake {
        Fake(BTreeMap::from([(PathBuf::from(path), yaml.to_string())]))
    }

    #[test]
    fn a_directory_answers_with_its_own_document_under_its_own_path() {
        let dir = holding(
            "/work/mixins/pg/lns.yaml",
            "apiVersion: lns.run/v1\nkind: mixin\nmetadata:\n  name: pg\nspec:\n  tools:\n    - python@3.12\n",
        );
        let fetched = read_directory_mixin(&dir, Path::new("/work/mixins/pg")).unwrap();
        assert_eq!(
            fetched.pinned, "/work/mixins/pg",
            "a directory has no digest, so its path is the only identity the disclosure can name it by"
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
            "apiVersion: lns.run/v1\nkind: mixin\nmetadata:\n  name: pg\nspec:\n  filesets:\n    - path: ./skills\n      mountPath: /root/.agent/skills\n",
        );
        let fetched = read_directory_mixin(&dir, Path::new("/work/mixins/pg")).unwrap();
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
            "apiVersion: lns.run/v1\nkind: mixin\nmetadata:\n  name: pg\nspec:\n  filesets:\n    - inline:\n        notes.md: hello\n      mountPath: /root/.agent/skills\n",
        );
        let fetched = read_directory_mixin(&dir, Path::new("/work/mixins/pg")).unwrap();
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
            "apiVersion: lns.run/v1\nkind: mixin\nmetadata:\n  name: pg\nspec:\n  volumes:\n    - type: bind\n      source: ./data\n      target: /data\n",
        );
        let fetched = read_directory_mixin(&dir, Path::new("/work/mixins/pg")).unwrap();
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
            "apiVersion: lns.run/v1\nkind: mixin\nmetadata:\n  name: pg\nspec:\n  volumes:\n    - type: volume\n      source: pg-data\n      target: /data\n",
        );
        let fetched = read_directory_mixin(&dir, Path::new("/work/mixins/pg")).unwrap();
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
            "apiVersion: lns.run/v1\nkind: mixin\nmetadata:\n  name: pg\nspec:\n  volumes:\n    - type: bind\n      source: ~/.config/pg\n      target: /config\n",
        );
        let fetched = read_directory_mixin(&dir, Path::new("/work/mixins/pg")).unwrap();
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
            "apiVersion: lns.run/v1\nkind: mixin\nmetadata:\n  name: pg\nspec:\n  filesets:\n    - path: /etc/agent/skills\n      mountPath: /root/.agent/skills\n",
        );
        let fetched = read_directory_mixin(&dir, Path::new("/work/mixins/pg")).unwrap();
        assert!(
            fetched.document.contains(r#""/etc/agent/skills""#),
            "an absolute path already names one file, so rooting it again would move it; got: {}",
            fetched.document
        );
    }

    #[test]
    fn a_directory_with_no_document_says_which_file_is_missing() {
        let err =
            read_directory_mixin(&Fake(BTreeMap::new()), Path::new("/work/mixins/pg")).unwrap_err();
        assert!(
            format!("{err:#}").contains("/work/mixins/pg/lns.yaml"),
            "a typo in a directory name is only correctable if the refusal names the file it looked for; got: {err:#}"
        );
    }

    #[test]
    fn a_document_that_is_not_yaml_names_the_file_it_could_not_parse() {
        let dir = holding("/work/mixins/pg/lns.yaml", "\tnot: [valid");
        let err = read_directory_mixin(&dir, Path::new("/work/mixins/pg")).unwrap_err();
        assert!(
            format!("{err:#}").contains("parsing /work/mixins/pg/lns.yaml"),
            "got: {err:#}"
        );
    }
}
