use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cucumber::{given, then, when};
use lns_cli::artifact::author::{DirEntry, Fs, map_dir_entries};
use lns_cli::cli::RunArgs;
use lns_cli::command::parse_args;
use lns_cli::run::progress::ProgressRenderer;
use lns_cli::run::target::{RunTarget, resolve};
use lns_cli::service::pre_phase_step;
use lns_ipc::{Response, encode_frame};

use crate::runner::CliRun;
use crate::world::BehaviourWorld;

struct StepFs {
    files: RefCell<HashMap<PathBuf, String>>,
}

impl Fs for StepFs {
    fn is_dir(&self, path: &Path) -> bool {
        self.files
            .borrow()
            .keys()
            .any(|held| held.ancestors().skip(1).any(|dir| dir == path))
    }

    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        self.files
            .borrow()
            .get(path)
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))
    }
    fn write(&self, path: &Path, contents: &str) -> std::io::Result<()> {
        self.files
            .borrow_mut()
            .insert(path.to_path_buf(), contents.to_string());
        Ok(())
    }
    fn exists(&self, path: &Path) -> bool {
        self.files.borrow().contains_key(path)
    }
    fn is_symlink(&self, _path: &Path) -> bool {
        false
    }
    fn read_limited(&self, path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        let mut bytes = self.read_to_string(path)?.into_bytes();
        bytes.truncate(max_bytes.saturating_add(1) as usize);
        Ok(bytes)
    }
    fn dir_entries(&self, dir: &Path) -> std::io::Result<Vec<DirEntry>> {
        map_dir_entries(self.files.borrow().keys(), dir)
    }
}

