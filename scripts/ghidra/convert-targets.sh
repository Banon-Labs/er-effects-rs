#!/usr/bin/env bash
# Convert the autoload menu-open investigation deobf VAs to dump VAs.
set -euo pipefail
cd "$(dirname "$0")/../.."
for va in 0x14083acac 0x14083004d 0x1407b04c7 0x1407ad2bc 0x1407aa2bb 0x1407a747c 0x140793346 0x1409b24e0 0x1409275b0 0x140762d50 0x142b03550; do
  printf "deobf %s -> " "$va"
  uv run --with capstone python3 scripts/dump-deobf-shift.py --reverse "$va" 2>/dev/null | tail -1
done
