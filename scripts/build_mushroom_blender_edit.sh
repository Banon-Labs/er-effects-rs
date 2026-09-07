#!/usr/bin/env bash
# Build and optionally launch an asset/runtime-DLL ME3 profile from the Blender EDIT_ME mesh.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

label="blender-edit"
launch_after=false
blend_path="target/mushroom-route-a-offline/blender-compare/mushroom_raw_game_compare.blend"
object_name="c2280"
donor_mesh_index=13
max_source_vertices=""
texture_dir="target/mushroom-route-a-offline/dsr/dsr-loose-mushroom/c2280-chrbnd-dcx/c2280-tpf"
texture_source_prefix="c2280"
blender_exe="${BLENDER_EXE:-}"
witchy="${WITCHY_BND:-}"
witchy_pty="${WITCHY_PTY:-}"
me3_exe="${ME3_EXE:-${ME3_BIN:-}}"

fc_high_src="target/mushroom-route-a-offline/er-naked-parts/fc_m_0000-partsbnd-dcx"
fc_low_src="target/mushroom-route-a-offline/er-naked-parts/fc_m_0000_l-partsbnd-dcx"
bd_high_fallback="target/mushroom-route-a-offline/prototype/mod/parts/bd_m_1010.partsbnd.dcx"
bd_low_fallback="target/mushroom-route-a-offline/prototype/mod/parts/bd_m_1010_l.partsbnd.dcx"
facegen_fallback="target/mushroom-route-a-offline/prototype/mod/facegen/facegen.fgbnd.dcx"
fg_face_fallback="target/mushroom-route-a-offline/prototype/mod/parts/fg_a_0000_m.partsbnd.dcx"
runtime_dll="${MUSHROOM_MAN_DLL:-}"

usage() {
	cat <<'EOF'
build_mushroom_blender_edit.sh

Build an asset/param ME3 profile from the editable mesh in the raw-game Blender scene.

Usage:
  bash scripts/build_mushroom_blender_edit.sh [--label name] [--blend path] [--object-name name] [--donor-mesh-index n] [--max-source-vertices n] [--texture-dir path] [--source-prefix c2270|c2280] [--launch]

Defaults:
  --label blender-edit
  --blend target/mushroom-route-a-offline/blender-compare/mushroom_raw_game_compare.blend
  --object-name c2280
  --donor-mesh-index 13
  --texture-dir target/mushroom-route-a-offline/dsr/dsr-loose-mushroom/c2280-chrbnd-dcx/c2280-tpf
  --source-prefix c2280

Environment overrides (each is discovered from PATH/$HOME when unset):
  BLENDER_EXE      blender executable
  WITCHY_BND       WitchyBND executable
  WITCHY_PTY       PTY wrapper used to run WitchyBND (its console refuses redirected stdout)
  ME3_EXE/ME3_BIN  me3 executable
  MUSHROOM_MAN_DLL prebuilt mushroom_man.dll instead of building one

Adult c2270 example targeting the visible FC donor mesh:
  bash scripts/build_mushroom_blender_edit.sh --label adult-user-scaled --blend target/mushroom-route-a-offline/blender-compare/mushroom_adult_raw_game_compare.blend --object-name EDIT_ME_ADULT --donor-mesh-index 13 --max-source-vertices 1500 --texture-dir target/mushroom-route-a-offline/dsr/dsr-loose-mushroom/c2270-chrbnd-dcx/c2270-tpf --source-prefix c2270
EOF
}

require_path() {
	local path="$1"
	if [[ ! -e "$path" ]]; then
		echo "missing required path: $path" >&2
		exit 1
	fi
}

