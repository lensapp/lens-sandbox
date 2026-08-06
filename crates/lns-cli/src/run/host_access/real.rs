use std::io;

use super::{HostCommandOutput, HostFacts};

pub struct RealHostFacts;

impl HostFacts for RealHostFacts {
    fn run(&self, program: &str, args: &[String]) -> io::Result<HostCommandOutput> {
        let output = std::process::Command::new(program).args(args).output()?;
        Ok(HostCommandOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        })
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }
}
