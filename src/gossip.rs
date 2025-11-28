//! Gossip subsystem based on Iroh.

use std::collections::HashMap;
use std::sync::Arc;

// use futures_util::StreamExt;
use iroh::{Endpoint, NodeId};
use iroh_gossip::{net::Gossip, proto::TopicId};
use log::{error, info, warn};
use tokio::sync::{mpsc, RwLock};

use crate::types::Update;

/// Runs the gossip subsystem.
pub async fn run_gossip(
    topic_id: String,
    bootstrap_peers: Vec<String>,
    mut update_rx: mpsc::Receiver<Update>,
    _state: Arc<RwLock<HashMap<String, String>>>,
) -> anyhow::Result<()> {
    // Create a new Iroh endpoint.
    let endpoint = Endpoint::builder().discovery_n0().bind().await?;
    let our_id = endpoint.node_id();
    info!("Gossip endpoint created with ID: {}", our_id);

    // Spawn gossip protocol
    let my_addr = endpoint.node_addr().await?;
    let _gossip = Gossip::from_endpoint(
        endpoint.clone(),
        iroh_gossip::proto::Config::default(),
        &my_addr.info,
    );

    // Decode topic ID
    let topic_bytes = hex::decode(&topic_id)?;
    let _topic_id = TopicId::from_bytes(
        topic_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid topic ID length"))?,
    );

    // Parse bootstrap peers
    let mut bootstrap_ids = Vec::new();
    for peer in bootstrap_peers {
        if let Ok(id) = peer.parse::<NodeId>() {
            bootstrap_ids.push(id);
        } else {
            warn!("Invalid bootstrap peer ID: {}", peer);
        }
    }

    // Subscribe to the topic
    // TODO: Fix gossip subscription for iroh-gossip 0.29
    /*
    let (sender, mut receiver) = gossip.subscribe(topic_id, bootstrap_ids).await?.split();

    // Spawn a task that listens for remote events.
    let state_clone = Arc::clone(&state);
    tokio::spawn(async move {
        while let Some(event) = receiver.next().await {
            match event {
                Ok(GossipEvent::Received(msg)) => {
                    // Attempt to deserialize the message into an Update.
                    match serde_json::from_slice::<Update>(&msg.content) {
                        Ok(update) => {
                            debug!("Received update: {:?}", update);
                            apply_update(update, &state_clone).await;
                        }
                        Err(e) => {
                            error!("Failed to deserialize gossip message: {}", e);
                        }
                    }
                }
                Ok(GossipEvent::NeighborUp(peer)) => {
                    info!("Gossip peer joined: {}", peer);
                }
                Ok(GossipEvent::NeighborDown(peer)) => {
                    info!("Gossip peer left: {}", peer);
                }
                Err(e) => {
                    error!("Gossip receiver error: {}", e);
                    break;
                }
            }
        }
    });
    */

    // Main loop: read local updates and broadcast
    while let Some(update) = update_rx.recv().await {
        let _bytes = match serde_json::to_vec(&update) {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to serialize update: {}", e);
                continue;
            }
        };
        info!("Broadcasting update (mock): {:?}", update);
        /*
        if let Err(e) = sender.broadcast(bytes.into()).await {
            error!("Failed to broadcast gossip message: {}", e);
        }
        */
    }
    info!("Gossip update channel closed, shutting down");
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
