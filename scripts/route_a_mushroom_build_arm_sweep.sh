#!/usr/bin/env bash
# Build a small asset/param ME3 sweep for arm-rig tuning.
#
# This does not launch either game. It builds several packed ModEngine2 package
# directories that differ only in the c2280 -> ER arm geometry/weight parameters.
# The point is to let the human choose the least-bad direction with short labels
# instead of trying to describe a screenshot by hand.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

sweep_root="target/mushroom-route-a-offline/arm-sweep"
witchy="${WITCHY_BND:-}"
witchy_pty="${WITCHY_PTY:-}"
me3_exe="${ME3_EXE:-${ME3_BIN:-}}"

bd_high_fallback="target/mushroom-route-a-offline/prototype/mod/parts/bd_m_1010.partsbnd.dcx"
bd_low_fallback="target/mushroom-route-a-offline/prototype/mod/parts/bd_m_1010_l.partsbnd.dcx"
fc_high_tpf_fallback="target/mushroom-route-a-offline/prototype/fc_m_0000-mushroom-parts/FC_M_0000.tpf"
fc_low_tpf_fallback="target/mushroom-route-a-offline/prototype/fc_m_0000_l-mushroom-parts/FC_M_0000_L.tpf"
facegen_fallback="target/mushroom-route-a-offline/prototype/mod/facegen/facegen.fgbnd.dcx"
fg_face_fallback="target/mushroom-route-a-offline/prototype/mod/parts/fg_a_0000_m.partsbnd.dcx"
hide_armor_script="scripts/route_a_mushroom_hide_armor_regulation.sh"
fc_high_src="target/mushroom-route-a-offline/er-naked-parts/fc_m_0000-partsbnd-dcx"
fc_low_src="target/mushroom-route-a-offline/er-naked-parts/fc_m_0000_l-partsbnd-dcx"

require_path() {
	local path="$1"
	if [[ ! -e "$path" ]]; then
		echo "missing required path: $path" >&2
		exit 1
	fi
}

