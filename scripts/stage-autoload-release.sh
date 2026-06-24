#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
out_dir="$repo_root/target/autoload-release"
build=1

usage() {
  cat <<'EOF'
Usage: scripts/stage-autoload-release.sh [--output DIR] [--no-build]

Stages the supported zero-input autoload release payload:
  dinput8.dll                  LazyLoader proxy
  lazyLoad.ini                 CHAINLOAD er_effects_rs.dll as the properly-loaded mod
  er_effects_rs.dll            repo DLL loaded through LazyLoader [CHAINLOAD]
  dllMods/                     available for other LazyLoader mods
  er-effects-autoload.txt.example
  er-effects-splash-skip.txt.example  optional built-in splash-skip toggle

Environment:
  LAZYLOADER_DIR  directory containing LazyLoader dinput8.dll
  ER_EFFECTS_DLL  prebuilt er_effects_rs.dll path (defaults to target release DLL)
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
    -h|--help)
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

lazyloader_dir="${LAZYLOADER_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game/dllMods.disabled/lazyloader-20260611-234916}"
lazyloader_proxy="$lazyloader_dir/dinput8.dll"
er_effects_dll="${ER_EFFECTS_DLL:-$repo_root/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll}"

if [[ "$build" == "1" ]]; then
  cargo xwin build --manifest-path "$repo_root/Cargo.toml" --target x86_64-pc-windows-msvc --release
fi

if [[ ! -f "$lazyloader_proxy" ]]; then
  echo "missing LazyLoader dinput8.dll: $lazyloader_proxy" >&2
  exit 1
fi
if [[ ! -f "$er_effects_dll" ]]; then
  echo "missing er_effects_rs.dll: $er_effects_dll" >&2
  exit 1
fi

out_dir=$(realpath -m "$out_dir")
tmp_dir="$out_dir.tmp"
rm -rf "$tmp_dir"
mkdir -p "$tmp_dir/dllMods"

cp -f "$lazyloader_proxy" "$tmp_dir/dinput8.dll"
cp -f "$er_effects_dll" "$tmp_dir/er_effects_rs.dll"
cat > "$tmp_dir/lazyLoad.ini" <<'EOF'
; LazyLoader by Church Guard
; er-effects-rs must be properly loaded, not lazy-loaded, so it is the CHAINLOAD DLL.
; Put additional LazyLoader mods in dllMods and list them under [LOADORDER].

[LAZYLOAD]
dllModFolderName=dllMods

[LOADORDER]

[CHAINLOAD]
dll=er_effects_rs.dll
EOF
cat > "$tmp_dir/er-effects-autoload.txt.example" <<'EOF'
# Product/default zero-input gold-load request.
# Do not set the direct-menu-load method here: that arms the experimental product_core/menu path only
# when er-effects-experimental-direct-menu-load.txt or ER_EFFECTS_EXPERIMENTAL_DIRECT_MENU_LOAD=1 is
# also present. The supported path keeps product_core off and uses the native Continue/PAB gates.
slot=0
EOF
cat > "$tmp_dir/er-effects-native-continue.txt.example" <<'EOF'
# Copy to er-effects-native-continue.txt next to eldenring.exe to enable the supported
# zero-input native Continue path.
EOF
cat > "$tmp_dir/er-effects-pab-advance.txt.example" <<'EOF'
# Copy to er-effects-pab-advance.txt next to eldenring.exe to enable the supported
# zero-input press-any-button/menu-open advance.
EOF
cat > "$tmp_dir/er-effects-splash-skip.txt.example" <<'EOF'
# Copy this file to er-effects-splash-skip.txt next to eldenring.exe to enable
# er-effects-rs' built-in current-version splash skip patch.
EOF
(
  cd "$tmp_dir"
  sha256sum dinput8.dll lazyLoad.ini er_effects_rs.dll er-effects-autoload.txt.example er-effects-native-continue.txt.example er-effects-pab-advance.txt.example er-effects-splash-skip.txt.example > SHA256SUMS.txt
)

rm -rf "$out_dir"
mv -f "$tmp_dir" "$out_dir"
printf 'staged_autoload_release=%s\n' "$out_dir"
