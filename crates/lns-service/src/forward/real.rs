use std::collections::HashMap;
use std::net::SocketAddr;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::sync::Mutex;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;

use super::{AcceptAction, ForwardError, ForwardSpec, PortForwarder, classify_accept_error};
use crate::run_registry;
use lns_ipc::Protocol;
use lns_session::{FORWARD_PORT, ForwardHeader, encode_frame};

const ACCEPT_BACKOFF: Duration = Duration::from_millis(20);

type VsockStream = tokio::net::UnixStream;

pub struct VsockForwarder {
    run_id: String,
    tasks: Mutex<HashMap<SocketAddr, JoinHandle<()>>>,
}

impl VsockForwarder {
    pub fn new(run_id: String) -> Self {
        Self {
            run_id,
            tasks: Mutex::new(HashMap::new()),
        }
    }
}

impl PortForwarder for VsockForwarder {
    fn bind(&self, spec: &ForwardSpec) -> Result<(), ForwardError> {
        if spec.protocol != Protocol::Tcp {
            return Err(ForwardError::Other(
                "udp publishing is not yet supported".into(),
            ));
        }
        let std_listener = std::net::TcpListener::bind(spec.bind).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AddrInUse {
                ForwardError::AddrInUse(spec.bind)
            } else {
                ForwardError::Other(format!("binding {}: {e}", spec.bind))
            }
        })?;
        std_listener
            .set_nonblocking(true)
            .map_err(|e| ForwardError::Other(e.to_string()))?;
        let listener = tokio::net::TcpListener::from_std(std_listener)
            .map_err(|e| ForwardError::Other(e.to_string()))?;
        let run_id = self.run_id.clone();
        let container_port = spec.container_port;
        let task = tokio::spawn(accept_loop(listener, run_id, container_port));
        self.tasks
            .lock()
            .expect("tasks poisoned")
            .insert(spec.bind, task);
        Ok(())
    }

    fn unbind(&self, bind: SocketAddr) {
        if let Some(task) = self.tasks.lock().expect("tasks poisoned").remove(&bind) {
            task.abort();
        }
    }
}

async fn accept_loop(listener: tokio::net::TcpListener, run_id: String, container_port: u16) {
    while let Some(stream) = next_connection(&listener).await {
        let run_id = run_id.clone();
        tokio::spawn(async move {
            if let Err(e) = bridge_one(stream, &run_id, container_port).await {
                crate::log::debug!("port-forward connection ended: {e}");
            }
        });
    }
}

async fn next_connection(listener: &tokio::net::TcpListener) -> Option<tokio::net::TcpStream> {
    loop {
        let e = match listener.accept().await {
            Ok((stream, _peer)) => return Some(stream),
            Err(e) => e,
        };
        match classify_accept_error(&e) {
            AcceptAction::Stop => {
                crate::log::warn!("port-forward listener stopping: {e}");
                return None;
            }
            AcceptAction::Backoff => {
                crate::log::debug!("port-forward accept hit resource pressure, backing off: {e}");
                tokio::time::sleep(ACCEPT_BACKOFF).await;
            }
            AcceptAction::Retry => {
                crate::log::debug!("port-forward accept failed, retrying: {e}");
            }
        }
    }
}

async fn bridge_one(
    mut host: tokio::net::TcpStream,
    run_id: &str,
    container_port: u16,
) -> anyhow::Result<()> {
    let connector = run_registry::connector(run_id)
        .ok_or_else(|| anyhow::anyhow!("no vsock connector for run {run_id}"))?;
    let fd = connector
        .connect(FORWARD_PORT, Duration::from_secs(30))
        .await?;
    let mut guest = vsock_stream_from_fd(fd)?;
    guest
        .write_all(&encode_frame(&ForwardHeader { container_port })?)
        .await?;
    tokio::io::copy_bidirectional(&mut host, &mut guest).await?;
    Ok(())
}

fn vsock_stream_from_fd(fd: RawFd) -> anyhow::Result<VsockStream> {
    // SAFETY: connect() transfers ownership of the AF_VSOCK fd; OwnedFd closes it on drop.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let std_stream = std::os::unix::net::UnixStream::from(owned);
    std_stream.set_nonblocking(true)?;
    Ok(VsockStream::from_std(std_stream)?)
}
