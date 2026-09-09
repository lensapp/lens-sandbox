#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::net::Ipv4Addr;

use super::{HostFiles, HostNetworkSource, Neighbors, parse_arp_neighbors};

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

pub struct RealNeighbors;

impl Neighbors for RealNeighbors {
    fn observed(&self) -> Vec<Ipv4Addr> {
        std::process::Command::new("/usr/sbin/arp")
            .arg("-an")
            .output()
            .ok()
            .map(|out| parse_arp_neighbors(&String::from_utf8_lossy(&out.stdout)))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
