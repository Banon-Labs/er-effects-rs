#!/usr/bin/env bash
# CLI wrapper for scripts/find-save-char.py -- find ER saves containing a named
# character under a root dir, with slot/level/runes/top-weapon-upgrade.
#   scripts/find-save-char.sh <root-dir> '<name>' [--exact] [--json]
#   e.g. scripts/find-save-char.sh ./ 'angrE'
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "$HERE/find-save-char.py" "$@"
