#!/usr/bin/env bash
set -euo pipefail

inbound="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd -- "$inbound/.." && pwd)"
attestation="$(cd -- "$repo/../logos-rfp-021" && pwd)"
policy="${1:?provide an absolute policy.json path}"
target="${2:?provide an absolute build directory}"
test -f "$policy"
test "$(git -C "$attestation" rev-parse HEAD)" = 18eea809d7f96291898edded2628babe7b2a9537

cpp="$attestation/.tools/risc0-cpp-2024.1.5/bin"
export RUSTC="$attestation/.tools/risc0-rust-1.94.1/bin/rustc"
export CC_riscv32im_risc0_zkvm_elf="$cpp/riscv32-unknown-elf-gcc"
export AR_riscv32im_risc0_zkvm_elf="$cpp/riscv32-unknown-elf-ar"
export CFLAGS_riscv32im_risc0_zkvm_elf="-march=rv32im -nostdlib"
export RUSTC_WRAPPER="" RUSTC_WORKSPACE_WRAPPER=""
export CARGO_TARGET_DIR="$target" CARGO_INCREMENTAL=0 SOURCE_DATE_EPOCH=0 TZ=UTC LC_ALL=C
export RISC0_FEATURE_bigint2="" RFP022_INBOUND_POLICY="$policy"

flags=(-C passes=lower-atomic -C link-arg=-Ttext=0x00200800 -C panic=abort --cfg 'getrandom_backend="custom"')
flags+=(--remap-path-prefix "$attestation=/arcane/021")
flags+=(--remap-path-prefix "$repo=/arcane/022")
flags+=(--remap-path-prefix "$target=/arcane/target")
flags+=(--remap-path-prefix "${CARGO_HOME:-$HOME/.cargo}=/arcane/cargo")
flags+=(--remap-path-prefix "$(dirname -- "$policy")=/arcane/input")
CARGO_ENCODED_RUSTFLAGS="$(IFS=$'\x1f'; printf '%s' "${flags[*]}")"
export CARGO_ENCODED_RUSTFLAGS

cargo +1.98.1 build --locked --manifest-path "$inbound/Cargo.toml" --release \
    --features guest --bin inbound-actor --target riscv32im-risc0-zkvm-elf -j 4
