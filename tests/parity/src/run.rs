use crate::cases::{self, Ctx, Image};
use crate::config::{Backend, Config, Images};
use crate::fixtures::{Activity, Fixtures, Sizes, refuse_unsuitable_bind};
use crate::host;
use crate::result::{
    BackendRecord, BinaryRecord, CaseResult, ImageRecord, RunResult, SCHEMA_VERSION, sha256_file,
    unix_ms,
};
use crate::sample::Sampler;
use crate::service::PrivateService;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct RunPlan {
    pub backend: Backend,
    pub images: Images,
    pub bind: Ipv4Addr,
    pub base_port: u16,
    pub guest_subnet: String,
    pub selected: Vec<String>,
    pub budgets: BTreeMap<String, u64>,
    pub work_dir: PathBuf,
    pub out: PathBuf,
}

pub fn select_cases(requested: &[String]) -> Result<Vec<String>> {
    let known = cases::names();
    if requested.is_empty() {
        return Ok(known.into_iter().map(str::to_string).collect());
    }
    for name in requested {
        if !known.contains(&name.as_str()) {
            bail!(
                "no case named {name}; this phase runs: {}",
                known.join(", ")
            );
        }
    }
    // The preflight joins any selection that needs a raw stream, so a held stream is named once rather than stalling each case that meets it.
    let needs_preflight = requested
        .iter()
        .any(|name| cases::needs_fixture_stream(name));
    Ok(known
        .into_iter()
        .filter(|name| {
            requested.iter().any(|wanted| wanted == name)
                || (needs_preflight && *name == cases::PREFLIGHT)
        })
        .map(str::to_string)
        .collect())
}

pub fn check_budgets(budgets: &BTreeMap<String, u64>) -> Result<()> {
    let known = cases::names();
    for (name, seconds) in budgets {
        if !known.contains(&name.as_str()) {
            bail!(
                "no case named {name}; this phase runs: {}",
                known.join(", ")
            );
        }
        if *seconds == 0 {
            bail!("--budget {name}=0 leaves the case no time at all");
        }
    }
    Ok(())
}

/// What the preflight settled for the cases behind it: a held stream skips every case that needs one, rather than letting each stall to its own budget.
#[derive(Debug, Default)]
pub struct Cascade {
    reason: Option<String>,
}

impl Cascade {
    pub fn observe(&mut self, case: &cases::Case, result: &CaseResult) {
        if case.name == cases::PREFLIGHT && result.status == crate::result::Status::Fail {
            self.reason = Some(
                result
                    .error
                    .clone()
                    .unwrap_or_else(|| "the preflight failed".to_string()),
            );
        }
    }

    pub fn skip_for(&self, case: &cases::Case) -> Option<CaseResult> {
        let reason = self.reason.as_ref()?;
        case.needs_fixture_stream.then(|| {
            CaseResult::new(case.name).skip(format!("fixture unreachable from the guest: {reason}"))
        })
    }
}

pub fn budget_failure(elapsed: Duration, activity: Activity) -> String {
    format!(
        "budget exceeded after {}s, {} bytes seen by the fixture",
        elapsed.as_secs(),
        activity.bytes()
    )
}

/// A case past its budget is a fail whatever it thought of itself; a skip stands, because a missing command is not a stall.
pub fn enforce_budget(
    result: CaseResult,
    elapsed: Duration,
    budget: Duration,
    activity: Activity,
) -> CaseResult {
    if elapsed <= budget || result.status == crate::result::Status::Skip {
        return result;
    }
    let duration_ms = result.duration_ms;
    let mut failed = result.fail(budget_failure(elapsed, activity));
    failed.duration_ms = duration_ms;
    failed
}

pub fn merge_backend(
    config: Option<&Config>,
    name: &str,
    overrides: BackendOverrides,
) -> Result<Backend> {
    let mut backend = match config {
        Some(config) if !config.backends.is_empty() => config.backend(name)?,
        _ => Backend {
            name: name.to_string(),
            lns: overrides
                .lns
                .clone()
                .context("--lns names the lns binary this backend runs")?,
            lns_service: overrides
                .lns_service
                .clone()
                .context("--lns-service names the lns-service binary this backend runs")?,
            env: BTreeMap::new(),
            expect: BTreeMap::new(),
            expected_differences: Vec::new(),
        },
    };
    if let Some(path) = overrides.lns {
        backend.lns = path;
    }
    if let Some(path) = overrides.lns_service {
        backend.lns_service = path;
    }
    backend.env.extend(overrides.env);
    Ok(backend)
}

