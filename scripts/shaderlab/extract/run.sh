#!/usr/bin/env bash
# Usage: run.sh <logicalPath> [outDir]
#        run.sh --list <regex> [dir]
set -euo pipefail
export SMITHBOX_BINARY_DIR=/home/banon/.local/share/smithbox/app
export PATH="$HOME/.dotnet:$PATH"
export DOTNET_CLI_TELEMETRY_OPTOUT=1
export DOTNET_NOLOGO=1
GAME_DIR="${ER_GAME_DIR:-/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
cd "$(dirname "$0")"
exec dotnet run -c Release -v quiet --property:WarningLevel=0 --property:MSBuildWarningsAsMessages=MSB3277 -- "$GAME_DIR" "$@"
