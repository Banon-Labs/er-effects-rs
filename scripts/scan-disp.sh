#!/usr/bin/env bash
# Scan a .text VA range for instructions referencing a given displacement pattern.
# Usage: scan-disp.sh <start VA hex> <nbytes dec> <regex>
set -euo pipefail
EXE="/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game/eldenring.exe"
START="$1"; NB="$2"; PAT="$3"
objdump -D -b binary -m i386:x86-64 -M intel \
  --adjust-vma=0x140000a00 \
  --start-address=$(($START)) \
  --stop-address=$(($START + NB)) \
  "$EXE" | rtk grep "$PAT"
