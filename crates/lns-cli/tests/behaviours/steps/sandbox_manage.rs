use cucumber::{given, then};
use lns_ipc::{
    ArtifactInspection, ImageInfo, Request, Response, RunConfig, RunDetails, RunStatus, RunSummary,
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

#[given(regex = r#"^the daemon refuses to remove the running run "([^"]+)"$"#)]
fn daemon_refuses_running_removal(w: &mut BehaviourWorld, name: String) {
    w.sandbox.response = Some(Response::Error {
        message: format!(
            "run {name} is still running; stop it first with `lns stop {name}` or force with `lns rm -f {name}`"
        ),
    });
}

#[given("a cached sandbox not used by any run")]
fn a_cached_sandbox_not_used(w: &mut BehaviourWorld) {
    w.sandbox.remove_image_response = Some(Response::ImageRemoved {
        reference: "registry.example.test/idle-sandbox:1".into(),
        reclaimed_bytes: 1024,
    });
}

#[given(regex = r#"^the reference "([^"]+)" resolves to a cached sandbox$"#)]
fn reference_resolves_to_cached(w: &mut BehaviourWorld, reference: String) {
    w.sandbox.response = Some(Response::Error {
        message: format!("no active run with id {reference}"),
    });
    w.sandbox.inspect_image_response = Some(Response::ImageInspected {
        inspection: ArtifactInspection::Sandbox(Box::new(SandboxView {
            mixins: Vec::new(),
            pinned_mixins: Vec::new(),
            contributions: Vec::new(),
            reference,
            digest: format!("sha256:{}", "a".repeat(64)),
            image: "docker.io/library/alpine@sha256:abc".into(),
            workdir: None,
            user: None,
            mounts: Vec::new(),
            ports: Vec::new(),
            filesets: Vec::new(),
            connectors: Vec::new(),
            env: Vec::new(),
            credentials: Vec::new(),
            tools: Vec::new(),
            policy_flags: Vec::new(),
            cpus: None,
            mem_mib: None,
        })),
    });
}

#[given(
    regex = r#"^the sandbox "([^"]+)" is cached and no other sandbox shares its base-image layers$"#
)]
fn cached_sandbox_sole_owner(w: &mut BehaviourWorld, reference: String) {
    w.sandbox.response = Some(Response::Error {
        message: format!("no active run with id {reference}"),
    });
    w.sandbox.remove_image_response = Some(Response::ImageRemoved {
        reference,
        reclaimed_bytes: 3 * 1024 * 1024,
    });
}

#[given("two cached sandboxes and one running sandbox")]
fn two_cached_one_running(w: &mut BehaviourWorld) {
    w.sandbox.response = Some(Response::ImagesPruned {
        removed: vec!["hermes:1.0".into(), "scribe:1.0".into()],
        reclaimed_bytes: 64 * 1024 * 1024,
    });
}

#[given(regex = r#"^a cached sandbox that names a volume "([^"]+)"$"#)]
fn cached_sandbox_names_a_volume(w: &mut BehaviourWorld, _volume: String) {
    w.sandbox.response = Some(Response::ImagesPruned {
        removed: vec!["hermes:1.0".into()],
        reclaimed_bytes: 32 * 1024 * 1024,
    });
}

#[then("the output reports reclaimed base-image layers")]
fn reports_reclaimed_layers(w: &mut BehaviourWorld) -> Result<(), String> {
    let out = &w.result.as_ref().ok_or("no CLI run captured")?.output;
    if out.contains("base-image layers") {
        Ok(())
    } else {
        Err(format!("expected a reclaimed-layers line, got: {out:?}"))
    }
}

#[then("the output reports reclaimed bytes")]
fn reports_reclaimed_bytes(w: &mut BehaviourWorld) -> Result<(), String> {
    let out = &w.result.as_ref().ok_or("no CLI run captured")?.output;
    if out.contains("reclaimed") {
        Ok(())
    } else {
        Err(format!("expected a reclaimed-bytes line, got: {out:?}"))
    }
}

#[then("the running sandbox and its layers are kept")]
fn running_sandbox_kept(w: &mut BehaviourWorld) -> Result<(), String> {
    let out = &w.result.as_ref().ok_or("no CLI run captured")?.output;
    let removed = out.lines().filter(|l| l.starts_with("removed ")).count();
    if removed == 2 {
        Ok(())
    } else {
        Err(format!(
            "prune must remove only the two cached sandboxes, got: {out:?}"
        ))
    }
}

#[then("the service received a PruneImages request")]
fn service_received_prune_images(w: &mut BehaviourWorld) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests.contains(&Request::PruneImages) {
        Ok(())
    } else {
        Err(format!("expected PruneImages among {requests:?}"))
    }
}

#[then(regex = r#"^the named volume "([^"]+)" still exists$"#)]
fn named_volume_still_exists(w: &mut BehaviourWorld, volume: String) -> Result<(), String> {
    let out = &w.result.as_ref().ok_or("no CLI run captured")?.output;
    if out.contains(&volume) {
        Err(format!(
            "a named volume must never be swept by sandbox prune: {out:?}"
        ))
    } else {
        Ok(())
    }
}

#[given(regex = r"^the service reports no cached sandboxes$")]
fn reports_no_cached_sandboxes(w: &mut BehaviourWorld) {
    w.sandbox.response = Some(Response::ImageList { images: Vec::new() });
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

#[cucumber::when(regex = r#"^I run "lns rmi" with its reference$"#)]
async fn i_run_lns_rmi(w: &mut BehaviourWorld) {
    crate::steps::sandbox_cli::drive_sandbox_command(w, "rmi registry.example.test/idle-sandbox:1")
        .await;
}

#[then(regex = r#"^it is removed exactly as "lns rm <ref>" did before the rename$"#)]
fn removed_as_rm_did(w: &mut BehaviourWorld) -> Result<(), String> {
    let run = w.result.as_ref().ok_or("no invocation ran")?;
    if run.exit_code == 0
        && run
            .output
            .contains("removed registry.example.test/idle-sandbox:1")
    {
        Ok(())
    } else {
        Err(format!(
            "the rename moved the verb, not the behaviour; got exit {} output {:?}",
            run.exit_code, run.output
        ))
    }
}
