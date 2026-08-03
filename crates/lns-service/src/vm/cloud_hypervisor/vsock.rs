#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::future::Future;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::future::BoxFuture;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use super::real::into_blocking_fd;
use crate::vm::connect::{ConnectOnce, connect_with};
use crate::vm::{GuestDialer, GuestTransport};

const MAX_REPLY_LINE: usize = 64;

const STOP_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) struct HybridVsockConnector {
    socket: PathBuf,
    api: PathBuf,
}

impl HybridVsockConnector {
    pub(crate) fn new(socket: impl Into<PathBuf>, api: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            api: api.into(),
        }
    }
}

impl GuestDialer for HybridVsockConnector {
    fn connect(&self, port: u32, timeout: Duration) -> BoxFuture<'_, Result<RawFd>> {
        Box::pin(connect_with(self, port, timeout))
    }
}

impl GuestTransport for HybridVsockConnector {
    fn request_stop(&self) {
        use std::io::Write;
        use std::os::unix::net::UnixStream as StdUnixStream;
        let Ok(mut stream) = StdUnixStream::connect(&self.api) else {
            return;
        };
        let _ = stream.set_write_timeout(Some(STOP_TIMEOUT));
        let _ = stream.write_all(
            b"PUT /api/v1/vm.shutdown HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n",
        );
    }
}

impl ConnectOnce for HybridVsockConnector {
    fn connect_once(&self, port: u32) -> impl Future<Output = Result<RawFd>> + Send {
        let socket = self.socket.clone();
        async move {
            let mut stream = UnixStream::connect(&socket).await.with_context(|| {
                format!("connecting to cloud-hypervisor vsock {}", socket.display())
            })?;
            stream
                .write_all(format!("CONNECT {port}\n").as_bytes())
                .await
                .context("writing hybrid-vsock CONNECT")?;
            let line = read_reply_line(&mut stream).await?;
            if !line.starts_with("OK") {
                bail!("cloud-hypervisor refused CONNECT {port}: {line:?}");
            }
            into_blocking_fd(stream)
        }
    }
}

