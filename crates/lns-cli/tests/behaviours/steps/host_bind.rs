use crate::world::{BehaviourWorld, ResolvedRunView};
use cucumber::{then, when};
use lns_cli::cli::{RunArgs, split_mounts};
use lns_cli::command::parse_args;

#[when(regex = r"^the mounts are resolved for `lns run ([^`]+)`$")]
fn resolve_mounts(world: &mut BehaviourWorld, flags: String) {
    let mut argv = vec!["lns".to_string(), "run".to_string()];
    argv.extend(flags.split_whitespace().map(str::to_string));
    let args: RunArgs = parse_args(&argv).expect("argv must parse against the CLI grammar");
    let (volumes, binds) = split_mounts(&args.mounts);
    world.resolved_run = Some(ResolvedRunView {
        volumes: volumes
            .iter()
            .map(|v| {
                let ro = if v.read_only { ":ro" } else { "" };
                format!("{}:{}{ro}", v.name, v.target)
            })
            .collect(),
        binds: binds
            .iter()
            .map(|b| {
                let ro = if b.read_only { ":ro" } else { "" };
                format!("{} -> {}{ro}", b.host_source, b.target)
            })
            .collect(),
        ..Default::default()
    });
}

#[then(regex = r#"^the resolved host binds are exactly "([^"]*)"$"#)]
fn resolved_binds_are(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    let view = world
        .resolved_run
        .as_ref()
        .ok_or_else(|| "no resolved run captured".to_string())?;
    let rendered = view.binds.join(", ");
    if rendered == expected {
        Ok(())
    } else {
        Err(format!(
            "expected host binds {expected:?}, got {rendered:?}"
        ))
    }
}
