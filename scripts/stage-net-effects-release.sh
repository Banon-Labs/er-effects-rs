#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
out_dir="$repo_root/target/net-effects-release"
build=1

usage() {
	cat <<'EOF'
Usage: scripts/stage-net-effects-release.sh [--output DIR] [--no-build]

Stages the standalone keyboard-controlled network effects payload:
  er_net_effects_dll.dll       standalone DLL, loaded by me3 as its own native
  er-net-effects.me3           me3 ModProfile loading the game-installed Seamless Co-op DLL plus net-effects DLL
  er-net-effects.toml.example  per-feature configuration file
  .er-net-effects-hotkeys.json.example  keyboard-trigger configuration
  er-net-effect-master-catalog.json     SpEffect metadata for selector labels/tags
  er-net-effect-catalogs/*.jsonc        commentable selector catalogs (network-test, visuals-only, sounds-only, stats, weapon buffs, etc.)

Install: keep the folder together anywhere (the profile references the net-effects
DLL relative to itself and references the game-installed Seamless Co-op DLL by
absolute path), copy the wanted er-net-effects config/catalog files next to
eldenring.exe, then launch:
  me3 launch -g eldenring -p /path/to/er-net-effects.me3

Environment:
  ER_NET_EFFECTS_DLL  prebuilt er_net_effects_dll.dll path
EOF
}

while [[ $# -gt 0 ]]; do
	case "$1" in
	--output)
		out_dir="$2"
		shift 2
		;;
	--no-build)
		build=0
		shift
		;;
	-h | --help)
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

net_effects_dll="${ER_NET_EFFECTS_DLL:-$repo_root/target/x86_64-pc-windows-msvc/release/er_net_effects_dll.dll}"

if [[ "$build" == "1" ]]; then
	cargo xwin build --manifest-path "$repo_root/Cargo.toml" --target x86_64-pc-windows-msvc --release -p er-net-effects-dll
fi

if [[ ! -f "$net_effects_dll" ]]; then
	echo "missing er_net_effects_dll.dll: $net_effects_dll" >&2
	exit 1
fi

out_dir=$(realpath -m "$out_dir")
tmp_dir="$out_dir.tmp"
rm -rf "$tmp_dir"
mkdir -p "$tmp_dir/er-net-effect-catalogs"

cp -f "$net_effects_dll" "$tmp_dir/er_net_effects_dll.dll"
cat >"$tmp_dir/er-net-effects.me3" <<'EOF'
profileVersion = "v1"

[[supports]]
game = "eldenring"

[[natives]]
path = 'C:\SteamLibrary\steamapps\common\ELDEN RING\Game\SeamlessCoop\ersc.dll'

[[natives]]
path = 'er_net_effects_dll.dll'
EOF
cat >"$tmp_dir/er-net-effects.toml.example" <<'EOF'
# Copy to er-net-effects.toml next to eldenring.exe.
# This file belongs to er_net_effects_dll.dll and is intentionally separate from
# er-effects-rs product/autoload configuration.
network_sync = true
# Start with the visible selector overlay shown. Press Alt+Numpad0,
# Alt+0, or Alt+Insert to hide/show it while in-game.
overlay_visible_on_start = true
hotkeys_file = ".er-net-effects-hotkeys.json"
selected_effect_file = ".er-net-effects-setting.txt"
selected_catalog_file = ".er-net-effects-catalog-setting.txt"
enabled_file = ".er-net-effects-enabled.txt"
command_file = "er-net-effects-command.txt"
telemetry_file = "er-net-effects-telemetry.json"
catalog_dir = "er-net-effect-catalogs"
master_catalog_file = "er-net-effect-master-catalog.json"
EOF
cat >"$tmp_dir/.er-net-effects-hotkeys.json.example" <<'EOF'
{
  "hotkeys": [
    {
      "name": "deathblight network test",
      "key": "numpad_multiply",
      "effect_id": 8355,
      "count": 1
    }
  ]
}
EOF
rich_master="$repo_root/target/er-net-effect-master-catalog-rich.json"
if [[ -f "$rich_master" ]]; then
	cp -f "$rich_master" "$tmp_dir/er-net-effect-master-catalog.json"
else
	python3 - "$tmp_dir" "$repo_root" <<'PY'
import json
import sys
from pathlib import Path

out = Path(sys.argv[1])
repo = Path(sys.argv[2])
effects = json.loads((repo / "data" / "effects.json").read_text(encoding="utf-8"))["calls"]
if not effects:
    raise SystemExit("data/effects.json contains no calls")
master = {
    "schema_version": 1,
    "kind": "sp_effect_master_catalog",
    "source": {
        "param": "SpEffectParam",
        "binder_version": "",
        "row_count": len(effects),
        "regulation_file": "data/effects.json",
        "paramdef_file": "",
        "names_file": "",
    },
    "field_index": {},
    "effects": [
        {
            "id": int(call["id"]),
            "name": str(call["name"]),
            "row_name": None,
            "community_name": None,
            "curated_name": None,
            "vfx": [],
            "tags": ["bundled"],
            "fields": {},
        }
        for call in effects
    ],
}
(out / "er-net-effect-master-catalog.json").write_text(
    json.dumps(master, indent=2) + "\n", encoding="utf-8"
)
PY
fi
python3 "$repo_root/scripts/generate-effect-discriminator-catalogs.py" \
	--master "$tmp_dir/er-net-effect-master-catalog.json" \
	--catalog-dir "$tmp_dir/er-net-effect-catalogs" \
	--effects "$repo_root/data/effects.json" \
	--clean

(
	cd "$tmp_dir"
	find . -type f -print0 | sort -z | xargs -0 sha256sum >SHA256SUMS.txt
)

rm -rf "$out_dir"
mv -f "$tmp_dir" "$out_dir"
printf 'staged_net_effects_release=%s\n' "$out_dir"