print_usage() {
	cat <<'EOF'
route_a_mushroom_build_arm_sweep.sh

Preset sweep:
  bash scripts/route_a_mushroom_build_arm_sweep.sh
  bash scripts/route_a_mushroom_build_arm_sweep.sh less-contorted more-out

Single configurable variant:
  bash scripts/route_a_mushroom_build_arm_sweep.sh --label my-test [slider args] [--launch]

Environment overrides (each is discovered from PATH/$HOME when unset):
  WITCHY_BND       WitchyBND executable
  WITCHY_PTY       PTY wrapper used to run WitchyBND (its console refuses redirected stdout)
  ME3_EXE/ME3_BIN  me3 executable

Useful slider args forwarded to route_a_mushroom_export:
  --arm-x-swell <float>                 width/sideways scaling for arm vertices
  --arm-y-swell <float>                 vertical scaling around the arm center
  --arm-z-swell <float>                 front/back volume for arm vertices
  --arm-shoulder-out <float>            outward shoulder offset
  --arm-upper-out <float>               outward upper-arm offset
  --arm-forearm-out <float>             outward forearm offset
  --arm-upper-to-shoulder-abs-x <float> inner upper-arm threshold remapped to shoulder
  --arm-forearm-to-upper-abs-x <float>  inner forearm threshold remapped to upper arm
  --arm-forearm-to-hand-abs-x <float>   outer forearm threshold remapped to hand/weapon grip
  --vertical-stretch <float>            mushroom height stretch
  --torso-x-scale <float>               shrink/widen non-arm torso/body band
  --torso-z-scale <float>               flatten/deepen non-arm torso/body band
  --cap-x-scale <float>                 shrink/widen cap band
  --cap-z-scale <float>                 flatten/deepen cap band
  --torso-start-norm-y <float>          normalized height where torso scaling starts
  --cap-start-norm-y <float>            normalized height where cap scaling starts

Example build-only custom profile:
  bash scripts/route_a_mushroom_build_arm_sweep.sh --label trial --arm-x-swell 1.04 --arm-z-swell 1.25 --arm-shoulder-out 0.44 --arm-upper-out 0.34 --arm-forearm-out 0.12 --arm-upper-to-shoulder-abs-x 0.48 --arm-forearm-to-upper-abs-x 0.52 --arm-forearm-to-hand-abs-x 0.48 --torso-x-scale 0.88 --cap-x-scale 0.92

Example build and launch custom profile:
  bash scripts/route_a_mushroom_build_arm_sweep.sh --label trial --launch --arm-x-swell 1.04 --arm-z-swell 1.25 --arm-shoulder-out 0.44 --arm-upper-out 0.34 --arm-forearm-out 0.12 --arm-upper-to-shoulder-abs-x 0.48 --arm-forearm-to-upper-abs-x 0.52 --arm-forearm-to-hand-abs-x 0.48 --torso-x-scale 0.88 --cap-x-scale 0.92
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
	print_usage
	exit 0
fi


# Tool discovery. Env override first, then path, then the current user's home; the /mnt/...
# entries are the retired WSL2 layout and are last-resort only, so on a native Linux box they
# cost one failed stat instead of masking a real install as "the tool is missing".
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

# WitchyBND's PromptPlus console layer refuses to start with stdout redirected
# ("PromptPlus requires a terminal/console without redirection environment!", exit 1). The WSL2
# path gave it a console via cmd.exe; a native Linux run needs a PTY wrapper instead.
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

# A Windows me3.exe driven from WSL needs `wslpath -w`; a native Linux me3 takes the path as is.
# Keyed on wslpath actually existing, never on a hard-coded host assumption.
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
	rm -rf "$dst"
	mkdir -p "$dst"
	shopt -s nullglob dotglob
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
	local variant_dir="$1"
	local label="$2"
	local mod_dir="$variant_dir/mod"
	local profile_path="$variant_dir/mushroom-arm-${label}.me3"
	local package_path
	package_path="$(maybe_windows_path "$mod_dir")"
	mkdir -p "$mod_dir/parts" "$mod_dir/facegen"
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
Offline Route A mushroom arm tuning variant: $label

Profile:
  ../mushroom-arm-${label}.me3

Payload is asset/param-only: natives = []. The regulation.bin override hides
equipped head/body/arms/legs armor visuals so the mushroom FC body remains
visible regardless of equipment. Equipment gameplay/stat effects remain from
the selected rows; only armor model fields are forced to default no-armor IDs.
EOF
}

stage_model_variant_aliases() {
	local mod_dir="$1"
	local summary_path="$2"
	python3 scripts/route_a_mushroom_stage_all_model_variants.py \
		--mod-dir "$mod_dir" \
		--summary "$summary_path"
}

# WSL2-only: hand WitchyBND.exe a real Windows console, from the directory the discovered
# binary actually lives in rather than a hard-coded drive letter.
write_witchy_cmd() {
	local cmd_path="$1"
	local input_path="$2"
	local input_win witchy_dir_win
	input_win="$(maybe_windows_path "$input_path")"
	witchy_dir_win="$(maybe_windows_path "$(dirname "$witchy")")"
	{
		printf '@echo off\r\n'
		printf 'cd /d "%s"\r\n' "$witchy_dir_win"
		printf '"%s" "%s"\r\n' "$(basename "$witchy")" "$input_win"
	} >"$cmd_path"
}

run_witchy_pack() {
	local variant_dir="$1"
	local tag="$2"
	local input_path="$3"
	local cmd_path="$variant_dir/pack-${tag}.cmd"
	local out_path="$variant_dir/pack-${tag}.out"
	local code=0
	: >"$out_path"
	if command -v cmd.exe >/dev/null 2>&1 && [[ "$witchy" == *.exe ]]; then
		write_witchy_cmd "$cmd_path" "$input_path"
		timeout 30s cmd.exe /c "$(maybe_windows_path "$cmd_path")" >"$out_path" 2>&1 || code=$?
	elif [[ -n "$witchy_pty" ]]; then
		timeout 30s python3 "$witchy_pty" --log "$out_path" -- "$witchy" -p "$input_path" >/dev/null 2>&1 || code=$?
	else
		timeout 30s "$witchy" -p "$input_path" >"$out_path" 2>&1 || code=$?
	fi
	case "$code" in
	0 | 82) ;;
	*)
		echo "WitchyBND pack failed for $tag with exit $code; see $out_path" >&2
		exit "$code"
		;;
	esac
	if [[ "$code" == 82 ]]; then
		echo "accepted WitchyBND exit=82 after output phase" >>"$out_path"
	fi
}

