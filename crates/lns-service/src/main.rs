use std::sync::Arc;
use std::thread;
use std::time::Instant;

use lns_service::{
    approval_flow::window::{self as approval_window, WindowState},
    credential_flow, ipc, log, paths,
    shutdown::Shutdown,
    tray,
};

fn main() -> anyhow::Result<()> {
    log::init();

    let socket = paths::socket_path()?;
    paths::ensure_parent_dir(&socket)?;

    let shutdown = Arc::new(Shutdown::new());
    let started_at = Instant::now();

    log::info!("Starting", "lns-service (socket: {})", socket.display());

    credential_flow::backend::install(lns_policy::keychain::select_credential_store(|| {
        lns_policy::keychain::real::KeyringBlob::open()
            .map(|blob| Arc::new(blob) as Arc<dyn lns_policy::keychain::KeychainBlob>)
    }));

    let window_state = WindowState::new();
    approval_window::install(window_state.clone());

    let ipc_shutdown = shutdown.clone();
    let ipc_socket = socket.clone();
    let ipc_handle = thread::spawn(move || run_ipc_runtime(ipc_socket, ipc_shutdown, started_at));

    if tray::display_present() {
        tray::run_tray(shutdown, ipc_handle, window_state)
    } else {
        tray::run_headless(shutdown, ipc_handle)
    }
}

fn run_ipc_runtime(
    socket: std::path::PathBuf,
    shutdown: Arc<Shutdown>,
    started_at: Instant,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        spawn_signal_listener(shutdown.clone());
        tokio::spawn(lns_service::update_check::run_periodic(shutdown.clone()));

        let result = ipc::run_server(socket, shutdown.clone(), started_at).await;
        shutdown.signal();
        result
    })
}

fn spawn_signal_listener(shutdown: Arc<Shutdown>) {
    use tokio::signal::unix::{SignalKind, signal};

    tokio::spawn(async move {
        let sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => Some(s),
            Err(e) => {
                log::error!("failed to install SIGTERM handler: {e}");
                None
            }
        };
        let sigint = match signal(SignalKind::interrupt()) {
            Ok(s) => Some(s),
            Err(e) => {
                log::error!("failed to install SIGINT handler: {e}");
                None
            }
        };

        match (sigterm, sigint) {
            (Some(mut term), Some(mut int)) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = int.recv() => {}
                }
            }
            (Some(mut term), None) => {
                let _ = term.recv().await;
            }
            (None, Some(mut int)) => {
                let _ = int.recv().await;
            }
            (None, None) => {
                log::error!(
                    "no SIGTERM/SIGINT handler could be installed; \
                     service can only be stopped via the tray Quit menu \
                     or `lns service stop`."
                );
                return;
            }
        }

        shutdown.signal();
    });
}
