use std::path::PathBuf;

use lns_service::vm::{BindAttachment, ExecSpec, build_kernel_cmdline};

/// Drives the host-bind slice the service owns without booting a VM: it builds
/// the kernel cmdline from `BindAttachment`s and records the audit line.
#[derive(Debug)]
pub struct BindRig {
    binds: Vec<BindAttachment>,
    audit_file: PathBuf,
    _tmp: tempfile::TempDir,
}

impl BindRig {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let audit_file = tmp.path().join("audit.jsonl");
        Self {
            binds: Vec::new(),
            audit_file,
            _tmp: tmp,
        }
    }

    pub fn request(&mut self, source: &str, target: &str, read_only: bool, drops: &[&str]) {
        self.binds.push(BindAttachment {
            host_source: source.into(),
            target: target.into(),
            read_only,
            dropped_paths: drops.iter().map(|s| s.to_string()).collect(),
            seeded_paths: Vec::new(),
        });
    }

    pub fn request_seeding(&mut self, source: &str, target: &str, drops: &[&str], seeds: &[&str]) {
        self.binds.push(BindAttachment {
            host_source: source.into(),
            target: target.into(),
            read_only: false,
            dropped_paths: drops.iter().map(|s| s.to_string()).collect(),
            seeded_paths: seeds.iter().map(|s| s.to_string()).collect(),
        });
    }

    pub fn cmdline(&self) -> String {
        let exec = ExecSpec {
            kernel_env: Vec::new(),
        };
        build_kernel_cmdline(
            &exec,
            "hvc0",
            true,
            "lns-content",
            None,
            false,
            &[],
            &self.binds,
        )
    }

    pub fn record_audit(&self, source: &str, target: &str, exposed: &[&str], dropped: &[&str]) {
        let exposed: Vec<String> = exposed.iter().map(|s| s.to_string()).collect();
        let dropped: Vec<String> = dropped.iter().map(|s| s.to_string()).collect();
        let cx = lns_service::ocsf_audit::OcsfCtx::at_unix(
            "test-run".into(),
            "calm-finch".into(),
            1_700_000_000,
        );
        lns_service::audit::record_bind_attached_at(
            &self.audit_file,
            &cx,
            source,
            target,
            &exposed,
            &dropped,
        )
        .expect("record bind audit event");
    }

    pub fn audit_contents(&self) -> String {
        std::fs::read_to_string(&self.audit_file).unwrap_or_default()
    }
}
