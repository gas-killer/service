#!/bin/bash
# Build the node-fast image (gas-killer-node + gk-fast-view revmc sidecar + LLVM-22).
# Cross-repo build: assembles the two-directory context the Dockerfile expects.
#   gk-fast-view/  <- gas-analyzer repo, branch ron/local-execution, crates/gk-fast-view
#   service/       <- this repo (branch ron/sharded-inference)
# Usage: TAG=v7 ANALYZER=/path/to/gas-analyzer ./build-node-fast.sh
set -euo pipefail
TAG=${TAG:?set TAG (deployed: v6 = analyzer 541c9e7 + service 3f61064-era)}
ANALYZER=${ANALYZER:?path to a gas-analyzer checkout on ron/local-execution}
SVC=$(cd "$(dirname "$0")/../.." && pwd)
CTX=$(mktemp -d)
trap 'rm -rf "$CTX"' EXIT
cp "$SVC/deploy/images/node-fast.Dockerfile" "$CTX/Dockerfile"
cp -R "$ANALYZER/crates/gk-fast-view" "$CTX/gk-fast-view"
rm -rf "$CTX/gk-fast-view/target" "$CTX/gk-fast-view/repro"
mkdir "$CTX/service"
for d in Cargo.toml Cargo.lock rust-toolchain.toml .cargo router common scripts node; do
  cp -R "$SVC/$d" "$CTX/service/$d"
done
gcloud builds submit "$CTX" --project gas-killer-testnet \
  --tag us-east4-docker.pkg.dev/gas-killer-testnet/gk-fast/node-fast:$TAG