#[derive(Debug, Default, Clone)]
pub struct BackendOverrides {
    pub lns: Option<PathBuf>,
    pub lns_service: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
}

pub fn execute(plan: RunPlan) -> Result<RunResult> {
    refuse_unsuitable_bind(plan.bind)?;
    std::fs::create_dir_all(&plan.work_dir)
        .with_context(|| format!("create {}", plan.work_dir.display()))?;

    let fixtures = Fixtures::start(plan.bind, plan.base_port, Sizes::default())?;
    let started_unix_ms = unix_ms();
    let started = Instant::now();

    let service = PrivateService::start(
        &plan.backend.lns,
        &plan.backend.lns_service,
        &plan.backend.env,
        &plan.work_dir,
    )?;
    let mut service = service;
    let pid = service.pid();
    eprintln!(
        "[service]  pid {} home {} socket {}",
        pid.unwrap_or_default(),
        service.home().display(),
        service.socket().display()
    );
    let sampler = pid.map(|pid| Sampler::start(pid, started));

    let lns = service.lns().clone();
    let ctx = Ctx::prepare(
        &lns,
        &fixtures,
        &plan.images,
        &plan.work_dir.join("project"),
        plan.guest_subnet.clone(),
        plan.backend.expect.clone(),
        pid,
    )?;

    let images = record_images(&ctx, &plan.images);
    let mut result = RunResult {
        schema_version: SCHEMA_VERSION,
        harness_revision: harness_revision(),
        backend: BackendRecord {
            name: plan.backend.name.clone(),
            env: plan.backend.env.clone(),
            expectations: plan.backend.expect.clone(),
            expected_differences: plan.backend.expected_differences.clone(),
        },
        binaries: binaries(&plan.backend)?,
        lns_version: lns.version().unwrap_or_default(),
        service_pid: pid,
        images,
        host: host::facts(),
        started_unix_ms,
        finished_unix_ms: unix_ms(),
        cases: Vec::new(),
        samples: Vec::new(),
    };

    let mut cascade = Cascade::default();
    for case in cases::all() {
        if !plan.selected.iter().any(|wanted| *wanted == case.name) {
            continue;
        }
        let budget = budget_for(&case, &plan.budgets);
        let outcome = match cascade.skip_for(&case) {
            Some(skipped) => skipped,
            None => {
                eprintln!("[case]  {} (budget {}s)", case.name, budget.as_secs());
                run_case(&ctx, &case, budget)
            }
        };
        eprintln!("[{}]  {}", outcome.status.as_str(), case.name);
        cascade.observe(&case, &outcome);
        result.cases.push(outcome);
        // Written after every case, because a run that stalls or is stopped still has to be readable.
        result.finished_unix_ms = unix_ms();
        result.samples = sampler.as_ref().map(Sampler::take).unwrap_or_default();
        result.write(&plan.out)?;
        fixtures.write_report(&plan.work_dir.join("fixtures.json"))?;
    }

    result.samples = sampler.as_ref().map(Sampler::take).unwrap_or_default();
    if let Some(sampler) = &sampler {
        sampler.stop();
    }
    service.stop()?;
    fixtures.write_report(&plan.work_dir.join("fixtures.json"))?;
    fixtures.shutdown();

    result.finished_unix_ms = unix_ms();
    result.write(&plan.out)?;
    Ok(result)
}

fn budget_for(case: &cases::Case, budgets: &BTreeMap<String, u64>) -> Duration {
    budgets
        .get(case.name)
        .map(|seconds| Duration::from_secs(*seconds))
        .unwrap_or(case.budget)
}

fn run_case(ctx: &Ctx, case: &cases::Case, budget: Duration) -> CaseResult {
    ctx.begin_case(budget);
    let started = Instant::now();
    let mut result = (case.run)(ctx);
    let elapsed = started.elapsed();
    let activity = ctx.activity();
    ctx.cleanup_case();
    cases::record_activity(&mut result, activity);
    enforce_budget(result, elapsed, budget, activity)
}

