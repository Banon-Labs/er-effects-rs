#!/usr/bin/env bash
# Build and optionally launch an asset-only ME3 profile from the Blender EDIT_ME mesh.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

label="blender-edit"
launch_after=false
blend_path="target/mushroom-route-a-offline/blender-compare/mushroom_raw_game_compare.blend"
blender_exe="/mnt/c/Program Files/Blender Foundation/Blender 4.4/blender.exe"
witchy="/mnt/d/Witchy BND/WitchyBND.exe"
me3_exe="/mnt/c/Users/choza/AppData/Local/garyttierney/me3/bin/me3.exe"

fc_high_src="target/mushroom-route-a-offline/er-naked-parts/fc_m_0000-partsbnd-dcx"
fc_low_src="target/mushroom-route-a-offline/er-naked-parts/fc_m_0000_l-partsbnd-dcx"
bd_high_fallback="target/mushroom-route-a-offline/prototype/mod/parts/bd_m_1010.partsbnd.dcx"
bd_low_fallback="target/mushroom-route-a-offline/prototype/mod/parts/bd_m_1010_l.partsbnd.dcx"
fc_high_tpf_fallback="target/mushroom-route-a-offline/prototype/fc_m_0000-mushroom-parts/FC_M_0000.tpf"
fc_low_tpf_fallback="target/mushroom-route-a-offline/prototype/fc_m_0000_l-mushroom-parts/FC_M_0000_L.tpf"
facegen_fallback="target/mushroom-route-a-offline/prototype/mod/facegen/facegen.fgbnd.dcx"
fg_face_fallback="target/mushroom-route-a-offline/prototype/mod/parts/fg_a_0000_m.partsbnd.dcx"

usage() {
	cat <<'EOF'
build_mushroom_blender_edit.sh

Build an asset-only ME3 profile from the editable mesh in the raw-game Blender scene.

Usage:
  bash scripts/build_mushroom_blender_edit.sh [--label name] [--blend path] [--launch]

Defaults:
  --label blender-edit
  --blend target/mushroom-route-a-offline/blender-compare/mushroom_raw_game_compare.blend
EOF
}

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

run_witchy_pack() {
	local work_dir="$1"
	local label_name="$2"
	local input_path="$3"
	local log_path="$work_dir/pack-${label_name}.log"
	set +e
	"$witchy" -p "$input_path" >"$log_path" 2>&1
	local status=$?
	set -e
	if [[ "$status" -ne 0 && "$status" -ne 82 ]]; then
		tail -80 "$log_path" >&2 || true
		exit "$status"
	fi
}

write_me3_profile() {
	local profile_path="$1"
	local mod_dir="$2"
	local package_path
	package_path="$(wslpath -w "$(realpath -m "$mod_dir")")"
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
}

print_launch_command() {
	local profile_path="$1"
	local profile_win
	profile_win="$(wslpath -w "$(realpath -m "$profile_path")")"
	printf '\nlaunch command:\n'
	printf 'cd %q && %q launch -g eldenring --online false -p %q\n' \
		"$repo_root" \
		"$me3_exe" \
		"$profile_win"
}

while [[ "$#" -gt 0 ]]; do
	case "$1" in
	--label)
		label="${2:?--label requires a value}"
		shift 2
		;;
	--blend)
		blend_path="${2:?--blend requires a value}"
		shift 2
		;;
	--launch)
		launch_after=true
		shift
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

if [[ ! "$label" =~ ^[A-Za-z0-9._-]+$ ]]; then
	echo "label must contain only letters, numbers, dots, underscores, or hyphens: $label" >&2
	exit 2
fi

require_path "$blender_exe"
require_path "$witchy"
require_path "$me3_exe"
require_path "$blend_path"
require_path "$fc_high_src/FC_M_0000.flver"
require_path "$fc_low_src/FC_M_0000_L.flver"
require_path "$bd_high_fallback"
require_path "$bd_low_fallback"
require_path "$fc_high_tpf_fallback"
require_path "$fc_low_tpf_fallback"
require_path "$facegen_fallback"
require_path "$fg_face_fallback"

