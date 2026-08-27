use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cucumber::{then, when};
use lns_artifact::walk::{DirEntry, map_dir_entries};
use lns_cli::artifact::author::Fs;
use lns_cli::cli::RunArgs;
use lns_cli::command::parse_args;
use lns_cli::run::summary::{fileset_source_display, print_run_summary};
use lns_cli::run::target::{RunTarget, resolve};

use crate::runner::CliRun;
use crate::world::{BehaviourWorld, ScriptedTerminal};

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
}

impl lns_artifact::walk::SnapshotFs for StepFs {
    fn read_limited(&self, path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        let mut bytes = self.read_to_string(path)?.into_bytes();
        bytes.truncate(max_bytes.saturating_add(1) as usize);
        Ok(bytes)
    }
    fn dir_entries(&self, dir: &Path) -> std::io::Result<Vec<DirEntry>> {
        map_dir_entries(self.files.borrow().keys(), dir)
    }
}

fn resolve_local(world: &BehaviourWorld) -> anyhow::Result<RunTarget> {
    let fs = StepFs {
        files: RefCell::new(world.author_files.clone()),
    };
    resolve(None, None, &fs, Path::new("/work"))
}

#[when("the local sandbox run is prepared")]
fn local_run_prepared(world: &mut BehaviourWorld) {
    let target = resolve_local(world).expect("the local sandbox must resolve");
    world.wire_definition = target.definition_json();
    let mut args: RunArgs = parse_args(["lns", "run"]).expect("bare run must parse");
    if let RunTarget::Local { def, .. } = &target {
        args.filesets = def
            .spec
            .filesets
            .iter()
            .map(|fileset| lns_cli::run::summary::FilesetSummary {
                source: fileset_source_display(fileset),
                guest_path: fileset.guest_path.clone(),
                owner: lns_cli::run::summary::fileset_owner_display(fileset.owner).to_string(),
                host_path: fileset.host_path.clone(),
                optional: fileset.optional,
                declared_by: None,
            })
            .collect();
    }
    if world.cwd.is_none() {
        world.cwd = Some(tempfile::TempDir::new().expect("create tempdir"));
    }
    let cwd = world.cwd.as_ref().expect("cwd").path().to_path_buf();
    let mut buf = Vec::<u8>::new();
    print_run_summary(
        &args,
        lns_cli::run::summary::resolved_size(Default::default(), &args),
        &cwd,
        &mut buf,
    )
    .expect("print_run_summary");
    world.summary_output = String::from_utf8(buf).expect("non-utf8 summary output");
}

#[cucumber::given(
    regex = r#"^a pulled sandbox whose view declares a packed fileset "([^"]+)" mounted at "([^"]+)"$"#
)]
fn pulled_view_with_fileset(world: &mut BehaviourWorld, path: String, mount: String) {
    world.pulled_view = Some(lns_ipc::SandboxView {
        mixins: Vec::new(),
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        reference: "registry.example.test/team/sandbox:1".into(),
        digest: format!("sha256:{}", "a".repeat(64)),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
        user: None,
        mounts: Vec::new(),
        ports: Vec::new(),
        filesets: vec![lns_ipc::SandboxFileset {
            path: Some(path),
            inline: false,
            host_path: None,
            optional: false,
            guest_path: mount,
            owner: lns_ipc::SandboxFilesetOwner::Workload,
        }],
        env: Vec::new(),
        tools: Vec::new(),
        scripts: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
        disk_bytes: None,
    });
}

