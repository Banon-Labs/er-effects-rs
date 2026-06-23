#!/usr/bin/env bash
# One-shot persistent import + auto-analysis of the dearxan-DEOBFUSCATED ER mapped image
# (eldenring-deobf.bin) into a reusable Ghidra project. Unlike the ermaporch DUMP project,
# this program's addresses are DEOBF-NATIVE (base 0x140000000, file offset == RVA) -- i.e.
# the same address space scripts/disas-deobf.sh / er_disasm use, with NO dump-vs-deobf shift.
# That makes it the right target for the RF function finder when you want VAs you can
# actually call/patch (run: scripts/ghidra/find-functions-rf.sh --proj-dir ... --proj-name erdeobf).
#
# The image is a RAW mapped blob (no PE headers), so it is imported with the Binary loader,
# x86-64, based at 0x140000000. Auto-analysis of a ~94MB blob is SLOW (many minutes) -- run
# this in the background. It is offline static analysis; there is no runtime-probe cap concern.
#
# Same tmpdir gotcha as the other Ghidra helpers: force java.io.tmpdir onto /home (the /tmp
# tmpfs is a near-full 32G and overflows when Ghidra unpacks/analyzes a large program).
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMG="$REPO_DIR/eldenring-deobf.bin"
PROJ=/home/banon/ghidra_maporch/proj-deobf
PROJ_NAME=erdeobf
TMP=/home/banon/ghidra_maporch/tmp
HEADLESS=/home/banon/tools/ghidra_12.1_PUBLIC/support/analyzeHeadless

if [[ ! -f "$IMG" ]]; then
  echo "deobf image not found: $IMG" >&2
  exit 2
fi

mkdir -p "$TMP" "$PROJ"
export TMPDIR="$TMP"
export GHIDRA_JAVA_OPTIONS="-Djava.io.tmpdir=$TMP"

"$HEADLESS" "$PROJ" "$PROJ_NAME" \
  -import "$IMG" \
  -loader BinaryLoader \
  -loader-baseAddr 0x140000000 \
  -processor x86:LE:64:default \
  -overwrite
echo "IMPORT_EXIT=$?"
