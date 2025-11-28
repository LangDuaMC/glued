# Glued

Glued is a lightweight daemon that provides cluster-wide DNS resolution for Docker containers. It uses a gossip protocol to share container IP addresses across multiple hosts, allowing you to address containers by name from any node in the cluster.

## Features

- **Zero-config Gossip**: Automatically discovers peers and shares container info.
- **DNS Server**: Responds to DNS queries for container names (e.g., `my-app`).
- **Docker Integration**: Watches for container lifecycle events (start/stop) to keep records up-to-date.
- **Resilient**: Handles upstream DNS failures and Docker daemon reconnections.

## Quick Start

### Running with Docker

The easiest way to run Glued is as a Docker container. You must mount the Docker socket so Glued can monitor other containers.

```bash
docker run -d \
  --name glued \
  --network host \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -e GLUED_NETWORK=my_overlay_network \
  -e RUST_LOG=info \
  ghcr.io/your-org/glued:latest
```

> **Note**: `--network host` is recommended so the DNS server binds to the host's interface and is accessible to other containers/hosts.

### Configuration

Glued can be configured via environment variables or a configuration file (`glued.toml` or `glued.json`).

| Environment Variable | Default | Description |
|----------------------|---------|-------------|
| `GLUED_NETWORK` | `glued_net` | The Docker network to monitor. |
| `GLUED_DNS_BIND` | `0.0.0.0:5353` | Address and port for the DNS server. |
| `GLUED_TOPIC_ID` | (random) | 32-byte hex string for the gossip topic. Must be same across cluster. |
| `GLUED_BOOTSTRAP_PEERS` | `[]` | Comma-separated list of peer IDs to bootstrap from. |
| `RUST_LOG` | `info` | Logging level (error, warn, info, debug, trace). |

### Using the DNS

Configure your other containers to use the Glued instance as their DNS server.

```bash
docker run --dns <HOST_IP> ...
```

Or update `/etc/resolv.conf` on the host to point to `127.0.0.1` (if bound to port 53).

## Architecture

Glued consists of three main components:
1. **Container Runtime**: Monitors Docker events to track running containers.
2. **Gossip Engine**: Uses `iroh-gossip` to broadcast updates to peers.
3. **DNS Server**: Uses `hickory-dns` to serve records and forward upstream queries.

For more details on the internal architecture, see [AGENT.md](AGENT.md).
