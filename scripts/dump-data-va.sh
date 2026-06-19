#!/usr/bin/env bash
# Dump raw bytes from a .data/.rdata VA in the on-disk decrypted eldenring.exe.
# DATA/.rdata mapping (verified): VA = file_offset + 0x140000000 => foff = VA - 0x140000000
# Usage: dump-data-va.sh <VA hex> [num_bytes_dec, default 176]
set -euo pipefail
EXE="/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game/eldenring.exe"
VA="$1"
NB="${2:-176}"
va_dec=$((VA))
objdump -s -b binary -m i386:x86-64 \
  --adjust-vma=0x140000000 \
  --start-address=$((va_dec)) \
  --stop-address=$((va_dec + NB)) \
  "$EXE"