build_variant() {
	local label="$1"
	local params_text="$2"
	local variant_dir="$sweep_root/$label"
	local export_dir="$variant_dir/c2280-rust-export"
	local fc_high_dst="$variant_dir/fc_m_0000-mushroom-parts"
	local fc_low_dst="$variant_dir/fc_m_0000_l-mushroom-parts"
	rm -rf "$variant_dir"
	mkdir -p "$variant_dir"
	read -r -a params <<<"$params_text"

	./target/route_a_mushroom_export --output-dir "$export_dir" "${params[@]}" >"$variant_dir/export.out"

	copy_donor_payload "$fc_high_src" "$fc_high_dst" "FC_M_0000.flver"
	copy_donor_payload "$fc_low_src" "$fc_low_dst" "FC_M_0000_L.flver"

	./target/route_a_mushroom_patch_donor \
		--obj "$export_dir/c2280_route_a_scaled.obj" \
		--weights "$export_dir/c2280_route_a_weights.tsv" \
		--donor-flver "$fc_high_src/FC_M_0000.flver" \
		--output-flver "$fc_high_dst/FC_M_0000.flver" \
		--summary "$variant_dir/fc_m_0000-summary.txt" \
		--donor-mesh-index 13 \
		>"$variant_dir/patch-fc.out"
	./target/route_a_mushroom_patch_donor \
		--obj "$export_dir/c2280_route_a_scaled.obj" \
		--weights "$export_dir/c2280_route_a_weights.tsv" \
		--donor-flver "$fc_low_src/FC_M_0000_L.flver" \
		--output-flver "$fc_low_dst/FC_M_0000_L.flver" \
		--summary "$variant_dir/fc_m_0000_l-summary.txt" \
		--donor-mesh-index 13 \
		>"$variant_dir/patch-fc-l.out"

	cp -f "$fc_high_tpf_fallback" "$fc_high_dst/FC_M_0000.tpf"
	cp -f "$fc_low_tpf_fallback" "$fc_low_dst/FC_M_0000_L.tpf"

	run_witchy_pack "$variant_dir" fc-parts "$fc_high_dst"
	run_witchy_pack "$variant_dir" fc-l-parts "$fc_low_dst"

	write_me3_profile "$variant_dir" "$label"
	cp -f "$bd_high_fallback" "$variant_dir/mod/parts/bd_m_1010.partsbnd.dcx"
	cp -f "$bd_low_fallback" "$variant_dir/mod/parts/bd_m_1010_l.partsbnd.dcx"
	cp -f "$variant_dir/fc_m_0000.partsbnd.dcx" "$variant_dir/mod/parts/fc_m_0000.partsbnd.dcx"
	cp -f "$variant_dir/fc_m_0000_l.partsbnd.dcx" "$variant_dir/mod/parts/fc_m_0000_l.partsbnd.dcx"
	cp -f "$fg_face_fallback" "$variant_dir/mod/parts/fg_a_0000_m.partsbnd.dcx"
	cp -f "$facegen_fallback" "$variant_dir/mod/facegen/facegen.fgbnd.dcx"
	stage_model_variant_aliases "$variant_dir/mod" "$variant_dir/model-variant-staging-summary.txt"
	bash "$hide_armor_script" \
		--output "$variant_dir/mod/regulation.bin" \
		--summary "$variant_dir/hide-armor-regulation-summary.txt" \
		>"$variant_dir/hide-armor-regulation.out"

	printf '%s\t%s\t%s\n' "$label" "$variant_dir/mushroom-arm-${label}.me3" "$params_text" >>"$sweep_root/variant-index.tsv"
}

