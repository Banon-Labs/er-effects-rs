#!/usr/bin/env bash
# Build the offline Route A mushroom prototype asset folders.
#
# This script never launches Elden Ring or Dark Souls. It compiles/runs the
# Rust-only Route A helper scripts, prepares high/low FC_M_0000 naked-body
# folders plus the earlier BD_M_1010 armor-body folders, patches their FLVER
# files with c2280 geometry, and stages c2280 texture bytes into the FC/BD
# donor TPF folders. WitchyBND packing is intentionally a separate
# phase because each pack/unpack step should stay individually bounded.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

proto_root="target/mushroom-route-a-offline/prototype"
mod_dir="$proto_root/mod"
profile_path="$proto_root/mushroom-route-a-assets.me3"
high_src="target/er-extract-parts-sample/bd_m_1010-partsbnd-dcx"
low_src="target/er-extract-parts-sample/bd_m_1010_l-partsbnd-dcx"
fc_high_src="target/mushroom-route-a-offline/er-naked-parts/fc_m_0000-partsbnd-dcx"
fc_low_src="target/mushroom-route-a-offline/er-naked-parts/fc_m_0000_l-partsbnd-dcx"
high_dst="$proto_root/bd_m_1010-mushroom-parts"
low_dst="$proto_root/bd_m_1010_l-mushroom-parts"
fc_high_dst="$proto_root/fc_m_0000-mushroom-parts"
fc_low_dst="$proto_root/fc_m_0000_l-mushroom-parts"
facegen_src="target/mushroom-route-a-offline/er-facegen/facegen-fgbnd-dcx"
facegen_dst="$proto_root/facegen-mushroom-fgbnd"
fg_face_src="target/mushroom-route-a-offline/er-face-parts/fg_a_0000_m-partsbnd-dcx"
fg_face_dst="$proto_root/fg_a_0000_m-mushroom-parts"

require_path() {
	local path="$1"
	if [[ ! -e "$path" ]]; then
		echo "missing required path: $path" >&2
		exit 1
	fi
}