# Tool discovery. Every default below is env-overridable and derived from the CURRENT user's
# home or PATH; no machine's literal home directory appears. The /mnt/... entries are the
# retired WSL2 layout and are kept only as LAST-resort candidates -- on a native Linux box they
# simply do not exist, so they cost one failed stat and never shadow a real install.
locate_me3() {
	if [[ -n "$me3_exe" ]]; then
		printf '%s\n' "$me3_exe"
		return
	fi
	local candidate
	for candidate in me3 me3.exe; do
		if command -v "$candidate" >/dev/null 2>&1; then
			command -v "$candidate"
			return
		fi
	done
	for candidate in \
		"$HOME/.local/bin/me3" \
		/mnt/c/Users/*/AppData/Local/garyttierney/me3/bin/me3.exe; do
		if [[ -f "$candidate" ]]; then
			printf '%s\n' "$candidate"
			return
		fi
	done
	echo "could not find me3; set ME3_EXE" >&2
	exit 1
}

locate_blender() {
	if [[ -n "$blender_exe" ]]; then
		printf '%s\n' "$blender_exe"
		return
	fi
	if command -v blender >/dev/null 2>&1; then
		command -v blender
		return
	fi
	local candidate
	for candidate in \
		/opt/blender/blender \
		/usr/local/bin/blender \
		"$HOME/.local/bin/blender" \
		/mnt/c/Program\ Files/Blender\ Foundation/Blender\ */blender.exe; do
		if [[ -x "$candidate" ]]; then
			printf '%s\n' "$candidate"
			return
		fi
	done
	echo "could not find blender; set BLENDER_EXE" >&2
	exit 1
}

locate_witchy() {
	if [[ -n "$witchy" ]]; then
		printf '%s\n' "$witchy"
		return
	fi
	if command -v WitchyBND >/dev/null 2>&1; then
		command -v WitchyBND
		return
	fi
	local candidate
	for candidate in \
		"$HOME/.local/share/witchybnd/runtime/WitchyBND" \
		"$repo_root/.deps/WitchyBND/WitchyBND" \
		"$repo_root/.deps/WitchyBND/WitchyBND.exe" \
		"$repo_root/../WitchyBND/WitchyBND" \
		"$repo_root/../WitchyBND/WitchyBND.exe" \
		"/mnt/d/Witchy BND/WitchyBND.exe"; do
		if [[ -f "$candidate" ]]; then
			printf '%s\n' "$candidate"
			return
		fi
	done
	echo "could not find WitchyBND; set WITCHY_BND" >&2
	exit 1
}

# WitchyBND's PromptPlus console layer refuses to start when stdout is redirected
# ("PromptPlus requires a terminal/console without redirection environment!", exit 1), so on a
# native Linux install the packing calls below MUST go through a PTY wrapper. Absent one we fall
# back to invoking WitchyBND directly, which is what the WSL2 console path did.
locate_witchy_pty() {
	if [[ -n "$witchy_pty" ]]; then
		printf '%s\n' "$witchy_pty"
		return
	fi
	local witchy_dir candidate
	witchy_dir="$(cd "$(dirname "$witchy")" && pwd)"
	for candidate in \
		"$witchy_dir/witchy-pty.py" \
		"$witchy_dir/../witchy-pty.py" \
		"$HOME/.local/share/witchybnd/witchy-pty.py" \
		"$HOME/er-extract/run-witchy-pty.py"; do
		if [[ -f "$candidate" ]]; then
			printf '%s\n' "$candidate"
			return
		fi
	done
	printf '\n'
}

# me3/Blender path spelling. A Windows me3.exe driven from WSL needs `wslpath -w`; a native
# Linux me3 and a native Linux Blender take the path as is. Keyed on wslpath actually existing,
# never on a hard-coded host assumption.
maybe_windows_path() {
	local path
	path="$(realpath -m "$1")"
	if command -v wslpath >/dev/null 2>&1; then
		wslpath -w "$path"
		return
	fi
	printf '%s\n' "$path"
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
	: >"$log_path"
	set +e
	if [[ -n "$witchy_pty" ]]; then
		python3 "$witchy_pty" --log "$log_path" -- "$witchy" -p "$input_path" >/dev/null 2>&1
	else
		"$witchy" -p "$input_path" >"$log_path" 2>&1
	fi
	local status=$?
	set -e
	if [[ "$status" -ne 0 && "$status" -ne 82 ]]; then
		tail -80 "$log_path" >&2 || true
		exit "$status"
	fi
}

build_mushroom_runtime_dll() {
	if [[ -n "$runtime_dll" ]]; then
		require_path "$runtime_dll"
		printf '%s\n' "$runtime_dll"
		return
	fi
	cargo xwin build --release --target x86_64-pc-windows-msvc -p mushroom-man-runtime
	runtime_dll="target/x86_64-pc-windows-msvc/release/mushroom_man.dll"
	require_path "$runtime_dll"
	printf '%s\n' "$runtime_dll"
}

