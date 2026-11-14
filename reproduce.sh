#!/usr/bin/env bash
# Run from the delivered outputs/reverse-settlement/ layout, including after archive extraction.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
WORK="$ROOT/work"
mkdir -p "$WORK/upstream" "$WORK/tools" "$HERE/evidence"
checkout() {
  local name="$1" url="$2" rev="$3"
  if [ ! -d "$WORK/upstream/$name/.git" ]; then
    git clone --filter=blob:none "$url" "$WORK/upstream/$name"
    git -C "$WORK/upstream/$name" checkout --detach "$rev"
  fi
  test "$(git -C "$WORK/upstream/$name" rev-parse HEAD)" = "$rev"
  test -z "$(git -C "$WORK/upstream/$name" status --porcelain)"
}
checkout lez https://github.com/logos-blockchain/logos-execution-zone.git 14cb5d092e70c3d1fc014d4921d0b13bea197b5d
checkout sdk https://github.com/logos-blockchain/logos-zone-sdk.git 1b5073a10caaa63cbbd825bd4397f2f1fb48b33a
checkout blockchain https://github.com/logos-blockchain/logos-blockchain.git a159f8c155df68d7e2f045a418ae737de246354e
cmp "$HERE/acceptance-core/src/native/merkle_tree/mod.rs" "$WORK/upstream/lez/lee/state_machine/src/merkle_tree/mod.rs"
cmp "$HERE/acceptance-core/src/native/merkle_tree/default_values.rs" "$WORK/upstream/lez/lee/state_machine/src/merkle_tree/default_values.rs"
cmp "$HERE/acceptance-core/src/encoding/persistent.rs" "$WORK/upstream/lez/lee/state_machine/src/encoding/persistent.rs"
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) R0_ARCH=aarch64-apple-darwin; RZ_ARCH=macOS-ARM64; TIME_FLAG=-l; ARCHIVE_SHA=3b50b0393b8e4158e7129add4cfe0cbc536e994b5abc6a3a435e1e6b841c0990 ;;
  Linux-x86_64) R0_ARCH=x86_64-unknown-linux-gnu; RZ_ARCH=Linux-X64; TIME_FLAG=-v; ARCHIVE_SHA=415ca205d4408f5d768b9e4d623f2aac6b869329a6cf965f0c57a88a6dbb5871 ;;
  *) echo 'This bootstrap supports macOS arm64 and Linux x86_64.' >&2; exit 1 ;;
esac
if [ ! -x "$WORK/tools/r0vm" ]; then
  curl -fL "https://github.com/risc0/risc0/releases/download/v3.1.0/cargo-risczero-$R0_ARCH.tgz" -o "$WORK/tools/risc0.tgz"
  printf "%s  %s\n" "$ARCHIVE_SHA" "$WORK/tools/risc0.tgz" | shasum -a 256 -c -
  tar -xzf "$WORK/tools/risc0.tgz" -C "$WORK/tools"
fi
"$WORK/tools/r0vm" --version | tee "$HERE/evidence/r0vm-version.txt"
"$WORK/tools/r0vm" --version | rg -q '3\.1\.0$'
if [ ! -x "$WORK/tools/rzup" ]; then
  curl -fL "https://artifacts.risczero.net/rzup/prod/$RZ_ARCH/rzup" -o "$WORK/tools/rzup"
  chmod +x "$WORK/tools/rzup"
fi
export RISC0_HOME="$WORK/risc0" RISC0_DEV_MODE=0
export RISC0_PROVER=ipc RISC0_EXECUTOR=ipc
export RISC0_BUILD_LOCKED=1
export RISC0_REAL_SERVER_PATH="$WORK/tools/r0vm"
export RISC0_SERVER_PATH="$HERE/scripts/r0vm-measured.sh" RISC0_METRICS_DIR="$HERE/evidence/prover-resources"
export RUSTFLAGS='--cap-lints=allow'
# Caps only lints: no validation, proving or verification is disabled.
if ! "$WORK/tools/rzup" show | rg -q '1\.91\.1'; then "$WORK/tools/rzup" install rust 1.91.1; fi
"$WORK/tools/rzup" default rust 1.91.1
rustup toolchain install 1.98.1 --profile minimal
if [ "${BOOTSTRAP_ONLY:-0}" = 1 ]; then exit 0; fi
cd "$HERE"
# The default always produces fresh cryptographic proofs. REUSE_PROOFS=1 is an explicit fast replay.
cargo +1.98.1 build --locked --bin reverse-settlement-prototype 2>&1 | tee evidence/build.log
/usr/bin/time "$TIME_FLAG" ./target/debug/reverse-settlement-prototype 2> evidence/settlement-time.log | tee evidence/settlement.log
cargo +1.98.1 build --locked --manifest-path native-auth/Cargo.toml 2>&1 | tee evidence/auth-build.log
/usr/bin/time "$TIME_FLAG" ./native-auth/target/debug/native-auth evidence/accepted-burn-block.borsh > evidence/native-auth.log 2> evidence/native-auth-time.log
cat evidence/native-auth.log
cargo +1.98.1 build --locked --features checkpoint --bin checkpoint 2>&1 | tee evidence/checkpoint-build.log
/usr/bin/time "$TIME_FLAG" ./target/debug/checkpoint 2> evidence/checkpoint-time.log | tee evidence/checkpoint.log
cargo +1.98.1 test --locked --features checkpoint --bin checkpoint > evidence/gate-tests.log 2>&1
cat evidence/gate-tests.log
