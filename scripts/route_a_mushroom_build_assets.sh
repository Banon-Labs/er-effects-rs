#!/usr/bin/env bash
# Build the offline Route A mushroom prototype asset folders.
#
# This script never launches Elden Ring or Dark Souls. It compiles/runs the
# Rust-only Route A helper scripts, prepares high/low BD_M_1010 donor folders,
# patches their FLVER files with c2280 geometry, and stages c2280 texture bytes
# into the donor TPF folders. WitchyBND packing is intentionally a separate
# phase because each pack/unpack step should stay individually bounded.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

proto_root="target/mushroom-route-a-offline/prototype"
high_src="target/er-extract-parts-sample/bd_m_1010-partsbnd-dcx"
low_src="target/er-extract-parts-sample/bd_m_1010_l-partsbnd-dcx"
high_dst="$proto_root/bd_m_1010-mushroom-parts"
low_dst="$proto_root/bd_m_1010_l-mushroom-parts"

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

require_path "target/mushroom-route-a-offline/dsr/dsr-loose-mushroom/c2280-chrbnd-dcx/c2280.flver"
require_path "$high_src/BD_M_1010.flver"
require_path "$low_src/BD_M_1010_L.flver"

mkdir -p "$proto_root"
rustc scripts/route_a_mushroom_export.rs -O -o target/route_a_mushroom_export
./target/route_a_mushroom_export > "$proto_root/export-smoke.out"

copy_donor_payload "$high_src" "$high_dst" "BD_M_1010.flver"
copy_donor_payload "$low_src" "$low_dst" "BD_M_1010_L.flver"

rustc scripts/route_a_mushroom_patch_donor.rs -O -o target/route_a_mushroom_patch_donor
./target/route_a_mushroom_patch_donor > "$proto_root/patch-smoke.out"
./target/route_a_mushroom_patch_donor \
  --donor-flver "$low_src/BD_M_1010_L.flver" \
  --output-flver "$low_dst/BD_M_1010_L.flver" \
  --summary "$proto_root/bd_m_1010_l-mushroom-parts-summary.txt" \
  > "$proto_root/patch-l-smoke.out"

rustc scripts/route_a_mushroom_stage_textures.rs -O -o target/route_a_mushroom_stage_textures
./target/route_a_mushroom_stage_textures > "$proto_root/stage-textures-smoke.out"
./target/route_a_mushroom_stage_textures \
  --parts-dir "$low_dst" \
  --tpf-dir-name BD_M_1010_L-tpf \
  --tpf-filename BD_M_1010_L.tpf \
  --texture-suffix _l \
  > "$proto_root/stage-textures-l-smoke.out"

printf 'built offline mushroom asset folders:\n'
printf '  %s\n' "$high_dst" "$low_dst"
printf 'next pack phase: pack each *-tpf folder with WitchyBND, then pack each *-mushroom-parts folder.\n'