write_me3_profile() {
	local profile_path="$1"
	local mod_dir="$2"
	local native_dll="$3"
	local package_path native_path
	package_path="$(maybe_windows_path "$mod_dir")"
	native_path="$(maybe_windows_path "$native_dll")"
	cat >"$profile_path" <<EOF
profileVersion = "v1"

[[natives]]
path = '$native_path'

[[supports]]
game = "eldenring"

[[packages]]
enabled = true
path = '$package_path'
load_after = []
load_before = []
EOF
}

stage_model_variant_aliases() {
	local mod_dir="$1"
	local summary_path="$2"
	python3 scripts/route_a_mushroom_stage_all_model_variants.py \
		--mod-dir "$mod_dir" \
		--fc-source-high "$fc_high_dst" \
		--fc-source-low "$fc_low_dst" \
		--summary "$summary_path"
}

print_launch_command() {
	local profile_path="$1"
	local profile_win
	profile_win="$(maybe_windows_path "$profile_path")"
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
	--object-name)
		object_name="${2:?--object-name requires a value}"
		shift 2
		;;
	--donor-mesh-index)
		donor_mesh_index="${2:?--donor-mesh-index requires a value}"
		shift 2
		;;
	--max-source-vertices)
		max_source_vertices="${2:?--max-source-vertices requires a value}"
		shift 2
		;;
	--texture-dir)
		texture_dir="${2:?--texture-dir requires a value}"
		shift 2
		;;
	--source-prefix)
		texture_source_prefix="${2:?--source-prefix requires a value}"
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

me3_exe="$(locate_me3)"
blender_exe="$(locate_blender)"
witchy="$(locate_witchy)"
witchy_pty="$(locate_witchy_pty)"
require_path "$blender_exe"
require_path "$witchy"
require_path "$me3_exe"
require_path "$blend_path"
require_path "$fc_high_src/FC_M_0000.flver"
require_path "$fc_low_src/FC_M_0000_L.flver"
require_path "$bd_high_fallback"
require_path "$bd_low_fallback"
require_path "$texture_dir"
require_path "$facegen_fallback"
require_path "$fg_face_fallback"
require_path "crates/mushroom-man-runtime/Cargo.toml"
require_path "scripts/route_a_mushroom_build_hidden_naked_slots.sh"
require_path "scripts/route_a_mushroom_stage_all_model_variants.py"
require_path "scripts/route_a_mushroom_stage_textures.rs"
require_path "scripts/audit_mushroom_export_weights.py"

variant_dir="target/mushroom-route-a-offline/blender-edit/$label"
export_dir="$variant_dir/export"
fc_high_dst="$variant_dir/fc_m_0000-blender-edit-parts"
fc_low_dst="$variant_dir/fc_m_0000_l-blender-edit-parts"
mod_dir="$variant_dir/mod"
profile_path="$variant_dir/mushroom-blender-edit-$label.me3"

rm -rf "$variant_dir"
mkdir -p "$export_dir" "$mod_dir/parts" "$mod_dir/facegen"

blender_export_args=(
	--object-name "$object_name"
	--output-dir "$(maybe_windows_path "$export_dir")"
)
if [[ -n "$max_source_vertices" ]]; then
	blender_export_args+=(--max-source-vertices "$max_source_vertices")
fi

"$blender_exe" --background "$(maybe_windows_path "$blend_path")" \
	--python "$(maybe_windows_path scripts/export_mushroom_blender_edit.py)" -- \
	"${blender_export_args[@]}" \
	>"$variant_dir/export-blender-edit.log" 2>&1

python3 scripts/audit_mushroom_export_weights.py \
	--obj "$export_dir/blender_edit_c2280.obj" \
	--weights "$export_dir/blender_edit_c2280_weights.tsv" \
	--json "$variant_dir/export-connectivity-audit.json" \
	--text "$variant_dir/export-connectivity-audit.txt" \
	--fail-on-isolated \
	>"$variant_dir/export-connectivity-audit.out"

rustc scripts/route_a_mushroom_patch_donor.rs -O -o target/route_a_mushroom_patch_donor
rustc scripts/route_a_mushroom_stage_textures.rs -O -o target/route_a_mushroom_stage_textures
copy_donor_payload "$fc_high_src" "$fc_high_dst" "FC_M_0000.flver"
copy_donor_payload "$fc_low_src" "$fc_low_dst" "FC_M_0000_L.flver"

