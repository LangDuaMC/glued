# Glued

Glued is a lightweight daemon that provides cluster-wide DNS resolution for Docker containers. It uses a gossip protocol to share container IP addresses across multiple hosts, allowing you to address containers by name from any node in the cluster.

## Features

- **Zero-config Gossip**: Automatically discovers peers and shares container info.
- **DNS Server**: Responds to DNS queries for container names (e.g., `my-app`).
- **Docker Integration**: Watches for container lifecycle events (start/stop) to keep records up-to-date.
- **Resilient**: Handles upstream DNS failures and Docker daemon reconnections.

## Quick Start

### Running with Docker

The daemon picks its role from `GLUED_NETWORK_NAME`:
- **Main**: leave `GLUED_NETWORK_NAME` unset. Runs DNS + registry only (no Docker socket required).
- **Replica**: set `GLUED_NETWORK_NAME` to a Docker overlay network. Watches containers on that network and gossips updates.

Main instance (no network provided):

```bash
docker run -d \
  --name glued-main \
  --network host \
  -e RUST_LOG=info \
  ghcr.io/langduamc/glued:latest
```

Replica (monitors `glued_net`, bootstraps to the main service via Docker DNS):

```bash
docker run -d \
  --name glued-replica \
  --network host \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -e GLUED_NETWORK_NAME=glued_net \
  -e GLUED_BOOTSTRAP_SERVICE=main \
  -e RUST_LOG=info \
  ghcr.io/langduamc/glued:latest
```

> **Note**: `--network host` is recommended so the DNS server binds to the host's interface and is accessible to other containers/hosts. Replicas need the Docker socket to watch containers; the main instance does not.

### Configuration

Glued can be configured via environment variables or a configuration file (`glued.toml` or `glued.json`).

| Environment Variable | Default | Description |
|----------------------|---------|-------------|
| `GLUED_NETWORK_NAME` | (unset) | When set, runs as a replica and monitors that Docker network. Leave unset to run the main instance. |
| `GLUED_DNS_BIND` | `0.0.0.0:53` | Address and port for the DNS server. |
| `GLUED_BIND_IP` | (none) | Fast IP configuration - sets the bind IP, keeping port at 53. |
| `GLUED_TOPIC_ID` | (random) | 32-byte hex string for the gossip topic. Must be same across cluster. |
| `GLUED_BOOTSTRAP_PEERS` | `[]` | Comma-separated list of peer IDs to bootstrap from. |
| `GLUED_BOOTSTRAP_SERVICE` | `main` | Swarm service name to resolve via Docker DNS for bootstrap peers. |
| `GLUED_CLUSTER_SECRET` | `default_insecure_secret` | Shared secret for cluster authentication. |
| `RUST_LOG` | `info` | Logging level (error, warn, info, debug, trace). |

### Using the DNS

Configure your other containers to use the Glued instance as their DNS server.

```bash
docker run --dns <HOST_IP> ...
```

Or update `/etc/resolv.conf` on the host to point to `127.0.0.1` (if bound to port 53).

### Docker Swarm stack

`docker-compose.prod.yml` is tailored for `docker stack deploy`:
- The **main** service runs on a manager node with a static IP on the overlay network (`172.16.238.254`).
- **Replicas** run in global mode on workers/managers, monitor the `glued_net` overlay, and bootstrap to the main service via Docker DNS.
- Create the overlay network and cluster secret before deploying (see comments in the file).

Deploy:

```bash
docker network create --driver overlay --subnet=172.16.238.0/24 glued_net
echo "a_very_secure_and_random_secret_phrase" | docker secret create glued_cluster_secret -
docker stack deploy -c docker-compose.prod.yml glued
```

### Pterodactyl (Ptero) network setup

If you run game servers with Pterodactyl/Wings, point replicas at the Wings Docker network (commonly `pterodactyl_nw`).

1. Check the exact network name on each node:

```bash
docker network ls | grep -i ptero
```

2. Run Glued replica on each Wings node:

```bash
docker run -d \
  --name glued-ptero \
  --restart unless-stopped \
  --network host \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -e GLUED_NETWORK_NAME=pterodactyl_nw \
  -e GLUED_BOOTSTRAP_SERVICE=main \
  -e GLUED_CLUSTER_SECRET=replace_with_a_shared_secret \
  -e GLUED_TOPIC_ID=replace_with_a_shared_64_hex_topic \
  -e RUST_LOG=info \
  ghcr.io/langduamc/glued:latest
```

3. Configure DNS for workloads that should resolve cross-node names:
- Set container DNS to the Glued host IP (for example in Docker `--dns <glued_host_ip>`).
- Keep the same `GLUED_CLUSTER_SECRET` and `GLUED_TOPIC_ID` on every node.
- Ensure inter-node routing/firewall allows Glued gossip traffic between nodes.


## Architecture

Glued consists of three main components:
1. **Container Runtime**: Monitors Docker events to track running containers.
2. **Gossip Engine**: Uses `iroh-gossip` to broadcast updates to peers.
3. **DNS Server**: Uses `hickory-dns` to serve records and forward upstream queries.

For more details on the internal architecture, see [AGENT.md](AGENT.md).
