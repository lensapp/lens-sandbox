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

#[derive(Debug, Clone, PartialEq)]
pub(super) enum KvmStatus {
    Accessible,
    Missing,
    NotAccessible,
}

pub(super) fn truncate_tail(content: &[u8], max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return String::from_utf8_lossy(content).into_owned();
    }
    let tail = &content[content.len() - max_bytes..];
    format!(
        "[…truncated, showing last {max_bytes} bytes…]\n{}",
        String::from_utf8_lossy(tail)
    )
}

pub(super) async fn run_async_with<S: process::Spawner>(
    spawner: &S,
    spec: VmSpec,
    env_get: impl Fn(&str) -> Option<OsString>,
    require_read_only_support: impl Fn(&Path) -> Result<()>,
    kvm_check: impl Fn() -> KvmStatus,
    read_console_tail: impl Fn(&Path) -> Option<String>,
    timeouts: &orchestrate::LaunchTimeouts,
) -> Result<()> {
    match kvm_check() {
        KvmStatus::Accessible => {}
        KvmStatus::Missing => {
            bail!(
                "/dev/kvm is not available. Load the KVM module \
                 (sudo modprobe kvm_intel || sudo modprobe kvm_amd) and ensure \
                 your user can access /dev/kvm (sudo usermod -aG kvm $USER, then re-login)."
            );
        }
        KvmStatus::NotAccessible => {
            bail!(
                "/dev/kvm is present but not read/writable. \
                 Add yourself to the kvm group: sudo usermod -aG kvm $USER \
                 (then re-login)."
            );
        }
    }
    let run_dir = spec
        .upper_disk
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let layout = launch::SocketLayout::for_run_dir(&run_dir);
    let bins = vmm_bin::resolve(env_get)?;
    require_read_only_support(&bins.virtiofsd)?;
    let guest_listeners: Vec<(u32, tokio::sync::mpsc::UnboundedSender<std::os::fd::RawFd>)> = spec
        .vsock
        .iter()
        .map(|c| (c.port, c.fd_tx.clone()))
        .collect();
    let running =
        orchestrate::launch(spawner, &spec, &bins, &layout, guest_listeners, timeouts).await?;
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
        let _ = child.wait().await;
    }
    if !status.success() {
        let tail_context = read_console_tail(&layout.console_log)
            .map(|t| format!("\n--- console.log tail ---\n{t}"))
            .unwrap_or_default();
        bail!("cloud-hypervisor exited unsuccessfully: {status}{tail_context}");
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
            run_id: "aa01".to_string(),
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
            workload_gid: Some(65534),
            debug: false,
            exec: ExecSpec::from_image_config(None, None, &["true".into()]),
            vsock: Vec::new(),
            no_nic: false,
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
        waited: Arc<AtomicBool>,
    }

    impl Child for FakeChild {
        async fn wait(&mut self) -> std::io::Result<ExitStatus> {
            self.waited.store(true, Ordering::SeqCst);
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
        virtiofsd_waited: Arc<AtomicBool>,
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
                    waited: self.virtiofsd_waited.clone(),
                })
            } else {
                Ok(FakeChild {
                    exit_raw: self.ch_exit_raw,
                    killed: Arc::new(AtomicBool::new(false)),
                    waited: Arc::new(AtomicBool::new(false)),
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
            virtiofsd_waited: Arc::new(AtomicBool::new(false)),
            spawned: Arc::new(Mutex::new(0)),
        }
    }

    fn kvm_ok() -> KvmStatus {
        KvmStatus::Accessible
    }

    fn no_console_tail(_: &Path) -> Option<String> {
        None
    }

    fn readonly_support(_: &Path) -> Result<()> {
        Ok(())
    }

    #[test]
    fn name_is_cloud_hypervisor() {
        assert_eq!(CloudHypervisor.name(), "cloud-hypervisor");
    }

    #[tokio::test]
    async fn run_bails_with_actionable_help_when_kvm_is_missing() {
        let d = tempfile::TempDir::new().unwrap();
        let spawner = idle_spawner();
        let err = run_async_with(
            &spawner,
            spec(d.path()),
            |_| None,
            readonly_support,
            || KvmStatus::Missing,
            no_console_tail,
            &fast(),
        )
        .await
        .expect_err("missing /dev/kvm must fail before any spawn");
        assert_eq!(
            *spawner.spawned.lock().unwrap(),
            0,
            "no processes may be spawned when kvm is missing"
        );
        let msg = format!("{err:#}");
        assert!(msg.contains("/dev/kvm is not available"), "got {msg}");
        assert!(
            msg.contains("sudo modprobe kvm_intel || sudo modprobe kvm_amd"),
            "got {msg}"
        );
        assert!(msg.contains("sudo usermod -aG kvm $USER"), "got {msg}");
    }

    #[tokio::test]
    async fn run_bails_with_actionable_help_when_kvm_is_not_accessible() {
        let d = tempfile::TempDir::new().unwrap();
        let spawner = idle_spawner();
        let err = run_async_with(
            &spawner,
            spec(d.path()),
            |_| None,
            readonly_support,
            || KvmStatus::NotAccessible,
            no_console_tail,
            &fast(),
        )
        .await
        .expect_err("inaccessible /dev/kvm must fail before any spawn");
        assert_eq!(
            *spawner.spawned.lock().unwrap(),
            0,
            "no processes may be spawned when kvm is not accessible"
        );
        let msg = format!("{err:#}");
        assert!(
            msg.contains("/dev/kvm is present but not read/writable"),
            "got {msg}"
        );
        assert!(msg.contains("sudo usermod -aG kvm $USER"), "got {msg}");
    }

    #[tokio::test]
    async fn run_bails_with_actionable_help_when_binaries_are_not_resolved() {
        let d = tempfile::TempDir::new().unwrap();
        let spawner = idle_spawner();
        let err = run_async_with(
            &spawner,
            spec(d.path()),
            |_| None,
            readonly_support,
            kvm_ok,
            no_console_tail,
            &fast(),
        )
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
    async fn run_refuses_a_virtiofsd_without_read_only_share_support() {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(d.path().join("disk")).unwrap();
        let ch = tempfile::NamedTempFile::new().unwrap();
        let vfsd = tempfile::NamedTempFile::new().unwrap();
        let env = override_env(ch.path().to_path_buf(), vfsd.path().to_path_buf());
        let spawner = idle_spawner();
        let err = run_async_with(
            &spawner,
            spec(d.path()),
            env,
            |path| {
                vmm_bin::require_read_only_support(path, |_| {
                    Ok(b"Usage: virtiofsd [OPTIONS]".to_vec())
                })
            },
            kvm_ok,
            no_console_tail,
            &fast(),
        )
        .await
        .expect_err("a writable-only virtiofsd must be refused before boot");

        assert_eq!(
            *spawner.spawned.lock().unwrap(),
            0,
            "an unsupported virtiofsd must be refused before any process starts"
        );
        let msg = format!("{err:#}");
        assert!(
            msg.contains("does not support read-only shares"),
            "got {msg}"
        );
        assert!(msg.contains("LNS_VIRTIOFSD_BIN"), "got {msg}");
    }

    #[tokio::test]
    async fn run_reports_when_virtiofsd_capabilities_cannot_be_checked() {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(d.path().join("disk")).unwrap();
        let ch = tempfile::NamedTempFile::new().unwrap();
        let vfsd = tempfile::NamedTempFile::new().unwrap();
        let vfsd_path = vfsd.path().display().to_string();
        let env = override_env(ch.path().to_path_buf(), vfsd.path().to_path_buf());
        let spawner = idle_spawner();
        let err = run_async_with(
            &spawner,
            spec(d.path()),
            env,
            |path| {
                vmm_bin::require_read_only_support(path, |_| {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "denied",
                    ))
                })
            },
            kvm_ok,
            no_console_tail,
            &fast(),
        )
        .await
        .expect_err("an unreadable virtiofsd must be refused before boot");

        assert_eq!(*spawner.spawned.lock().unwrap(), 0);
        let msg = format!("{err:#}");
        assert!(
            msg.contains(&format!("checking virtiofsd capabilities at {vfsd_path}")),
            "got {msg}"
        );
        assert!(msg.contains("denied"), "got {msg}");
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
        let virtiofsd_waited = spawner.virtiofsd_waited.clone();
        run_async_with(
            &spawner,
            spec(d.path()),
            env,
            readonly_support,
            kvm_ok,
            no_console_tail,
            &fast(),
        )
        .await
        .expect("clean boot+exit");
        assert!(
            virtiofsd_killed.load(Ordering::SeqCst),
            "virtiofsd must be torn down once cloud-hypervisor exits"
        );
        assert!(
            virtiofsd_waited.load(Ordering::SeqCst),
            "virtiofsd must be waited (reaped) after kill to avoid zombies"
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
            virtiofsd_waited: Arc::new(AtomicBool::new(false)),
            spawned: Arc::new(Mutex::new(0)),
        };
        let err = run_async_with(
            &spawner,
            spec(d.path()),
            env,
            readonly_support,
            kvm_ok,
            no_console_tail,
            &fast(),
        )
        .await
        .expect_err("a failed VMM must surface as an error");
        assert!(
            format!("{err:#}").contains("cloud-hypervisor exited unsuccessfully"),
            "got {err:#}"
        );
    }

    #[tokio::test]
    async fn run_includes_console_tail_on_nonzero_exit() {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(d.path().join("disk")).unwrap();
        let ch = tempfile::NamedTempFile::new().unwrap();
        let vfsd = tempfile::NamedTempFile::new().unwrap();
        let env = override_env(ch.path().to_path_buf(), vfsd.path().to_path_buf());
        let spawner = FakeSpawner {
            ch_exit_raw: 256,
            virtiofsd_killed: Arc::new(AtomicBool::new(false)),
            virtiofsd_waited: Arc::new(AtomicBool::new(false)),
            spawned: Arc::new(Mutex::new(0)),
        };
        let err = run_async_with(
            &spawner,
            spec(d.path()),
            env,
            readonly_support,
            kvm_ok,
            |_| Some("Error: KvmCheck failed".to_string()),
            &fast(),
        )
        .await
        .expect_err("non-zero exit must bail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("cloud-hypervisor exited unsuccessfully"),
            "got {msg}"
        );
        assert!(msg.contains("console.log tail"), "got {msg}");
        assert!(msg.contains("Error: KvmCheck failed"), "got {msg}");
    }

    #[tokio::test]
    async fn run_nonzero_exit_without_console_tail_still_reports_base_error() {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(d.path().join("disk")).unwrap();
        let ch = tempfile::NamedTempFile::new().unwrap();
        let vfsd = tempfile::NamedTempFile::new().unwrap();
        let env = override_env(ch.path().to_path_buf(), vfsd.path().to_path_buf());
        let spawner = FakeSpawner {
            ch_exit_raw: 256,
            virtiofsd_killed: Arc::new(AtomicBool::new(false)),
            virtiofsd_waited: Arc::new(AtomicBool::new(false)),
            spawned: Arc::new(Mutex::new(0)),
        };
        let err = run_async_with(
            &spawner,
            spec(d.path()),
            env,
            readonly_support,
            kvm_ok,
            no_console_tail,
            &fast(),
        )
        .await
        .expect_err("non-zero exit must bail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("cloud-hypervisor exited unsuccessfully"),
            "got {msg}"
        );
        assert!(
            !msg.contains("console.log tail"),
            "no console content → no tail section: {msg}"
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
            virtiofsd_waited: Arc::new(AtomicBool::new(false)),
            spawned: Arc::new(Mutex::new(0)),
        };
        run_async_with(
            &spawner,
            s,
            env,
            readonly_support,
            kvm_ok,
            no_console_tail,
            &fast(),
        )
        .await
        .expect("clean boot+exit");
        assert!(
            connector_rx.await.is_ok(),
            "the orchestrator must receive the guest connector"
        );
    }

    #[test]
    fn truncate_tail_returns_full_content_when_short() {
        let data = b"short content";
        let result = truncate_tail(data, 4096);
        assert_eq!(result, "short content");
    }

    #[test]
    fn truncate_tail_truncates_large_content() {
        let data = vec![b'x'; 8192];
        let result = truncate_tail(&data, 4096);
        assert!(result.contains("[…truncated, showing last 4096 bytes…]"));
        assert!(result.ends_with(&"x".repeat(4096)));
    }

    #[test]
    fn truncate_tail_exact_boundary() {
        let data = vec![b'y'; 4096];
        let result = truncate_tail(&data, 4096);
        assert!(!result.contains("truncated"));
        assert_eq!(result, "y".repeat(4096));
    }
}
