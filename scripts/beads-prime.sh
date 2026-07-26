#!/usr/bin/env bash
# Bounded `bd prime` for hooks (SessionStart, PreCompact) and any caller.
#
# Regenerates a titles-only `.beads/PRIME.md`, then runs `bd prime`, which emits
# that bounded file instead of the multi-MB default (every-memory-body) dump.
# Keeping regeneration here means the index is always fresh AND always bounded,
# and any bare `bd prime` (Pi, Codex, manual) also gets the bounded output.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BD="${BD_REAL_BIN:-/home/choza/.local/bin/bd}"

# Regeneration is best-effort: a failure must never break the hook / prime.
python3 "$DIR/scripts/gen-beads-prime.py" > "$DIR/.beads/PRIME.md.tmp" 2>/dev/null \
  && mv -f "$DIR/.beads/PRIME.md.tmp" "$DIR/.beads/PRIME.md" \
  || rm -f "$DIR/.beads/PRIME.md.tmp"

exec "$BD" prime
