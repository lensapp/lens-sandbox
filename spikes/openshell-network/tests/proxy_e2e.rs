use std::sync::{Arc, atomic::AtomicU32};
use std::time::Duration;

use lns_openshell_spike::Gate;
use openshell_core::policy::ProxyPolicy;
use openshell_supervisor_network::{identity::BinaryIdentityCache, proxy::ProxyHandle};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};

#[tokio::test]
async fn real_connect_waits_for_lns_decision_before_returning_denial() {
    let (tx, mut pending) = mpsc::unbounded_channel();
    let gate = Gate::new(tx).unwrap();
    let (_ready, ready) = watch::channel(true);
    let proxy = ProxyHandle::start_with_bind_addr(
        &ProxyPolicy {
            http_addr: Some("127.0.0.1:0".parse().unwrap()),
        },
        None,
        gate.engine.clone(),
        Arc::new(BinaryIdentityCache::new()),
        Arc::new(AtomicU32::new(std::process::id())),
        None,
        None,
        None,
        None,
        None,
        ready,
        &Default::default(),
    )
    .await
    .unwrap();
    let mut socket = TcpStream::connect(proxy.local_addr().unwrap())
        .await
        .unwrap();
    socket
        .write_all(b"CONNECT api.example.com:443 HTTP/1.1\r\nHost: api.example.com:443\r\n\r\n")
        .await
        .unwrap();
    let mut response = [0; 1024];
    let request = tokio::select! {
        request = pending.recv() => request.expect("approval channel remains open"),
        result = socket.read(&mut response) => panic!("proxy completed before approval: {result:?}: {}", String::from_utf8_lossy(&response)),
        () = tokio::time::sleep(Duration::from_secs(5)) => panic!("proxy did not request approval"),
    };
    assert_eq!(request.host, "api.example.com");
    assert_eq!(request.port, 443);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), socket.read(&mut response))
            .await
            .is_err()
    );
    request.answer.send(false).unwrap();
    let count = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&response[..count]).starts_with("HTTP/1.1 403"));
}
