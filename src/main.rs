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
    let mut cfg = Config::load()?;

    // If a network name is provided, act as a replica (watch containers and gossip);
    // otherwise run in DNS-only mode.
    let is_replica = cfg.network_name.is_some();
    let role = if is_replica { "replica" } else { "dns-only" };
    info!("Running as {} role", role);

    // Replica-specific logic
    if is_replica {
        // Infer peers from Docker DNS if a bootstrap service name is provided
        if let Ok(service_name) = std::env::var("GLUED_BOOTSTRAP_SERVICE") {
            info!("Attempting to discover bootstrap peers for service '{}' via DNS.", service_name);
            // Use hickory-resolver (previously trust-dns-resolver)
            use hickory_resolver::TokioAsyncResolver;
            use hickory_resolver::config::{ResolverConfig, ResolverOpts};

            // Create a resolver with default configuration
            let resolver = TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default());
            
            // The hostname for a Docker Swarm service's tasks is `tasks.<service_name>`
            let lookup_name = format!("tasks.{}", service_name);
            
            match resolver.lookup_ip(lookup_name.clone()).await {
                Ok(response) => {
                    let discovered_peers: Vec<String> = response.iter().map(|ip| ip.to_string()).collect();
                    if discovered_peers.is_empty() {
                        info!("DNS lookup for '{}' was successful but returned no IPs.", lookup_name);
                    } else {
                        info!("Discovered bootstrap peers: {:?}", discovered_peers);
                        cfg.bootstrap_peers.extend(discovered_peers);
                        // Remove duplicates
                        cfg.bootstrap_peers.sort();
                        cfg.bootstrap_peers.dedup();
                    }
                }
                Err(e) => {
                    error!("DNS lookup for '{}' failed: {}. Continuing with configured peers.", lookup_name, e);
                }
            }
        }
    }

    info!("Starting Glued daemon with config: {:?}", cfg);

    // Shared state
    let state: Arc<RwLock<HashMap<String, String>>> = Arc::new(RwLock::new(HashMap::new()));

    // Update channel
    let (update_tx, update_rx) = mpsc::channel(128);

    // Conditionally start the Container Runtime monitor for replicas
    let runtime_handle = if is_replica {
        info!("Starting container runtime monitor...");
        let runtime = DockerRuntime::new(cfg.network_name.clone());
        let handle = tokio::spawn(async move {
            if let Err(e) = runtime.monitor(update_tx).await {
                error!("Container runtime failed: {}", e);
            }
        });
        Some(handle)
    } else {
        None
    };

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
    if let Some(handle) = runtime_handle {
        handle.abort();
    }
    gossip_handle.abort();
    dns_handle.abort();

    info!("Shutdown complete.");
    Ok(())
}
