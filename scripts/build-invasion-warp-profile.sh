#!/usr/bin/env bash
# Build er_invasion_warp_dll.dll and write an me3 profile that loads it.
#
# WHY THIS EXISTS. `scripts/check-rust-build.sh` type-checks the invasion-warp crates with
# `cargo xwin check --tests`, which NEVER LINKS a cdylib, and `default-members` is only
# `crates/er-effects-rs`, so the documented `cargo xwin build --release` does not build this
# DLL either. Before this script the only way to obtain a loadable artifact was an ad-hoc
# `-p` invocation, and no profile in the repo referenced it -- so a green gate could coexist
# with a DLL that does not link and cannot be loaded (bd er-effects-rs-5es review).
#
# The invasion-warp shell owns no Present detour and no MinHook instance, so unlike
# `er_loading_portrait_dll.dll` it is SAFE alongside the product DLL in one profile. That
# stays true only while it installs no detours; the first detour it adds must go through the
# `er-hook` union, and this comment should be revisited then.
#
# This script does NOT launch the game. It prints the profile path; launching is the caller's
# (or the user's) decision.
#
# Usage:
#   bash scripts/build-invasion-warp-profile.sh [OUT_DIR]
#
# Env:
#   INVASION_WARP_WITH_PRODUCT=0   omit er_effects_rs.dll (invasion-warp alone)
#   INVASION_WARP_WITH_SEAMLESS=1  also load the game-installed SeamlessCoop/ersc.dll
#   SEAMLESS_DLL                   override the Seamless DLL path
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
out_dir="${1:-$repo_root/target/invasion-warp-profile}"
target="x86_64-pc-windows-msvc"
release_dir="$repo_root/target/$target/release"

with_product="${INVASION_WARP_WITH_PRODUCT:-1}"
with_seamless="${INVASION_WARP_WITH_SEAMLESS:-0}"
seamless_dll="${SEAMLESS_DLL:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll}"

fatal() {
  echo "[invasion-warp-profile] $*" >&2
  exit 1
}

# Real LINK of the cdylib, not a metadata check. This is the step whose absence let an
# unlinkable DLL pass every gate.
echo "[invasion-warp-profile] cargo xwin build --release -p er-invasion-warp-dll --target $target"
cargo xwin build --release -p er-invasion-warp-dll \
  --manifest-path "$repo_root/Cargo.toml" --target "$target"

invasion_dll="$release_dir/er_invasion_warp_dll.dll"
[[ -f "$invasion_dll" ]] || fatal "cdylib did not link: $invasion_dll"

natives=()
if [[ "$with_seamless" == "1" ]]; then
  # Referenced in place by absolute path and never copied or staged (Do-not-bundle-ersc rule).
  [[ -f "$seamless_dll" ]] || fatal "SEAMLESS requested but not found: $seamless_dll"
  natives+=("$seamless_dll")
fi
if [[ "$with_product" == "1" ]]; then
  product_dll="$release_dir/er_effects_rs.dll"
  [[ -f "$product_dll" ]] || fatal "product DLL missing: $product_dll (build: cargo xwin build --release --target $target)"
  natives+=("$product_dll")
fi
natives+=("$invasion_dll")

mkdir -p "$out_dir"
profile="$out_dir/invasion-warp.me3"
{
  printf 'profileVersion = "v1"\n'
  printf 'start_online = false\n\n'
  printf '[[supports]]\n'
  printf 'game = "eldenring"\n'
  for n in "${natives[@]}"; do
    printf '\n[[natives]]\n'
    printf "path = '%s'\n" "$n"
  done
} > "$profile"

echo "[invasion-warp-profile] wrote $profile"
for n in "${natives[@]}"; do
  echo "[invasion-warp-profile]   native: $n"
done
cat <<'EOF'
[invasion-warp-profile]
[invasion-warp-profile] HOW TO TRY IT. Load into the world, then:
[invasion-warp-profile]   F7  warp to the NEAREST invasion spawn point (skips the one you are
[invasion-warp-profile]       standing on, so pressing it repeatedly keeps moving)
[invasion-warp-profile]   F8  warp to the NEXT point in catalog order -- this one crosses the
[invasion-warp-profile]       map, because the order is by block id, not by distance
[invasion-warp-profile]   F9  warp to another AREA entirely (base game <-> Shadow of the
[invasion-warp-profile]       Erdtree). F7 can only see points in the area you are already in,
[invasion-warp-profile]       because ranking by distance needs world coordinates and the engine
[invasion-warp-profile]       only converts blocks in the resident area. F8 and F9 have no such
[invasion-warp-profile]       limit: the warp takes BLOCK-LOCAL coordinates and the game converts
[invasion-warp-profile]       them after the destination loads.
[invasion-warp-profile] Both only respond while the Elden Ring window has focus. Each warp is a
[invasion-warp-profile] real area reload, so expect a loading screen exactly like a grace warp.
[invasion-warp-profile]
[invasion-warp-profile] There is still NO WORLD-MAP UI: no pins, no menu entry, no tab. The map
[invasion-warp-profile] surface is the next slice; these hotkeys exist to prove the warp payload
[invasion-warp-profile] underneath it.
[invasion-warp-profile]
[invasion-warp-profile] Evidence written next to the game exe:
[invasion-warp-profile]   er-invasion-warp-dll.log        every warp, refusal and verdict
[invasion-warp-profile]   er-invasion-warp-telemetry.json oracle 1, the catalog read
[invasion-warp-profile]   er-invasion-warp-run.json       the per-warp oracle document
[invasion-warp-profile] Oracle 1 passes on an EXACT cardinality match (365 blocks / 7073
[invasion-warp-profile] targets / 2 areas with dlc02; 257 / 4482 / 1 without) -- not "> 0".
[invasion-warp-profile] A WARP counts as proven only on "verdict":"arrived" -- the player read
[invasion-warp-profile] back in the destination block within 5m of the requested point. Landing
[invasion-warp-profile] in the right block at the wrong spot reports "mislanded" and is a
[invasion-warp-profile] FAILURE (it means the explicit-spawn slot did not take), and a warp that
[invasion-warp-profile] was merely issued never reports passed.
EOF
