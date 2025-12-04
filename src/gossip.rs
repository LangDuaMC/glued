//! Gossip subsystem based on Iroh.

use std::collections::HashMap;
use std::sync::Arc;

use iroh::{Endpoint, NodeId};
use iroh_gossip::{net::Gossip, proto::TopicId};
use log::{error, info, warn};
use sha2::Digest;
use tokio::sync::{mpsc, RwLock};

use crate::types::Update;

/// Runs the gossip subsystem.
/// Runs the gossip subsystem.
pub async fn run_gossip(
    topic_id: String,
    bootstrap_peers: Vec<String>,
    mut update_rx: mpsc::Receiver<Update>,
    _state: Arc<RwLock<HashMap<String, String>>>,
    cluster_secret: String,
) -> anyhow::Result<()> {
    // Create a new Iroh endpoint.
    let endpoint = Endpoint::builder().discovery_n0().bind().await?;
    let our_id = endpoint.node_id();
    info!("Gossip endpoint created with ID: {}", our_id);

    // Spawn gossip protocol
    let my_addr = endpoint.node_addr().await?;
    let gossip = Gossip::from_endpoint(
        endpoint.clone(),
        iroh_gossip::proto::Config::default(),
        &my_addr.info,
    );

    // Decode topic ID
    let topic_bytes = hex::decode(&topic_id)?;
    let _topic_id_struct = TopicId::from_bytes(
        topic_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid topic ID length"))?,
    );

    // Parse bootstrap peers and filter out self
    let mut bootstrap_ids = Vec::new();
    for peer in bootstrap_peers {
        if let Ok(id) = peer.parse::<NodeId>() {
            if id != our_id {
                bootstrap_ids.push(id);
            }
        } else {
            warn!("Invalid bootstrap peer ID: {}", peer);
        }
    }

    // Join the gossip topic
    // Note: In iroh-gossip 0.29, we join via the router or similar.
    // For now, we'll assume standard join logic if available, or just use the gossip handle.
    // The original code had commented out subscription. We will re-enable basic join if possible,
    // but first let's handle the authentication and connection management.

    // Authentication Handler Task
    let auth_endpoint = endpoint.clone();
    let auth_secret = cluster_secret.clone();
    let auth_node_id = our_id;
    tokio::spawn(async move {
        while let Some(incoming) = auth_endpoint.accept().await {
            let secret = auth_secret.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_incoming_connection(incoming, secret, auth_node_id).await {
                    warn!("Incoming connection failed auth: {}", e);
                }
            });
        }
    });

    // Connection Retry / Maintenance Task
    let conn_endpoint = endpoint.clone();
    let conn_bootstrap_ids = bootstrap_ids.clone();
    let conn_secret = cluster_secret.clone();
    tokio::spawn(async move {
        loop {
            for &peer_id in &conn_bootstrap_ids {
                // Check if connected
                // This is a simplification; iroh might manage connections automatically.
                // But we want to enforce our auth.
                match conn_endpoint.connect(peer_id, b"glued/auth/1").await {
                    Ok(connection) => {
                        if let Err(e) = perform_auth_handshake(connection, &conn_secret).await {
                            warn!(
                                "Failed to authenticate with bootstrap peer {}: {}",
                                peer_id, e
                            );
                        } else {
                            info!("Authenticated with bootstrap peer {}", peer_id);
                            // If auth succeeds, we can add them to gossip
                            // gossip.add_neighbor(topic_id_struct, peer_id); // Hypothetical API
                        }
                    }
                    Err(e) => {
                        warn!("Failed to connect to bootstrap peer {}: {}", peer_id, e);
                    }
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        }
    });

    // Note: iroh-gossip 0.29 API has changed. The subscribe method is not directly available.
    // For now, we'll keep the gossip instance alive and rely on the connection retry task
    // to establish authenticated connections. Full gossip integration will be completed
    // when the API is stabilized.

    // Keep gossip alive
    let _gossip_handle = gossip;

    // Main loop: read local updates and broadcast
    // TODO: Integrate with gossip once API is stable
    while let Some(update) = update_rx.recv().await {
        let _bytes = match serde_json::to_vec(&update) {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to serialize update: {}", e);
                continue;
            }
        };
        info!(
            "Broadcasting update (pending gossip integration): {:?}",
            update
        );
        // TODO: Use gossip to broadcast once API is available
    }
    info!("Gossip update channel closed, shutting down");
    Ok(())
}

async fn handle_incoming_connection(
    incoming: iroh::endpoint::Incoming,
    secret: String,
    our_id: NodeId,
) -> anyhow::Result<()> {
    let connection = incoming.await?;
    let (mut send, mut recv) = connection.open_bi().await?;

    // 1. Wait for AUTH_INIT
    let mut buf = vec![0u8; 9];
    recv.read_exact(&mut buf).await?;
    if buf.as_slice() != b"AUTH_INIT" {
        anyhow::bail!("Invalid protocol init");
    }

    // 2. Send our NodeId
    send.write_all(our_id.as_bytes()).await?;
    send.finish()?;

    // 3. Receive Hash(Secret + OurNodeId)
    let mut received_hash = vec![0u8; 32];
    recv.read_exact(&mut received_hash).await?;

    // 4. Verify Hash
    let mut hasher = sha2::Sha256::new();
    hasher.update(secret.as_bytes());
    hasher.update(our_id.as_bytes());
    let expected_hash = hasher.finalize();

    if received_hash != expected_hash.as_slice() {
        anyhow::bail!("Authentication failed: Invalid hash");
    }

    // 5. Send AUTH_OK
    send.write_all(b"AUTH_OK").await?;
    send.finish()?;
    Ok(())
}

async fn perform_auth_handshake(
    connection: iroh::endpoint::Connection,
    secret: &str,
) -> anyhow::Result<()> {
    let (mut send, mut recv) = connection.open_bi().await?;

    // 1. Send AUTH_INIT
    send.write_all(b"AUTH_INIT").await?;

    // 2. Receive Responder NodeId
    let mut node_id_bytes = [0u8; 32];
    recv.read_exact(&mut node_id_bytes).await?;
    let responder_id = NodeId::from_bytes(&node_id_bytes)?;

    // 3. Hash(Secret + ResponderNodeId)
    let mut hasher = sha2::Sha256::new();
    hasher.update(secret.as_bytes());
    hasher.update(responder_id.as_bytes());
    let hash = hasher.finalize();

    // 4. Send Hash
    send.write_all(&hash).await?;
    send.finish()?;
    // 5. Wait for AUTH_OK
    let mut buf = vec![0u8; 7];
    recv.read_exact(&mut buf).await?;
    if buf.as_slice() != b"AUTH_OK" {
        anyhow::bail!("Auth failed");
    }

    Ok(())
}

#[allow(dead_code)]
async fn apply_update(update: Update, state: &Arc<RwLock<HashMap<String, String>>>) {
    match update {
        Update::Add { name, ip } => {
            let mut map = state.write().await;
            map.insert(name.clone(), ip.clone());
            info!("Applied update: Added {} -> {}", name, ip);
        }
        Update::Remove { name } => {
            let mut map = state.write().await;
            map.remove(&name);
            info!("Applied update: Removed {}", name);
        }
    }
}
