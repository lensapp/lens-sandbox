//! Staging for a run's `pre-start` scripts (`docs/sandbox-spec.md` §3.1.13): files rather than kernel-cmdline keys, because a multi-line body cannot ride a space-joined cmdline.

use anyhow::{Context, Result};
use lns_artifact::sandbox::ScriptStep;
use lns_session::{SCRIPTS_DIR, SCRIPTS_MANIFEST_PATH, ScriptManifest, ScriptManifestStep};

use crate::artifact::fileset::MaterializedFilesets;
use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};

/// Read-only and root-owned: a workload that could rewrite a staged script could change what the next run of the same sandbox executes.
const SCRIPT_MODE: u32 = 0o555;
const MANIFEST_MODE: u32 = 0o444;

/// The runtime-layer specs a run's scripts need, in run order; an empty list stages nothing at all, so an absent manifest stays the guest's signal that there is no work.
pub fn scripts_runtime_specs(scripts: &[ScriptStep]) -> Result<Vec<RuntimeFileSpec>> {
    if scripts.is_empty() {
        return Ok(Vec::new());
    }
    let mut specs = Vec::with_capacity(scripts.len() + 1);
    let steps = scripts
        .iter()
        .enumerate()
        .map(|(index, script)| {
            let guest_path = script_path(index);
            specs.push(RuntimeFileSpec {
                guest_path: guest_path.clone(),
                mode: SCRIPT_MODE,
                source: RuntimeSource::Bytes(script.run.clone().into_bytes()),
            });
            ScriptManifestStep {
                script: guest_path,
                user: script.user.clone(),
                label: script.label(),
            }
        })
        .collect();
    let manifest = serde_json::to_vec(&ScriptManifest { steps })
        .context("serializing the pre-start script manifest")?;
    specs.push(RuntimeFileSpec {
        guest_path: SCRIPTS_MANIFEST_PATH.to_string(),
        mode: MANIFEST_MODE,
        source: RuntimeSource::Bytes(manifest),
    });
    Ok(specs)
}

/// Add a run's scripts to what the layer already carries. They are never workload-owned, so they go in beside the filesets rather than through the chown path.
pub fn absorb(scripts: &[ScriptStep], into: &mut MaterializedFilesets) -> Result<()> {
    into.specs.extend(scripts_runtime_specs(scripts)?);
    Ok(())
}

/// The manifest a staged spec list carries, for a caller that has the specs rather than the documents — the guest's own view of what it will run.
pub fn staged_manifest(specs: &[RuntimeFileSpec]) -> Option<ScriptManifest> {
    specs
        .iter()
        .find(|spec| spec.guest_path == SCRIPTS_MANIFEST_PATH)
        .and_then(|spec| spec.source.as_bytes())
        .and_then(|bytes| serde_json::from_slice(bytes).ok())
}

