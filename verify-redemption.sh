#!/usr/bin/env bash
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="$(cd "$HERE/../.." && pwd)/work"
BOOTSTRAP_ONLY=1 "$HERE/reproduce.sh"
cd "$HERE"
export RISC0_HOME="$WORK/risc0" RISC0_BUILD_LOCKED=1
export RISC0_DEV_MODE=0 RISC0_PROVER=ipc RISC0_EXECUTOR=ipc
export RISC0_SERVER_PATH="$WORK/tools/r0vm"
export RUSTFLAGS=--cap-lints=allow CARGO_TARGET_DIR="$HERE/target"

cargo +1.98.1 test --locked --manifest-path redemption/Cargo.toml -p redemption-core
cargo +1.98.1 build --locked --manifest-path redemption/Cargo.toml -p redemption-runner
EXECUTE_ONLY=1 ./target/debug/redemption-runner
cargo +1.98.1 build --locked --features bridge --bin bridge-exits
./target/debug/bridge-exits | tee evidence/redemption-actor-tests.log

# Use published, pinned EVM verifier bytecode generated from its source, in a local EVM.
EVM="$WORK/redemption-evm"
mkdir -p "$EVM/upstream"
cp redemption/evm/package.json redemption/evm/package-lock.json "$EVM/"
if [ ! -d "$EVM/node_modules/ganache" ]; then
  npm ci --prefix "$EVM" --cache "$WORK/redemption-npm-cache" --no-audit --no-fund
fi
if [ ! -f "$EVM/upstream/contracts/src/groth16/ControlID.sol" ]; then
  curl -fLsS 'https://codeload.github.com/risc0/risc0-ethereum/tar.gz/ff30da77ba4afdd496290ebebedb205348b15d83' -o "$EVM/risc0-ethereum.tgz"
  printf '%s  %s\n' '64ca2334dd6396fea7e5073cfd4db876d2a86600bf2daddb9f7171d23e3ac25b' "$EVM/risc0-ethereum.tgz" | shasum -a 256 -c -
  tar -xzf "$EVM/risc0-ethereum.tgz" --strip-components=1 -C "$EVM/upstream"
fi
UWS_USE_FALLBACK=1 node redemption/evm/test/vault.cjs "$EVM" | tee evidence/redemption-evm-tests.log

if [ "${VERIFY_NATIVE_V2_PROOF:-0}" = 1 ]; then
  PROOF_LOG=redemption-native-v2-generation
  if [ -n "${REUSE_REDEMPTION:-}" ]; then PROOF_LOG=redemption-native-v2-reverification; fi
  NATIVE_V2_REDEMPTION=1 ./target/debug/redemption-runner | tee "evidence/$PROOF_LOG.log"
fi

if [ "${VERIFY_NATIVE_REDEMPTION:-0}" = 1 ]; then
  cargo +1.98.1 build --locked --bin reverse-settlement-prototype
  NATIVE_REDEMPTION_FIXTURE=1 ./target/debug/reverse-settlement-prototype | tee evidence/redemption-native-settlement.log
  NATIVE_REDEMPTION_FIXTURE=1 ./target/debug/redemption-runner | tee evidence/redemption-native-proof.log
fi
