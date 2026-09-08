#!/usr/bin/env bash
# Bounded `bd prime` for hooks (SessionStart, PreCompact) and any caller.
#
# Regenerates a small `.beads/PRIME.md` (see scripts/gen-beads-prime.py), then runs
# `bd prime`, which emits that bounded file instead of the multi-MB default
# (every-memory-body) dump. Keeping regeneration here means the index is always fresh
# and always bounded, and any bare `bd prime` (Pi, Codex, manual) also gets it.
#
# Regeneration is skipped when PRIME.md is younger than BEADS_PRIME_MAX_AGE_SECONDS
# (default 6h). It costs a 4.6 MB `bd memories --json` read plus a 212 KB `bd ready
# --json` read, and this hook also fires on PreCompact -- the one moment where the
# session is already under pressure and the memory index has not meaningfully changed
# since SessionStart. Set BEADS_PRIME_MAX_AGE_SECONDS=0 to force regeneration.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PRIME="$DIR/.beads/PRIME.md"
INDEX="$DIR/.beads/PRIME-memory-index.txt"
MAX_AGE="${BEADS_PRIME_MAX_AGE_SECONDS:-21600}"

# Resolve the real bd binary rather than hard-coding one developer's home. A literal
# /home/<someone>/.local/bin/bd here made this hook -- and therefore every SessionStart
# and PreCompact -- fail outright for every other user on the machine.
# The bare `bd` command is deliberately not a candidate: it is an interactive-shell guard
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

needs_regen() {
	[[ -s "$PRIME" && -s "$INDEX" ]] || return 0
	# A non-numeric or zero MAX_AGE means "always regenerate" rather than "crash the hook".
	[[ "$MAX_AGE" =~ ^[0-9]+$ ]] || return 0
	((MAX_AGE > 0)) || return 0
	local age
	age=$(($(date +%s) - $(date -r "$PRIME" +%s)))
	((age >= MAX_AGE))
}

if ! BD="$(resolve_bd)"; then
	echo "beads-prime: no bd binary found (set BD_REAL_BIN to override)" >&2
	exit 127
fi

# Regeneration is best-effort: a failure must never break the hook / prime.
# Pass the already-resolved bd so the generator never falls back to path guessing.
if needs_regen; then
	if BD_REAL_BIN="$BD" python3 "$DIR/scripts/gen-beads-prime.py" --index "$INDEX" \
		>"$PRIME.tmp" 2>/dev/null; then
		mv -f "$PRIME.tmp" "$PRIME" || rm -f "$PRIME.tmp"
	else
		rm -f "$PRIME.tmp"
	fi
fi

exec "$BD" prime
