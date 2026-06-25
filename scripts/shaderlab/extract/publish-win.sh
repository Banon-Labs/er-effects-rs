#!/usr/bin/env bash
# Publish the extractor as a self-contained win-x64 exe so it can run under wine,
# where Elden Ring's oo2core_6_win64.dll (Oodle Kraken) loads natively.
set -euo pipefail
export PATH="$HOME/.dotnet:$PATH"
export DOTNET_CLI_TELEMETRY_OPTOUT=1
export DOTNET_NOLOGO=1
cd "$(dirname "$0")"
dotnet publish -c Release -r win-x64 --self-contained true \
  -v quiet --property:MSBuildWarningsAsMessages=MSB3277 \
  -p:PublishSingleFile=false 2>&1 | tail -20
echo "PUBLISH_EXIT=${PIPESTATUS[0]}"
