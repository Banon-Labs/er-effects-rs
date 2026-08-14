#!/usr/bin/env bash
# Disassemble a VA range from the dearxan-DEOBFUSCATED ER mapped image (repo-local, gitignored).
# Mapped image: file offset == RVA, image base 0x140000000 -> VA = offset + 0x140000000.
# Usage: scripts/disas-deobf.sh [--color=auto|always|never] <VA> [nbytes]
# Set ER_DISAS_COLOR=always to force ANSI escapes through non-TTY capture layers.
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMG="${ER_DEOBF_IMAGE:-$SCRIPT_DIR/../eldenring-deobf.bin}"
if [[ ! -f "$IMG" ]] && GIT_COMMON_DIR="$(git -C "$SCRIPT_DIR/.." rev-parse --path-format=absolute --git-common-dir 2>/dev/null)"; then
  WORKSPACE_IMAGE="$(dirname "$GIT_COMMON_DIR")/eldenring-deobf.bin"
  if [[ -f "$WORKSPACE_IMAGE" ]]; then
    IMG="$WORKSPACE_IMAGE"
  fi
fi
if [[ ! -f "$IMG" ]]; then
  echo "Missing deobfuscated image: $IMG (set ER_DEOBF_IMAGE to override)" >&2
  exit 1
fi
COLORIZER="$SCRIPT_DIR/colorize-disasm.py"
COLOR="${ER_DISAS_COLOR:-auto}"
if [[ "${1:-}" == --color=* ]]; then
  COLOR="${1#--color=}"
  shift
fi
if [[ $# -lt 1 ]]; then
  echo "Usage: scripts/disas-deobf.sh [--color=auto|always|never] <VA> [nbytes]" >&2
  exit 2
fi
VA=$(printf '%d' "$1"); N=$(printf '%d' "${2:-0x40}")
START=$VA
STOP=$((VA + N))
run_objdump() {
  objdump -D -b binary -m i386:x86-64 --adjust-vma=0x140000000 \
    --start-address="$START" --stop-address="$STOP" "$IMG" 2>/dev/null
}
if command -v python3 >/dev/null 2>&1 && [[ -f "$COLORIZER" ]]; then
  run_objdump | sed -n '/^ *[0-9a-f]\{6,\}:/p' | python3 "$COLORIZER" --color="$COLOR"
else
  run_objdump | sed -n '/^ *[0-9a-f]\{6,\}:/p'
fi
