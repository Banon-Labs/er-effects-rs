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
  er-net-effects.me3           me3 ModProfile loading only the net-effects DLL
  er-net-effects.toml.example  per-feature configuration file
  .er-net-effects-hotkeys.json.example  keyboard-trigger configuration
  er-net-effect-catalogs/example.json   example selector catalog

Install: keep the folder together anywhere (the profile references the DLL
relative to itself), copy the wanted er-net-effects config/catalog files next to
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
path = 'er_net_effects_dll.dll'
EOF
cat >"$tmp_dir/er-net-effects.toml.example" <<'EOF'
# Copy to er-net-effects.toml next to eldenring.exe.
# This file belongs to er_net_effects_dll.dll and is intentionally separate from
# er-effects-rs product/autoload configuration.
network_sync = true
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
cat >"$tmp_dir/er-net-effect-catalogs/example.json" <<'EOF'
[
  8355
]
EOF
(
	cd "$tmp_dir"
	sha256sum er_net_effects_dll.dll er-net-effects.me3 er-net-effects.toml.example .er-net-effects-hotkeys.json.example er-net-effect-catalogs/example.json >SHA256SUMS.txt
)

rm -rf "$out_dir"
mv -f "$tmp_dir" "$out_dir"
printf 'staged_net_effects_release=%s\n' "$out_dir"
