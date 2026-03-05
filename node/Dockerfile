# Build stage
FROM rust:1.83-slim AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests and cargo config
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY .cargo ./.cargo

# Copy workspace crates
COPY router ./router
COPY common ./common
COPY scripts ./scripts
COPY node ./node

# Prefetch dependencies to warm cargo cache
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo fetch

# Build release binary and copy it out of the cache-mounted target dir
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release -p gas-killer-node && \
    cp /app/target/release/gas-killer-node /usr/local/bin/gas-killer-node

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    git \
    procps \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user
RUN useradd -m -u 1000 -s /bin/bash appuser

# Copy the binary from builder
COPY --from=builder /usr/local/bin/gas-killer-node /usr/local/bin/gas-killer-node

# Set ownership and working directory
WORKDIR /app
RUN chown -R appuser:appuser /app

# Switch to non-root user
USER appuser

# Expose port for P2P
EXPOSE 3001

# Run the binary
ENTRYPOINT ["gas-killer-node"]
