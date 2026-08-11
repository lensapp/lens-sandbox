#[path = "support/registry.rs"]
mod registry;
mod specutil;
mod steps;

use cucumber::World;
use specutil::CliResult;
use std::path::PathBuf;
use tempfile::TempDir;

#[derive(Debug, Default, World)]
pub struct E2eWorld {
    pub result: Option<CliResult>,
    pub results: Vec<CliResult>,
    pub home: Option<TempDir>,
    pub service_dir: Option<TempDir>,
    pub service_socket: Option<PathBuf>,
    pub detached_runs: Vec<String>,
    pub last_run_id: Option<String>,
    pub created_volumes: Vec<String>,
    pub policy_dir: Option<TempDir>,
    pub policy_path: Option<PathBuf>,
    pub host_bind_dir: Option<TempDir>,
    pub registry: Option<registry::LocalRegistry>,
    pub version_index: Option<registry::VersionIndex>,
    pub pushed_ref: Option<String>,
    pub pushed_digest: Option<String>,
    pub project: Option<TempDir>,
    pub project_connectors: Vec<String>,
    pub project_command: Option<String>,
    pub project_env: Vec<(String, String)>,
    /// Declared credentials as (env var, injection domain); the placeholder is derived so a guest assertion can name it.
    pub project_credentials: Vec<(String, String)>,
    /// Ports the project definition declares, as (host, container).
    pub project_ports: Vec<(Option<u16>, u16)>,
    /// Path filesets the project declares, as (directory, file inside it, mountPath).
    pub project_filesets: Vec<(String, String, String, Option<String>)>,
    pub project_inline_filesets: Vec<(String, String, String, Option<String>)>,
    pub project_tools: Vec<String>,
    pub project_image: Option<String>,
    /// Raises the per-run budget for a tool whose upstream payload is far larger than the usual one.
    pub run_budget: Option<std::time::Duration>,
    /// Destinations the definition's own policy allows, so a scenario whose workload really fetches something is not left at an approval prompt.
    pub project_egress: Vec<String>,
}

impl E2eWorld {
    pub fn run_with_service_env(&self, args: &[&str]) -> CliResult {
        if let Some(sock) = &self.service_socket {
            let sock_str = sock.to_string_lossy();
            let envs = [("LNS_SOCKET_PATH", sock_str.as_ref())];
            specutil::run_cli_with_env(args.iter().copied(), envs)
        } else {
            specutil::run_cli(args.iter().copied())
        }
    }

    pub fn ensure_service_dir(&mut self) {
        if self.service_dir.is_none() {
            let dir = TempDir::new().expect("tempdir for service socket");
            self.service_socket = Some(dir.path().join("lns.sock"));
            self.service_dir = Some(dir);
        }
    }

    pub fn shutdown_service(&self) {
        if let Some(sock) = &self.service_socket
            && sock.exists()
        {
            let _ = self.run_with_service_env(&["service", "stop"]);
        }
    }

    pub fn kill_detached_runs(&self) {
        for id in &self.detached_runs {
            let _ = self.run_with_service_env(&["sandbox", "kill", id]);
        }
    }

    pub fn remove_created_volumes(&self) {
        for name in &self.created_volumes {
            let _ = self.run_with_service_env(&["volume", "rm", name]);
        }
    }
}

impl Drop for E2eWorld {
    fn drop(&mut self) {
        self.kill_detached_runs();
        self.remove_created_volumes();
        self.shutdown_service();
    }
}

#[tokio::main]
async fn main() {
    let features_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("features");
    let microvm_only = std::env::var_os("LNS_E2E_MICROVM").is_some();

    // Optional dev filter: run only features or scenarios whose name contains this substring.
    let only_feature = std::env::var("LNS_E2E_FEATURE")
        .ok()
        .filter(|s| !s.is_empty());
    let mut runner = E2eWorld::cucumber().fail_on_skipped();
    if microvm_only {
        runner = runner.max_concurrent_scenarios(1);
    }
    runner
        .filter_run_and_exit(features_dir, move |feat, _, sc| {
            let tagged =
                |tag: &str| feat.tags.iter().any(|t| t == tag) || sc.tags.iter().any(|t| t == tag);
            if tagged("gui") {
                return false;
            }
            if let Some(needle) = &only_feature
                && !feat.name.contains(needle.as_str())
                && !sc.name.contains(needle.as_str())
            {
                return false;
            }
            if microvm_only {
                tagged("microvm")
            } else {
                !tagged("microvm")
            }
        })
        .await;
}
