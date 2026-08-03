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
[invasion-warp-profile] THIS FEATURE HAS NO UI. It installs zero detours: there are no map
[invasion-warp-profile] markers and no menu entry. The only way to observe it is to read
[invasion-warp-profile] these two files next to the game exe after loading into the world:
[invasion-warp-profile]   er-invasion-warp-telemetry.json
[invasion-warp-profile]   er-invasion-warp-dll.log
[invasion-warp-profile] Oracle 1 passes on an EXACT cardinality match (365 blocks / 7073
[invasion-warp-profile] targets / 2 areas with dlc02; 257 / 4482 / 1 without) -- not "> 0".
[invasion-warp-profile] It compares counts and block-id extremes only, NOT bytes and NOT
[invasion-warp-profile] coordinates, and proves nothing about the UI or the warp itself.
EOF