#[given(regex = r#"^the registry serves the sandbox "([^"]+)"$"#)]
fn registry_serves_sandbox(w: &mut BehaviourWorld, reference: String) {
    let digest = format!("sha256:{}", "a".repeat(64));
    w.sandbox.response = Some(lns_ipc::Response::ImagePulled {
        image: lns_ipc::ImageInfo {
            kind: lns_ipc::CachedKind::Sandbox,
            reference: reference.clone(),
            digest: digest.clone(),
            size_bytes: 3 * 1024 * 1024,
            layers: 1,
            pulled: "2026-01-01T00:00:00Z".into(),
            in_use_by: None,
        },
        warnings: Vec::new(),
    });
    w.sandbox.inspect_image_response = Some(Response::ImageInspected {
        inspection: lns_ipc::ArtifactInspection::Sandbox(Box::new(lns_ipc::SandboxView {
            mixins: Vec::new(),
            pinned_mixins: Vec::new(),
            contributions: Vec::new(),
            reference,
            digest,
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

#[given(regex = r#"^the registry serves the mixin "([^"]+)"$"#)]
fn registry_serves_mixin(w: &mut BehaviourWorld, reference: String) {
    let digest = format!("sha256:{}", "b".repeat(64));
    w.sandbox.response = Some(lns_ipc::Response::MixinPulled {
        reference: reference.clone(),
        digest: digest.clone(),
        cached_mixins: 2,
    });
    w.sandbox.inspect_image_response = Some(Response::ImageInspected {
        inspection: lns_ipc::ArtifactInspection::Mixin(Box::new(lns_ipc::MixinView {
            reference,
            digest,
            mixins: Vec::new(),
            mounts: Vec::new(),
            ports: Vec::new(),
            filesets: Vec::new(),
            env: Vec::new(),
            credentials: Vec::new(),
            tools: vec!["some-tool@1.2.3".into()],
            scripts: Vec::new(),
            policy_flags: Vec::new(),
        })),
    });
}

#[given(regex = r#"^the registry serves a mixin with no digest at "([^"]+)"$"#)]
fn registry_serves_a_digestless_mixin(w: &mut BehaviourWorld, reference: String) {
    w.sandbox.inspect_image_response = Some(Response::ImageInspected {
        inspection: lns_ipc::ArtifactInspection::Mixin(Box::new(lns_ipc::MixinView {
            reference,
            digest: String::new(),
            mixins: Vec::new(),
            mounts: Vec::new(),
            ports: Vec::new(),
            filesets: Vec::new(),
            env: Vec::new(),
            credentials: Vec::new(),
            tools: Vec::new(),
            scripts: Vec::new(),
            policy_flags: Vec::new(),
        })),
    });
}

#[given(regex = r#"^the reference "([^"]+)" is a plain OCI image, not a sandbox$"#)]
fn reference_is_plain_image(w: &mut BehaviourWorld, reference: String) {
    w.sandbox_run.refusal = Some(format!(
        "{reference} is not a sandbox; run `lns init` to author an lns.yaml, \
         or pass a published sandbox reference"
    ));
}

#[when(regex = r#"^the user runs "lns run(.*)"$"#)]
fn user_runs_lns_run(w: &mut BehaviourWorld, rest: String) {
    let mut argv = vec!["lns".to_string(), "run".to_string()];
    argv.extend(rest.split_whitespace().map(str::to_string));
    drive_run(w, &argv);
}

#[when(regex = r#"^the user runs "lns sandbox run(.*)"$"#)]
fn user_runs_lns_sandbox_run(w: &mut BehaviourWorld, rest: String) {
    let mut argv = vec!["lns".to_string(), "sandbox".to_string(), "run".to_string()];
    argv.extend(rest.split_whitespace().map(str::to_string));
    drive_run(w, &argv);
}

fn parse_run_args(argv: &[String]) -> Result<RunArgs, clap::Error> {
    if argv[1] == "sandbox" {
        let args: lns_cli::sandbox::SandboxArgs = parse_args(argv)?;
        match args.command {
            lns_cli::sandbox::SandboxCommand::Run(run) => Ok(*run),
            _ => unreachable!("a `lns sandbox run` argv always decodes to the run variant"),
        }
    } else {
        parse_args(argv)
    }
}

fn drive_run(w: &mut BehaviourWorld, argv: &[String]) {
    let args = match parse_run_args(argv) {
        Ok(args) => args,
        Err(e) => {
            w.result = Some(CliRun {
                exit_code: e.exit_code(),
                output: e.to_string(),
            });
            return;
        }
    };
    let cwd = Path::new("/work");
    let fs = StepFs {
        files: RefCell::new(w.author_files.clone()),
    };
    match resolve(args.image.as_deref(), args.file.as_deref(), &fs, cwd) {
        Err(e) => {
            w.result = Some(CliRun {
                exit_code: 1,
                output: format!("{e:#}"),
            });
        }
        Ok(target) => run_resolved(w, &target, cwd),
    }
}

fn run_resolved(w: &mut BehaviourWorld, target: &RunTarget, cwd: &Path) {
    if let Some(refusal) = w.sandbox_run.refusal.clone() {
        w.result = Some(surface_service_refusal(&refusal));
        return;
    }
    w.sandbox_run.request_image = Some(target.image());
    w.sandbox_run.verify_sandbox = Some(target.verify_sandbox());
    w.sandbox_run.definition = target.definition_json();
    w.sandbox_run.project_dir = target.project_dir().map(Path::to_path_buf);
    w.sandbox_run.decisions = Some(lns_cli::run::summary::policy_path(
        target.project_dir().unwrap_or(cwd),
    ));
    w.result = Some(CliRun {
        exit_code: 0,
        output: String::new(),
    });
}

/// Drive the CLI's real pre-phase surfacing of a daemon `Error` frame, exactly as `lns run` renders a service refusal.
fn surface_service_refusal(message: &str) -> CliRun {
    let frame = encode_frame(&Response::Error {
        message: message.to_string(),
    })
    .expect("encode error frame");
    let mut buf = Vec::new();
    let err = pre_phase_step(&frame, &mut buf, &mut ProgressRenderer::new(false), false)
        .expect_err("a daemon Error frame must surface as an error");
    CliRun {
        exit_code: 1,
        output: format!("{err:#}"),
    }
}

#[given("a valid lns.yaml declaring egress, connectors, and resources")]
fn lns_yaml_with_policy_connectors_resources(w: &mut BehaviourWorld) {
    w.author_files.insert(
        PathBuf::from("/work/lns.yaml"),
        "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n  egress:\n    http: []\n  connectors:\n    - some-provider\n  resources:\n    cpu: 2\n    memory: 1Gi\n"
            .to_string(),
    );
}

#[then("the service request carries the definition's egress, connectors, and resources")]
fn request_carries_definition(w: &mut BehaviourWorld) -> Result<(), String> {
    let json = w
        .sandbox_run
        .definition
        .as_deref()
        .ok_or("the run request carried no definition")?;
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("definition was not json: {e}"))?;
    let spec = &value["spec"];
    if spec["egress"].is_null() || spec["connectors"].is_null() || spec["resources"].is_null() {
        return Err(format!(
            "expected egress, connectors, and resources in the definition, got: {spec}"
        ));
    }
    Ok(())
}

#[then("the service received a request to run a sandbox")]
fn service_received_run_request(w: &mut BehaviourWorld) -> Result<(), String> {
    match &w.sandbox_run.request_image {
        Some(image) if !image.is_empty() => Ok(()),
        _ => Err("no run request was built for the service".to_string()),
    }
}