copy_donor_payload() {
	local src="$1"
	local dst="$2"
	local flver_name="$3"
	require_path "$src"
	rm -rf "$dst"
	mkdir -p "$dst"
	shopt -s nullglob dotglob
	local item
	for item in "$src"/*; do
		if [[ "$(basename "$item")" == "$flver_name" ]]; then
			continue
		fi
		if [[ -d "$item" ]]; then
			cp -rf "$item" "$dst/"
		else
			cp -f "$item" "$dst/"
		fi
	done
	shopt -u nullglob dotglob
}

write_me3_profile() {
	local package_path
	package_path="$(wslpath -w "$mod_dir")"
	mkdir -p "$mod_dir/parts"
	cat >"$profile_path" <<EOF
profileVersion = "v1"
natives = []

[[supports]]
game = "eldenring"

[[packages]]
enabled = true
path = '$package_path'
load_after = []
load_before = []
EOF
	cat >"$mod_dir/README.txt" <<EOF
Offline Route A mushroom prototype mod payload.

ME3 profile:
  ../mushroom-route-a-assets.me3

The profile is intentionally asset-only:
  natives = []
  packages = this mod directory only

Expected payload after the separate WitchyBND pack phase:
  parts/fc_m_0000.partsbnd.dcx
  parts/fc_m_0000_l.partsbnd.dcx
  parts/bd_m_1010.partsbnd.dcx
  parts/bd_m_1010_l.partsbnd.dcx
  parts/fg_a_0000_m.partsbnd.dcx
  facegen/facegen.fgbnd.dcx

Runtime intent:
  Use an existing or new Body Type A / Wretch-style character through the normal naked-body path. FC_M_0000 is the primary naked/full-body target; BD_M_1010 remains an armor-body fallback. The build stages c2280 diffuse/spec/normal texture bytes into the FC and BD TPFs so the mushroom geometry is not left on the original human skin texture set. The FG_A_0000_M face-part override zeroes the rendered generated-face/head mesh; the facegen override also zeroes base face/eye face-set indices. No er_effects_rs.dll or unrelated repo DLL features are required by this profile.

This package has not been runtime-tested yet. No game launch is performed by the offline build scripts.
EOF
}

require_path "target/mushroom-route-a-offline/dsr/dsr-loose-mushroom/c2280-chrbnd-dcx/c2280.flver"
require_path "$high_src/BD_M_1010.flver"
require_path "$low_src/BD_M_1010_L.flver"
require_path "$fc_high_src/FC_M_0000.flver"
require_path "$fc_low_src/FC_M_0000_L.flver"
require_path "$facegen_src/face.flver"
require_path "$fg_face_src/FG_A_0000_M.flver"

mkdir -p "$proto_root"
rustc scripts/route_a_mushroom_export.rs -O -o target/route_a_mushroom_export
./target/route_a_mushroom_export >"$proto_root/export-smoke.out"

copy_donor_payload "$high_src" "$high_dst" "BD_M_1010.flver"
copy_donor_payload "$low_src" "$low_dst" "BD_M_1010_L.flver"
copy_donor_payload "$fc_high_src" "$fc_high_dst" "FC_M_0000.flver"
copy_donor_payload "$fc_low_src" "$fc_low_dst" "FC_M_0000_L.flver"
copy_donor_payload "$facegen_src" "$facegen_dst" "face.flver"
copy_donor_payload "$fg_face_src" "$fg_face_dst" "FG_A_0000_M.flver"

rustc scripts/route_a_mushroom_patch_donor.rs -O -o target/route_a_mushroom_patch_donor
./target/route_a_mushroom_patch_donor >"$proto_root/patch-smoke.out"
./target/route_a_mushroom_patch_donor \
	--donor-flver "$low_src/BD_M_1010_L.flver" \
	--output-flver "$low_dst/BD_M_1010_L.flver" \
	--summary "$proto_root/bd_m_1010_l-mushroom-parts-summary.txt" \
	>"$proto_root/patch-l-smoke.out"
./target/route_a_mushroom_patch_donor \
	--donor-flver "$fc_high_src/FC_M_0000.flver" \
	--output-flver "$fc_high_dst/FC_M_0000.flver" \
	--summary "$proto_root/fc_m_0000-mushroom-parts-summary.txt" \
	--donor-mesh-index 13 \
	>"$proto_root/patch-fc-smoke.out"
./target/route_a_mushroom_patch_donor \
	--donor-flver "$fc_low_src/FC_M_0000_L.flver" \
	--output-flver "$fc_low_dst/FC_M_0000_L.flver" \
	--summary "$proto_root/fc_m_0000_l-mushroom-parts-summary.txt" \
	--donor-mesh-index 13 \
	>"$proto_root/patch-fc-l-smoke.out"

rustc scripts/route_a_mushroom_hide_flver_faces.rs -O -o target/route_a_mushroom_hide_flver_faces
./target/route_a_mushroom_hide_flver_faces \
	--source-flver "$facegen_src/face.flver" \
	--output-flver "$facegen_dst/face.flver" \
	--summary "$proto_root/facegen-mushroom-summary.txt" \
	>"$proto_root/hide-facegen-smoke.out"
./target/route_a_mushroom_hide_flver_faces \
	--source-flver "$fg_face_src/FG_A_0000_M.flver" \
	--output-flver "$fg_face_dst/FG_A_0000_M.flver" \
	--summary "$proto_root/fg_a_0000_m-mushroom-summary.txt" \
	>"$proto_root/hide-fg-a-0000-m-smoke.out"

rustc scripts/route_a_mushroom_stage_textures.rs -O -o target/route_a_mushroom_stage_textures
./target/route_a_mushroom_stage_textures >"$proto_root/stage-textures-smoke.out"
./target/route_a_mushroom_stage_textures \
	--parts-dir "$low_dst" \
	--tpf-dir-name BD_M_1010_L-tpf \
	--tpf-filename BD_M_1010_L.tpf \
	--texture-suffix _l \
	>"$proto_root/stage-textures-l-smoke.out"
./target/route_a_mushroom_stage_textures \
	--parts-dir "$fc_high_dst" \
	--tpf-dir-name FC_M_0000-tpf \
	--tpf-filename FC_M_0000.tpf \
	--target-prefix FC_M_0000 \
	--manifest-kind fc \
	>"$proto_root/stage-textures-fc-smoke.out"
./target/route_a_mushroom_stage_textures \
	--parts-dir "$fc_low_dst" \
	--tpf-dir-name FC_M_0000_L-tpf \
	--tpf-filename FC_M_0000_L.tpf \
	--texture-suffix _l \
	--target-prefix FC_M_0000 \
	--manifest-kind fc \
	>"$proto_root/stage-textures-fc-l-smoke.out"

write_me3_profile

printf 'built offline mushroom asset folders:\n'
printf '  %s\n' "$fc_high_dst" "$fc_low_dst" "$high_dst" "$low_dst" "$facegen_dst" "$fg_face_dst"
printf 'wrote decoupled asset-only me3 profile:\n'
printf '  %s\n' "$profile_path"
printf 'next pack phase: pack each *-tpf folder with WitchyBND, pack each *-mushroom-parts folder into %s/parts, and pack facegen-mushroom-fgbnd into %s/facegen.\n' "$mod_dir" "$mod_dir"
