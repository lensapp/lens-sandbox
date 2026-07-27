use std::path::Path;

use cucumber::{given, then, when};
use lns_cli::cli::RunArgs;
use lns_cli::command::parse_args;
use lns_cli::run::declarative::{Defaults, resolve};
use lns_cli::run::host_bind::{DirScan, resolve_binds};
use lns_policy::host_bind_decisions::{HostBindDecisionFile, HostBindDecisionStore};

use crate::world::{BehaviourWorld, HostBindOutcome, ResolvedRunView};

fn definition(world: &BehaviourWorld) -> lns_artifact::sandbox::Definition {
    let yaml = world
        .author_files
        .get(Path::new("/work/lns.yaml"))
        .expect("the scenario must install lns.yaml");
    let value: serde_json::Value = serde_yaml::from_str(yaml).expect("valid fixture yaml");
    let json = serde_json::to_vec(&value).expect("serializable fixture yaml");
    lns_artifact::sandbox::parse(&json).expect("valid sandbox fixture")
}

fn declarative_yaml(bind_source: &str) -> String {
    format!(
        "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: some-sandbox\nspec:\n  image: example.test/runtime:1\n  workdir: /workspace\n  volumes:\n    - type: bind\n      source: {bind_source}\n      target: /workspace\n    - type: volume\n      source: some-cache\n      target: /home/node/.cache\n      readOnly: true\n"
    )
}

#[given("an lns.yaml declaring workdir and declarative mounts")]
fn definition_with_settings(world: &mut BehaviourWorld) {
    world
        .author_files
        .insert("/work/lns.yaml".into(), declarative_yaml("."));
}

#[given(regex = r#"^an lns.yaml declaring a bind source "([^"]+)"$"#)]
fn definition_with_bind_source(world: &mut BehaviourWorld, source: String) {
    world.author_files.insert(
        "/work/lns.yaml".into(),
        format!(
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: some-sandbox\nspec:\n  image: example.test/runtime:1\n  volumes:\n    - type: bind\n      source: '{source}'\n      target: /workspace\n"
        ),
    );
}

#[given("a published sandbox declaring a relative bind and workdir")]
fn published_settings(world: &mut BehaviourWorld) {
    world
        .author_files
        .insert("/work/lns.yaml".into(), declarative_yaml("."));
}

fn resolve_defaults(world: &mut BehaviourWorld, defaults: &Defaults, project: &Path, flags: &str) {
    let mut argv = vec!["lns".to_string(), "run".to_string()];
    argv.extend(flags.split_whitespace().map(str::to_string));
    let args: RunArgs = parse_args(&argv).expect("override flags must parse");
    let resolved = resolve(defaults, project, args.workdir, args.mounts)
        .expect("declarative settings must resolve");
    let (volumes, binds) = lns_cli::cli::split_mounts(&resolved.mounts);
    world.resolved_run = Some(ResolvedRunView {
        workdir: resolved.workdir,
        volumes: volumes
            .iter()
            .map(|v| {
                format!(
                    "{}:{}{}",
                    v.name,
                    v.target,
                    if v.read_only { ":ro" } else { "" }
                )
            })
            .collect(),
        binds: binds
            .iter()
            .map(|b| {
                format!(
                    "{} -> {}{}",
                    b.host_source,
                    b.target,
                    if b.read_only { ":ro" } else { "" }
                )
            })
            .collect(),
        ..Default::default()
    });
}

#[when("the local sandbox launch settings are resolved with no overrides")]
fn resolve_local_without_overrides(world: &mut BehaviourWorld) {
    let defaults = Defaults::from_definition(&definition(world));
    resolve_defaults(world, &defaults, Path::new("/work"), "");
}

#[when(regex = r#"^the local sandbox launch settings are resolved with "([^"]+)"$"#)]
fn resolve_local_with_overrides(world: &mut BehaviourWorld, flags: String) {
    let defaults = Defaults::from_definition(&definition(world));
    resolve_defaults(world, &defaults, Path::new("/work"), &flags);
}

#[when(regex = r#"^the published sandbox launch settings are resolved from "([^"]+)"$"#)]
fn resolve_published(world: &mut BehaviourWorld, project: String) {
    let def = definition(world);
    let view = lns_ipc::SandboxView {
        reference: "registry.example.test/team/sandbox:1".into(),
        digest: format!("sha256:{}", "a".repeat(64)),
        image: def.spec.image.clone(),
        workdir: def.spec.workdir.clone(),
        mounts: def
            .spec
            .volumes
            .iter()
            .map(|volume| lns_ipc::SandboxMount {
                kind: if volume.is_bind() {
                    lns_ipc::SandboxMountKind::Bind
                } else {
                    lns_ipc::SandboxMountKind::Volume
                },
                source: volume.source().to_string(),
                target: volume.target.clone(),
                read_only: volume.read_only(),
            })
            .collect(),
        ports: Vec::new(),
        filesets: Vec::new(),
        connectors: Vec::new(),
        env: Vec::new(),
        credentials: Vec::new(),
        tools: Vec::new(),
        policy_flags: Vec::new(),
    };
    resolve_defaults(world, &Defaults::from_view(&view), Path::new(&project), "");
}

struct FakeDir {
    entries: Vec<String>,
    lensignore: Option<String>,
    missing: bool,
}

impl DirScan for FakeDir {
    fn exists(&self, _path: &Path) -> bool {
        !self.missing
    }

    fn entries(&self, _dir: &Path) -> Vec<String> {
        self.entries.clone()
    }

    fn read_to_string(&self, path: &Path) -> Option<String> {
        path.ends_with(".lensignore")
            .then(|| self.lensignore.clone())
            .flatten()
    }
}

struct FakeStore(std::sync::Mutex<HostBindDecisionFile>);

impl HostBindDecisionStore for FakeStore {
    fn load(&self) -> std::io::Result<HostBindDecisionFile> {
        Ok(self.0.lock().unwrap().clone())
    }

    fn save(&self, state: &HostBindDecisionFile) -> std::io::Result<()> {
        *self.0.lock().unwrap() = state.clone();
        Ok(())
    }
}

#[when("the declarative host binds are resolved interactively")]
fn resolve_declarative_binds(world: &mut BehaviourWorld) {
    let defaults = Defaults::from_definition(&definition(world));
    let resolved = resolve(&defaults, Path::new("/work"), None, Vec::new()).unwrap();
    let bind_specs = lns_cli::cli::split_mounts(&resolved.mounts).1;
    let dir = FakeDir {
        entries: world.host_bind.entries.clone(),
        lensignore: world.host_bind.lensignore.clone(),
        missing: world.host_bind.missing,
    };
    let store = FakeStore(std::sync::Mutex::new(world.host_bind.decisions.clone()));
    let mut input = std::io::Cursor::new(world.host_bind.answer.clone().unwrap_or_default());
    let mut output = Vec::new();
    let result = resolve_binds(&bind_specs, &dir, &store, true, &mut input, &mut output)
        .map_err(|error| error.to_string());
    world.host_bind.outcome = Some(HostBindOutcome {
        result,
        prompt: String::from_utf8(output).unwrap(),
        persisted: store.0.into_inner().unwrap(),
        summary: String::new(),
    });
}

#[then(regex = r#"^the resolved workdir is "([^"]+)"$"#)]
fn resolved_workdir(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    let actual = world
        .resolved_run
        .as_ref()
        .and_then(|view| view.workdir.as_deref());
    if actual == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(format!("expected workdir {expected:?}, got {actual:?}"))
    }
}
