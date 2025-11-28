# Glued: Cross‑Host Docker DNS via Gossip

Glued is a Rust daemon that provides **cluster‑wide name resolution for Docker containers**.  When you run Glued on multiple hosts, containers attached to a designated network can be reached by name from any host in the cluster.  The daemon watches local containers, gossips name→IP mappings to peers via the [iroh‑gossip](https://github.com/n0-computer/iroh-gossip) protocol, and runs a DNS server that answers queries for these names.  FQDN queries are transparently forwarded to upstream resolvers.  Glued does *not* manage routing or overlay networks; engineers are responsible for connecting the subnets (e.g. via Tailscale or WireGuard) and configuring VIPs.  The sole goal of Glued is to **bridge the DNS gap** between Docker hosts.

## Motivation and Background

In Docker, containers on the same user‑defined network can resolve each other by name through Docker’s built‑in DNS.  However, this resolution is limited to a single host.  When deploying a service across multiple hosts—especially in ad‑hoc clusters or on developer machines—one must manually maintain host files or service discovery layers.  Glued automates this by:

1. **Monitoring containers** on a specific Docker network and extracting their IP addresses.  The same information can be obtained via the CLI with `docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' <container>`【542453841895269†L177-L196】 or by inspecting the network to list containers and their IPs【542453841895269†L204-L225】.  Glued uses the Bollard library to query the Docker API directly.
2. **Disseminating updates** to peers using the epidemic broadcast tree protocol implemented in `iroh-gossip`.  The gossip module is built on HyParView/PlumTree algorithms【696640087599326†L320-L326】 and uses a topic to group peers.  Example code in the iroh‑gossip repository shows how to create an endpoint, spawn a router, subscribe to a topic, and broadcast messages【79180854385140†L320-L342】【79180854385140†L345-L380】.
3. **Serving DNS** for container names using the Trust‑DNS server framework.  A `ServerFuture` is registered with UDP and TCP listeners and a custom `RequestHandler` to process queries【265517038311283†L514-L551】.  The handler can return A/AAAA records for single‑label names and forward FQDN queries to upstream resolvers【265517038311283†L565-L571】.

By combining these pieces, Glued lets you run micro‑services across multiple machines while still using simple Docker names to address them.

## System Architecture

The daemon comprises three main subsystems, orchestrated in `src/main.rs`:

1. **Docker Monitor (`src/docker_monitor.rs`)** – Connects to the local Docker daemon via Bollard, polls the list of running containers every few seconds, and inspects each container.  For the configured network name, it extracts the container’s IP address (preferring IPv4 over IPv6) and maintains a local mapping.  Any additions or removals are sent over a channel as `Update` messages and the shared state map is updated.  The polling strategy is deliberately simple but yields eventual consistency; CLI commands like `docker inspect` illustrate how to extract IP information manually【542453841895269†L177-L196】【542453841895269†L204-L225】.  Future enhancements could subscribe to Docker events to reduce latency.

2. **Gossip Subsystem (`src/gossip.rs`)** – Uses Iroh’s QUIC endpoint to join a peer‑to‑peer network and the iroh‑gossip protocol to broadcast `Update` messages.  Each node subscribes to a topic (identified by a 32‑byte ID) and shares updates with others.  On receiving a message, the handler deserializes it and applies the addition or removal to the shared state map.  The gossip protocol is derived from epidemic broadcast trees (HyParView and PlumTree) and ensures eventual delivery without a central broker【696640087599326†L320-L326】.  The example in the iroh‑gossip readme demonstrates spawning a router, subscribing to a topic with bootstrap peers, and broadcasting a message【79180854385140†L320-L342】【79180854385140†L345-L380】.

3. **DNS Server (`src/dns_server.rs`)** – Implements a custom `RequestHandler` using the Trust‑DNS server library.  The server listens on the configured UDP/TCP socket (default `0.0.0.0:5353` for unprivileged use) and performs two actions:
   - For queries with a **single‑label name** (no dots), it looks up the name in the shared container map and returns an A or AAAA record with a short TTL.  Unknown names yield NXDOMAIN.
   - For **FQDNs** (names containing a dot), it forwards the query to the system’s upstream resolver using `trust-dns-resolver` and returns the upstream response【348553690246282†L500-L571】.  This preserves normal DNS functionality for internet and corporate domains.  The server registers UDP and TCP listeners, as shown in the Dev.to example【265517038311283†L514-L551】, and runs until the process terminates.

The daemon uses a shared `HashMap` wrapped in an `Arc<RwLock<…>>` to store the global name→IP table.  All subsystems read or write this map under the appropriate lock.

## Building and Running

1. **Prerequisites**: Install Rust (>= 1.65) and have Docker running on each host.  To forward queries to upstream DNS, ensure `/etc/resolv.conf` is correctly configured.

2. **Clone and build**:

```sh
git clone <repo>
cd glued
cargo build --release
```

3. **Set up the network**: Create a user‑defined Docker network (e.g. `glued_net`) on each host.  Each network should have a non‑overlapping subnet.  Attach containers that need cross‑host resolution to this network.  You must also ensure that traffic between these subnets is routed (e.g. through Tailscale or WireGuard) and assign any necessary VIPs manually.  Glued does not configure routing itself.

4. **Run Glued** on each host:

```sh
RUST_LOG=info GLUED_NETWORK=glued_net GLUED_DNS_BIND=0.0.0.0:5353 ./target/release/glued
```

By default, Glued uses a random topic ID and does not specify bootstrap peers.  For nodes to discover each other, you can export a topic ID (32‑byte hex) via `GLUED_TOPIC_ID` or modify `main.rs` accordingly and specify peer IDs.  Alternatively, start one node first and have others connect to it via Iroh’s peer exchange.

5. **Configure DNS for containers**: Containers need to use Glued’s DNS server.  You can run Glued on the host and point containers to the host IP (e.g. use `--dns <host-ip>` when starting containers).  Alternatively, run Glued in its own container attached to the network and set its IP as DNS for other containers.  On the host itself, update `/etc/resolv.conf` to `nameserver 127.0.0.1` (or whatever address Glued binds) to resolve container names locally.

## Extending the Prototype

This prototype focuses solely on name resolution.  It omits route management, access control, authentication, and CRDT‑based conflict resolution.  Future improvements might include:

* **Event stream monitoring** – Listen to Docker’s event stream for lower latency and efficient updates.
* **Multiple IPs per name** – Support duplicate container names by returning multiple A/AAAA records.
* **Service discovery** – Integrate with service orchestration frameworks or API to advertise services beyond single containers.
* **Authentication and encryption** – Sign gossip messages or use TLS to prevent unauthorized peers from injecting entries.

## References

* The `iroh-gossip` crate implements epidemic broadcast trees derived from HyParView and PlumTree【696640087599326†L320-L326】.  Its readme shows how to set up a gossip endpoint and broadcast messages【79180854385140†L320-L342】【79180854385140†L345-L380】.
* Trust‑DNS’s `ServerFuture` and `RequestHandler` are used to build the DNS server.  An example registers UDP and TCP listeners and runs the server until completion【265517038311283†L514-L551】.  The trait requires implementing `handle_request` to generate responses【265517038311283†L565-L571】.
* Container IP addresses can be extracted via `docker inspect` commands【542453841895269†L177-L196】【542453841895269†L204-L225】 or `docker inspect --format='{{.NetworkSettings.IPAddress}}'`【376644620767737†L193-L203】; Glued performs equivalent logic via the Docker API.
* Forwards of FQDN queries use the upstream resolver, as recommended when combining a custom DNS responder with normal DNS behaviour【348553690246282†L500-L571】.

---

**Disclaimer**: This is a prototype intended for experimentation and educational purposes.  Use at your own risk.  For production environments, consider robust service discovery solutions and network overlay technologies.