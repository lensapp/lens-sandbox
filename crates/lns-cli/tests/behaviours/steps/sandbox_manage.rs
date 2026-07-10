use cucumber::given;
use lns_ipc::{
    ArtifactInspection, ImageInfo, Response, RunConfig, RunDetails, RunStatus, RunSummary,
    SandboxView,
};

use crate::world::BehaviourWorld;

fn hexid(n: u32) -> String {
    format!("{n:08x}{}", "0".repeat(24))
}

#[given(regex = r#"^the reference "([^"]+)" resolves to a running sandbox$"#)]
fn reference_resolves_to_running(w: &mut BehaviourWorld, name: String) {
    w.sandbox.response = Some(Response::RunInspect {
        details: Box::new(RunDetails {
            summary: RunSummary {
                id: hexid(3),
                name,
                image: "some-image".into(),
                command: "some-command".into(),
                status: RunStatus::Running,
                started: "2026-01-01T00:00:00Z".into(),
            },
            config: RunConfig::default(),
        }),
    });
}

#[given(regex = r#"^the reference "([^"]+)" resolves to a cached sandbox$"#)]
fn reference_resolves_to_cached(w: &mut BehaviourWorld, reference: String) {
    w.sandbox.response = Some(Response::Error {
        message: format!("no active run with id {reference}"),
    });
    w.sandbox.inspect_image_response = Some(Response::ImageInspected {
        inspection: ArtifactInspection::Sandbox(SandboxView {
            reference,
            image: "docker.io/library/alpine@sha256:abc".into(),
            integrations: Vec::new(),
            policy_flags: Vec::new(),
        }),
    });
}

#[given(regex = r#"^the service reports one cached sandbox "([^"]+)"$"#)]
fn reports_one_cached_sandbox(w: &mut BehaviourWorld, reference: String) {
    w.sandbox.response = Some(Response::ImageList {
        images: vec![ImageInfo {
            reference,
            digest: format!("sha256:{}", "a".repeat(64)),
            size_bytes: 14 * 1024 * 1024,
            layers: 3,
            pulled: "2026-01-01T00:00:00Z".into(),
            in_use_by: None,
        }],
    });
}
