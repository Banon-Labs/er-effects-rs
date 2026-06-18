#!/usr/bin/env bash
# Disassemble a VA range from the dearxan-DEOBFUSCATED ER mapped image (repo-local, gitignored).
# Mapped image: file offset == RVA, image base 0x140000000 -> VA = offset + 0x140000000.
# Usage: scripts/disas-deobf.sh <VA> [nbytes]
set -uo pipefail
IMG="$(dirname "$0")/../eldenring-deobf.bin"
VA=$(printf '%d' "$1"); N=$(printf '%d' "${2:-0x40}")
START=$VA
STOP=$((VA + N))
objdump -D -b binary -m i386:x86-64 --adjust-vma=0x140000000 \
  --start-address="$START" --stop-address="$STOP" "$IMG" 2>/dev/null \
  | sed -n '/^ *[0-9a-f]\{6,\}:/p'
