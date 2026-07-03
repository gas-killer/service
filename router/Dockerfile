# Build stage
# Keep the builder at or above the minimum rustc the lockfile needs (>= 1.91);
# rust-toolchain.toml (channel = "stable") would otherwise download a toolchain
# at image-build time. Pin the -bookworm variant so the binary links against the
# same glibc as the bookworm runtime stage below (the default rust:slim tracks
# Debian trixie, whose newer glibc is absent on bookworm → GLIBC_2.38 not found).
FROM rust:1.96-slim-bookworm AS builder
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
    cargo build --release -p gas-killer-router && \
    cp /app/target/release/gas-killer-router /usr/local/bin/gas-killer-router && \
    cp /app/target/release/generate_key /usr/local/bin/generate_key

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    procps \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user
RUN useradd -m -u 1000 -s /bin/bash appuser

# Copy binaries from builder
COPY --from=builder /usr/local/bin/gas-killer-router /usr/local/bin/gas-killer-router
COPY --from=builder /usr/local/bin/generate_key /usr/local/bin/generate_key

# Copy configuration files
COPY config /app/config

# Set ownership. /app/data is the aggregation-engine journal directory
# (STORAGE_DIR); creating it owned by appuser here means a named volume
# mounted there inherits writable ownership.
RUN mkdir -p /app/data && chown -R appuser:appuser /app

# Switch to non-root user
USER appuser

# Set working directory
WORKDIR /app

# Expose port 3000 for the application
EXPOSE 3000

# Run the binary
ENTRYPOINT ["gas-killer-router"]
