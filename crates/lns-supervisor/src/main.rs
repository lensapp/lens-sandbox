mod config;
mod dispatcher;

use std::io::IsTerminal;
use std::sync::Arc;

use lens_sandbox_core::config::SandboxMode;
use lens_sandbox_core::lifecycle::OrphanReaper;
use lens_sandbox_core::network;
use lens_sandbox_core::privilege;
use lens_sandbox_core::proxy;

/// Resolve the sandbox user for privilege drop, or `None` to run the agent as root with capabilities dropped.
fn resolve_agent_creds(fallback_username: &str) -> Option<privilege::SandboxCredentials> {
    match privilege::SandboxCredentials::resolve(fallback_username) {
        Ok(Some(creds)) => {
            tracing::info!(
                user = fallback_username,
                "resolved sandbox uid/gid for privilege drop"
            );
            Some(creds)
        }
        Ok(None) => {
            tracing::info!(
                user = fallback_username,
                "sandbox user absent in image — agent will run as root with capabilities dropped"
            );
            None
        }
        Err(e) => {
            tracing::error!("sandbox user lookup failed: {e}");
            std::process::exit(1);
        }
    }
}

#[tokio::main]
async fn main() {
    // Install the ring crypto provider for rustls (needed for MITM TLS).
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Suppress tracing on a real terminal so it doesn't corrupt the agent's TUI output.
    let default_level = if std::io::stdin().is_terminal() {
        "off"
    } else {
        "info"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_level)),
        )
        .with_target(false)
        .init();

    // Make the supervisor non-dumpable before loading any token or policy material so the agent can't ptrace it.
    if let Err(e) = privilege::harden_supervisor() {
        tracing::error!(
            "PR_SET_DUMPABLE failed; refusing to run with the supervisor ptrace-readable: {e}"
        );
        std::process::exit(1);
    }

    // Reap orphans reparented to PID 1; agent sessions register their protected PID via the dispatcher.
    let reaper = Arc::new(OrphanReaper::spawn());

    let config = Arc::new(config::load_config());

    // lns only drives server mode over the vsock relay; fail loud if a policy file flips the mode.
    let SandboxMode::Server { ws_url, .. } = &config.core.mode else {
        tracing::error!("lns-supervisor supports server mode only");
        std::process::exit(1);
    };
    tracing::info!(
        ws_url = %ws_url,
        workspace = ?config.workspace_path,
        agent_command = %config.agent_command,
        idle_timeout_s = config.core.idle_timeout.map(|d| d.as_secs()),
        drop_privileges = config.core.is_root,
        "starting agent sandbox (server mode)"
    );

    let sandbox_creds = if config.core.is_root {
        resolve_agent_creds(&config.core.sandbox_user)
    } else {
        None
    };

    let proxy_state = if config.core.is_root {
        tracing::info!("running as root — enabling proxy enforcement + privilege drop");

        if let Err(e) = network::install_network_lockdown() {
            tracing::error!("Failed to install network lockdown: {e}");
            std::process::exit(1);
        }

        let (proxy_server, state) = proxy::ProxyServer::new(
            config.core.proxy_listen_addr,
            config.core.transparent_listen_addr,
            config.core.dns_stub_listen_addr,
            sandbox_creds.clone(),
            lens_sandbox_core::config::bootstrap_dns_hosts(&config.core.mode),
        );

        tokio::spawn(async move {
            if let Err(e) = proxy_server.run().await {
                tracing::error!("Proxy server failed: {e}");
                std::process::exit(1);
            }
        });

        Some(state)
    } else {
        tracing::warn!(
            "supervisor is not running as root — no nftables cage, proxy, or privilege drop installed; the agent has unrestricted network access"
        );
        None
    };

    let has_proxy = proxy_state.is_some();

    let dispatcher =
        dispatcher::AgentDispatcher::new(config.clone(), sandbox_creds, reaper.clone());
    lens_sandbox_core::client::run(&config.core, &proxy_state, &dispatcher).await;

    if has_proxy {
        network::cleanup_network_lockdown();
    }
}