me3_exe="$(locate_me3)"
witchy="$(locate_witchy)"
witchy_pty="$(locate_witchy_pty)"
require_path "$witchy"
require_path "$me3_exe"
require_path "$bd_high_fallback"
require_path "$bd_low_fallback"
require_path "$fc_high_tpf_fallback"
require_path "$fc_low_tpf_fallback"
require_path "$facegen_fallback"
require_path "$fg_face_fallback"
require_path "$hide_armor_script"
require_path "scripts/route_a_mushroom_build_hidden_naked_slots.sh"
require_path "scripts/route_a_mushroom_stage_all_model_variants.py"
require_path "$fc_high_src/FC_M_0000.flver"
require_path "$fc_low_src/FC_M_0000_L.flver"

mkdir -p target
rustc scripts/route_a_mushroom_export.rs -O -o target/route_a_mushroom_export
rustc scripts/route_a_mushroom_patch_donor.rs -O -o target/route_a_mushroom_patch_donor
bash scripts/route_a_mushroom_build_hidden_naked_slots.sh >"$sweep_root/hidden-naked-slots.out"

variant_params() {
	case "$1" in
	edge-stub) printf '%s' "" ;;
	more-out) printf '%s' "--arm-shoulder-out 0.45 --arm-upper-out 0.36 --arm-forearm-out 0.16 --arm-forearm-to-upper-abs-x 0.50" ;;
	less-contorted) printf '%s' "--arm-x-swell 1.08 --arm-z-swell 1.35 --arm-shoulder-out 0.42 --arm-upper-out 0.34 --arm-forearm-out 0.12 --arm-upper-to-shoulder-abs-x 0.46 --arm-forearm-to-upper-abs-x 0.50" ;;
	*)
		echo "unknown arm sweep variant: $1" >&2
		exit 2
		;;
	esac
}


remove_variant_from_index() {
	local variant="$1"
	if [[ -e "$sweep_root/variant-index.tsv" ]]; then
		python3 - "$sweep_root/variant-index.tsv" "$variant" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
variant = sys.argv[2]
lines = path.read_text(encoding="utf-8").splitlines()
path.write_text("\n".join(line for line in lines if not line.startswith(f"{variant}\t")) + "\n", encoding="utf-8")
PY
	fi
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

if [[ "${1:-}" == "--label" ]]; then
	if [[ "$#" -lt 2 ]]; then
		echo "--label requires a variant label" >&2
		exit 2
	fi
	label="$2"
	shift 2
	if [[ ! "$label" =~ ^[A-Za-z0-9._-]+$ ]]; then
		echo "label must contain only letters, numbers, dots, underscores, or hyphens: $label" >&2
		exit 2
	fi
	launch_after=false
	exporter_args=()
	while [[ "$#" -gt 0 ]]; do
		case "$1" in
		--launch)
			launch_after=true
			shift
			;;
		--)
			shift
			exporter_args+=("$@")
			break
			;;
		*)
			exporter_args+=("$1")
			shift
			;;
		esac
	done
	mkdir -p "$sweep_root"
	if [[ ! -e "$sweep_root/variant-index.tsv" ]]; then
		printf 'label\tprofile\tparams\n' >"$sweep_root/variant-index.tsv"
	fi
	remove_variant_from_index "$label"
	params_text="${exporter_args[*]}"
	build_variant "$label" "$params_text"
	profile_path="$sweep_root/$label/mushroom-arm-${label}.me3"
	printf 'built configurable arm variant:\n'
	tail -n 1 "$sweep_root/variant-index.tsv"
	print_launch_command "$profile_path"
	if [[ "$launch_after" == true ]]; then
		"$me3_exe" launch -g eldenring --online false -p "$(maybe_windows_path "$profile_path")"
	fi
	exit 0
fi

if [[ "$#" -eq 0 ]]; then
	rm -rf "$sweep_root"
	mkdir -p "$sweep_root"
	printf 'label\tprofile\tparams\n' >"$sweep_root/variant-index.tsv"
	selected_variants=(edge-stub more-out less-contorted)
else
	mkdir -p "$sweep_root"
	if [[ ! -e "$sweep_root/variant-index.tsv" ]]; then
		printf 'label\tprofile\tparams\n' >"$sweep_root/variant-index.tsv"
	fi
	selected_variants=("$@")
fi

for variant in "${selected_variants[@]}"; do
	remove_variant_from_index "$variant"
	build_variant "$variant" "$(variant_params "$variant")"
done

printf 'built arm tuning sweep:\n'
cat "$sweep_root/variant-index.tsv"
