mod quote;

use lens_sandbox_core::config::CoreConfig;

pub struct AgentConfig {
    pub core: CoreConfig,
    pub agent_command: String,
    /// Explicit `WORKSPACE_PATH`; `None` defers the cwd to the post-setuid home rather than pinning the agent to root's `/root`.
    pub workspace_path: Option<String>,
}

pub fn load_config() -> AgentConfig {
    let core = lens_sandbox_core::config::load_core_config();

    let workspace_path = std::env::var("WORKSPACE_PATH")
        .ok()
        .filter(|s| !s.is_empty());
    let mut agent_command = std::env::var("AGENT_COMMAND").unwrap_or_else(|_| "sh".to_string());

    let cli_args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    quote::append_cli_args(&mut agent_command, &cli_args);

    AgentConfig {
        core,
        agent_command,
        workspace_path,
    }
}
