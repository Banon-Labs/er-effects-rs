#!/usr/bin/env bash
# One-shot setup of the Ghidra MCP + RF-finder stack on a fresh machine, so another user can
# get to the same capability without reconstructing every step. Idempotent: re-running skips
# work already done.
#
# WHAT THIS DOES (all in-repo + local, no sudo):
#   1. install Ghidra's MachineLearning extension (ships with Ghidra) for the RF finder
#   2. clone + build 13bm GhidraMCP from source (Go bridge + Java extension, our local patch)
#   3. install the built MCP extension into Ghidra
#   4. (optional) import + analyze the deobf binary into the persistent `erdeobf` project
#
# WHAT YOU MUST SUPPLY (NOT in the repo -- copyrighted game data):
#   - eldenring-deobf.bin            the dearxan-deobfuscated mapped image (repo-local, gitignored)
#   - (optional) the runtime gzf     for the `ermaporch` semantics project (set GZF=...)
# and the toolchain: a Ghidra 12.1 install, Go, a JDK 21, and gradle 8.14 (see checks below).
#
# Override any path via env: GHIDRA_INSTALL_DIR, MCP_SRC, JAVA_HOME, GRADLE, GHIDRA_MAPORCH.
# After this completes: start the daemon (scripts/ghidra/mcp-ghidra-daemon.sh start) and reload
# your MCP client so it picks up .mcp.json.
set -uo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GHIDRA_INSTALL_DIR=${GHIDRA_INSTALL_DIR:-/home/banon/tools/ghidra_12.1_PUBLIC}
MCP_SRC=${MCP_SRC:-/home/banon/tools/GhidraMCP-13bm}
GHIDRA_MAPORCH=${GHIDRA_MAPORCH:-/home/banon/ghidra_maporch}
DEOBF_BIN="$REPO_DIR/eldenring-deobf.bin"

ok()   { echo "  [ok]   $*"; }
warn() { echo "  [warn] $*" >&2; }
die()  { echo "  [FAIL] $*" >&2; exit 1; }

echo "== preflight =="
command -v go >/dev/null || die "go not found (needed to build the MCP bridge)"
ok "go: $(go version 2>&1 | awk '{print $3}')"
[[ -x "$GHIDRA_INSTALL_DIR/support/analyzeHeadless" ]] || die "Ghidra not at GHIDRA_INSTALL_DIR=$GHIDRA_INSTALL_DIR"
ok "ghidra: $GHIDRA_INSTALL_DIR"
# JDK 21 + gradle are needed only to build the MCP extension (step 3). build-ghidramcp.sh pins them.
[[ -n "${JAVA_HOME:-}" ]] || warn "JAVA_HOME unset; build-ghidramcp.sh defaults to a local JDK 21 -- edit it if absent"

echo "== 1/4 Ghidra MachineLearning extension =="
if [[ -d "$GHIDRA_INSTALL_DIR/Ghidra/Extensions/MachineLearning" ]]; then
  ok "already installed"
else
  ML_ZIP=$(ls "$GHIDRA_INSTALL_DIR"/Extensions/Ghidra/*MachineLearning*.zip 2>/dev/null | head -1)
  [[ -n "$ML_ZIP" ]] || die "MachineLearning extension zip not found under $GHIDRA_INSTALL_DIR/Extensions/Ghidra"
  unzip -oq "$ML_ZIP" -d "$GHIDRA_INSTALL_DIR/Ghidra/Extensions/" && ok "installed from $ML_ZIP"
fi

echo "== 2/4 clone 13bm GhidraMCP =="
if [[ -d "$MCP_SRC/.git" ]]; then
  ok "already cloned at $MCP_SRC"
else
  git clone --depth 1 https://github.com/13bm/GhidraMCP "$MCP_SRC" && ok "cloned" || die "clone failed"
fi

echo "== 3/4 build + install MCP (bridge + extension) =="
MCP_SRC="$MCP_SRC" GHIDRA_INSTALL_DIR="$GHIDRA_INSTALL_DIR" bash "$REPO_DIR/scripts/ghidra/build-ghidramcp.sh" \
  || die "build-ghidramcp.sh failed"
EXT_ZIP=$(ls "$MCP_SRC"/dist/*GhidraMCP*.zip 2>/dev/null | head -1)
if [[ -n "$EXT_ZIP" ]]; then
  unzip -oq "$EXT_ZIP" -d "$GHIDRA_INSTALL_DIR/Ghidra/Extensions/" && ok "installed MCP extension from $EXT_ZIP"
else
  warn "no built MCP extension zip found in $MCP_SRC/dist"
fi

echo "== 4/4 deobf project (erdeobf) =="
if [[ -d "$GHIDRA_MAPORCH/proj-deobf/erdeobf.rep" ]]; then
  ok "erdeobf project already built"
elif [[ -f "$DEOBF_BIN" ]]; then
  warn "importing + analyzing $DEOBF_BIN -- this is SLOW (~1-2h). Run in background:"
  echo "        bash $REPO_DIR/scripts/ghidra/import-deobf.sh   # watch: scripts/ghidra/tail-analysis-progress.sh"
else
  warn "deobf binary not present ($DEOBF_BIN) -- supply it, then run scripts/ghidra/import-deobf.sh"
fi

echo
echo "== done =="
echo "Next:"
echo "  - start the warm MCP server:  scripts/ghidra/mcp-ghidra-daemon.sh start"
echo "  - reload your MCP client so it picks up .mcp.json (the 'ghidra' server)"
echo "  - RF finder on the deobf project once built:"
echo "      scripts/ghidra/find-functions-rf.sh --proj-dir $GHIDRA_MAPORCH/proj-deobf --proj-name erdeobf"
