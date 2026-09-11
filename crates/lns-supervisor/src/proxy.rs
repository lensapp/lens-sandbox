use lns_openshell_spike::{Error, Gate};
use openshell_supervisor_network::l7::tls::{
    CertCache, ProxyTlsState, SandboxCa, build_upstream_client_config, read_system_ca_bundle,
    write_ca_files,
};
use openshell_supervisor_network::{identity::BinaryIdentityCache, proxy::ProxyHandle};
use std::sync::{Arc, atomic::AtomicU32};

pub struct Proxy {
    _handle: ProxyHandle,
    pub ca_file_paths: (std::path::PathBuf, std::path::PathBuf),
}

pub async fn start(
    listener: std::net::TcpListener,
    gate: &Gate,
    pid: Arc<AtomicU32>,
) -> Result<Proxy, Error> {
    let ca = SandboxCa::generate().map_err(|error| error.to_string())?;
    let bundle = read_system_ca_bundle();
    let dir = std::path::Path::new(openshell_core::container_paths::TLS_ROOT);
    let ca_file_paths = write_ca_files(&ca, dir, &bundle).map_err(|error| error.to_string())?;
    let upstream = build_upstream_client_config(&bundle).map_err(|error| error.to_string())?;
    let tls = Arc::new(ProxyTlsState::new(CertCache::new(ca), upstream));
    let (_ready, ready_rx) = tokio::sync::watch::channel(true);
    let handle = ProxyHandle::start_with_listener(
        tokio::net::TcpListener::from_std(listener)?,
        gate.engine.clone(),
        Arc::new(BinaryIdentityCache::new()),
        pid,
        Some(tls),
        None,
        None,
        None,
        None,
        ready_rx,
        &Default::default(),
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(Proxy {
        _handle: handle,
        ca_file_paths,
    })
}
