use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::sandbox::author::{Fs, LNS_YAML, load_definition_json_at};

/// What `lns run` launches: a registry reference the service resolves and classifies, or a local `lns.yaml` sandbox definition run directly, rooted at its own directory.
#[derive(Debug)]
pub enum RunTarget {
    Reference(String),
    Local {
        def: Box<lns_artifact::sandbox::Definition>,
        json: String,
        project_dir: PathBuf,
    },
}

/// Resolve the run reference: a path-shaped REF (`.`, `lns.yaml`, `./…`, `../…`, `/…`) or the `--file` selector names a local definition, an omitted REF runs the current directory's `lns.yaml`, and anything else is a registry coordinate.
pub fn resolve<F: Fs>(
    reference: Option<&str>,
    file: Option<&Path>,
    fs: &F,
    cwd: &Path,
) -> Result<RunTarget> {
    let file = match (reference, file) {
        (Some(_), Some(_)) => bail!("pass a sandbox reference or --file, not both"),
        (Some(reference), None) if !is_definition_path(reference) => {
            return Ok(RunTarget::Reference(reference.to_string()));
        }
        (Some(reference), None) => definition_file(reference, cwd),
        (None, Some(file)) => lns_artifact::sandbox::fold_path(&cwd.join(file)),
        (None, None) => cwd.join(LNS_YAML),
    };
    let project_dir = file
        .parent()
        .context("resolving the definition's directory")?
        .to_path_buf();
    if !fs.exists(&file) {
        if file.file_name() != Some(LNS_YAML.as_ref()) {
            bail!("no sandbox definition at {}", file.display());
        }
        bail!(
            "no {LNS_YAML} in {}; run `lns init` there to create one",
            project_dir.display()
        );
    }
    let json = load_definition_json_at(fs, &file)?;
    let def = lns_artifact::sandbox::parse(&json)?;
    let problems = crate::sandbox::fileset::path_fileset_problems(fs, &project_dir, &def);
    if !problems.is_empty() {
        bail!("{}", problems.join("\n"));
    }
    let json = absolutize_fileset_paths(json, &def, &project_dir)?;
    let json = String::from_utf8(json).context("definition json was not utf-8")?;
    Ok(RunTarget::Local {
        def: Box::new(def),
        json,
        project_dir,
    })
}

/// Docker-familiar muscle memory types `lns run .` or `lns run lns.yaml`; a conservative path shape keeps every plausible registry coordinate (even `team/config.yaml`) a reference.
pub(crate) fn is_definition_path(reference: &str) -> bool {
    reference == "."
        || reference == ".."
        || reference == LNS_YAML
        || reference.starts_with("./")
        || reference.starts_with("../")
        || reference.starts_with('/')
        || reference.ends_with(&format!("/{LNS_YAML}"))
}

pub(crate) fn definition_file(reference: &str, cwd: &Path) -> PathBuf {
    let path = lns_artifact::sandbox::fold_path(&cwd.join(reference));
    if is_yaml_file_name(&path) {
        path
    } else {
        path.join(LNS_YAML)
    }
}

fn is_yaml_file_name(path: &Path) -> bool {
    matches!(
        path.extension().and_then(std::ffi::OsStr::to_str),
        Some("yaml" | "yml")
    )
}

/// The wire definition roots each path fileset in the consumer's project, the same way bind sources resolve, so the service snapshots exactly the directories this machine declared.
fn absolutize_fileset_paths(
    json: Vec<u8>,
    def: &lns_artifact::sandbox::Definition,
    cwd: &Path,
) -> Result<Vec<u8>> {
    if def.spec.filesets.iter().all(|f| f.path.is_none()) {
        return Ok(json);
    }
    let mut value: serde_json::Value =
        serde_json::from_slice(&json).context("re-reading the definition for fileset rooting")?;
    let entries = value["spec"]["filesets"]
        .as_array_mut()
        .context("spec.filesets is not an array")?;
    for (index, fileset) in def.spec.filesets.iter().enumerate() {
        if let Some(path) = &fileset.path {
            let rooted = crate::run::declarative::resolve_bind_source(path, cwd, None)
                .with_context(|| format!("fileset {path}"))?;
            entries[index]["path"] = serde_json::Value::String(rooted);
        }
    }
    serde_json::to_vec(&value).context("serializing the rooted definition")
}

