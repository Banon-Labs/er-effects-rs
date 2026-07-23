#!/usr/bin/env bash
# Build hidden default naked-slot part binders for the Mushroom Man package.
#
# The mushroom body lives in FC_* files. If armor visuals are forced to the
# game's default no-armor rows, Elden Ring can still draw default naked HD/BD/AM/LG
# slot models. This helper zeroes those default slot FLVER face indices for both
# body types and LODs so no human head/body/arms/legs remain over the mushroom.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

output_root="target/mushroom-route-a-offline/hidden-naked-slots"
witchy="${WITCHY_BND:-}"

usage() {
	cat <<'EOF'
route_a_mushroom_build_hidden_naked_slots.sh

Build hidden HD/BD/AM/LG default naked-slot binders for M/F high/low variants.

Usage:
  bash scripts/route_a_mushroom_build_hidden_naked_slots.sh [--output-root path] [--witchy path]

Environment:
  WITCHY_BND   explicit WitchyBND.exe path
EOF
}

require_path() {
	local path="$1"
	if [[ ! -e "$path" ]]; then
		echo "missing required path: $path" >&2
		exit 1
	fi
}

find_witchy() {
	if [[ -n "$witchy" ]]; then
		printf '%s\n' "$witchy"
		return
	fi
	local candidate
	for candidate in \
		"$repo_root/.deps/WitchyBND/WitchyBND.exe" \
		"$repo_root/../WitchyBND/WitchyBND.exe" \
		"/mnt/d/Witchy BND/WitchyBND.exe"; do
		if [[ -f "$candidate" ]]; then
			printf '%s\n' "$candidate"
			return
		fi
	done
	echo "could not find WitchyBND.exe; pass --witchy or set WITCHY_BND" >&2
	exit 1
}

pack_with_witchy() {
	local input_dir="$1"
	local log_path="$2"
	local status=0
	"$witchy" -p "$input_dir" >"$log_path" 2>&1 || status=$?
	case "$status" in
	0 | 82) ;;
	*)
		tail -80 "$log_path" >&2 || true
		exit "$status"
		;;
	esac
}

while [[ "$#" -gt 0 ]]; do
	case "$1" in
	--output-root)
		output_root="${2:?--output-root requires a value}"
		shift 2
		;;
	--witchy)
		witchy="${2:?--witchy requires a value}"
		shift 2
		;;
	--help | -h)
		usage
		exit 0
		;;
	*)
		echo "unknown argument: $1" >&2
		usage >&2
		exit 2
		;;
	esac
done

witchy="$(find_witchy)"
require_path "$witchy"
require_path "scripts/route_a_mushroom_hide_flver_faces.rs"
require_path "target/mushroom-route-a-offline/er-naked-parts/hd_m_0000-partsbnd-dcx/HD_M_0000.flver"
require_path "target/mushroom-route-a-offline/er-naked-parts/am_m_0000-partsbnd-dcx/AM_M_0000.flver"
require_path "target/mushroom-route-a-offline/er-naked-parts/lg_m_0000-partsbnd-dcx/LG_M_0000.flver"
require_path "target/mushroom-route-a-offline/er-body0000/bd_m_0000-partsbnd-dcx/BD_M_0000.flver"

mkdir -p target "$output_root"
rustc scripts/route_a_mushroom_hide_flver_faces.rs -O -o target/route_a_mushroom_hide_flver_faces

python3 - "$output_root" <<'PY'
from __future__ import annotations

import shutil
import sys
from pathlib import Path

output_root = Path(sys.argv[1])
repo_root = Path.cwd()

sources = {
    ("hd", False): repo_root / "target/mushroom-route-a-offline/er-naked-parts/hd_m_0000-partsbnd-dcx",
    ("hd", True): repo_root / "target/mushroom-route-a-offline/er-naked-parts/hd_m_0000_l-partsbnd-dcx",
    ("bd", False): repo_root / "target/mushroom-route-a-offline/er-body0000/bd_m_0000-partsbnd-dcx",
    ("bd", True): repo_root / "target/mushroom-route-a-offline/er-body0000/bd_m_0000_l-partsbnd-dcx",
    ("am", False): repo_root / "target/mushroom-route-a-offline/er-naked-parts/am_m_0000-partsbnd-dcx",
    ("am", True): repo_root / "target/mushroom-route-a-offline/er-naked-parts/am_m_0000_l-partsbnd-dcx",
    ("lg", False): repo_root / "target/mushroom-route-a-offline/er-naked-parts/lg_m_0000-partsbnd-dcx",
    ("lg", True): repo_root / "target/mushroom-route-a-offline/er-naked-parts/lg_m_0000_l-partsbnd-dcx",
}
slot_roots = {
    "hd": "Head",
    "bd": "Body",
    "am": "Arm",
    "lg": "Leg",
}

for slot in ["hd", "bd", "am", "lg"]:
    for gender in ["m", "f"]:
        for low in [False, True]:
            source_dir = sources[(slot, low)]
            suffix = "_l" if low else ""
            source_flver = source_dir / f"{slot.upper()}_M_0000{suffix.upper()}.flver"
            if not source_flver.exists():
                raise SystemExit(f"missing source flver: {source_flver}")
            out_name = f"{slot}_{gender}_0000{suffix}"
            out_dir = output_root / f"{out_name}-partsbnd-dcx"
            if out_dir.exists():
                shutil.rmtree(out_dir)
            shutil.copytree(source_dir, out_dir)
            for flver in out_dir.glob("*.flver"):
                flver.unlink()
            target_flver_name = f"{slot.upper()}_{gender.upper()}_0000{suffix.upper()}.flver"
            shutil.copy2(source_flver, out_dir / target_flver_name)
            xml_path = out_dir / "_witchy-bnd4.xml"
            xml = xml_path.read_text(encoding="utf-8-sig")
            xml = xml.replace(f"<filename>{slot}_m_0000{suffix}.partsbnd.dcx</filename>", f"<filename>{out_name}.partsbnd.dcx</filename>")
            xml = xml.replace(f"<path>{slot.upper()}_M_0000{suffix.upper()}.flver</path>", f"<path>{target_flver_name}</path>")
            xml = xml.replace(
                f"N:\\GR\\data\\INTERROOT_win64\\parts\\{slot_roots[slot]}\\{slot.upper()}_M_0000",
                f"N:\\GR\\data\\INTERROOT_win64\\parts\\{slot_roots[slot]}\\{slot.upper()}_{gender.upper()}_0000",
            )
            xml_path.write_text(xml, encoding="utf-8")
PY

while IFS= read -r -d '' flver; do
	summary="${flver%.flver}-hide-summary.txt"
	./target/route_a_mushroom_hide_flver_faces \
		--source-flver "$flver" \
		--output-flver "$flver" \
		--summary "$summary" >/dev/null
done < <(find "$output_root" -name '*.flver' -print0)

shopt -s nullglob
for part_dir in "$output_root"/*-partsbnd-dcx; do
	pack_with_witchy "$part_dir" "${part_dir}-pack.log"
done
shopt -u nullglob

printf 'built hidden naked-slot binders in %s\n' "$output_root"
python3 - "$output_root" <<'PY'
from pathlib import Path
import sys
for path in sorted(Path(sys.argv[1]).glob('*.partsbnd.dcx')):
    print(f"  {path.name}")
PY
