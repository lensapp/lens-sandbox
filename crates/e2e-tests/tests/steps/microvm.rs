use crate::E2eWorld;
use crate::specutil::{arg_parser::split_args, run_cli_with_timeout};
use cucumber::when;
use std::time::Duration;

const MICROVM_RUN_TIMEOUT: Duration = Duration::from_secs(120);

#[when(regex = r#"^the user runs a microVM command "([^"]*)"$"#)]
fn run_microvm_command(world: &mut E2eWorld, cmd_line: String) {
    let mut args = vec!["run".to_string(), "--".to_string()];
    args.extend(split_args(&cmd_line));
    let envs: Vec<(&str, std::ffi::OsString)> = world
        .service_socket
        .as_ref()
        .map(|socket| vec![("LNS_SOCKET_PATH", socket.clone().into())])
        .unwrap_or_default();
    world.result = Some(run_cli_with_timeout(args, envs, MICROVM_RUN_TIMEOUT));
}
