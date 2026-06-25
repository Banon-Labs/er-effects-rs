#!/usr/bin/env bash
# Wrapper for the prebuilt Linux DirectX Shader Compiler (dxc).
# Disassemble a compiled ER shader member:  dxc.sh -dumpbin <member.vpo>
set -euo pipefail
DXC_ROOT="${DXC_ROOT:-/home/banon/tools/dxc}"
export LD_LIBRARY_PATH="$DXC_ROOT/lib:${LD_LIBRARY_PATH:-}"
exec "$DXC_ROOT/bin/dxc" "$@"