fn script_path(index: usize) -> String {
    format!("{SCRIPTS_DIR}/{index:03}.sh")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staged(scripts: &[ScriptStep]) -> Vec<RuntimeFileSpec> {
        scripts_runtime_specs(scripts).expect("a manifest of owned strings serializes")
    }

    /// Every staged script's body, in run order; the manifest is not one.
    fn staged_bodies(specs: &[RuntimeFileSpec]) -> Vec<String> {
        specs
            .iter()
            .filter(|spec| spec.guest_path != SCRIPTS_MANIFEST_PATH)
            .filter_map(|spec| spec.source.as_bytes())
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .collect()
    }

    fn script(run: &str, user: Option<&str>, description: Option<&str>) -> ScriptStep {
        ScriptStep {
            when: lns_artifact::sandbox::ScriptSlot::PreStart,
            run: run.to_string(),
            user: user.map(str::to_string),
            description: description.map(str::to_string),
        }
    }

    #[test]
    fn a_run_declaring_no_scripts_stages_nothing() {
        assert!(
            scripts_runtime_specs(&[])
                .expect("nothing to serialize")
                .is_empty(),
            "the guest reads an absent manifest as 'no work', so staging an empty one would make every run pay for a block almost none declares"
        );
    }

    #[test]
    fn each_script_is_staged_at_its_own_path_in_run_order() {
        let specs = staged(&[script("first", None, None), script("second", None, None)]);
        let manifest = staged_manifest(&specs).expect("a manifest is staged");
        let paths: Vec<&str> = manifest.steps.iter().map(|s| s.script.as_str()).collect();
        assert_eq!(
            paths,
            ["/.lens/scripts/000.sh", "/.lens/scripts/001.sh"],
            "each script gets its own path so two with identical bodies stay two files; the manifest, not the filename, is what carries run order"
        );
    }

    #[test]
    fn a_staged_script_carries_its_body_verbatim() {
        let body = "apt-get update\napt-get install -y jq";
        let specs = staged(&[script(body, None, None)]);
        assert_eq!(
            staged_bodies(&specs),
            vec![body.to_string()],
            "the guest hands this file to sh, so a body the staging reflows is a script the author never wrote"
        );
    }

    #[test]
    fn a_staged_script_is_not_writable_by_anyone() {
        for spec in &staged(&[script("true", None, None)]) {
            assert_eq!(
                spec.mode & 0o222,
                0,
                "a workload that could rewrite a staged script could change what a later run of the same sandbox executes: {}",
                spec.guest_path
            );
        }
    }

    #[test]
    fn a_scripts_user_and_label_travel_in_the_manifest() {
        let specs = staged(&[script(
            "apt-get install -y jq",
            Some("root"),
            Some("the jq the prompts assume"),
        )]);
        let step = staged_manifest(&specs)
            .expect("a manifest is staged")
            .steps
            .remove(0);
        assert_eq!(step.user.as_deref(), Some("root"));
        assert_eq!(
            step.label, "the jq the prompts assume",
            "only the guest can resolve a user against its own passwd, and a failure has no name to report but this label"
        );
    }

    #[test]
    fn a_script_with_no_description_is_labelled_by_its_first_line() {
        let specs = staged(&[script(
            "\n  apt-get update\napt-get install -y jq",
            None,
            None,
        )]);
        let step = staged_manifest(&specs)
            .expect("a manifest is staged")
            .steps
            .remove(0);
        assert_eq!(step.label, "apt-get update");
        assert!(
            step.user.is_none(),
            "an absent user has to stay absent, or the guest cannot tell 'defer to the run's identity' from a named one"
        );
    }

    #[test]
    fn absorb_adds_the_scripts_without_claiming_them_for_the_workload() {
        let mut materialized = MaterializedFilesets::default();
        absorb(&[script("true", None, None)], &mut materialized).expect("staging succeeds");
        assert!(
            materialized.owned_paths.is_empty(),
            "a staged script is root's, so it must never reach the chown manifest that hands paths to the workload user"
        );
        assert_eq!(materialized.specs.len(), 2);
    }

    #[test]
    fn specs_carrying_no_usable_manifest_report_none() {
        assert!(
            staged_manifest(&[]).is_none(),
            "a caller asking what a run will execute has to be able to learn that the answer is nothing"
        );
        assert!(
            staged_manifest(&[RuntimeFileSpec {
                guest_path: SCRIPTS_MANIFEST_PATH.to_string(),
                mode: MANIFEST_MODE,
                source: RuntimeSource::Bytes(b"not json".to_vec()),
            }])
            .is_none(),
            "a manifest that does not parse is not an empty one, and the reader must not pass it off as such"
        );
        assert!(
            staged_manifest(&[RuntimeFileSpec {
                guest_path: SCRIPTS_MANIFEST_PATH.to_string(),
                mode: MANIFEST_MODE,
                source: RuntimeSource::HostFile("/somewhere/else".into()),
            }])
            .is_none(),
            "the manifest is written inline by this module, so anything else at that path is not one it can answer for"
        );
    }
}
