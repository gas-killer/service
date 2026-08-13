FROM rust:1-bookworm AS sidecar-builder
RUN apt-get update && apt-get install -y wget gnupg ca-certificates pkg-config libssl-dev git cmake build-essential zlib1g-dev libzstd-dev libtinfo-dev && \
    wget -qO- https://apt.llvm.org/llvm-snapshot.gpg.key | gpg --dearmor -o /usr/share/keyrings/llvm.gpg && \
    echo "deb [signed-by=/usr/share/keyrings/llvm.gpg] http://apt.llvm.org/bookworm/ llvm-toolchain-bookworm-22 main" > /etc/apt/sources.list.d/llvm.list && \
    apt-get update && apt-get install -y llvm-22 llvm-22-dev libpolly-22-dev
ENV LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
WORKDIR /build
COPY gk-fast-view ./gk-fast-view
WORKDIR /build/gk-fast-view
RUN cargo build --release --bin gk-fast-view && cp target/release/gk-fast-view /usr/local/bin/gk-fast-view

FROM rust:1.83-slim AS service-builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev git && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY service/Cargo.toml service/Cargo.lock service/rust-toolchain.toml ./
COPY service/.cargo ./.cargo
COPY service/router ./router
COPY service/common ./common
COPY service/scripts ./scripts
COPY service/node ./node
RUN cargo build --release -p gas-killer-node && cp target/release/gas-killer-node /usr/local/bin/gas-killer-node

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y wget gnupg ca-certificates libssl3 curl procps && \
    wget -qO- https://apt.llvm.org/llvm-snapshot.gpg.key | gpg --dearmor -o /usr/share/keyrings/llvm.gpg && \
    echo "deb [signed-by=/usr/share/keyrings/llvm.gpg] http://apt.llvm.org/bookworm/ llvm-toolchain-bookworm-22 main" > /etc/apt/sources.list.d/llvm.list && \
    apt-get update && apt-get install -y llvm-22 && rm -rf /var/lib/apt/lists/*
RUN useradd -m -u 1000 -s /bin/bash appuser
COPY --from=service-builder /usr/local/bin/gas-killer-node /usr/local/bin/gas-killer-node
COPY --from=sidecar-builder /usr/local/bin/gk-fast-view /usr/local/bin/gk-fast-view
ENV GK_FAST_VIEW_BIN=/usr/local/bin/gk-fast-view
WORKDIR /app
RUN chown -R appuser:appuser /app
USER appuser
EXPOSE 3001
ENTRYPOINT ["gas-killer-node"]
