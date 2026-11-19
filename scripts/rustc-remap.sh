#!/usr/bin/env bash
# Guest ELFs include source paths in panic locations. Normalize these paths
# before they can affect the image ID.
set -euo pipefail
rustc="$1"
shift

case " $* " in
  *' --target riscv32im-risc0-zkvm-elf '*|*' --target=riscv32im-risc0-zkvm-elf '*) ;;
  *) exec "$rustc" "$@" ;;
esac

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(cd "$here/../.." && pwd)/work"
# RISC Zero removes CARGO_* variables before spawning the nested Cargo.
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
[[ "$cargo_home" == /* ]] || cargo_home="$PWD/$cargo_home"

remaps=()
map_path() {
  local source="$1" destination="$2" physical
  remaps+=("--remap-path-prefix=$source=$destination")
  physical="$(cd "$source" && pwd -P)"
  if [[ "$physical" != "$source" ]]; then
    remaps+=("--remap-path-prefix=$physical=$destination")
  fi
}

map_path "$work" /build/work
map_path "$here" /build/outputs/reverse-settlement
map_path "$cargo_home" /cargo
exec "$rustc" "$@" "${remaps[@]}"
