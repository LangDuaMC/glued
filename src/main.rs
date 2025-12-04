//! Glued daemon entry point.

use std::collections::HashMap;
use std::sync::Arc;

use log::{error, info};
use tokio::signal;
use tokio::sync::{mpsc, RwLock};

mod config;
mod dns_server;
mod gossip;
mod runtime;
mod types;

use config::Config;
use dns_server::run_dns_server;
use gossip::run_gossip;
use runtime::{ContainerRuntime, DockerRuntime};
// use types::Update;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    env_logger::init();

    // Load configuration
    let cfg = Config::load()?;
    info!("Starting Glued daemon with config: {:?}", cfg);

    // Shared state
    let state: Arc<RwLock<HashMap<String, String>>> = Arc::new(RwLock::new(HashMap::new()));

    // Update channel
    let (update_tx, update_rx) = mpsc::channel(128);

    // Container Runtime (Docker)
    let runtime = DockerRuntime::new(cfg.network_name.clone());
    let runtime_handle = tokio::spawn(async move {
        if let Err(e) = runtime.monitor(update_tx).await {
            error!("Container runtime failed: {}", e);
        }
    });

    // Gossip Subsystem
    let state_for_gossip = Arc::clone(&state);
    let topic_id = cfg.topic_id.clone();
    let bootstrap_peers = cfg.bootstrap_peers.clone();
    let cluster_secret = cfg.cluster_secret.clone();
    let gossip_handle = tokio::spawn(async move {
        if let Err(e) = run_gossip(
            topic_id,
            bootstrap_peers,
            update_rx,
            state_for_gossip,
            cluster_secret,
        )
        .await
        {
            error!("Gossip subsystem failed: {}", e);
        }
    });

    // DNS Server
    let state_for_dns = Arc::clone(&state);
    let dns_bind = cfg.dns_bind;
    let dns_handle = tokio::spawn(async move {
        if let Err(e) = run_dns_server(dns_bind, state_for_dns).await {
            error!("DNS server failed: {}", e);
        }
    });

    // Graceful Shutdown
    match signal::ctrl_c().await {
        Ok(()) => {
            info!("Received Ctrl+C, shutting down...");
        }
        Err(err) => {
            error!("Unable to listen for shutdown signal: {}", err);
        }
    }

    // Abort tasks
    runtime_handle.abort();
    gossip_handle.abort();
    dns_handle.abort();

    info!("Shutdown complete.");
    Ok(())
}
