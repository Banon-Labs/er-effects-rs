#!/usr/bin/env bash
# Run the win-x64 extractor under wine so oo2core (Oodle Kraken) loads natively.
# Args are forwarded to extract.exe: <logicalPath|--shaders> [outDir]
# Linux paths are reachable from wine via the Z: drive.
set -euo pipefail

SMITHBOX_LINUX=/home/banon/.local/share/smithbox/app
GAME_LINUX="${ER_GAME_DIR:-/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
PUBDIR="$(dirname "$0")/bin/Release/net10.0/win-x64/publish"

# Oodle DLL must sit beside the exe (Windows loader searches the exe dir first).
cp -f "$SMITHBOX_LINUX/oo2core_6_win64.dll" "$PUBDIR/" 2>/dev/null || true

export WINEPREFIX="${WINEPREFIX:-/home/banon/.local/share/smithbox/wineprefix}"
export WINEDEBUG=-all
# Suppress wine's crash dialog / winedbg backtrace spam (headless: no GUI popups).
unset DISPLAY WAYLAND_DISPLAY
export WINEDLLOVERRIDES="winedbg.exe=d"
# Transitive Andre deps resolve from the Smithbox install (Z: maps the Linux root).
export SMITHBOX_BINARY_DIR='Z:\home\banon\.local\share\smithbox\app'

WINE_BIN="$(command -v wine || echo /usr/sbin/wine)"
to_z() { printf 'Z:%s' "$(printf '%s' "$1" | tr '/' '\\')"; }

GAME_WIN="$(to_z "$GAME_LINUX")"
ARGS=()
for a in "$@"; do
  case "$a" in
    /tmp/*|/home/*) ARGS+=("$(to_z "$a")") ;;  # host filesystem out-dir -> Z: drive
    *) ARGS+=("$a") ;;                          # VFS logical path (/shader/...) or flag, as-is
  esac
done

cd "$PUBDIR"
exec "$WINE_BIN" extract.exe "$GAME_WIN" "${ARGS[@]}"
