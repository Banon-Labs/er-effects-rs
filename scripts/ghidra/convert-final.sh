#!/usr/bin/env bash
# Convert dump VAs (semantics) -> deobf VAs (addresses) for the final report.
set -euo pipefail
cd "$(dirname "$0")/../.."
for va in 0x14082ff10 0x14082ce50 0x140765120 0x14082d1c0 0x140837500 0x14082d800 0x14082d5a0 0x14082c860 0x140e563c0 0x1407a7340 0x14082dd60 0x14082ecd0 0x1407b8260 0x140762e40; do
  printf "dump %s -> " "$va"
  uv run --with capstone python3 scripts/dump-deobf-shift.py "$va" 2>/dev/null | tail -1
done
