use cucumber::{given, then};
use lns_ipc::{
    ArtifactInspection, CachedKind, ImageInfo, Request, Response, RunConfig, RunDetails, RunStatus,
    RunSummary, SandboxView,
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
    w.sandbox.cached_references = vec![reference.clone()];
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
            scripts: Vec::new(),
            policy_flags: Vec::new(),
            cpus: None,
            mem_mib: None,
            disk_bytes: None,
        })),
    });
}

#[given(
    regex = r#"^the sandbox "([^"]+)" is cached and no other sandbox shares its base-image layers$"#
)]
fn cached_sandbox_sole_owner(w: &mut BehaviourWorld, reference: String) {
    w.sandbox.cached_references = vec![reference.clone()];
    w.sandbox.response = Some(Response::Error {
        message: format!("no active run with id {reference}"),
    });
    w.sandbox.inspect_image_response = Some(Response::ImageInspected {
        inspection: ArtifactInspection::Image(lns_ipc::ImageView {
            reference: reference.clone(),
            digest: format!("sha256:{}", "a".repeat(64)),
        }),
    });
    w.sandbox.remove_image_response = Some(Response::ImageRemoved {
        reference,
        reclaimed_bytes: 3 * 1024 * 1024,
    });
}

#[given("two cached sandboxes and one running sandbox")]
fn two_cached_one_running(w: &mut BehaviourWorld) {
    w.sandbox.prunable_references = vec!["hermes:1.0".into(), "scribe:1.0".into()];
    w.sandbox.response = Some(Response::ImagesPruned {
        removed: vec!["hermes:1.0".into(), "scribe:1.0".into()],
        reclaimed_bytes: 64 * 1024 * 1024,
    });
}

#[given("every cached artifact is held by a running sandbox")]
fn every_artifact_held(w: &mut BehaviourWorld) {
    w.sandbox.response = Some(Response::ImagesPruned {
        removed: Vec::new(),
        reclaimed_bytes: 128 * 1024 * 1024,
    });
}

#[given(regex = r#"^a cached sandbox that names a volume "([^"]+)"$"#)]
fn cached_sandbox_names_a_volume(w: &mut BehaviourWorld, _volume: String) {
    w.sandbox.prunable_references = vec!["hermes:1.0".into()];
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

fn cached(reference: &str, kind: CachedKind, holder: Option<String>) -> ImageInfo {
    ImageInfo {
        reference: reference.to_string(),
        kind,
        digest: format!("sha256:{}", "a".repeat(64)),
        size_bytes: 14 * 1024 * 1024,
        layers: 3,
        pulled: "2026-01-01T00:00:00Z".into(),
        in_use_by: holder,
    }
}

#[given(regex = r#"^the service reports one cached sandbox "([^"]+)"$"#)]
fn reports_one_cached_sandbox(w: &mut BehaviourWorld, reference: String) {
    w.sandbox.response = Some(Response::ImageList {
        images: vec![cached(&reference, CachedKind::Sandbox, None)],
    });
}

#[given(regex = r#"^the service reports one cached sandbox "([^"]+)" held by run (\d+)$"#)]
fn reports_one_held_cached_sandbox(w: &mut BehaviourWorld, reference: String, run: u32) {
    w.sandbox.response = Some(Response::ImageList {
        images: vec![cached(&reference, CachedKind::Sandbox, Some(hexid(run)))],
    });
}

#[given(regex = r#"^the service reports a cached sandbox "([^"]+)" and a cached image "([^"]+)"$"#)]
fn reports_a_sandbox_and_an_image(w: &mut BehaviourWorld, sandbox: String, image: String) {
    w.sandbox.response = Some(Response::ImageList {
        images: vec![
            cached(&sandbox, CachedKind::Sandbox, None),
            cached(&image, CachedKind::Image, None),
        ],
    });
}

#[given(regex = r#"^the service reports a cached sandbox "([^"]+)" and a cached mixin "([^"]+)"$"#)]
fn reports_a_sandbox_and_a_mixin(w: &mut BehaviourWorld, sandbox: String, mixin: String) {
    w.sandbox.response = Some(Response::ImageList {
        images: vec![
            cached(&sandbox, CachedKind::Sandbox, None),
            cached(&mixin, CachedKind::Mixin, None),
        ],
    });
}

#[given(regex = r#"^"([^"]+)" names both a sandbox and a cached artifact$"#)]
fn names_both(w: &mut BehaviourWorld, name: String) {
    reference_resolves_to_running(w, name.clone());
    w.sandbox.cached_references = vec![name.clone()];
    w.sandbox.inspect_image_response = Some(Response::ImageInspected {
        inspection: ArtifactInspection::Image(lns_ipc::ImageView {
            reference: name,
            digest: format!("sha256:{}", "a".repeat(64)),
        }),
    });
}

#[given(regex = r#"^"([^"]+)" names neither a sandbox nor a cached artifact$"#)]
fn names_neither(w: &mut BehaviourWorld, name: String) {
    w.sandbox.response = Some(Response::Error {
        message: format!("no active run with id {name}"),
    });
    w.sandbox.inspect_image_response = Some(Response::Error {
        message: format!("no such image: {name}"),
    });
}

#[given(regex = r#"^"([^"]+)" names a sandbox, and the cache holds nothing$"#)]
fn names_a_sandbox_only(w: &mut BehaviourWorld, name: String) {
    reference_resolves_to_running(w, name);
    w.sandbox.cached_references = Vec::new();
}

#[given(regex = r#"^the service refuses to remove the running sandbox "([^"]+)"$"#)]
fn service_refuses_a_running_removal(w: &mut BehaviourWorld, name: String) {
    reference_resolves_to_running(w, name.clone());
    w.sandbox.remove_run_response = Some(Response::Error {
        message: format!("{name} is running; stop it first, or pass -f to stop and remove it"),
    });
}

#[then(regex = r#"^the service received a RemoveRun for "([^"]+)"$"#)]
fn service_received_remove(w: &mut BehaviourWorld, run: String) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests
        .iter()
        .any(|r| matches!(r, Request::RemoveRun { run: asked, .. } if *asked == run))
    {
        Ok(())
    } else {
        Err(format!(
            "expected a RemoveRun for {run:?} among {requests:?}"
        ))
    }
}

