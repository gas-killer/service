#!/bin/bash
#
# Stage the chat-native guest program for the compose harness (UNBOUNDED_V3):
# config/guest/<GK_E2E_GUEST_ELF> is what docker-compose.gkvm.yml mounts into the
# router and every node as GK_GUEST_PROGRAM. The image is `guest/answer.py` of the
# solidity-sdk `onchain-llm-native` example, built by the sdk's `gk build` (docker +
# MicroPython at the pinned commit; byte-reproducible) unless GK_GUEST_ELF names a
# prebuilt one. Either way the staged bytes must keccak to the PROGRAM_HASH the
# consumer's generated binding commits to — the hash the operators verify at startup
# and the consumer passes to the precompile — or nothing is staged.
# Prints `GUEST_PROGRAM_HASH=<hash>` on success.
#
# Env:
#   GK_GUEST_ELF          prebuilt guest image (skips the build)
#   GK_E2E_GUEST_ELF      staged file name under config/guest (default answer.elf)
#   GK_SDK_REPO / GK_SDK_REF   consumer source (default: the gkvm M5 branch)
#   GK_SDK_DIR            checkout cache dir (default .gk-solidity-sdk-native); point it
#                         at an existing solidity-sdk checkout to use that as-is
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

SDK_REPO="${GK_SDK_REPO:-https://github.com/gas-killer/solidity-sdk}"
SDK_REF="${GK_SDK_REF:-Rubydusa/gkvm-m5-dx}"
SDK_DIR="${GK_SDK_DIR:-$PROJECT_ROOT/.gk-solidity-sdk-native}"
GUEST_DIR="$PROJECT_ROOT/config/guest"
STAGED="$GUEST_DIR/${GK_E2E_GUEST_ELF:-answer.elf}"

EXAMPLE=src/examples/onchain-llm-native

if [ ! -e "$SDK_DIR/.git" ]; then
    git clone --depth 1 -b "$SDK_REF" "$SDK_REPO" "$SDK_DIR" >&2
    git -C "$SDK_DIR" submodule update --init --recursive --depth 1 >&2
fi

BINDING="$SDK_DIR/$EXAMPLE/gen/GkAnswer.sol"
EXPECTED=$(grep -o 'PROGRAM_HASH = 0x[0-9a-fA-F]\{64\}' "$BINDING" | grep -o '0x[0-9a-fA-F]*')
if [ -z "$EXPECTED" ]; then
    echo "PROGRAM_HASH not found in $BINDING" >&2
    exit 1
fi

if [ -n "${GK_GUEST_ELF:-}" ]; then
    ELF="$GK_GUEST_ELF"
    echo "Using prebuilt guest image $ELF" >&2
else
    echo "Building the guest image from $EXAMPLE/guest/answer.py..." >&2
    (cd "$SDK_DIR" && python3 tools/gk build "$EXAMPLE/guest/answer.py" \
        --out cache/gkvm/build/native-answer --no-binding) >&2
    ELF="$SDK_DIR/cache/gkvm/build/native-answer/guest.elf"
fi

ACTUAL=$(python3 "$SDK_DIR/tools/gk" hash "$ELF")
if [ "$ACTUAL" != "$EXPECTED" ]; then
    echo "guest image $ELF hashes to $ACTUAL but the consumer's binding commits to $EXPECTED" >&2
    exit 1
fi

mkdir -p "$GUEST_DIR"
cp "$ELF" "$STAGED"
chmod 644 "$STAGED"
echo "Staged $STAGED ($ACTUAL)" >&2
echo "GUEST_PROGRAM_HASH=$ACTUAL"
