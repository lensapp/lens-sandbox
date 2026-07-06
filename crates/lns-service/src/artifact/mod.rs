use anyhow::{Result, bail};

pub mod assembly;
pub mod fetch;
pub mod resolve;
pub mod spec;

use spec::Kind;

pub const BUNDLE_ARTIFACT_TYPE: &str = "application/vnd.lens.bundle.v1+json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunPath {
    SingleImage,
    AssembleBundle,
}

pub fn dispatch(artifact_type: Option<&str>, config_media_type: Option<&str>) -> Result<RunPath> {
    let artifact_type = artifact_type.filter(|t| !t.is_empty());
    let config_media_type = config_media_type.filter(|t| !t.is_empty());
    let kind = artifact_type
        .and_then(Kind::from_artifact_type)
        .or_else(|| config_media_type.and_then(Kind::from_config_media_type));
    match kind {
        Some(Kind::AgentSystem) => Ok(RunPath::AssembleBundle),
        Some(other) => bail!(
            "a {} artifact is not directly runnable; \
             lns run takes a plain OCI image or an AgentSystem bundle",
            other.as_str()
        ),
        None => match artifact_type {
            Some(unknown) => bail!(
                "unsupported artifact type {unknown}; \
                 lns run can launch a plain OCI image or an AgentSystem bundle"
            ),
            None => Ok(RunPath::SingleImage),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_artifact_type_and_config_type_is_treated_as_a_plain_image() {
        assert_eq!(dispatch(Some(""), Some("")).unwrap(), RunPath::SingleImage);
    }
}
