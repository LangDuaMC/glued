use super::ContainerRuntime;
use crate::types::Update;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use bollard::container::ListContainersOptions;
use bollard::system::EventsOptions;
use bollard::Docker;
use futures_util::stream::StreamExt;
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::env;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;

pub struct DockerRuntime {
    network_name: Option<String>,
}

impl DockerRuntime {
    pub fn new(network_name: Option<String>) -> Self {
        Self { network_name }
    }

    async fn connect() -> Result<Docker> {
        // Connect to the local Docker daemon using default settings.
        // This handles unix socket on Linux.
        Docker::connect_with_local_defaults().map_err(Into::into)
    }

    /// Inspects the current container to find the first attached overlay network.
    /// This is used by replicas to auto-discover the network to monitor.
    async fn autodetect_overlay_network(docker: &Docker) -> Result<String> {
        info!("`NETWORK_NAME` not specified, attempting to auto-detect overlay network...");
        // In a Docker container, the hostname is typically the container ID.
        let hostname = env::var("HOSTNAME")?;
        let container_detail = docker.inspect_container(&hostname, None).await?;

        if let Some(networks) = container_detail
            .network_settings
            .and_then(|s| s.networks)
        {
            for (name, _) in networks {
                let network_detail = docker.inspect_network(&name, None::<String>).await?;
                if let Some(driver) = network_detail.driver {
                    if driver == "overlay" {
                        info!("Auto-detected overlay network: {}", name);
                        return Ok(name);
                    }
                }
            }
        }

        Err(anyhow!(
            "Could not auto-detect an overlay network for this container."
        ))
    }

    async fn get_initial_state(
        docker: &Docker,
        network_name: &str,
    ) -> Result<HashMap<String, String>> {
        let mut map = HashMap::new();
        let opts = ListContainersOptions::<String> {
            all: false,
            ..Default::default()
        };
        let containers = docker.list_containers(Some(opts)).await?;

        for c in containers {
            let name = c
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|n| n.trim_start_matches('/').to_string());
            let id = c.id.as_ref().map(|s| s.to_string());
            let name = match (name, id) {
                (Some(n), _) => n,
                (_, Some(id)) => id,
                _ => continue,
            };

            if let Ok(detail) = docker.inspect_container(&name, None).await {
                if let Some(ip) = get_ip_for_network(&detail, network_name) {
                    map.insert(name, ip);
                }
            }
        }
        Ok(map)
    }
}

#[async_trait]
impl ContainerRuntime for DockerRuntime {
    async fn monitor(&self, update_tx: mpsc::Sender<Update>) -> Result<()> {
        loop {
            let docker = match Self::connect().await {
                Ok(d) => d,
                Err(e) => {
                    error!("Failed to connect to Docker: {}. Retrying in 5s...", e);
                    sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };

            // Auto-detect network if not provided
            let network_name = match &self.network_name {
                Some(name) => name.clone(),
                None => match Self::autodetect_overlay_network(&docker).await {
                    Ok(name) => name,
                    Err(e) => {
                        error!(
                            "Network discovery failed: {}. Retrying in 10s...",
                            e
                        );
                        sleep(Duration::from_secs(10)).await;
                        continue;
                    }
                },
            };
            info!("Starting Docker monitor for network: {}", network_name);

            // Initial scan
            match Self::get_initial_state(&docker, &network_name).await {
                Ok(initial_map) => {
                    info!("Initial scan found {} containers", initial_map.len());
                    for (name, ip) in initial_map {
                        if let Err(e) = update_tx
                            .send(Update::Add {
                                name: name.clone(),
                                ip: ip.clone(),
                            })
                            .await
                        {
                            error!("Failed to send initial update for {}: {}", name, e);
                            return Err(anyhow::anyhow!("Channel closed"));
                        }
                    }
                }
                Err(e) => {
                    error!("Failed initial scan: {}. Retrying...", e);
                    sleep(Duration::from_secs(5)).await;
                    continue;
                }
            }

            // Event stream
            let opts = EventsOptions::<String> {
                filters: [
                    ("type", ["container"].as_slice()),
                    ("event", ["start", "die", "kill", "stop"].as_slice()),
                ]
                .iter()
                .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
                .collect(),
                ..Default::default()
            };

            let mut stream = docker.events(Some(opts));

            info!("Listening for Docker events...");
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(event) => {
                        if let Some(actor) = event.actor {
                            if let Some(attributes) = actor.attributes {
                                let name = attributes.get("name").cloned().unwrap_or_default();
                                let id = actor.id.unwrap_or_default();
                                let container_name =
                                    if !name.is_empty() { name } else { id.clone() };

                                if container_name.is_empty() {
                                    continue;
                                }

                                let action = event.action.unwrap_or_default();
                                debug!("Container event: {} for {}", action, container_name);

                                match action.as_str() {
                                    "start" => {
                                        // Inspect to get IP
                                        match docker.inspect_container(&container_name, None).await
                                        {
                                            Ok(detail) => {
                                                if let Some(ip) =
                                                    get_ip_for_network(&detail, &network_name)
                                                {
                                                    info!(
                                                        "Container started: {} -> {}",
                                                        container_name, ip
                                                    );
                                                    if let Err(e) = update_tx
                                                        .send(Update::Add {
                                                            name: container_name,
                                                            ip,
                                                        })
                                                        .await
                                                    {
                                                        error!("Failed to send Add update: {}", e);
                                                        return Err(anyhow::anyhow!(
                                                            "Channel closed"
                                                        ));
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                warn!(
                                                    "Failed to inspect started container {}: {}",
                                                    container_name, e
                                                );
                                            }
                                        }
                                    }
                                    "die" | "kill" | "stop" => {
                                        info!("Container stopped: {}", container_name);
                                        if let Err(e) = update_tx
                                            .send(Update::Remove {
                                                name: container_name,
                                            })
                                            .await
                                        {
                                            error!("Failed to send Remove update: {}", e);
                                            return Err(anyhow::anyhow!("Channel closed"));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Error in Docker event stream: {}", e);
                        break; // Break inner loop to reconnect
                    }
                }
            }

            warn!("Docker event stream ended. Reconnecting in 2s...");
            sleep(Duration::from_secs(2)).await;
        }
    }
}

fn get_ip_for_network(
    detail: &bollard::models::ContainerInspectResponse,
    network_name: &str,
) -> Option<String> {
    if let Some(settings) = &detail.network_settings {
        if let Some(networks) = &settings.networks {
            if let Some(net) = networks.get(network_name) {
                if let Some(ipv4) = &net.ip_address {
                    if !ipv4.is_empty() {
                        return Some(ipv4.clone());
                    }
                }
                if let Some(ipv6) = &net.global_ipv6_address {
                    if !ipv6.is_empty() {
                        return Some(ipv6.clone());
                    }
                }
            }
        }
    }
    None
}
