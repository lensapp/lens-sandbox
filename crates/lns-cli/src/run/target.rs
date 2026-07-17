use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::sandbox::author::{Fs, LNS_YAML, load_definition_json};

/// What `lns run` launches: a registry reference the service resolves and classifies, or the local `./lns.yaml` sandbox definition run directly.
#[derive(Debug)]
pub enum RunTarget {
    Reference(String),
    Local {
        def: Box<lns_artifact::sandbox::Definition>,
        json: String,
    },
}

/// Resolve the run reference: a given REF is a registry coordinate; an omitted REF runs the current directory's `lns.yaml`.
pub fn resolve<F: Fs>(reference: Option<&str>, fs: &F, cwd: &Path) -> Result<RunTarget> {
    match reference {
        Some(reference) => Ok(RunTarget::Reference(reference.to_string())),
        None => {
            if !fs.exists(&cwd.join(LNS_YAML)) {
                bail!("no lns.yaml in the current directory; run `lns init` to create one");
            }
            let json = load_definition_json(fs, cwd)?;
            let def = lns_artifact::sandbox::parse(&json)?;
            let problems = crate::sandbox::fileset::path_fileset_problems(fs, cwd, &def);
            if !problems.is_empty() {
                bail!("{}", problems.join("\n"));
            }
            let json = absolutize_fileset_paths(json, &def, cwd)?;
            let json = String::from_utf8(json).context("definition json was not utf-8")?;
            Ok(RunTarget::Local {
                def: Box::new(def),
                json,
            })
        }
    }
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
            let rooted = crate::run::declarative::resolve_bind_source(path, cwd)
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

    /// The base OCI image the service boots: the reference itself, or the local definition's declared base image.
    pub fn image(&self) -> String {
        match self {
            RunTarget::Reference(reference) => reference.clone(),
            RunTarget::Local { def, .. } => def.spec.image.clone(),
        }
    }

    /// The local definition's canonical JSON for the wire, so the service applies its command, env, policy, integrations, and resources like a published sandbox's.
    pub fn definition_json(&self) -> Option<String> {
        match self {
            RunTarget::Reference(_) => None,
            RunTarget::Local { json, .. } => Some(json.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sandbox::test_support::MapFs;

    fn fake(path: &str, contents: &str) -> MapFs {
        MapFs::with(&[(path, contents)])
    }

    fn cwd() -> &'static Path {
        Path::new("/work")
    }

    fn local_yaml() -> &'static str {
        "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: ghcr.io/team/base:1\n  command: agent --serve\n  env:\n    MODE: research\n"
    }

    #[test]
    fn a_reference_is_passed_through_for_the_service_to_classify() {
        let fs = MapFs::default();
        let target = resolve(Some("ghcr.io/team/hermes:1.4.0"), &fs, cwd()).unwrap();
        assert_eq!(target.image(), "ghcr.io/team/hermes:1.4.0");
        assert!(
            target.verify_sandbox(),
            "a bare reference must be classified by the service"
        );
    }

    #[test]
    fn no_reference_resolves_the_local_definition() {
        let fs = fake("/work/lns.yaml", local_yaml());
        let target = resolve(None, &fs, cwd()).unwrap();
        assert_eq!(target.image(), "ghcr.io/team/base:1");
        assert!(
            !target.verify_sandbox(),
            "a local sandbox's base image runs directly, not re-classified"
        );
    }

    #[test]
    fn a_local_definition_travels_as_canonical_json() {
        let fs = fake("/work/lns.yaml", local_yaml());
        let target = resolve(None, &fs, cwd()).unwrap();
        let json = target
            .definition_json()
            .expect("a local run carries its definition");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["spec"]["image"], "ghcr.io/team/base:1");
        assert_eq!(value["spec"]["command"], "agent --serve");
        assert_eq!(value["spec"]["env"]["MODE"], "research");
    }

    #[test]
    fn a_reference_carries_no_definition() {
        let fs = MapFs::default();
        let target = resolve(Some("alpine:3.20"), &fs, cwd()).unwrap();
        assert_eq!(target.definition_json(), None);
    }

    #[test]
    fn no_reference_without_a_definition_names_lns_init() {
        let fs = MapFs::default();
        let err = resolve(None, &fs, cwd()).unwrap_err();
        assert!(format!("{err:#}").contains("no lns.yaml"), "got: {err:#}");
        assert!(format!("{err:#}").contains("lns init"), "got: {err:#}");
    }

    #[test]
    fn a_local_path_fileset_is_rooted_in_the_project_on_the_wire() {
        let fs = MapFs::with(&[
            (
                "/work/lns.yaml",
                "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: x:1\n  filesets:\n    - path: ./skills\n      mountPath: /root/.agent/skills\n",
            ),
            ("/work/skills/prompts.md", "p"),
        ]);
        let target = resolve(None, &fs, cwd()).unwrap();
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
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: x:1\n  filesets:\n    - path: ./skills\n      mountPath: /root/.agent/skills\n",
        );
        let err = resolve(None, &fs, cwd()).unwrap_err();
        assert!(format!("{err:#}").contains("./skills"), "got: {err:#}");
    }

    #[test]
    fn a_local_run_refuses_an_out_of_project_fileset_path() {
        let fs = fake(
            "/work/lns.yaml",
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: x:1\n  filesets:\n    - path: /etc\n      mountPath: /root/.agent/skills\n",
        );
        let err = resolve(None, &fs, cwd()).unwrap_err();
        assert!(
            format!("{err:#}").contains("must be relative to the project"),
            "an absolute fileset path would let the service read files outside the project: {err:#}"
        );
    }

    #[test]
    fn an_invalid_local_definition_surfaces_the_parse_error() {
        let fs = fake(
            "/work/lns.yaml",
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec: {}\n",
        );
        let err = resolve(None, &fs, cwd()).unwrap_err();
        assert!(
            format!("{err:#}").contains("must carry an image"),
            "got: {err:#}"
        );
    }
}
