#!/usr/bin/env bash
# Convert the online-mode global + guard + TitleTop ctor dump VAs to deobf VAs.
set -euo pipefail
cd "$(dirname "$0")/../.."
for va in 0x144588afc 0x144588b00 0x1409a82d0 0x1409b3050 0x1408377f0 0x140cab4f0; do
  printf "dump %s -> " "$va"
  uv run --with capstone python3 scripts/dump-deobf-shift.py "$va" 2>/dev/null | tail -1
done
