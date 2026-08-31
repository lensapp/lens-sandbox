use std::collections::BTreeMap;
use std::path::Path;

use lns_artifact::spec::Kind;

use super::author::Fs;

pub use lns_artifact::walk::walk;

/// The offline validate/run guard: every path fileset must name a readable directory in the project that its kind is allowed to ship. Reading the directory is what lets a connector be held to §3.2.3 here rather than only at push.
pub fn path_fileset_problems<F: Fs + ?Sized>(
    fs: &F,
    project_dir: &Path,
    paths: &[&str],
    methods: &[lns_artifact::connector::Method],
    kind: Kind,
) -> Vec<String> {
    let mut problems = Vec::new();
    let mut read: BTreeMap<String, BTreeMap<String, Vec<u8>>> = BTreeMap::new();
    for path in paths {
        match walk(fs, &project_dir.join(path), kind) {
            Ok(entries) => {
                read.insert(
                    (*path).to_string(),
                    lns_artifact::build::bytes_by_name(&entries),
                );
            }
            Err(e) => problems.push(format!("fileset {path}: {e:#}")),
        }
    }
    if problems.is_empty() {
        for method in methods {
            if let Err(e) =
                lns_artifact::connector::refuse_a_secret_shaped_file_carrying_no_declared_placeholder(
                    method, &read,
                )
            {
                problems.push(format!("{e:#}"));
            }
        }
    }
    problems
}

/// The document a local mixin reference names: a directory is read as the `lns.yaml` inside it, and a path naming the document is that document.
pub fn local_mixin_document<F: Fs + ?Sized>(
    fs: &F,
    project_dir: &Path,
    reference: &str,
) -> std::path::PathBuf {
    let named = lns_artifact::sandbox::fold_path(&project_dir.join(reference));
    if fs.is_dir(&named) {
        named.join("lns.yaml")
    } else {
        named
    }
}

/// A directory mixin is a file on this machine, so `validate` can say the document names one that is not there rather than leaving it to the run.
pub fn directory_mixin_problems<F: Fs + ?Sized>(
    fs: &F,
    project_dir: &Path,
    mixins: &[String],
) -> Vec<String> {
    mixins
        .iter()
        .filter(|reference| lns_artifact::sandbox::names_a_local_path(reference))
        .filter_map(|reference| {
            let document = local_mixin_document(fs, project_dir, reference);
            fs.read_to_string(&document)
                .err()
                .map(|e| format!("mixin {reference}: reading {}: {e}", document.display()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::test_support::MapFs;

    fn definition(mixins: &str) -> lns_artifact::sandbox::Definition {
        lns_artifact::sandbox::parse(
            format!(
                r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{{"image":"x:1","mixins":{mixins}}}}}"#
            )
            .as_bytes(),
        )
        .expect("the fixture is a valid definition")
    }

    #[test]
    fn validate_accepts_a_mixin_named_by_its_document() {
        let fs = MapFs::with(&[("/work/mixins/pg/lns.yaml", "kind: mixin")]);
        let problems = directory_mixin_problems(
            &fs,
            Path::new("/work"),
            &definition(r#"["./mixins/pg/lns.yaml"]"#).spec.mixins,
        );
        assert!(
            problems.is_empty(),
            "a run resolves this entry, so validate calling it broken is validate being wrong; got {problems:?}"
        );
    }

    #[test]
    fn validate_names_a_mixin_path_that_holds_no_document() {
        let fs = MapFs::with(&[("/work/mixins/present/lns.yaml", "kind: mixin")]);
        let problems = directory_mixin_problems(
            &fs,
            Path::new("/work"),
            &definition(r#"["./mixins/present","./mixins/absent"]"#)
                .spec
                .mixins,
        );
        assert_eq!(
            problems.len(),
            1,
            "a typo in a directory name is cheapest to correct at author time; got {problems:?}"
        );
        assert!(
            problems[0].contains("/work/mixins/absent"),
            "a path that is neither a directory nor a document is named as the author wrote it, rather than as a file inside a directory that is not there; got {problems:?}"
        );
    }

    #[test]
    fn validate_leaves_a_published_mixin_to_the_run() {
        let problems = directory_mixin_problems(
            &MapFs::with(&[]),
            Path::new("/work"),
            &definition(&format!(
                r#"["ghcr.io/acme/obs@sha256:{}"]"#,
                "c".repeat(64)
            ))
            .spec
            .mixins,
        );
        assert!(
            problems.is_empty(),
            "validate reads one document offline, so a reference it would have to pull is not its to check; got {problems:?}"
        );
    }

    #[test]
    fn path_fileset_problems_reports_only_broken_path_entries() {
        let fs = MapFs::with(&[("/work/skills/prompts.md", "p")]);
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"s","spec":{"image":"x:1","filesets":[{"path":"./skills","guestPath":"/a"},{"path":"./missing","guestPath":"/b"},{"inline":{"a.md":"x"},"guestPath":"/c"}]}}"#,
        )
        .unwrap();
        let paths: Vec<&str> = lns_artifact::merge::path_filesets(&def.spec)
            .map(|(_, _, path)| path)
            .collect();
        let problems = path_fileset_problems(&fs, Path::new("/work"), &paths, &[], Kind::Sandbox);
        assert_eq!(problems.len(), 1, "got: {problems:?}");
        assert!(problems[0].contains("./missing"), "got: {problems:?}");
    }

    fn connector_with_a_path_fileset() -> lns_artifact::connector::ConnectorDefinition {
        lns_artifact::connector::parse(
                br#"{"apiVersion":"lns.run/v1","kind":"connector","name":"some-provider","spec":{"serves":["api.some-provider.example"],"methods":[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}],"filesets":[{"path":"./some-provider","guestPath":"~/.some-provider"}]}]}}"#,
            )
        .expect("the fixture is a valid connector")
    }

    #[test]
    fn validate_accepts_a_connector_path_file_that_carries_the_declared_placeholder() {
        let fs = MapFs::with(&[(
            "/work/some-provider/credentials.json",
            r#"{"token":"some_LNSPLACEHOLDER0000000000"}"#,
        )]);
        let def = connector_with_a_path_fileset();
        let problems = path_fileset_problems(
            &fs,
            Path::new("/work"),
            &["./some-provider"],
            &def.spec.methods,
            Kind::Connector,
        );
        assert!(
            problems.is_empty(),
            "§3.2.3 makes a connector the one kind whose fileset exists to write this exact file, so refusing it by name leaves the placeholder rule with nothing to check; got {problems:?}"
        );
    }

    #[test]
    fn validate_refuses_a_connector_path_file_that_declares_no_placeholder() {
        let fs = MapFs::with(&[(
            "/work/some-provider/credentials.json",
            r#"{"token":"sk-live-real"}"#,
        )]);
        let def = connector_with_a_path_fileset();
        let problems = path_fileset_problems(
            &fs,
            Path::new("/work"),
            &["./some-provider"],
            &def.spec.methods,
            Kind::Connector,
        );
        assert_eq!(problems.len(), 1, "got {problems:?}");
        assert!(
            problems[0].contains("carries no placeholder"),
            "§5 runs the §3.2.3 read over a `path` directory beside the document, so validate must refuse this for the same reason push does rather than by the name alone; got {problems:?}"
        );
    }
}