async fn read_reply_line(stream: &mut UnixStream) -> Result<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream
            .read(&mut byte)
            .await
            .context("reading hybrid-vsock reply")?;
        if n == 0 || byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
        if buf.len() > MAX_REPLY_LINE {
            bail!("hybrid-vsock reply line exceeded {MAX_REPLY_LINE} bytes");
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

pub(crate) fn guest_listener_path(vsock_socket: &Path, port: u32) -> PathBuf {
    let mut s = vsock_socket.as_os_str().to_os_string();
    s.push(format!("_{port}"));
    PathBuf::from(s)
}

pub(crate) fn bind_guest_listener(vsock_socket: &Path, port: u32) -> Result<UnixListener> {
    let path = guest_listener_path(vsock_socket, port);
    let _ = std::fs::remove_file(&path);
    UnixListener::bind(&path)
        .with_context(|| format!("binding guest-vsock listener {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::super::real::spawn_accept_loop;
    use super::*;
    use std::io::Read;
    use std::os::fd::FromRawFd;
    use std::os::unix::net::UnixStream as StdUnixStream;

    fn temp_socket(name: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(name);
        (dir, path)
    }

    #[test]
    fn guest_listener_path_appends_underscore_port() {
        assert_eq!(
            guest_listener_path(Path::new("/runs/7/vsock.sock"), 1024),
            PathBuf::from("/runs/7/vsock.sock_1024")
        );
    }

    #[tokio::test]
    async fn connect_once_handshakes_and_preserves_post_ok_payload() {
        let (_dir, socket) = temp_socket("vsock.sock");
        let listener = UnixListener::bind(&socket).unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let req = read_reply_line(&mut stream).await.unwrap();
            assert_eq!(req, "CONNECT 1029");
            stream.write_all(b"OK 1029\n").await.unwrap();
            stream.write_all(b"broker-hello").await.unwrap();
            stream.flush().await.unwrap();
            // Hold the connection open until the client has read.
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        let connector = HybridVsockConnector::new(&socket, &socket);
        let fd = connector
            .connect(1029, Duration::from_secs(2))
            .await
            .unwrap();

        let payload = tokio::task::spawn_blocking(move || {
            // SAFETY: the connector/listener transferred ownership of this fd, so reconstructing the stream here is the sole owner.
            let mut s = unsafe { StdUnixStream::from_raw_fd(fd) };
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut buf = [0u8; 12];
            s.read_exact(&mut buf).unwrap();
            buf.to_vec()
        })
        .await
        .unwrap();

        assert_eq!(
            &payload, b"broker-hello",
            "bytes after the OK line must not be swallowed by reply parsing"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connect_once_errors_when_the_guest_port_is_refused() {
        let (_dir, socket) = temp_socket("vsock.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_reply_line(&mut stream).await;
            // Guest not listening: CH closes without an OK.
            drop(stream);
        });

        let connector = HybridVsockConnector::new(&socket, &socket);
        let err = connector.connect_once(1029).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("refused CONNECT 1029"),
            "got {err:#}"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connect_once_errors_on_an_overlong_reply_line() {
        let (_dir, socket) = temp_socket("vsock.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_reply_line(&mut stream).await;
            // A reply with no newline, longer than MAX_REPLY_LINE, must be rejected.
            stream.write_all(&[b'x'; 128]).await.unwrap();
            stream.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        });

        let connector = HybridVsockConnector::new(&socket, &socket);
        let err = connector.connect_once(1029).await.unwrap_err();
        assert!(format!("{err:#}").contains("exceeded"), "got {err:#}");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connect_once_errors_when_the_socket_is_absent() {
        let (_dir, socket) = temp_socket("missing.sock");
        let connector = HybridVsockConnector::new(&socket, &socket);
        let err = connector.connect_once(1029).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("connecting to cloud-hypervisor vsock"),
            "got {err:#}"
        );
    }

    #[tokio::test]
    async fn guest_listener_forwards_each_accepted_connection_as_a_usable_fd() {
        let (_dir, socket) = temp_socket("vsock.sock");
        let listener = bind_guest_listener(&socket, 1024).unwrap();
        let listen_path = guest_listener_path(&socket, 1024);
        assert!(listen_path.exists(), "listener socket must be created");

        let (fd_tx, mut fd_rx) = tokio::sync::mpsc::unbounded_channel::<RawFd>();
        spawn_accept_loop(listener, fd_tx);

        // Simulate CH dialing the host port on the guest's behalf.
        let client = tokio::spawn(async move {
            let mut s = UnixStream::connect(&listen_path).await.unwrap();
            s.write_all(b"guest-says-hi").await.unwrap();
            s.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        let fd = tokio::time::timeout(Duration::from_secs(2), fd_rx.recv())
            .await
            .expect("an fd should arrive")
            .expect("channel open");

        let got = tokio::task::spawn_blocking(move || {
            // SAFETY: the connector/listener transferred ownership of this fd, so reconstructing the stream here is the sole owner.
            let mut s = unsafe { StdUnixStream::from_raw_fd(fd) };
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut buf = [0u8; 13];
            s.read_exact(&mut buf).unwrap();
            buf.to_vec()
        })
        .await
        .unwrap();

        assert_eq!(&got, b"guest-says-hi");
        client.await.unwrap();
    }

    #[tokio::test]
    async fn accept_loop_exits_when_the_receiver_is_dropped() {
        let (_dir, socket) = temp_socket("vsock.sock");
        let listener = bind_guest_listener(&socket, 1030).unwrap();
        let listen_path = guest_listener_path(&socket, 1030);
        let (fd_tx, fd_rx) = tokio::sync::mpsc::unbounded_channel::<RawFd>();
        drop(fd_rx);
        spawn_accept_loop(listener, fd_tx);

        // A connection after the receiver is gone makes the loop send-fail and stop.
        let mut s = UnixStream::connect(&listen_path).await.unwrap();
        s.write_all(b"x").await.unwrap();
        s.flush().await.unwrap();
        // Nothing to assert beyond "no panic / no hang"; the loop breaks on send error.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[test]
    fn request_stop_sends_a_graceful_shutdown_to_the_api_socket() {
        use std::io::Read as _;
        use std::os::unix::net::UnixListener as StdUnixListener;

        let dir = tempfile::TempDir::new().unwrap();
        let api = dir.path().join("cloud-hypervisor.sock");
        let listener = StdUnixListener::bind(&api).unwrap();

        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 256];
            let n = conn.read(&mut buf).unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });

        let connector = HybridVsockConnector::new(dir.path().join("vsock.sock"), &api);
        connector.request_stop();

        let request = server.join().unwrap();
        assert!(
            request.contains("PUT /api/v1/vm.shutdown"),
            "request_stop must ask cloud-hypervisor to shut down gracefully: {request:?}"
        );
    }

    #[test]
    fn request_stop_is_silent_when_the_api_socket_is_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let connector = HybridVsockConnector::new(
            dir.path().join("vsock.sock"),
            dir.path().join("nonexistent.sock"),
        );
        // No listener bound: a failed connect must not panic.
        connector.request_stop();
    }
}
