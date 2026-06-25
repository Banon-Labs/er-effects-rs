#!/usr/bin/env bash
set -euo pipefail
export SMITHBOX_BINARY_DIR=/home/banon/.local/share/smithbox/app
export PATH="$HOME/.dotnet:$PATH"
export DOTNET_CLI_TELEMETRY_OPTOUT=1
export DOTNET_NOLOGO=1
cd "$(dirname "$0")"
dotnet run -c Release -v quiet --property:MSBuildWarningsAsMessages=MSB3277 2>&1 | tail -200
