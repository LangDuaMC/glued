//! Data structures used throughout the glue daemon.
//!
//! These types are serialised using [`serde`](https://serde.rs/) and
//! broadcast over the gossip network.  They represent high level
//! operations on the container registry such as adding or removing
//! entries.  The fields are kept minimal to reduce bandwidth usage.

use serde::{Deserialize, Serialize};

/// An update message describing a change in the container mapping.
///
/// This enum is sent via iroh‑gossip to all peers.  Each message
/// either adds a new name → IP entry or removes an existing entry.
/// Timestamps or generation numbers can be added in the future to
/// improve conflict resolution; currently the last update wins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Update {
    /// A container has been discovered or updated on a host.  `name` is
    /// the container name (single label) and `ip` is its IPv4/IPv6
    /// address on the designated network.
    Add { name: String, ip: String },
    /// A container has stopped or detached from the network.  Only
    /// the name is required to remove the mapping.
    Remove { name: String },
}
