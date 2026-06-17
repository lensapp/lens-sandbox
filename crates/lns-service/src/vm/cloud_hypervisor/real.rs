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
