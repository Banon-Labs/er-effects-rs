#!/usr/bin/env bash
# Decode the real ER0000.sl2 save (active Steam account) via save-slot-oracle.py and dump every
# slot's character identity, so a probe-environment save-presence check can confirm the real
# character is actually staged where the launched game reads it. Writes a data artifact to the path
# given as $1 (default: a /tmp scratch file).
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
SAVE="${SAVE:-$HOME/.local/share/Steam/steamapps/compatdata/1245620/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing/76561197986456766/ER0000.sl2}"
OUT="${1:-/tmp/oracle-out.txt}"
{
  echo "=== save: $SAVE ==="
  stat -c 'size=%s mtime=%y' "$SAVE" 2>&1
  echo "=== python: $(python3 --version 2>&1) ==="
  for slot in 0 1 2 3 4 5; do
    echo "--- slot $slot ---"
    python3 scripts/save-slot-oracle.py --save "$SAVE" --slot "$slot" 2>&1
    echo "(exit $?)"
  done
} > "$OUT" 2>&1
echo "wrote $OUT"
