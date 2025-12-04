# Builder stage
FROM rust:alpine AS builder

# Install build dependencies
RUN apk add --no-cache musl-dev openssl-dev

WORKDIR /app

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Create dummy main to cache dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release

# Copy source code
COPY src ./src

# Build the application
# Touch main.rs to force rebuild
RUN touch src/main.rs
RUN cargo build --release

# Runner stage
FROM alpine:latest

# Install runtime dependencies
RUN apk add --no-cache libgcc

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/glued /app/glued

# Expose ports
# DNS (standard port)
EXPOSE 53/udp
# Gossip (Iroh) - uses random ports, host networking recommended for p2p

# Entrypoint
ENTRYPOINT ["/app/glued"]
