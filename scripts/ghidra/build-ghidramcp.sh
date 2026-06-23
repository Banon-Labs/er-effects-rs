#!/usr/bin/env bash
# Build the 13bm GhidraMCP server FROM SOURCE (both halves):
#   - the Go bridge   (mcp-bridge/mcp_bridge)   -- the native binary the plugin auto-launches
#   - the Ghidra Java extension (dist/*.zip)     -- installed into Ghidra to expose 70 RE tools
#
# We build from source rather than using the prebuilt GitHub release so the auto-launched
# native bridge binary is ours. Ghidra's gradle build needs a JDK 21 toolchain (the system
# JDK here is 26, which gradle 8.14 will not run on), so we pin JAVA_HOME to the local JDK 21.
#
# Prereqs (all local, no sudo): go, the cloned repo, a local gradle 8.14 and JDK 21.
# Adjust the paths below if those move.
set -euo pipefail

MCP_SRC=${MCP_SRC:-/home/banon/tools/GhidraMCP-13bm}
export JAVA_HOME=${JAVA_HOME:-/home/banon/tools/jdk-21.0.11+10}
export GHIDRA_INSTALL_DIR=${GHIDRA_INSTALL_DIR:-/home/banon/tools/ghidra_12.1_PUBLIC}
GRADLE=${GRADLE:-/home/banon/tools/gradle-8.14/bin/gradle}
export PATH="$JAVA_HOME/bin:$PATH"

cd "$MCP_SRC"

echo "== building Go bridge =="
( cd mcp-bridge && go build -o mcp_bridge . )
echo "bridge: $MCP_SRC/mcp-bridge/mcp_bridge"

echo "== building Ghidra extension (JDK 21 toolchain) =="
"$GRADLE" buildExtension --console=plain

echo "== extension ZIP(s) =="
ls -1 "$MCP_SRC"/dist/*.zip
