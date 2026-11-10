#!/usr/bin/env bash
set -euo pipefail
mkdir -p "${RISC0_METRICS_DIR:?set metrics directory}"
LOG="$(mktemp "$RISC0_METRICS_DIR/r0vm.XXXXXX")"
if [ "$(uname -s)" = Darwin ]; then FLAG=-l; else FLAG=-v; fi
exec /usr/bin/time "$FLAG" "${RISC0_REAL_SERVER_PATH:?set pinned r0vm path}" "$@" 2> "$LOG"
