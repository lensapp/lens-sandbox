use anyhow::{Result, bail};

pub mod assembly;
pub mod resolve;

pub const BUNDLE_ARTIFACT_TYPE: &str = "application/vnd.lens.bundle.v1+json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunPath {
    SingleImage,
    AssembleBundle,
}

pub fn dispatch(artifact_type: Option<&str>) -> Result<RunPath> {
    match artifact_type {
        None | Some("") => Ok(RunPath::SingleImage),
        Some(t) if t == BUNDLE_ARTIFACT_TYPE => Ok(RunPath::AssembleBundle),
        Some(other) => bail!(
            "unsupported artifact type {other}; \
             lns run can launch a plain OCI image or an AgentSystem bundle"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_artifact_type_is_treated_as_a_plain_image() {
        assert_eq!(dispatch(Some("")).unwrap(), RunPath::SingleImage);
    }
}
