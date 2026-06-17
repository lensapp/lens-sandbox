#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::{VmSpec, VmmBackend};

mod launch;
mod orchestrate;
mod process;
mod vmm_bin;
mod vsock;

use process::Child;

pub struct CloudHypervisor;

impl VmmBackend for CloudHypervisor {
    fn name(&self) -> &'static str {
        "cloud-hypervisor"
    }

    fn run(&self, spec: VmSpec) -> Result<()> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("building cloud-hypervisor runtime")?;
        rt.block_on(run_async_with(
            &process::RealSpawner,
            spec,
            |k| std::env::var_os(k),
            &orchestrate::LaunchTimeouts::default(),
        ))
    }
}

async fn run_async_with<S: process::Spawner>(
    spawner: &S,
    spec: VmSpec,
    env_get: impl Fn(&str) -> Option<OsString>,
    timeouts: &orchestrate::LaunchTimeouts,
) -> Result<()> {
    let run_dir = spec
        .upper_disk
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let layout = launch::SocketLayout::for_run_dir(&run_dir);
    let bins = vmm_bin::resolve(env_get)?;
    let relay_fd_tx = spec.vsock.as_ref().map(|c| c.fd_tx.clone());
    let running =
        orchestrate::launch(spawner, &spec, &bins, &layout, relay_fd_tx, timeouts).await?;
    let orchestrate::RunningVm {
        mut virtiofsd,
        mut cloud_hypervisor,
        connector,
    } = running;
    if let Some(tx) = spec.connector_tx {
        let _ = tx.send(std::sync::Arc::new(connector));
    }
    let status = cloud_hypervisor
        .wait()
        .await
        .context("waiting for cloud-hypervisor to exit")?;
    let _ = virtiofsd.start_kill();
    if !status.success() {
        bail!("cloud-hypervisor exited unsuccessfully: {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::ExecSpec;
    use orchestrate::LaunchTimeouts;
    use process::Spawner;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn spec(run_dir: &Path) -> VmSpec {
        VmSpec {
            run_id: 1,
            cpus: 1,
            memory_mib: 512,
            kernel: PathBuf::from("/kernel"),
            initrd: PathBuf::from("/initrd"),
            composefs_descriptor: PathBuf::from("/descriptor"),
            content_share: PathBuf::from("/content/share"),
            content_tag: "lns-content".into(),
            descriptor_sha256: None,
            upper_disk: run_dir.join("disk/upper"),
            volumes: vec![],
            binds: vec![],
            debug: false,
            exec: ExecSpec::from_image_config(None, &["true".into()]),
            vsock: None,
            connector_tx: None,
            #[cfg(target_os = "macos")]
            console_fd: -1,
        }
    }

    fn fast() -> LaunchTimeouts {
        LaunchTimeouts {
            virtiofsd: Duration::from_millis(200),
            cloud_hypervisor: Duration::from_millis(200),
        }
    }

    struct FakeChild {
        exit_raw: i32,
        killed: Arc<AtomicBool>,
    }

    impl Child for FakeChild {
        async fn wait(&mut self) -> std::io::Result<ExitStatus> {
            Ok(ExitStatus::from_raw(self.exit_raw))
        }
        fn start_kill(&mut self) -> std::io::Result<()> {
            self.killed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FakeSpawner {
        ch_exit_raw: i32,
        virtiofsd_killed: Arc<AtomicBool>,
        spawned: Arc<Mutex<usize>>,
    }

    fn socket_from_args(args: &[String]) -> Option<PathBuf> {
        for a in args {
            if a.contains("cid=")
                && let Some(idx) = a.find("socket=")
            {
                return Some(PathBuf::from(&a[idx + "socket=".len()..]));
            }
        }
        for a in args {
            if let Some(p) = a.strip_prefix("--socket-path=") {
                return Some(PathBuf::from(p));
            }
        }
        None
    }

    impl Spawner for FakeSpawner {
        type Child = FakeChild;
        fn spawn(&self, _program: &Path, args: &[String]) -> std::io::Result<FakeChild> {
            *self.spawned.lock().unwrap() += 1;
            if let Some(path) = socket_from_args(args) {
                std::fs::write(&path, b"").unwrap();
            }
            let is_virtiofsd = args.iter().any(|a| a.starts_with("--shared-dir="));
            if is_virtiofsd {
                Ok(FakeChild {
                    exit_raw: 0,
                    killed: self.virtiofsd_killed.clone(),
                })
            } else {
                Ok(FakeChild {
                    exit_raw: self.ch_exit_raw,
                    killed: Arc::new(AtomicBool::new(false)),
                })
            }
        }
    }

    struct PanicSpawner;
    impl Spawner for PanicSpawner {
        type Child = FakeChild;
        fn spawn(&self, _p: &Path, _a: &[String]) -> std::io::Result<FakeChild> {
            panic!("must not spawn before binaries are resolved");
        }
    }

    #[test]
    fn name_is_cloud_hypervisor() {
        assert_eq!(CloudHypervisor.name(), "cloud-hypervisor");
    }

    #[tokio::test]
    async fn run_bails_with_actionable_help_when_binaries_are_not_resolved() {
        let d = tempfile::TempDir::new().unwrap();
        let err = run_async_with(&PanicSpawner, spec(d.path()), |_| None, &fast())
            .await
            .expect_err("no LNS_*_BIN override and no published pin → error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("cloud-hypervisor was not found on PATH"),
            "got {msg}"
        );
        assert!(
            msg.contains("LNS_CLOUD_HYPERVISOR_BIN=/path/to/cloud-hypervisor"),
            "got {msg}"
        );
    }

    #[tokio::test]
    async fn run_boots_waits_and_reports_a_clean_exit() {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(d.path().join("disk")).unwrap();
        let ch = tempfile::NamedTempFile::new().unwrap();
        let vfsd = tempfile::NamedTempFile::new().unwrap();
        let (ch_path, vfsd_path) = (ch.path().to_path_buf(), vfsd.path().to_path_buf());
        let env = move |k: &str| match k {
            "LNS_CLOUD_HYPERVISOR_BIN" => Some(ch_path.clone().into_os_string()),
            "LNS_VIRTIOFSD_BIN" => Some(vfsd_path.clone().into_os_string()),
            _ => None,
        };
        let virtiofsd_killed = Arc::new(AtomicBool::new(false));
        let spawner = FakeSpawner {
            ch_exit_raw: 0,
            virtiofsd_killed: virtiofsd_killed.clone(),
            spawned: Arc::new(Mutex::new(0)),
        };
        run_async_with(&spawner, spec(d.path()), env, &fast())
            .await
            .expect("clean boot+exit");
        assert!(
            virtiofsd_killed.load(Ordering::SeqCst),
            "virtiofsd must be torn down once cloud-hypervisor exits"
        );
    }

    #[tokio::test]
    async fn run_propagates_a_nonzero_cloud_hypervisor_exit() {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(d.path().join("disk")).unwrap();
        let ch = tempfile::NamedTempFile::new().unwrap();
        let vfsd = tempfile::NamedTempFile::new().unwrap();
        let (ch_path, vfsd_path) = (ch.path().to_path_buf(), vfsd.path().to_path_buf());
        let env = move |k: &str| match k {
            "LNS_CLOUD_HYPERVISOR_BIN" => Some(ch_path.clone().into_os_string()),
            "LNS_VIRTIOFSD_BIN" => Some(vfsd_path.clone().into_os_string()),
            _ => None,
        };
        let spawner = FakeSpawner {
            ch_exit_raw: 256, // WEXITSTATUS == 1
            virtiofsd_killed: Arc::new(AtomicBool::new(false)),
            spawned: Arc::new(Mutex::new(0)),
        };
        let err = run_async_with(&spawner, spec(d.path()), env, &fast())
            .await
            .expect_err("a failed VMM must surface as an error");
        assert!(
            format!("{err:#}").contains("cloud-hypervisor exited unsuccessfully"),
            "got {err:#}"
        );
    }
}