#[cucumber::given(
    regex = r#"^a pulled sandbox whose view declares an inline fileset at \"([^\"]+)\" owned by root$"#
)]
fn pulled_view_with_inline_fileset(world: &mut BehaviourWorld, mount: String) {
    world.pulled_view = Some(lns_ipc::SandboxView {
        mixins: Vec::new(),
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        reference: "registry.example.test/team/sandbox:1".into(),
        digest: format!("sha256:{}", "a".repeat(64)),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
        user: None,
        mounts: Vec::new(),
        ports: Vec::new(),
        filesets: vec![lns_ipc::SandboxFileset {
            path: None,
            inline: true,
            host_path: None,
            optional: false,
            guest_path: mount,
            owner: lns_ipc::SandboxFilesetOwner::Root,
        }],
        env: Vec::new(),
        tools: Vec::new(),
        scripts: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
        disk_bytes: None,
    });
}

#[when("the pulled sandbox run is prepared")]
fn pulled_run_prepared(world: &mut BehaviourWorld) {
    let view = world.pulled_view.take().expect("a pulled view is staged");
    let mut args: RunArgs = parse_args(["lns", "run", "registry.example.test/team/sandbox:1"])
        .expect("run with ref must parse");
    args.filesets = lns_cli::run::summary::fileset_summaries_from_view(&view);
    if world.cwd.is_none() {
        world.cwd = Some(tempfile::TempDir::new().expect("create tempdir"));
    }
    let cwd = world.cwd.as_ref().expect("cwd").path().to_path_buf();
    let mut buf = Vec::<u8>::new();
    print_run_summary(
        &args,
        lns_cli::run::summary::resolved_size(Default::default(), &args),
        &cwd,
        &mut buf,
    )
    .expect("print_run_summary");
    world.summary_output = String::from_utf8(buf).expect("non-utf8 summary output");
}

#[when("the user runs the local sandbox")]
fn user_runs_local_sandbox(world: &mut BehaviourWorld) {
    world.result = Some(match resolve_local(world) {
        Ok(_) => CliRun {
            exit_code: 0,
            output: String::new(),
        },
        Err(e) => CliRun {
            exit_code: 1,
            output: format!("{e:#}"),
        },
    });
}

#[then("the definition sent to the service roots the fileset path under the project")]
fn wire_definition_roots_path(world: &mut BehaviourWorld) -> Result<(), String> {
    let wire = world.wire_definition.as_ref().ok_or("no wire definition")?;
    if wire.contains("\"/work/skills\"") {
        Ok(())
    } else {
        Err(format!("expected a project-rooted path in: {wire}"))
    }
}

#[then("the definition sent to the service carries the inline file unchanged")]
fn wire_definition_keeps_inline(world: &mut BehaviourWorld) -> Result<(), String> {
    let wire = world.wire_definition.as_ref().ok_or("no wire definition")?;
    let value: serde_json::Value =
        serde_json::from_str(wire).map_err(|error| format!("invalid wire json: {error}"))?;
    let inline = &value["spec"]["filesets"][0]["inline"];
    if inline[".claude/settings.json"] == serde_json::json!(r#"{"marker":"do-not-print"}"#) {
        Ok(())
    } else {
        Err(format!("expected unchanged inline content, got {inline}"))
    }
}

#[then(
    regex = r#"^the run summary discloses an inline fileset at \"([^\"]+)\" owned by (?:the )?(workload|root)$"#
)]
fn summary_discloses_inline(
    world: &mut BehaviourWorld,
    mount: String,
    owner: String,
) -> Result<(), String> {
    let expected = format!("Fileset:   inline -> {mount} (owner: {owner})");
    if world.summary_output.contains(&expected) {
        Ok(())
    } else {
        Err(format!(
            "expected {expected:?} in:\n{}",
            world.summary_output
        ))
    }
}

#[then(regex = r#"^the run summary does not contain \"([^\"]+)\"$"#)]
fn summary_omits_content(world: &mut BehaviourWorld, content: String) -> Result<(), String> {
    if world.summary_output.contains(&content) {
        Err(format!(
            "summary leaked {content:?}:\n{}",
            world.summary_output
        ))
    } else {
        Ok(())
    }
}

#[then("the run summary does not contain the inline file content")]
fn summary_omits_inline_content(world: &mut BehaviourWorld) -> Result<(), String> {
    summary_omits_content(world, "do-not-print".to_string())
}

