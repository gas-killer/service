# Build stage
FROM rust:1.83-slim AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev git && rm -rf /var/lib/apt/lists/*

# Copy manifest files
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./

# Pre-build deps  
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs

RUN --mount=type=secret,id=GIT_AUTH_TOKEN \
    echo "Checking for secret..." && \
    ls -la /run/secrets/ && \
    if [ -f /run/secrets/GIT_AUTH_TOKEN ]; then \
        echo "Secret file found" && \
        TOKEN=$(cat /run/secrets/GIT_AUTH_TOKEN) && \
        git config --global url."https://${TOKEN}@github.com/".insteadOf "https://github.com/"; \
    else \
        echo "ERROR: Secret file not found at /run/secrets/GIT_AUTH_TOKEN" && \
        exit 1; \
    fi

RUN cargo build --release && rm -rf src

COPY src ./src
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user
RUN useradd -m -u 1000 -s /bin/bash appuser

# Copy the binary from builder
COPY --from=builder /app/target/release/gas-killer-router /usr/local/bin/gas-killer-router

# Copy configuration files
COPY config /app/config

# Set ownership
RUN chown -R appuser:appuser /app

# Switch to non-root user
USER appuser

# Set working directory
WORKDIR /app

# Expose port 3000 for the application
EXPOSE 3000

# Run the binary
ENTRYPOINT ["gas-killer-router"]

