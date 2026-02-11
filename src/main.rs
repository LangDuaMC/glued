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

#[derive(Debug, Clone)]
enum Role {
    Main,
    Replica(String),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    env_logger::init();

    // Load configuration
    let mut cfg = Config::load()?;

    // If a network name is provided, act as a replica (watch containers and gossip);
    // otherwise run as the main instance (DNS + registry only).
    let role = match cfg.network_name.clone() {
        Some(network) => Role::Replica(network),
        None => Role::Main,
    };
    let role_label = match &role {
        Role::Main => "main",
        Role::Replica(_) => "replica",
    };
    info!("Running as {} role", role_label);

    // Replica-specific logic
    if let Role::Replica(_) = role {
        // Infer peers from Docker DNS if a bootstrap service name is provided
        if let Some(service_name) = cfg.bootstrap_service.clone() {
            info!(
                "Attempting to discover bootstrap peers for service '{}' via Docker DNS.",
                service_name
            );
            // Use hickory-resolver (previously trust-dns-resolver)
            use hickory_resolver::config::{ResolverConfig, ResolverOpts};
            use hickory_resolver::TokioAsyncResolver;

            // Create a resolver with default configuration
            let resolver =
                TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default());

            // The hostname for a Docker Swarm service's tasks is `tasks.<service_name>`
            let lookup_name = format!("tasks.{}", service_name);

            match resolver.lookup_ip(lookup_name.clone()).await {
                Ok(response) => {
                    let discovered_peers: Vec<String> =
                        response.iter().map(|ip| ip.to_string()).collect();
                    if discovered_peers.is_empty() {
                        info!(
                            "DNS lookup for '{}' was successful but returned no IPs.",
                            lookup_name
                        );
                    } else {
                        info!("Discovered bootstrap peers: {:?}", discovered_peers);
                        // Prioritise the main instance by inserting discovered peers first.
                        let mut ordered = discovered_peers;
                        ordered.extend(cfg.bootstrap_peers.clone());
                        cfg.bootstrap_peers = ordered;
                        // Remove duplicates
                        cfg.bootstrap_peers.sort();
                        cfg.bootstrap_peers.dedup();
                    }
                }
                Err(e) => {
                    error!(
                        "DNS lookup for '{}' failed: {}. Continuing with configured peers.",
                        lookup_name, e
                    );
                }
            }
        }
    }

    info!("Starting Glued daemon with config: {:?}", cfg);

    // Shared state
    let state: Arc<RwLock<HashMap<String, String>>> = Arc::new(RwLock::new(HashMap::new()));

    // Update channels
    let (local_update_tx, local_update_rx) = mpsc::channel(128);
    let (gossip_out_tx, gossip_out_rx) = mpsc::channel(128);
    let (gossip_in_tx, gossip_in_rx) = mpsc::channel(128);

    // Conditionally start the Container Runtime monitor for replicas
    let runtime_handle = if let Role::Replica(network_name) = role.clone() {
        info!("Starting container runtime monitor...");
        let runtime = DockerRuntime::new(network_name);
        let handle = tokio::spawn(async move {
            if let Err(e) = runtime.monitor(local_update_tx).await {
                error!("Container runtime failed: {}", e);
            }
        });
        Some(handle)
    } else {
        None
    };

    // Local registry updater: apply local discoveries and forward to gossip.
    let registry_for_local = Arc::clone(&state);
    let gossip_out_forward = gossip_out_tx.clone();
    let registry_local_handle = tokio::spawn(async move {
        let mut updates = local_update_rx;
        while let Some(update) = updates.recv().await {
            gossip::apply_update(update.clone(), &registry_for_local).await;
            if let Err(e) = gossip_out_forward.send(update).await {
                error!("Failed to forward update to gossip pipeline: {}", e);
                break;
            }
        }
    });

    // Remote registry updater: apply gossip results into local registry.
    let registry_for_remote = Arc::clone(&state);
    let registry_remote_handle = tokio::spawn(async move {
        let mut updates = gossip_in_rx;
        while let Some(update) = updates.recv().await {
            gossip::apply_update(update, &registry_for_remote).await;
        }
    });

    // Gossip Subsystem
    let topic_id = cfg.topic_id.clone();
    let bootstrap_peers = cfg.bootstrap_peers.clone();
    let cluster_secret = cfg.cluster_secret.clone();
    let gossip_handle = tokio::spawn(async move {
        if let Err(e) = run_gossip(
            topic_id,
            bootstrap_peers,
            gossip_out_rx,
            gossip_in_tx,
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
    if let Some(handle) = runtime_handle {
        handle.abort();
    }
    registry_local_handle.abort();
    registry_remote_handle.abort();
    gossip_handle.abort();
    dns_handle.abort();

    info!("Shutdown complete.");
    Ok(())
}
