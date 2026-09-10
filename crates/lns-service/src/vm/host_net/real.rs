#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::net::Ipv4Addr;

use super::{
    CommandOutput, HostFiles, HostNetwork, HostNetworkSource, Neighbors, NetworkMemory,
    observe_neighbors, parse_remembered, render_remembered,
};

pub struct RealHostFiles;

impl HostFiles for RealHostFiles {
    fn read(&self, path: &str) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }
}

pub struct RealHostNetwork;

impl HostNetworkSource for RealHostNetwork {
    fn observe(&self) -> std::io::Result<String> {
        let output = std::process::Command::new("/sbin/ifconfig")
            .arg("bridge100")
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "ifconfig bridge100 exited with {}",
                output.status
            )));
        }
        String::from_utf8(output.stdout)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }
}

pub struct RealCommandOutput;

impl CommandOutput for RealCommandOutput {
    fn output(&self, program: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
        std::process::Command::new(program).args(args).output()
    }
}

pub struct RealNeighbors;

impl Neighbors for RealNeighbors {
    fn observed(&self) -> std::io::Result<Vec<Ipv4Addr>> {
        observe_neighbors(&RealCommandOutput)
    }
}

/// Written beside the rest of the service's own state, never inside a run: it records what this host runs, not what a run did.
const MEMORY_FILE: &str = "host-network";

pub struct RealNetworkMemory;

fn memory_path() -> Option<std::path::PathBuf> {
    lns_ipc::lns_home().ok().map(|home| home.join(MEMORY_FILE))
}

impl NetworkMemory for RealNetworkMemory {
    fn recall(&self) -> Option<HostNetwork> {
        parse_remembered(&std::fs::read_to_string(memory_path()?).ok()?)
    }

    fn remember(&self, network: &HostNetwork) {
        let Some(path) = memory_path() else { return };
        let Some(parent) = path.parent() else { return };
        if std::fs::create_dir_all(parent).is_ok() {
            let _ = std::fs::write(path, render_remembered(network));
        }
    }
}

/// How long a booting guest is given to bring the shared network up before the host stops trying to learn it.
const LEARN_ATTEMPTS: usize = 20;
const LEARN_RETRY: std::time::Duration = std::time::Duration::from_millis(250);

/// Only the macOS backend attaches a guest to a shared network, so there is nothing for a Linux host to learn.
pub fn learn_shared_network() {
    if cfg!(target_os = "macos") {
        tokio::spawn(async {
            super::learn_host_network(
                &RealHostNetwork,
                &RealNetworkMemory,
                LEARN_ATTEMPTS,
                LEARN_RETRY,
            )
            .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial(env)]
    fn what_the_host_was_observed_running_survives_the_file_it_is_kept_in() {
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        let memory = RealNetworkMemory;
        assert_eq!(memory.recall(), None, "a host lns has never seen says so");

        let observed = HostNetwork {
            network: std::net::Ipv4Addr::new(192, 168, 66, 0),
            prefix_len: 24,
            gateway: std::net::Ipv4Addr::new(192, 168, 66, 1),
        };
        memory.remember(&observed);
        assert_eq!(
            memory.recall(),
            Some(observed),
            "the next cold start reads what this one wrote"
        );
        assert!(home.path().join(MEMORY_FILE).is_file());
    }

    #[test]
    fn the_real_reader_returns_a_file_and_names_a_missing_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("leases");
        std::fs::write(&path, "{\n\tip_address=192.168.64.2\n}\n").expect("write");
        let files = RealHostFiles;
        assert!(
            files
                .read(path.to_str().expect("utf-8 path"))
                .expect("read")
                .contains("192.168.64.2")
        );
        let missing = files
            .read(dir.path().join("absent").to_str().expect("utf-8 path"))
            .expect_err("a host without a shared network has no lease file");
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
    }
}
