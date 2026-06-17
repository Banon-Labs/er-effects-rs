#!/usr/bin/env bash
# Disassemble a function in the on-disk decrypted eldenring.exe by virtual address.
# Usage: disas-va.sh <VA hex, e.g. 0x140792460> [num_bytes_dec, default 256]
# .text mapping (verified): VA = file_offset + 0x140000a00  =>  foff = VA - 0x140000a00
set -euo pipefail
EXE="/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game/eldenring.exe"
VA="$1"
NB="${2:-256}"
BASE_DELTA=$((0x140000a00))
va_dec=$((VA))
objdump -D -b binary -m i386:x86-64 -M intel \
  --adjust-vma="$BASE_DELTA" \
  --start-address=$((va_dec)) \
  --stop-address=$((va_dec + NB)) \
  "$EXE" | grep -E '^\s+[0-9a-f]+:'
