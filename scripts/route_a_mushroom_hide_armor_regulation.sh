#!/usr/bin/env bash
# Build a Mushroom Man regulation.bin that hides every equipped armor model visually.
# The edited EquipParamProtector rows keep gameplay/stat equipment intact but make
# head/body/arms/legs render their default no-armor model IDs, so the FC mushroom
# body stays visible regardless of equipped armor.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

input_regulation="${ER_REGULATION_BIN:-}"
output_regulation="target/mushroom-route-a-offline/prototype/mod/regulation.bin"
summary_path="target/mushroom-route-a-offline/prototype/hide-armor-regulation-summary.txt"
smithbox_dir="${SMITHBOX_BINARY_DIR:-}"

usage() {
	cat <<'EOF'
route_a_mushroom_hide_armor_regulation.sh

Create a ModEngine2/ME3 regulation.bin override that hides all equipped armor
models by setting EquipParamProtector visual model fields to the default no-armor
slot models. This is the asset/param route for keeping the mushroom FC body
visible regardless of what the player has equipped.

Usage:
  bash scripts/route_a_mushroom_hide_armor_regulation.sh [--input regulation.bin] [--output mod/regulation.bin] [--summary summary.txt] [--smithbox-dir path]

Environment overrides:
  ER_REGULATION_BIN     input regulation.bin when --input is omitted
  SMITHBOX_BINARY_DIR   Smithbox binary install containing Andre.Formats.dll
EOF
}

require_file() {
	local path="$1"
	local label="$2"
	if [[ ! -f "$path" ]]; then
		echo "missing $label: $path" >&2
		exit 1
	fi
}

require_dir() {
	local path="$1"
	local label="$2"
	if [[ ! -d "$path" ]]; then
		echo "missing $label: $path" >&2
		exit 1
	fi
}

xml_escape() {
	python3 -c 'import html,sys; print(html.escape(sys.argv[1], quote=True))' "$1"
}

windows_path() {
	wslpath -w "$(realpath -m "$1")"
}

find_smithbox_dir() {
	if [[ -n "$smithbox_dir" ]]; then
		printf '%s\n' "$smithbox_dir"
		return
	fi
	local candidate
	for candidate in "$repo_root/.deps/Smithbox" "$repo_root/../Smithbox" "$repo_root/../smithbox" /mnt/d/Smithbox; do
		if [[ -f "$candidate/Andre.Formats.dll" && -f "$candidate/Andre.SoulsFormats.dll" ]]; then
			printf '%s\n' "$candidate"
			return
		fi
	done
	echo "could not find Smithbox binary dir; pass --smithbox-dir or set SMITHBOX_BINARY_DIR" >&2
	exit 1
}

read_acf_value() {
	local manifest="$1"
	local key="$2"
	python3 - "$manifest" "$key" <<'PY'
from pathlib import Path
import re
import sys
text = Path(sys.argv[1]).read_text(encoding="utf-8", errors="replace")
match = re.search(r'"' + re.escape(sys.argv[2]) + r'"\s+"([^"]+)"', text)
if match:
    print(match.group(1))
PY
}

find_elden_ring_regulation() {
	if [[ -n "$input_regulation" ]]; then
		printf '%s\n' "$input_regulation"
		return
	fi
	local manifest install_dir steamapps game_dir candidate
	for manifest in /mnt/?/{SteamLibrary,steam,Steam}/steamapps/appmanifest_1245620.acf; do
		[[ -f "$manifest" ]] || continue
		install_dir="$(read_acf_value "$manifest" installdir)"
		[[ -n "$install_dir" ]] || continue
		steamapps="$(dirname "$manifest")"
		game_dir="$steamapps/common/$install_dir/Game"
		candidate="$game_dir/regulation.bin"
		if [[ -f "$candidate" ]]; then
			printf '%s\n' "$candidate"
			return
		fi
	done
	echo "could not find Elden Ring regulation.bin; pass --input or set ER_REGULATION_BIN" >&2
	exit 1
}

while [[ "$#" -gt 0 ]]; do
	case "$1" in
	--input)
		input_regulation="${2:?--input requires a value}"
		shift 2
		;;
	--output)
		output_regulation="${2:?--output requires a value}"
		shift 2
		;;
	--summary)
		summary_path="${2:?--summary requires a value}"
		shift 2
		;;
	--smithbox-dir)
		smithbox_dir="${2:?--smithbox-dir requires a value}"
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

input_regulation="$(find_elden_ring_regulation)"
smithbox_dir="$(find_smithbox_dir)"
require_file "$input_regulation" "input regulation.bin"
require_dir "$smithbox_dir" "Smithbox binary dir"
require_file "$smithbox_dir/Andre.Formats.dll" "Andre.Formats.dll"
require_file "$smithbox_dir/Andre.SoulsFormats.dll" "Andre.SoulsFormats.dll"
require_file "$smithbox_dir/Assets/PARAM/ER/Defs/EquipParamProtector.xml" "ER EquipParamProtector paramdef"
require_file "scripts/route_a_mushroom_hide_armor_regulation.cs" "C# regulation patcher source"

project_dir="target/mushroom-regulation-patcher"
mkdir -p "$project_dir"
smithbox_win="$(windows_path "$smithbox_dir")"
andref="$(xml_escape "$smithbox_win\\Andre.Formats.dll")"
soulsf="$(xml_escape "$smithbox_win\\Andre.SoulsFormats.dll")"
cat >"$project_dir/mushroom-regulation-patcher.csproj" <<EOF
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net9.0</TargetFramework>
    <ImplicitUsings>enable</ImplicitUsings>
    <Nullable>enable</Nullable>
  </PropertyGroup>
  <ItemGroup>
    <Reference Include="Andre.Formats" HintPath="$andref" />
    <Reference Include="Andre.SoulsFormats" HintPath="$soulsf" />
  </ItemGroup>
</Project>
EOF
cp -f scripts/route_a_mushroom_hide_armor_regulation.cs "$project_dir/Program.cs"
oodle_source=""
for candidate in "$smithbox_dir"/oo2core_*_win64.dll "$(dirname "$input_regulation")"/oo2core_*_win64.dll; do
	if [[ -f "$candidate" ]]; then
		oodle_source="$candidate"
		break
	fi
done
if [[ -n "$oodle_source" ]]; then
	mkdir -p "$project_dir/bin/Release/net9.0"
	cp -f "$oodle_source" "$project_dir/"
	cp -f "$oodle_source" "$project_dir/bin/Release/net9.0/"
else
	echo "missing oo2core_*_win64.dll beside Smithbox or regulation.bin" >&2
	exit 1
fi
mkdir -p "$(dirname "$output_regulation")" "$(dirname "$summary_path")"

project_win="$(windows_path "$project_dir")"
input_win="$(windows_path "$input_regulation")"
output_win="$(windows_path "$output_regulation")"
smithbox_arg_win="$(windows_path "$smithbox_dir")"
summary_win="$(windows_path "$summary_path")"
powershell.exe -NoProfile -Command \
	"\$ErrorActionPreference = 'Stop'; Set-Location -LiteralPath '$project_win'; dotnet run --configuration Release -- '$input_win' '$output_win' '$smithbox_arg_win' '$summary_win'"
