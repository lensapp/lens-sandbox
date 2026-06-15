#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use anyhow::{Result, bail};

use super::{VmSpec, VmmBackend};

pub struct CloudHypervisor;

impl VmmBackend for CloudHypervisor {
    fn name(&self) -> &'static str {
        "cloud-hypervisor"
    }

    fn run(&self, spec: VmSpec) -> Result<()> {
        bail!(
            "lns: Cloud Hypervisor backend is not yet wired for the composefs path.\n\
             \n\
             The content store at {} needs a vhost-user-fs daemon (e.g. virtiofsd)\n\
             pointed at it; CH's `fs[].socket` is the daemon's socket, not a\n\
             directory. Spawning virtiofsd from this backend is the open follow-up.\n\
             \n\
             Use the Vz backend on macOS for end-to-end runs today.",
            spec.content_share.display()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::CloudHypervisor;
    use crate::vm::{ExecSpec, VmSpec, VmmBackend};
    use std::path::PathBuf;

    fn spec() -> VmSpec {
        VmSpec {
            run_id: 1,
            cpus: 1,
            memory_mib: 512,
            kernel: PathBuf::from("/kernel"),
            initrd: PathBuf::from("/initrd"),
            composefs_descriptor: PathBuf::from("/descriptor"),
            content_share: PathBuf::from("/content/share"),
            content_tag: "tag".into(),
            descriptor_sha256: None,
            upper_disk: PathBuf::from("/upper"),
            volumes: vec![],
            binds: vec![],
            debug: false,
            exec: ExecSpec {
                kernel_env: Vec::new(),
            },
            #[cfg(target_os = "macos")]
            vsock: None,
            #[cfg(target_os = "macos")]
            connector_tx: None,
            #[cfg(target_os = "macos")]
            console_fd: -1,
        }
    }

    #[test]
    fn name_is_cloud_hypervisor() {
        assert_eq!(CloudHypervisor.name(), "cloud-hypervisor");
    }

    #[test]
    fn run_bails_until_the_virtiofsd_path_is_wired() {
        let err = CloudHypervisor.run(spec()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("not yet wired"), "got: {msg}");
        assert!(
            msg.contains("/content/share"),
            "error should name the content share: {msg}"
        );
    }
}
