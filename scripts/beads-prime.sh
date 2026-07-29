#!/usr/bin/env bash
# Bounded `bd prime` for hooks (SessionStart, PreCompact) and any caller.
#
# Regenerates a titles-only `.beads/PRIME.md`, then runs `bd prime`, which emits
# that bounded file instead of the multi-MB default (every-memory-body) dump.
# Keeping regeneration here means the index is always fresh AND always bounded,
# and any bare `bd prime` (Pi, Codex, manual) also gets the bounded output.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Resolve the real bd binary rather than hard-coding one developer's home. A literal
# /home/<someone>/.local/bin/bd here made this hook -- and therefore every SessionStart
# and PreCompact -- fail outright for every other user on the machine.
# The bare `bd` command is deliberately NOT a candidate: it is an interactive-shell guard
# *function* that errors out unless BD_REAL_BIN is exported, and hooks run non-interactively.
resolve_bd() {
	local candidate
	for candidate in \
		"${BD_REAL_BIN:-}" \
		"$HOME/.local/bin/bd" \
		/home/*/.local/bin/bd \
		/usr/local/bin/bd; do
		if [[ -n "$candidate" && -x "$candidate" ]]; then
			printf '%s\n' "$candidate"
			return 0
		fi
	done
	return 1
}

if ! BD="$(resolve_bd)"; then
	echo "beads-prime: no bd binary found (set BD_REAL_BIN to override)" >&2
	exit 127
fi

# Regeneration is best-effort: a failure must never break the hook / prime.
# Pass the already-resolved bd so the generator never falls back to path guessing.
BD_REAL_BIN="$BD" python3 "$DIR/scripts/gen-beads-prime.py" > "$DIR/.beads/PRIME.md.tmp" 2>/dev/null \
  && mv -f "$DIR/.beads/PRIME.md.tmp" "$DIR/.beads/PRIME.md" \
  || rm -f "$DIR/.beads/PRIME.md.tmp"

exec "$BD" prime