impl RunTarget {
    /// True when the service must classify the reference and refuse a plain image; false for a local base-image run the CLI already validated.
    pub fn verify_sandbox(&self) -> bool {
        matches!(self, RunTarget::Reference(_))
    }

    /// A local definition's relative bind sources and filesets root at its own directory, compose-style.
    pub fn project_dir(&self) -> Option<&Path> {
        match self {
            RunTarget::Reference(_) => None,
            RunTarget::Local { project_dir, .. } => Some(project_dir),
        }
    }

    /// The base OCI image the service boots: the reference itself, or the local definition's declared base image.
    pub fn image(&self) -> String {
        match self {
            RunTarget::Reference(reference) => reference.clone(),
            RunTarget::Local { def, .. } => def.spec.image.clone(),
        }
    }

    /// The local definition's canonical JSON for the wire, so the service applies its command, env, policy, connectors, and resources like a published sandbox's.
    pub fn definition_json(&self) -> Option<String> {
        match self {
            RunTarget::Reference(_) => None,
            RunTarget::Local { json, .. } => Some(json.clone()),
        }
    }
}

/// A directory the user types is theirs, so it roots where they typed it; the service reads an absolute path or nothing.
pub fn root_named_directories(mixins: &[String], cwd: &Path) -> Result<Vec<String>> {
    mixins
        .iter()
        .map(|reference| {
            if !lns_artifact::sandbox::names_a_local_directory(reference) {
                return Ok(reference.clone());
            }
            let rooted = lns_artifact::sandbox::fold_path(&cwd.join(reference));
            rooted
                .to_str()
                .map(str::to_string)
                .with_context(|| format!("mixin directory {} is not utf-8", rooted.display()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_the_user_names_roots_where_they_typed_it() {
        let rooted = root_named_directories(
            &["./mixins/pg".to_string(), "ghcr.io/acme/obs:2".to_string()],
            Path::new("/work"),
        )
        .unwrap();
        assert_eq!(
            rooted,
            ["/work/mixins/pg", "ghcr.io/acme/obs:2"],
            "only the caller knows where the user typed a relative directory, and a reference is not a path to root"
        );
    }

    use crate::sandbox::test_support::MapFs;

    fn fake(path: &str, contents: &str) -> MapFs {
        MapFs::with(&[(path, contents)])
    }

    fn cwd() -> &'static Path {
        Path::new("/work")
    }

    fn local_yaml() -> &'static str {
        "apiVersion: lns.run/v1\nkind: sandbox\nmetadata:\n  name: hermes\nspec:\n  image: ghcr.io/team/base:1\n  command: agent --serve\n  env:\n    MODE: research\n"
    }

    #[test]
    fn a_reference_is_passed_through_for_the_service_to_classify() {
        let fs = MapFs::default();
        let target = resolve(Some("ghcr.io/team/hermes:1.4.0"), None, &fs, cwd()).unwrap();
        assert_eq!(target.image(), "ghcr.io/team/hermes:1.4.0");
        assert!(
            target.verify_sandbox(),
            "a bare reference must be classified by the service"
        );
    }

    #[test]
    fn no_reference_resolves_the_local_definition() {
        let fs = fake("/work/lns.yaml", local_yaml());
        let target = resolve(None, None, &fs, cwd()).unwrap();
        assert_eq!(target.image(), "ghcr.io/team/base:1");
        assert!(
            !target.verify_sandbox(),
            "a local sandbox's base image runs directly, not re-classified"
        );
    }

    #[test]
    fn a_local_definition_travels_as_canonical_json() {
        let fs = fake("/work/lns.yaml", local_yaml());
        let target = resolve(None, None, &fs, cwd()).unwrap();
        let json = target
            .definition_json()
            .expect("a local run carries its definition");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["spec"]["image"], "ghcr.io/team/base:1");
        assert_eq!(value["spec"]["command"], "agent --serve");
        assert_eq!(value["spec"]["env"]["MODE"], "research");
    }

    #[test]
    fn a_reference_carries_no_definition_and_no_project_dir() {
        let fs = MapFs::default();
        let target = resolve(Some("alpine:3.20"), None, &fs, cwd()).unwrap();
        assert_eq!(target.definition_json(), None);
        assert_eq!(target.project_dir(), None);
    }

    #[test]
    fn no_reference_without_a_definition_names_lns_init() {
        let fs = MapFs::default();
        let err = resolve(None, None, &fs, cwd()).unwrap_err();
        assert!(format!("{err:#}").contains("no lns.yaml"), "got: {err:#}");
        assert!(format!("{err:#}").contains("lns init"), "got: {err:#}");
    }

    #[test]
    fn dot_and_yaml_shaped_references_resolve_the_local_definition() {
        let fs = fake("/work/lns.yaml", local_yaml());
        for reference in [".", "lns.yaml", "./lns.yaml", "./"] {
            let target = resolve(Some(reference), None, &fs, cwd()).unwrap();
            assert_eq!(target.image(), "ghcr.io/team/base:1", "ref {reference:?}");
            assert!(!target.verify_sandbox(), "ref {reference:?}");
            assert_eq!(target.project_dir(), Some(Path::new("/work")));
        }
    }

    #[test]
    fn a_relative_or_absolute_path_resolves_that_directorys_definition() {
        let fs = fake("/other/lns.yaml", local_yaml());
        for reference in ["../other", "../other/lns.yaml", "/other", "/other/lns.yaml"] {
            let target = resolve(Some(reference), None, &fs, cwd()).unwrap();
            assert_eq!(target.image(), "ghcr.io/team/base:1", "ref {reference:?}");
            assert_eq!(
                target.project_dir(),
                Some(Path::new("/other")),
                "ref {reference:?}"
            );
        }
    }

    #[test]
    fn a_path_definitions_filesets_root_at_its_own_directory() {
        let fs = MapFs::with(&[
            (
                "/other/lns.yaml",
                "apiVersion: lns.run/v1\nkind: sandbox\nmetadata:\n  name: hermes\nspec:\n  image: x:1\n  filesets:\n    - path: ./skills\n      mountPath: /root/.agent/skills\n",
            ),
            ("/other/skills/prompts.md", "p"),
        ]);
        let target = resolve(Some("/other"), None, &fs, cwd()).unwrap();
        let json = target.definition_json().expect("local definition");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["spec"]["filesets"][0]["path"], "/other/skills");
    }

    #[test]
    fn a_yaml_file_reference_resolves_that_files_definition() {
        let fs = MapFs::with(&[
            ("/work/lns.dev.yaml", local_yaml()),
            ("/other/app.yml", local_yaml()),
        ]);
        for (reference, project) in [
            ("./lns.dev.yaml", "/work"),
            ("/other/app.yml", "/other"),
            ("../other/app.yml", "/other"),
        ] {
            let target = resolve(Some(reference), None, &fs, cwd()).unwrap();
            assert_eq!(target.image(), "ghcr.io/team/base:1", "ref {reference:?}");
            assert_eq!(
                target.project_dir(),
                Some(Path::new(project)),
                "ref {reference:?}"
            );
        }
    }

    #[test]
    fn a_yaml_file_reference_selects_the_variant_over_the_default() {
        let variant = "apiVersion: lns.run/v1\nkind: sandbox\nmetadata:\n  name: dev\nspec:\n  image: ghcr.io/team/dev:1\n";
        let fs = MapFs::with(&[
            ("/work/lns.yaml", local_yaml()),
            ("/work/lns.dev.yaml", variant),
        ]);
        let target = resolve(Some("./lns.dev.yaml"), None, &fs, cwd()).unwrap();
        assert_eq!(target.image(), "ghcr.io/team/dev:1");
    }

    #[test]
    fn the_file_selector_resolves_the_named_definition() {
        let variant = "apiVersion: lns.run/v1\nkind: sandbox\nmetadata:\n  name: dev\nspec:\n  image: ghcr.io/team/dev:1\n";
        let fs = MapFs::with(&[
            ("/work/lns.yaml", local_yaml()),
            ("/work/lns.dev.yaml", variant),
        ]);
        let target = resolve(None, Some(Path::new("lns.dev.yaml")), &fs, cwd()).unwrap();
        assert_eq!(target.image(), "ghcr.io/team/dev:1");
        assert_eq!(target.project_dir(), Some(Path::new("/work")));
    }

    #[test]
    fn the_file_selector_roots_the_project_at_the_files_directory() {
        let fs = fake("/other/lns.dev.yaml", local_yaml());
        let target = resolve(None, Some(Path::new("../other/lns.dev.yaml")), &fs, cwd()).unwrap();
        assert_eq!(target.project_dir(), Some(Path::new("/other")));
    }

    #[test]
    fn a_reference_and_the_file_selector_together_are_refused() {
        let fs = fake("/work/lns.dev.yaml", local_yaml());
        let err = resolve(
            Some("ghcr.io/team/hermes:1"),
            Some(Path::new("lns.dev.yaml")),
            &fs,
            cwd(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("not both"), "got: {err:#}");
    }

    #[test]
    fn a_missing_file_selector_errors_with_its_path() {
        let fs = MapFs::default();
        let err = resolve(None, Some(Path::new("lns.dev.yaml")), &fs, cwd()).unwrap_err();
        assert!(
            format!("{err:#}").contains("no sandbox definition at /work/lns.dev.yaml"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_missing_named_definition_file_errors_with_its_path() {
        let fs = MapFs::default();
        let err = resolve(Some("./lns.dev.yaml"), None, &fs, cwd()).unwrap_err();
        let text = format!("{err:#}");
        assert!(
            text.contains("no sandbox definition at /work/lns.dev.yaml"),
            "got: {text}"
        );
        assert!(!text.contains("lns init"), "got: {text}");
    }

    #[test]
    fn a_path_shaped_reference_without_a_definition_names_its_directory() {
        let fs = MapFs::default();
        let err = resolve(Some("../other"), None, &fs, cwd()).unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("no lns.yaml in /other"), "got: {text}");
        assert!(text.contains("lns init"), "got: {text}");
    }

    #[test]
    fn a_repository_name_ending_in_yaml_stays_a_registry_reference() {
        let fs = MapFs::default();
        let target = resolve(Some("team/config.yaml"), None, &fs, cwd()).unwrap();
        assert!(target.verify_sandbox());
        assert_eq!(target.image(), "team/config.yaml");
    }

    #[test]
    fn a_local_path_fileset_is_rooted_in_the_project_on_the_wire() {
        let fs = MapFs::with(&[
            (
                "/work/lns.yaml",
                "apiVersion: lns.run/v1\nkind: sandbox\nmetadata:\n  name: hermes\nspec:\n  image: x:1\n  filesets:\n    - path: ./skills\n      mountPath: /root/.agent/skills\n",
            ),
            ("/work/skills/prompts.md", "p"),
        ]);
        let target = resolve(None, None, &fs, cwd()).unwrap();
        let json = target.definition_json().expect("local definition");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["spec"]["filesets"][0]["path"], "/work/skills");
        assert!(
            matches!(
                &target,
                RunTarget::Local { def, .. }
                    if def.spec.filesets[0].path.as_deref() == Some("./skills")
            ),
            "the parsed definition keeps the author's declared path for display"
        );
    }

    #[test]
    fn a_missing_fileset_directory_refuses_the_local_run() {
        let fs = fake(
            "/work/lns.yaml",
            "apiVersion: lns.run/v1\nkind: sandbox\nmetadata:\n  name: hermes\nspec:\n  image: x:1\n  filesets:\n    - path: ./skills\n      mountPath: /root/.agent/skills\n",
        );
        let err = resolve(None, None, &fs, cwd()).unwrap_err();
        assert!(format!("{err:#}").contains("./skills"), "got: {err:#}");
    }

    #[test]
    fn an_invalid_local_definition_surfaces_the_parse_error() {
        let fs = fake(
            "/work/lns.yaml",
            "apiVersion: lns.run/v1\nkind: sandbox\nmetadata:\n  name: hermes\nspec: {}\n",
        );
        let err = resolve(None, None, &fs, cwd()).unwrap_err();
        assert!(
            format!("{err:#}").contains("must carry an image"),
            "got: {err:#}"
        );
    }
}