#[then(regex = r#"^the service received a forced RemoveRun for "([^"]+)"$"#)]
fn service_received_forced_remove(w: &mut BehaviourWorld, run: String) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests
        .iter()
        .any(|r| matches!(r, Request::RemoveRun { run: asked, force: true } if *asked == run))
    {
        Ok(())
    } else {
        Err(format!(
            "expected a forced RemoveRun for {run:?} among {requests:?}"
        ))
    }
}

#[given("the service reports one running sandbox and one that stopped")]
fn one_running_one_stopped(w: &mut BehaviourWorld) {
    w.sandbox.response = Some(Response::RunList {
        runs: vec![
            RunSummary {
                id: hexid(3),
                name: "reviewer".into(),
                image: "some-image".into(),
                command: "some-command".into(),
                status: RunStatus::Running,
                started: "2026-01-01T00:00:00Z".into(),
            },
            RunSummary {
                id: hexid(4),
                name: "scribe".into(),
                image: "some-image".into(),
                command: "some-command".into(),
                status: RunStatus::Exited { code: 0 },
                started: "2026-01-01T00:00:00Z".into(),
            },
        ],
    });
    w.sandbox.stats_response = Some(Response::RunStats {
        stats: lns_ipc::RunStatsInfo {
            cpu_permille: 125,
            mem_used_bytes: 92_274_688,
            mem_total_bytes: 536_870_912,
        },
    });
}

#[then("the service asked for stats exactly once")]
fn stats_asked_once(w: &mut BehaviourWorld) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    let probes = requests
        .iter()
        .filter(|r| matches!(r, Request::RunStats { .. }))
        .count();
    if probes == 1 {
        Ok(())
    } else {
        Err(format!(
            "a stopped sandbox has no guest to sample, so only the running one may be probed; got {probes} probes"
        ))
    }
}

#[given(regex = r#"^the service will sweep the stopped sandboxes "([^"]+)" and "([^"]+)"$"#)]
fn service_will_sweep(w: &mut BehaviourWorld, first: String, second: String) {
    w.sandbox.list_runs_response = Some(Response::RunList {
        runs: vec![stopped_run(5, &first), stopped_run(6, &second)],
    });
    w.sandbox.response = Some(Response::RunsPruned {
        removed: vec![first, second],
    });
}

#[given("the service reports one running sandbox and none stopped")]
fn one_running_none_stopped(w: &mut BehaviourWorld) {
    w.sandbox.list_runs_response = Some(Response::RunList {
        runs: vec![RunSummary {
            id: hexid(3),
            name: "reviewer".into(),
            image: "some-image".into(),
            command: "some-command".into(),
            status: RunStatus::Running,
            started: "2026-01-01T00:00:00Z".into(),
        }],
    });
}

fn stopped_run(n: u32, name: &str) -> RunSummary {
    RunSummary {
        id: hexid(n),
        name: name.into(),
        image: "some-image".into(),
        command: "some-command".into(),
        status: RunStatus::Exited { code: 0 },
        started: "2026-01-01T00:00:00Z".into(),
    }
}

#[given("the service reports no stopped sandboxes to sweep")]
fn service_sweeps_nothing(w: &mut BehaviourWorld) {
    w.sandbox.response = Some(Response::RunsPruned {
        removed: Vec::new(),
    });
}
