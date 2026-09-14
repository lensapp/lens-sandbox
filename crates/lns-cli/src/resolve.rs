//! The preflight every verb that builds a Containerfile takes before it hands the service a plan: a build step is the document's own run with one instruction in its place, so the document reaches the plan as merged as a run of it would be.

use anyhow::{Result, bail};
use lns_ipc::{Request, Response};

use crate::service::client::SandboxService;

/// What a build boots its steps from: the merged document, and the policy its other sources authored.
#[derive(Debug)]
pub struct Resolved {
    pub definition: String,
    pub authored_egress: Option<String>,
    pub packed_filesets: Vec<lns_ipc::PackedFilesetSource>,
}

/// Merge the document's mixins through the service, or hand back what was read when it declares none.
pub async fn before_building(
    svc: &impl SandboxService,
    definition: &str,
    definition_dir: &str,
) -> Result<Resolved> {
    if !declares_a_mixin(definition.as_bytes()) {
        return Ok(Resolved {
            definition: definition.to_string(),
            authored_egress: None,
            packed_filesets: Vec::new(),
        });
    }
    match svc
        .one_shot(Request::ResolveDefinition {
            definition: definition.to_string(),
            project_dir: definition_dir.to_string(),
            mixins: Vec::new(),
        })
        .await?
    {
        Response::DefinitionResolved {
            definition,
            authored_egress,
            packed_filesets,
            ..
        } => Ok(Resolved {
            definition,
            authored_egress: Some(authored_egress),
            packed_filesets,
        }),
        Response::Error { message } => Err(crate::service::reply::failure(&message)),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

pub fn declares_a_mixin(definition: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(definition)
        .ok()
        .and_then(|value| value["spec"]["mixins"].as_array().map(|m| !m.is_empty()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_service::CannedService;

    const DOCUMENT: &str = r#"{"spec":{"image":"./image","mixins":["./egress.yaml"]}}"#;

    #[test]
    fn a_document_that_is_not_json_declares_no_mixin() {
        assert!(!declares_a_mixin(b"not json"));
    }

    #[test]
    fn an_empty_mixin_list_is_no_mixin() {
        assert!(!declares_a_mixin(br#"{"spec":{"mixins":[]}}"#));
    }

    #[tokio::test]
    async fn a_document_declaring_no_mixin_is_built_from_what_was_read() {
        let svc = CannedService::new(Response::Error {
            message: "no request may reach the service".into(),
        });
        let resolved = before_building(&svc, r#"{"spec":{"image":"./image"}}"#, "/work")
            .await
            .expect("a document declaring no mixin needs no round trip");
        assert_eq!(resolved.definition, r#"{"spec":{"image":"./image"}}"#);
        assert!(resolved.authored_egress.is_none());
        assert!(svc.requests.lock().expect("the log reads").is_empty());
    }

    #[tokio::test]
    async fn a_document_declaring_a_mixin_is_built_from_what_the_service_merged() {
        let svc = CannedService::new(Response::DefinitionResolved {
            definition: r#"{"spec":{"image":"./image"}}"#.into(),
            mixins: vec!["./egress.yaml".into()],
            pinned_mixins: Vec::new(),
            contributions: Vec::new(),
            authored_egress: r#"{"http":[]}"#.into(),
            packed_filesets: Vec::new(),
        });
        let resolved = before_building(&svc, DOCUMENT, "/work")
            .await
            .expect("the service merged the document");
        assert_eq!(resolved.definition, r#"{"spec":{"image":"./image"}}"#);
        assert_eq!(resolved.authored_egress.as_deref(), Some(r#"{"http":[]}"#));
    }

    #[tokio::test]
    async fn what_the_service_refuses_the_resolution_with_is_what_the_user_reads() {
        let svc = CannedService::new(Response::Error {
            message: "./egress.yaml: no such file".into(),
        });
        let err = before_building(&svc, DOCUMENT, "/work")
            .await
            .expect_err("a refused resolution stops the build");
        assert!(
            err.to_string().contains("./egress.yaml: no such file"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn an_answer_to_another_question_is_named_rather_than_taken_for_a_resolution() {
        let svc = CannedService::new(Response::Pong);
        let err = before_building(&svc, DOCUMENT, "/work")
            .await
            .expect_err("only a resolution resolves");
        assert!(err.to_string().contains("unexpected response"), "{err}");
    }
}
