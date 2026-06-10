use crate::E2eWorld;
use crate::specutil::arg_parser::split_args;
use cucumber::when;

#[when(regex = r#"^I run sandbox command "([^"]*)" against the service$"#)]
fn run_sandbox_command(world: &mut E2eWorld, cmd_line: String) {
    let mut args = vec!["sandbox".to_string()];
    args.extend(split_args(&cmd_line));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    world.result = Some(world.run_with_service_env(&arg_refs));
}