#[then(regex = r"^the run summary shows a Fileset line `([^`]+)`$")]
fn summary_shows_fileset_line(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    let needle = format!("Fileset:   {expected}");
    if world.summary_output.contains(&needle) {
        Ok(())
    } else {
        Err(format!("expected {needle:?} in:\n{}", world.summary_output))
    }
}

#[then(regex = r#"^the command fails naming "([^"]+)"$"#)]
fn command_fails_naming(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let result = world.result.as_ref().ok_or("no CLI run captured")?;
    if result.exit_code != 0 && result.output.contains(&needle) {
        Ok(())
    } else {
        Err(format!(
            "expected a failure naming {needle:?}, got exit {} with: {}",
            result.exit_code, result.output
        ))
    }
}

#[cucumber::given(
    regex = r#"^a pulled sandbox whose view declares a hostPath fileset "([^"]+)" at "([^"]+)" and optional$"#
)]
fn pulled_view_with_host_path_fileset(world: &mut BehaviourWorld, source: String, mount: String) {
    world.pulled_view = Some(lns_ipc::SandboxView {
        mixins: Vec::new(),
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        reference: "registry.example.test/team/sandbox:1".into(),
        digest: format!("sha256:{}", "a".repeat(64)),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
        user: None,
        mounts: Vec::new(),
        ports: Vec::new(),
        filesets: vec![lns_ipc::SandboxFileset {
            path: None,
            inline: false,
            host_path: Some(source),
            guest_path: mount,
            owner: lns_ipc::SandboxFilesetOwner::Workload,
            optional: true,
        }],
        env: Vec::new(),
        tools: Vec::new(),
        scripts: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
        disk_bytes: None,
    });
}

#[when("the pulled sandbox effects are confirmed with no answer")]
fn pulled_effects_confirmed(world: &mut BehaviourWorld) {
    let view = world.pulled_view.take().expect("a pulled view is staged");
    let filesets = lns_cli::run::summary::fileset_summaries_from_view(&view);
    let effects = lns_cli::run::pull_confirm::PulledEffects {
        reference: &view.reference,
        binds: &[],
        volumes: &[],
        filesets: &filesets,
        tools: &[],
        scripts: &[],
    };
    let mut terminal = ScriptedTerminal::answering(&[]);
    let mut out = Vec::new();
    let outcome = lns_cli::run::pull_confirm::confirm_pulled_effects(
        &effects,
        false,
        &mut terminal,
        &mut out,
    );
    world.summary_output = String::from_utf8(out).expect("non-utf8 disclosure");
    world.result = Some(CliRun {
        exit_code: i32::from(outcome.is_err()),
        output: outcome.err().map(|e| format!("{e:#}")).unwrap_or_default(),
    });
}

#[then(regex = r#"^the disclosure names the host file "([^"]+)"$"#)]
fn disclosure_names_host_file(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    if world.summary_output.contains(&needle) {
        Ok(())
    } else {
        Err(format!(
            "expected {needle:?} in the disclosure:\n{}",
            world.summary_output
        ))
    }
}

#[then("the disclosure does not call the host file author-published")]
fn disclosure_does_not_claim_author_published(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.summary_output.contains("author-published") {
        Err(format!(
            "a host file is read from the consumer's own machine, so calling it author-published inverts what the operator is consenting to:\n{}",
            world.summary_output
        ))
    } else {
        Ok(())
    }
}

#[then("the run is refused without a confirmation")]
fn run_refused_without_confirmation(world: &mut BehaviourWorld) -> Result<(), String> {
    let result = world.result.as_ref().ok_or("no run captured")?;
    if result.exit_code != 0 && result.output.contains("declined") {
        Ok(())
    } else {
        Err(format!(
            "a host file must not be read without consent, got exit {} with: {}",
            result.exit_code, result.output
        ))
    }
}
