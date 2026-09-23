#!/bin/bash
#
# Stage the FLAGSHIP guest for the compose harness (UNBOUNDED_V3, GK_E2E_GUEST=qwen): the
# real Qwen3-0.6B model behind GasKillerChatQwen. Into config/guest/ go
#   qwen.elf       gas-analyzer's committed crates/gkvm/tests/fixtures/qwen-c.elf (fetched at
#                  GK_GA_REF), which must keccak to the PROGRAM_HASH the consumer's binding
#                  (solidity-sdk gen/GkQwen.sol) commits to;
#   weights.bin, tokenizer.bin   the qwen3-0.6b-onchain-v1 release bytes (sha256-pinned) — the
#                  same bytes the V2 overlay consumer settles on Sepolia, served to the guest as
#                  a manifest-v3 artifact whose root every operator verifies at startup.
# Prints `GUEST_PROGRAM_HASH=<hash>` and `ARTIFACT_ROOT=<root>` on success.
#
# Env:
#   GK_GA_REF             gas-analyzer ref to fetch the ELF from (default: the gkvm branch)
#   GK_GUEST_ELF          prebuilt qwen-c.elf (skips the fetch)
#   GK_QWEN_DIR           where the release blobs are cached (default config/guest itself)
#   GK_SDK_REPO / GK_SDK_REF / GK_SDK_DIR   as in stage_guest_program.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
SDK_REPO="${GK_SDK_REPO:-https://github.com/gas-killer/solidity-sdk}"
SDK_REF="${GK_SDK_REF:-RonTuretzky/gkvm-m5-dx}"
SDK_DIR="${GK_SDK_DIR:-$PROJECT_ROOT/.gk-solidity-sdk-native}"
GA_REF="${GK_GA_REF:-RonTuretzky/gkvm-m6-host}"
GUEST_DIR="$PROJECT_ROOT/config/guest"
BLOB_DIR="${GK_QWEN_DIR:-$GUEST_DIR}"
RELEASE="https://github.com/gas-killer/solidity-sdk/releases/download/qwen3-0.6b-onchain-v1"
# Manifest-v3 root of the two release files (gas-analyzer crates/gkvm/scripts/flagship/run.sh
# recomputes and checks it with `gk-run --print-artifact-root`).
ARTIFACT_ROOT=0xad3abf5617f9c7e1862d7a3e0a2cf368939e09ae69f094bb2e3bf42279e99115
WEIGHTS_SHA=7135c9509db58a12f80671d409e528b4f7cc45bbdf9c5c8ee737fc954297db8a
TOKENIZER_SHA=ec813734e9e01a2784e7a2c9ee68b39c0a42a57e6b2bcc0e9a7a6f00d1041dc0

if [ ! -e "$SDK_DIR/.git" ]; then
    git clone --depth 1 -b "$SDK_REF" "$SDK_REPO" "$SDK_DIR" >&2
    git -C "$SDK_DIR" submodule update --init --recursive --depth 1 >&2
fi
BINDING="$SDK_DIR/src/examples/onchain-llm-native/gen/GkQwen.sol"
EXPECTED=$(grep -o 'PROGRAM_HASH = 0x[0-9a-fA-F]\{64\}' "$BINDING" | grep -o '0x[0-9a-fA-F]*')
[ -n "$EXPECTED" ] || { echo "PROGRAM_HASH not found in $BINDING" >&2; exit 1; }

mkdir -p "$GUEST_DIR" "$BLOB_DIR"
if [ -n "${GK_GUEST_ELF:-}" ]; then
    ELF="$GK_GUEST_ELF"
else
    ELF="$GUEST_DIR/qwen.elf.download"
    echo "Fetching gas-analyzer@$GA_REF crates/gkvm/tests/fixtures/qwen-c.elf..." >&2
    curl -fsSL -o "$ELF" "https://raw.githubusercontent.com/gas-killer/gas-analyzer/$GA_REF/crates/gkvm/tests/fixtures/qwen-c.elf"
fi
ACTUAL=$(python3 "$SDK_DIR/tools/gk" hash "$ELF")
if [ "$ACTUAL" != "$EXPECTED" ]; then
    echo "guest image $ELF hashes to $ACTUAL but the consumer's binding commits to $EXPECTED" >&2
    exit 1
fi
cp "$ELF" "$GUEST_DIR/qwen.elf"
chmod 644 "$GUEST_DIR/qwen.elf"
rm -f "$GUEST_DIR/qwen.elf.download"

sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
for f in weights.bin tokenizer.bin; do
    [ -s "$BLOB_DIR/$f" ] || { echo "Fetching $RELEASE/$f..." >&2; curl -fsSL -o "$BLOB_DIR/$f" "$RELEASE/$f"; }
done
[ "$(sha "$BLOB_DIR/weights.bin")" = "$WEIGHTS_SHA" ] || { echo "weights.bin sha256 mismatch" >&2; exit 1; }
[ "$(sha "$BLOB_DIR/tokenizer.bin")" = "$TOKENIZER_SHA" ] || { echo "tokenizer.bin sha256 mismatch" >&2; exit 1; }
if [ "$BLOB_DIR" != "$GUEST_DIR" ]; then
    ln -sf "$(cd "$BLOB_DIR" && pwd)/weights.bin" "$GUEST_DIR/weights.bin"
    ln -sf "$(cd "$BLOB_DIR" && pwd)/tokenizer.bin" "$GUEST_DIR/tokenizer.bin"
fi
echo "Staged $GUEST_DIR/qwen.elf ($ACTUAL) + weights.bin/tokenizer.bin (root $ARTIFACT_ROOT)" >&2
echo "GUEST_PROGRAM_HASH=$ACTUAL"
echo "ARTIFACT_ROOT=$ARTIFACT_ROOT"