./target/route_a_mushroom_patch_donor \
	--obj "$export_dir/blender_edit_c2280.obj" \
	--weights "$export_dir/blender_edit_c2280_weights.tsv" \
	--donor-flver "$fc_high_src/FC_M_0000.flver" \
	--output-flver "$fc_high_dst/FC_M_0000.flver" \
	--summary "$variant_dir/fc_m_0000-blender-edit-summary.txt" \
	--donor-mesh-index "$donor_mesh_index" \
	>"$variant_dir/patch-fc.out"
./target/route_a_mushroom_patch_donor \
	--obj "$export_dir/blender_edit_c2280.obj" \
	--weights "$export_dir/blender_edit_c2280_weights.tsv" \
	--donor-flver "$fc_low_src/FC_M_0000_L.flver" \
	--output-flver "$fc_low_dst/FC_M_0000_L.flver" \
	--summary "$variant_dir/fc_m_0000_l-blender-edit-summary.txt" \
	--donor-mesh-index "$donor_mesh_index" \
	>"$variant_dir/patch-fc-l.out"

./target/route_a_mushroom_stage_textures \
	--texture-dir "$texture_dir" \
	--parts-dir "$fc_high_dst" \
	--tpf-dir-name FC_M_0000-tpf \
	--tpf-filename FC_M_0000.tpf \
	--texture-suffix "" \
	--target-prefix FC_M_0000 \
	--source-prefix "$texture_source_prefix" \
	--manifest-kind fc \
	>"$variant_dir/stage-textures-fc.out"
./target/route_a_mushroom_stage_textures \
	--texture-dir "$texture_dir" \
	--parts-dir "$fc_low_dst" \
	--tpf-dir-name FC_M_0000_L-tpf \
	--tpf-filename FC_M_0000_L.tpf \
	--texture-suffix _l \
	--target-prefix FC_M_0000 \
	--source-prefix "$texture_source_prefix" \
	--manifest-kind fc \
	>"$variant_dir/stage-textures-fc-l.out"
run_witchy_pack "$variant_dir" fc-tpf "$fc_high_dst/FC_M_0000-tpf"
run_witchy_pack "$variant_dir" fc-l-tpf "$fc_low_dst/FC_M_0000_L-tpf"
run_witchy_pack "$variant_dir" fc-parts "$fc_high_dst"
run_witchy_pack "$variant_dir" fc-l-parts "$fc_low_dst"
cp -f "$bd_high_fallback" "$mod_dir/parts/bd_m_1010.partsbnd.dcx"
cp -f "$bd_low_fallback" "$mod_dir/parts/bd_m_1010_l.partsbnd.dcx"
cp -f "$variant_dir/fc_m_0000.partsbnd.dcx" "$mod_dir/parts/fc_m_0000.partsbnd.dcx"
cp -f "$variant_dir/fc_m_0000_l.partsbnd.dcx" "$mod_dir/parts/fc_m_0000_l.partsbnd.dcx"
cp -f "$fg_face_fallback" "$mod_dir/parts/fg_a_0000_m.partsbnd.dcx"
cp -f "$facegen_fallback" "$mod_dir/facegen/facegen.fgbnd.dcx"
bash scripts/route_a_mushroom_build_hidden_naked_slots.sh >"$variant_dir/hidden-naked-slots.out"
stage_model_variant_aliases "$mod_dir" "$variant_dir/model-variant-staging-summary.txt"
runtime_dll_built="$(build_mushroom_runtime_dll)"
cp -f "$runtime_dll_built" "$mod_dir/mushroom_man.dll"
rm -f "$mod_dir/regulation.bin"
write_me3_profile "$profile_path" "$mod_dir" "$mod_dir/mushroom_man.dll"

printf 'built Blender-edit mushroom profile:\n%s\n' "$profile_path"
printf 'export connectivity audit:\n%s\n' "$variant_dir/export-connectivity-audit.txt"
print_launch_command "$profile_path"

if [[ "$launch_after" == true ]]; then
	"$me3_exe" launch -g eldenring --online false -p "$(maybe_windows_path "$profile_path")"
fi
