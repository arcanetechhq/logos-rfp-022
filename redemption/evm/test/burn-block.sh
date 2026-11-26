#!/usr/bin/env bash
set -euo pipefail

if [ "$#" != 2 ]; then
  echo "usage: burn-block.sh BURN_BLOCK_RECEIPT RUN_DIRECTORY" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
WORK="$(cd "$ROOT/../.." && pwd)/work"
RECEIPT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
mkdir -p "$2"
RUN="$(cd "$2" && pwd)"
TARGET="$(cd "${CARGO_TARGET_DIR:-$ROOT/target}" && pwd)"

export RISC0_HOME="$WORK/risc0" RISC0_SERVER_PATH="$WORK/tools/r0vm"
export RISC0_EXECUTOR=ipc RISC0_PROVER=ipc RISC0_DEV_MODE=0

cd "$ROOT"
"$TARGET/debug/burn-block" fixture "$RECEIPT" "$RUN"
cp evidence/redemption-encoding-vector.borsh "$RUN/evidence/"
cd "$RUN"
if [ ! -s evidence/redemption-native-v2-evm-seal.bin ] ||
   [ ! -s evidence/redemption-native-v2-image-id.bin ] ||
   [ ! -s evidence/redemption-native-v2-journal.borsh ]; then
  if [ "${LIMIT_WRAP_MEMORY:-0}" = 1 ]; then
    export RFP022_REAL_DOCKER="$(command -v docker)"
    export PATH="$ROOT/scripts/redemption-prover:$PATH"
  fi
  NATIVE_V2_REDEMPTION=1 WRAP_GROTH16=1 "$TARGET/debug/redemption-runner"
fi

RFP022_EVM_CHECKPOINT="$RUN/evm-checkpoint.json" VERIFY_GROTH16=1 UWS_USE_FALLBACK=1 \
  node "$ROOT/redemption/evm/test/vault.cjs" "$WORK/redemption-evm"
