mod quote;

use lens_sandbox_core::config::CoreConfig;

pub struct AgentConfig {
    pub core: CoreConfig,
    pub agent_command: String,
    /// Explicit `WORKSPACE_PATH`; `None` defers the cwd to the post-setuid home rather than pinning the agent to root's `/root`.
    pub workspace_path: Option<String>,
    /// The staged `pre-start` scripts, in run order; empty is the common case.
    pub scripts: Vec<lns_session::ScriptManifestStep>,
}

/// The staged manifest, injected so the "cannot be read" and "does not parse" branches are reachable without a guest.
pub trait ManifestSource {
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>>;
}

pub struct StagedManifest;

impl ManifestSource for StagedManifest {
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }
}

/// The scripts the run staged: an absent manifest means none, while one we cannot read or parse is refused rather than silently skipped.
pub fn load_scripts(
    source: &dyn ManifestSource,
) -> Result<Vec<lns_session::ScriptManifestStep>, String> {
    let body = match source.read(lns_session::SCRIPTS_MANIFEST_PATH) {
        Ok(body) => body,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(format!(
                "reading {}: {err}",
                lns_session::SCRIPTS_MANIFEST_PATH
            ));
        }
    };
    serde_json::from_slice::<lns_session::ScriptManifest>(&body)
        .map(|manifest| manifest.steps)
        .map_err(|err| format!("parsing {}: {err}", lns_session::SCRIPTS_MANIFEST_PATH))
}

pub fn load_config() -> AgentConfig {
    let core = lens_sandbox_core::config::load_core_config();

    let workspace_path = std::env::var("WORKSPACE_PATH")
        .ok()
        .filter(|s| !s.is_empty());
    let mut agent_command = std::env::var("AGENT_COMMAND").unwrap_or_else(|_| "sh".to_string());

    let cli_args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    quote::append_cli_args(&mut agent_command, &cli_args);

    AgentConfig {
        core,
        agent_command,
        workspace_path,
        scripts: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Staged(std::io::Result<Vec<u8>>);

    impl ManifestSource for Staged {
        fn read(&self, _path: &str) -> std::io::Result<Vec<u8>> {
            match &self.0 {
                Ok(body) => Ok(body.clone()),
                Err(err) => Err(std::io::Error::new(err.kind(), err.to_string())),
            }
        }
    }

    fn staged(body: &str) -> Staged {
        Staged(Ok(body.as_bytes().to_vec()))
    }

    #[test]
    fn no_manifest_means_no_scripts() {
        let absent = Staged(Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no such file",
        )));
        assert!(
            load_scripts(&absent)
                .expect("an absent manifest is not a failure")
                .is_empty(),
            "almost no run declares scripts, so the absent case has to be the quiet one"
        );
    }

    #[test]
    fn a_staged_manifest_is_read_in_run_order() {
        let steps = load_scripts(&staged(
            r#"{"steps":[{"script":"/.lens/scripts/000.sh","label":"first"},{"script":"/.lens/scripts/001.sh","user":"root","label":"second"}]}"#,
        ))
        .expect("a well-formed manifest loads");
        let labels: Vec<&str> = steps.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["first", "second"]);
        assert_eq!(steps[1].user.as_deref(), Some("root"));
        assert!(
            steps[0].user.is_none(),
            "an absent user has to survive the load, or the guest cannot tell it from a named one"
        );
    }

    #[test]
    fn a_manifest_that_does_not_parse_is_refused_rather_than_read_as_empty() {
        let err = load_scripts(&staged("{not json")).expect_err("a broken manifest has no answer");
        assert!(
            err.contains("parsing"),
            "reading it as empty would silently skip every script the consumer approved; got: {err}"
        );
    }

    #[test]
    fn a_manifest_that_cannot_be_read_is_refused() {
        let unreadable = Staged(Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        )));
        let err = load_scripts(&unreadable).expect_err("an unreadable manifest has no answer");
        assert!(
            err.contains("reading"),
            "only NotFound means 'none declared'; every other error is a manifest we were meant to run; got: {err}"
        );
    }
}
