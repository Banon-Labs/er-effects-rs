#!/usr/bin/env bash
set -euo pipefail

ME3_REAL_BIN="${ME3_REAL_BIN:-$HOME/.local/bin/me3}"

if [[ ! -x "$ME3_REAL_BIN" ]]; then
  echo "me3-quiet: missing executable ME3_REAL_BIN=$ME3_REAL_BIN" >&2
  exit 127
fi

exec "$ME3_REAL_BIN" --quiet "$@"
