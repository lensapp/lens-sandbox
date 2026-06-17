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
}

impl Drop for E2eWorld {
    fn drop(&mut self) {
        self.shutdown_service();
    }
}

#[tokio::main]
async fn main() {
    let features_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("features");
    let microvm_only = std::env::var_os("LNS_E2E_MICROVM").is_some();

    E2eWorld::cucumber()
        .fail_on_skipped()
        .filter_run_and_exit(features_dir, move |feat, _, sc| {
            let tagged =
                |tag: &str| feat.tags.iter().any(|t| t == tag) || sc.tags.iter().any(|t| t == tag);
            if tagged("gui") {
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
