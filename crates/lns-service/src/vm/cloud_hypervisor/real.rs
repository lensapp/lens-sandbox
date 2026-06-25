#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::os::fd::{IntoRawFd, RawFd};

use anyhow::{Context, Result};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc::UnboundedSender;

use super::{CloudHypervisor, orchestrate, process, run_async_with};
use crate::log;
use crate::vm::{VmSpec, VmmBackend};

pub(super) fn into_blocking_fd(stream: UnixStream) -> Result<RawFd> {
    let std_stream = stream.into_std().context("UnixStream into_std")?;
    std_stream
        .set_nonblocking(false)
        .context("clearing O_NONBLOCK on vsock fd")?;
    Ok(std_stream.into_raw_fd())
}

pub(super) fn spawn_accept_loop(listener: UnixListener, fd_tx: UnboundedSender<RawFd>) {
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => match into_blocking_fd(stream) {
                    Ok(fd) => {
                        if fd_tx.send(fd).is_err() {
                            break;
                        }
                    }
                    Err(e) => log::warn!("dropping guest vsock connection: {e:#}"),
                },
                Err(e) => {
                    log::warn!("guest vsock listener accept failed: {e:#}");
                    break;
                }
            }
        }
    });
}

fn real_kvm_check() -> super::KvmStatus {
    use std::fs::OpenOptions;
    use std::path::Path;

    let path = Path::new("/dev/kvm");
    if !path.exists() {
        return super::KvmStatus::Missing;
    }
    match OpenOptions::new().read(true).write(true).open(path) {
        Ok(_) => super::KvmStatus::Accessible,
        Err(_) => super::KvmStatus::NotAccessible,
    }
}

fn real_read_console_tail(path: &std::path::Path) -> Option<String> {
    use std::io::Read;

    const MAX_TAIL_BYTES: usize = 4096;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    if buf.is_empty() {
        return None;
    }
    Some(super::truncate_tail(&buf, MAX_TAIL_BYTES))
}

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
            real_kvm_check,
            real_read_console_tail,
            &orchestrate::LaunchTimeouts::default(),
        ))
    }
}