fn binaries(backend: &Backend) -> Result<Vec<BinaryRecord>> {
    Ok(vec![
        BinaryRecord {
            role: "lns".to_string(),
            path: backend.lns.display().to_string(),
            sha256: sha256_file(&backend.lns)?,
        },
        BinaryRecord {
            role: "lns-service".to_string(),
            path: backend.lns_service.display().to_string(),
            sha256: sha256_file(&backend.lns_service)?,
        },
    ])
}

fn record_images(ctx: &Ctx, images: &Images) -> Vec<ImageRecord> {
    [
        (Image::Alpine, images.alpine.clone(), "parity-digest-alpine"),
        (Image::Curl, images.curl.clone(), "parity-digest-curl"),
    ]
    .into_iter()
    .map(|(image, reference, name)| ImageRecord {
        reference,
        digest: image_digest(ctx, image, name),
    })
    .collect()
}

fn image_digest(ctx: &Ctx, image: Image, name: &str) -> Option<String> {
    ctx.guest(image, name, "true").ok()?;
    let inspected = ctx
        .lns
        .run(&["sandbox", "inspect", name, "--format", "json"]);
    let _ = ctx.remove(name);
    let value: serde_json::Value = serde_json::from_str(&inspected.ok()?.stdout).ok()?;
    first_digest(&value)
}

fn first_digest(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => text
            .rsplit_once("sha256:")
            .map(|(_, digest)| format!("sha256:{digest}")),
        serde_json::Value::Array(items) => items.iter().find_map(first_digest),
        serde_json::Value::Object(fields) => fields.values().find_map(first_digest),
        _ => None,
    }
}

