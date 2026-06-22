#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::VmSpec;

mod launch;
mod orchestrate;
mod process;
mod real;
mod vmm_bin;
mod vsock;

use process::Child;

pub struct CloudHypervisor;

pub(super) async fn run_async_with<S: process::Spawner>(
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
    for child in &mut virtiofsd {
        let _ = child.start_kill();
    }
    if !status.success() {
        bail!("cloud-hypervisor exited unsuccessfully: {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::{ExecSpec, VmmBackend};
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
            workload_uid: Some(65534),
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

    fn socket_from_args(args: &[String]) -> PathBuf {
        args.iter()
            .find_map(|a| {
                if a.contains("cid=") {
                    a.find("socket=")
                        .map(|i| PathBuf::from(&a[i + "socket=".len()..]))
                } else {
                    a.strip_prefix("--socket-path=").map(PathBuf::from)
                }
            })
            .expect("fake spawner: every spawned VMM arg set carries a socket path")
    }

    impl Spawner for FakeSpawner {
        type Child = FakeChild;
        fn spawn(&self, _program: &Path, args: &[String]) -> std::io::Result<FakeChild> {
            *self.spawned.lock().unwrap() += 1;
            std::fs::write(socket_from_args(args), b"").unwrap();
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

    fn override_env(ch: PathBuf, vfsd: PathBuf) -> impl Fn(&str) -> Option<OsString> {
        let map: std::collections::HashMap<String, OsString> = [
            ("LNS_CLOUD_HYPERVISOR_BIN".to_string(), ch.into_os_string()),
            ("LNS_VIRTIOFSD_BIN".to_string(), vfsd.into_os_string()),
        ]
        .into_iter()
        .collect();
        move |k: &str| map.get(k).cloned()
    }

    fn idle_spawner() -> FakeSpawner {
        FakeSpawner {
            ch_exit_raw: 0,
            virtiofsd_killed: Arc::new(AtomicBool::new(false)),
            spawned: Arc::new(Mutex::new(0)),
        }
    }

    #[test]
    fn name_is_cloud_hypervisor() {
        assert_eq!(CloudHypervisor.name(), "cloud-hypervisor");
    }

    #[tokio::test]
    async fn run_bails_with_actionable_help_when_binaries_are_not_resolved() {
        let d = tempfile::TempDir::new().unwrap();
        let spawner = idle_spawner();
        let err = run_async_with(&spawner, spec(d.path()), |_| None, &fast())
            .await
            .expect_err("no LNS_*_BIN override and no published pin → error");
        assert_eq!(
            *spawner.spawned.lock().unwrap(),
            0,
            "resolution must fail before any process is spawned"
        );
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
        let env = override_env(ch.path().to_path_buf(), vfsd.path().to_path_buf());
        let spawner = idle_spawner();
        let virtiofsd_killed = spawner.virtiofsd_killed.clone();
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
        let env = override_env(ch.path().to_path_buf(), vfsd.path().to_path_buf());
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

    #[tokio::test]
    async fn run_surfaces_the_connector_through_the_spec_channel() {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(d.path().join("disk")).unwrap();
        let ch = tempfile::NamedTempFile::new().unwrap();
        let vfsd = tempfile::NamedTempFile::new().unwrap();
        let env = override_env(ch.path().to_path_buf(), vfsd.path().to_path_buf());
        let (connector_tx, connector_rx) =
            tokio::sync::oneshot::channel::<Arc<dyn crate::vm::GuestTransport>>();
        let mut s = spec(d.path());
        s.connector_tx = Some(connector_tx);
        let spawner = FakeSpawner {
            ch_exit_raw: 0,
            virtiofsd_killed: Arc::new(AtomicBool::new(false)),
            spawned: Arc::new(Mutex::new(0)),
        };
        run_async_with(&spawner, s, env, &fast())
            .await
            .expect("clean boot+exit");
        assert!(
            connector_rx.await.is_ok(),
            "the orchestrator must receive the guest connector"
        );
    }
}
