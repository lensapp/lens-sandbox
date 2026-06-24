use crate::world::BehaviourWorld;
use clap::Parser;
use cucumber::{given, then, when};
use lns_cli::cli::{Cli, Command};
use lns_cli::registry::RegistryClient;
use lns_cli::run::resolve::resolve_explicit_mounts;
use lns_policy::artifact::Family;

#[given(regex = r#"^a fileset artifact "([^"]+)" mounting at "([^"]+)"$"#)]
async fn given_fileset(world: &mut BehaviourWorld, reference: String, path: String) {
    let yaml = format!(
        "apiVersion: lens.dev/v1alpha1\nkind: FileSet\n\
         metadata:\n  name: some-fileset\n\
         mount:\n  path: {path}\n  readOnly: true\nspec: {{}}\n"
    );
    let blob = lns_policy::artifact::to_config_blob(yaml.as_bytes()).expect("fileset yaml");
    world
        .registry
        .push_artifact(
            &reference,
            &Family::Fileset.artifact_type(),
            &Family::Fileset.config_media_type(),
            &blob,
            &[],
        )
        .await
        .expect("store fileset artifact");
}

#[when(regex = r#"^the developer runs an image with mount "([^"]+)"$"#)]
async fn when_runs_with_mount(world: &mut BehaviourWorld, mount: String) {
    let cli = Cli::try_parse_from(["lns", "run", "some-image:1", "--mount", &mount])
        .expect("parse run argv");
    let Command::Run(args) = cli.command else {
        panic!("expected a run command");
    };
    let mut buf = Vec::new();
    match resolve_explicit_mounts(args, &world.registry, &mut buf).await {
        Ok(args) => world.resolved_mounts = args.artifact_mounts,
        Err(e) => world.resolve_error = Some(format!("{e:#}")),
    }
    world.resolve_writer = String::from_utf8_lossy(&buf).into_owned();
}

#[then(regex = r#"^an artifact mount targets "([^"]+)" from "([^"]+)"$"#)]
fn then_artifact_mount(
    world: &mut BehaviourWorld,
    path: String,
    reference: String,
) -> Result<(), String> {
    match world
        .resolved_mounts
        .iter()
        .find(|m| m.reference == reference)
    {
        Some(m) if m.path == path => Ok(()),
        Some(m) => Err(format!(
            "mount {reference} targets {:?}, expected {path:?}",
            m.path
        )),
        None => Err(format!(
            "no artifact mount for {reference}: {:?}",
            world.resolved_mounts
        )),
    }
}