fn harness_revision() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn summarise(result: &RunResult, out: &Path) -> String {
    let counts = result.counts();
    let line = counts
        .iter()
        .map(|(status, count)| format!("{count} {status}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{}: {line}\nwritten to {}",
        result.backend.name,
        out.display()
    )
}

pub fn exit_code(result: &RunResult) -> i32 {
    let failed = result
        .cases
        .iter()
        .any(|case| case.status == crate::result::Status::Fail);
    i32::from(failed)
}

pub fn all_skipped(results: &[CaseResult]) -> bool {
    !results.is_empty()
        && results
            .iter()
            .all(|case| case.status == crate::result::Status::Skip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::{HostFacts, Status};

    fn result_with(cases: Vec<CaseResult>) -> RunResult {
        RunResult {
            schema_version: SCHEMA_VERSION,
            harness_revision: "abc".into(),
            backend: BackendRecord {
                name: "netstack".into(),
                env: BTreeMap::new(),
                expectations: BTreeMap::new(),
                expected_differences: vec![],
            },
            binaries: vec![],
            lns_version: "lns 0.25.0".into(),
            service_pid: None,
            images: vec![],
            host: HostFacts::default(),
            started_unix_ms: 0,
            finished_unix_ms: 0,
            cases,
            samples: vec![],
        }
    }

    #[test]
    fn no_selection_runs_every_case_in_the_order_the_phase_fixes() {
        assert_eq!(select_cases(&[]).unwrap(), cases::names());
    }

    #[test]
    fn a_selection_keeps_the_registry_order_whatever_order_it_was_asked_in() {
        let selected =
            select_cases(&["udp-echo".to_string(), "lease-and-resolver".to_string()]).unwrap();
        assert_eq!(selected, vec!["lease-and-resolver", "udp-echo"]);
    }

    #[test]
    fn a_case_this_phase_does_not_run_names_the_ones_it_does() {
        let err = select_cases(&["dns-fixture".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("dns-fixture"), "{err}");
        assert!(err.contains("upload-100m"), "{err}");
    }

    fn registered(name: &str) -> cases::Case {
        cases::all()
            .into_iter()
            .find(|case| case.name == name)
            .expect("the registry holds this case")
    }

    #[test]
    fn a_selection_that_needs_a_raw_stream_runs_the_preflight_that_proves_one() {
        let selected = select_cases(&["download-100m".to_string()]).unwrap();
        assert_eq!(selected, vec![cases::PREFLIGHT, "download-100m"]);

        let selected = select_cases(&["lease-and-resolver".to_string()]).unwrap();
        assert_eq!(selected, vec!["lease-and-resolver"]);
    }

    #[test]
    fn a_failed_preflight_skips_every_case_that_needs_the_fixture_rather_than_stalling_each() {
        let mut cascade = Cascade::default();
        let preflight = registered(cases::PREFLIGHT);
        cascade.observe(
            &preflight,
            &CaseResult::new(cases::PREFLIGHT).fail("the guest sent 16 bytes and read 0 back"),
        );

        let skipped = cascade.skip_for(&registered("upload-100m")).unwrap();
        assert_eq!(skipped.status, Status::Skip);
        assert_eq!(
            skipped.reason.as_deref(),
            Some("fixture unreachable from the guest: the guest sent 16 bytes and read 0 back")
        );
        assert!(
            cascade
                .skip_for(&registered("lease-and-resolver"))
                .is_none()
        );
        assert!(cascade.skip_for(&registered("loopback-witness")).is_none());
    }

    #[test]
    fn a_preflight_that_passed_lets_every_case_behind_it_run() {
        let mut cascade = Cascade::default();
        cascade.observe(
            &registered(cases::PREFLIGHT),
            &CaseResult::new(cases::PREFLIGHT).pass(),
        );
        assert!(cascade.skip_for(&registered("upload-100m")).is_none());
    }

    #[test]
    fn a_case_past_its_budget_fails_with_what_the_fixture_saw_while_it_ran() {
        let activity = Activity {
            connections: 1,
            bytes_in: 0,
            bytes_out: 0,
        };
        let over = enforce_budget(
            CaseResult::new("upload-100m").pass(),
            Duration::from_secs(121),
            Duration::from_secs(120),
            activity,
        );

        assert_eq!(over.status, Status::Fail);
        assert_eq!(
            over.error.as_deref(),
            Some("budget exceeded after 121s, 0 bytes seen by the fixture")
        );
    }

    #[test]
    fn a_case_inside_its_budget_and_a_skip_past_it_keep_their_own_verdict() {
        let activity = Activity::default();
        let inside = enforce_budget(
            CaseResult::new("upload-100m").pass(),
            Duration::from_secs(30),
            Duration::from_secs(120),
            activity,
        );
        assert_eq!(inside.status, Status::Pass);

        let skipped = enforce_budget(
            CaseResult::new("guest-half-close").skip("the image's nc has neither -N nor -q"),
            Duration::from_secs(200),
            Duration::from_secs(120),
            activity,
        );
        assert_eq!(skipped.status, Status::Skip);
    }

    #[test]
    fn a_budget_override_replaces_the_one_the_registry_declares() {
        let case = registered("download-100m");
        assert_eq!(
            budget_for(&case, &BTreeMap::new()),
            Duration::from_secs(120)
        );
        assert_eq!(
            budget_for(&case, &BTreeMap::from([("download-100m".to_string(), 300)])),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn a_budget_for_a_case_this_phase_does_not_run_is_refused() {
        let err = check_budgets(&BTreeMap::from([("dns-fixture".to_string(), 60)]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("dns-fixture"), "{err}");

        let err = check_budgets(&BTreeMap::from([("upload-100m".to_string(), 0)]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("no time at all"), "{err}");

        check_budgets(&BTreeMap::from([("upload-100m".to_string(), 240)])).unwrap();
    }

    #[test]
    fn a_backend_may_be_given_whole_on_the_command_line() {
        let backend = merge_backend(
            None,
            "netstack",
            BackendOverrides {
                lns: Some(PathBuf::from("bin/lns")),
                lns_service: Some(PathBuf::from("bin/lns-service")),
                env: BTreeMap::from([("LNS_NETDEV".to_string(), "netstack".to_string())]),
            },
        )
        .unwrap();

        assert_eq!(backend.name, "netstack");
        assert_eq!(backend.lns, PathBuf::from("bin/lns"));
        assert_eq!(backend.env["LNS_NETDEV"], "netstack");
    }

    #[test]
    fn a_backend_with_no_binaries_and_no_config_is_refused() {
        let err = merge_backend(None, "netstack", BackendOverrides::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("--lns"), "{err}");
    }

    #[test]
    fn a_flag_overrides_the_configured_backend_it_names() {
        let config = Config {
            backends: vec![Backend {
                name: "netstack".into(),
                lns: PathBuf::from("bin/lns"),
                lns_service: PathBuf::from("bin/lns-service"),
                env: BTreeMap::from([("LNS_NETDEV".to_string(), "netstack".to_string())]),
                expect: BTreeMap::new(),
                expected_differences: vec![],
            }],
            ..Config::default()
        };
        let backend = merge_backend(
            Some(&config),
            "netstack",
            BackendOverrides {
                lns: Some(PathBuf::from("/tmp/other-lns")),
                lns_service: None,
                env: BTreeMap::from([("LNS_GUEST_SUBNET".to_string(), "192.168.66.0/24".into())]),
            },
        )
        .unwrap();

        assert_eq!(backend.lns, PathBuf::from("/tmp/other-lns"));
        assert_eq!(backend.lns_service, PathBuf::from("bin/lns-service"));
        assert_eq!(backend.env["LNS_NETDEV"], "netstack");
        assert_eq!(backend.env["LNS_GUEST_SUBNET"], "192.168.66.0/24");
    }

    #[test]
    fn the_image_digest_is_read_out_of_whatever_field_inspect_carries_it_in() {
        let value = serde_json::json!({
            "name": "parity-digest-alpine",
            "image": {
                "reference": "docker.io/library/alpine:3.20",
                "resolved": "docker.io/library/alpine@sha256:abcdef"
            }
        });
        assert_eq!(first_digest(&value), Some("sha256:abcdef".to_string()));
        assert_eq!(first_digest(&serde_json::json!({"name": "x"})), None);
        assert_eq!(
            first_digest(&serde_json::json!(["a", "b@sha256:11"])),
            Some("sha256:11".to_string())
        );
    }

    #[test]
    fn a_run_with_one_failed_case_exits_non_zero() {
        assert_eq!(
            exit_code(&result_with(vec![CaseResult::new("a").pass()])),
            0
        );
        assert_eq!(
            exit_code(&result_with(vec![
                CaseResult::new("a").pass(),
                CaseResult::new("b").fail("no"),
            ])),
            1
        );
        assert_eq!(
            exit_code(&result_with(vec![CaseResult::new("a").skip("no host")])),
            0
        );
    }

    #[test]
    fn a_run_that_skipped_everything_is_told_apart_from_one_that_proved_something() {
        assert!(all_skipped(&[CaseResult::new("a").skip("no host")]));
        assert!(!all_skipped(&[
            CaseResult::new("a").skip("no host"),
            CaseResult::new("b").pass(),
        ]));
        assert!(!all_skipped(&[]));
    }

    #[test]
    fn the_summary_counts_the_outcomes_and_names_the_file_it_wrote() {
        let mut case = CaseResult::new("upload-100m");
        case.status = Status::Pass;
        let text = summarise(&result_with(vec![case]), Path::new("/tmp/netstack.json"));

        assert!(text.contains("netstack: 1 pass"), "{text}");
        assert!(text.contains("/tmp/netstack.json"), "{text}");
    }

    #[test]
    fn the_harness_records_the_revision_it_ran_from() {
        let revision = harness_revision();
        assert!(revision == "unknown" || revision.len() == 40, "{revision}");
    }

    #[test]
    fn a_bind_address_the_guest_cannot_reach_stops_the_run_before_it_starts_a_service() {
        let plan = RunPlan {
            backend: Backend {
                name: "netstack".into(),
                lns: PathBuf::from("bin/lns"),
                lns_service: PathBuf::from("bin/lns-service"),
                env: BTreeMap::new(),
                expect: BTreeMap::new(),
                expected_differences: vec![],
            },
            images: Images::default(),
            bind: Ipv4Addr::LOCALHOST,
            base_port: 0,
            guest_subnet: "192.168.127".into(),
            selected: vec![],
            budgets: BTreeMap::new(),
            work_dir: std::env::temp_dir().join("parity-never-created"),
            out: std::env::temp_dir().join("parity-never-written.json"),
        };
        let err = execute(plan).unwrap_err().to_string();

        assert!(err.contains("loopback"), "{err}");
        assert!(!std::env::temp_dir().join("parity-never-created").exists());
    }
}