variant_dir="target/mushroom-route-a-offline/blender-edit/$label"
export_dir="$variant_dir/export"
fc_high_dst="$variant_dir/fc_m_0000-blender-edit-parts"
fc_low_dst="$variant_dir/fc_m_0000_l-blender-edit-parts"
mod_dir="$variant_dir/mod"
profile_path="$variant_dir/mushroom-blender-edit-$label.me3"

rm -rf "$variant_dir"
mkdir -p "$export_dir" "$mod_dir/parts" "$mod_dir/facegen"

"$blender_exe" --background "$(wslpath -w "$(realpath -m "$blend_path")")" \
	--python "$(wslpath -w scripts/export_mushroom_blender_edit.py)" -- \
	--output-dir "$(wslpath -w "$(realpath -m "$export_dir")")" \
	>"$variant_dir/export-blender-edit.log" 2>&1

rustc scripts/route_a_mushroom_patch_donor.rs -O -o target/route_a_mushroom_patch_donor
copy_donor_payload "$fc_high_src" "$fc_high_dst" "FC_M_0000.flver"
copy_donor_payload "$fc_low_src" "$fc_low_dst" "FC_M_0000_L.flver"
cp -f "$fc_high_tpf_fallback" "$fc_high_dst/FC_M_0000.tpf"
cp -f "$fc_low_tpf_fallback" "$fc_low_dst/FC_M_0000_L.tpf"

./target/route_a_mushroom_patch_donor \
	--obj "$export_dir/blender_edit_c2280.obj" \
	--weights "$export_dir/blender_edit_c2280_weights.tsv" \
	--donor-flver "$fc_high_src/FC_M_0000.flver" \
	--output-flver "$fc_high_dst/FC_M_0000.flver" \
	--summary "$variant_dir/fc_m_0000-blender-edit-summary.txt" \
	--donor-mesh-index 13 \
	>"$variant_dir/patch-fc.out"
./target/route_a_mushroom_patch_donor \
	--obj "$export_dir/blender_edit_c2280.obj" \
	--weights "$export_dir/blender_edit_c2280_weights.tsv" \
	--donor-flver "$fc_low_src/FC_M_0000_L.flver" \
	--output-flver "$fc_low_dst/FC_M_0000_L.flver" \
	--summary "$variant_dir/fc_m_0000_l-blender-edit-summary.txt" \
	--donor-mesh-index 13 \
	>"$variant_dir/patch-fc-l.out"

run_witchy_pack "$variant_dir" fc-parts "$fc_high_dst"
run_witchy_pack "$variant_dir" fc-l-parts "$fc_low_dst"
cp -f "$bd_high_fallback" "$mod_dir/parts/bd_m_1010.partsbnd.dcx"
cp -f "$bd_low_fallback" "$mod_dir/parts/bd_m_1010_l.partsbnd.dcx"
cp -f "$variant_dir/fc_m_0000.partsbnd.dcx" "$mod_dir/parts/fc_m_0000.partsbnd.dcx"
cp -f "$variant_dir/fc_m_0000_l.partsbnd.dcx" "$mod_dir/parts/fc_m_0000_l.partsbnd.dcx"
cp -f "$fg_face_fallback" "$mod_dir/parts/fg_a_0000_m.partsbnd.dcx"
cp -f "$facegen_fallback" "$mod_dir/facegen/facegen.fgbnd.dcx"
write_me3_profile "$profile_path" "$mod_dir"

printf 'built Blender-edit mushroom profile:\n%s\n' "$profile_path"
print_launch_command "$profile_path"

if [[ "$launch_after" == true ]]; then
	"$me3_exe" launch -g eldenring --online false -p "$(wslpath -w "$(realpath -m "$profile_path")")"
fi
