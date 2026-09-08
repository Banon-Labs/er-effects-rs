#!/usr/bin/env bash
# Move the forcing er-quickload-*.txt trigger files aside (off) or back (on), so a save-trace probe can
# run with the DLL doing only the env-gated diagnostics (no pab-advance / native-continue / offline /
# diagnostics forcing the boot). Usage: toggle-er-triggers.sh off|on
set -uo pipefail
MODE="${1:?usage: toggle-er-triggers.sh off|on}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
OFFDIR="$GAME_DIR/er-quickload-triggers-off"
FILES="er-quickload-native-continue.txt er-quickload-pab-advance.txt er-quickload-offline.txt er-quickload-grsysmsg-log.txt er-quickload-anti-antidebug.txt"
mkdir -p "$OFFDIR"
case "$MODE" in
  off)
    for f in $FILES; do
      if [ -f "$GAME_DIR/$f" ]; then mv -f "$GAME_DIR/$f" "$OFFDIR/"; echo "moved off: $f"; fi
    done
    ;;
  on)
    for f in $FILES; do
      if [ -f "$OFFDIR/$f" ]; then mv -f "$OFFDIR/$f" "$GAME_DIR/"; echo "restored: $f"; fi
    done
    ;;
  *) echo "unknown mode: $MODE" >&2; exit 2 ;;
esac
remaining=$(find "$GAME_DIR" -maxdepth 1 -name 'er-quickload-*.txt' 2>/dev/null | wc -l)
echo "remaining er-quickload-*.txt in game dir: $remaining"
