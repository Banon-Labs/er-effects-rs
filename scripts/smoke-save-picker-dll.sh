#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="x86_64-pc-windows-msvc"
PROFILE_NAME="save-picker-dll-standalone.me3"
ARTIFACT_DIR="$REPO_ROOT/target/runtime-probe/save-picker-dll-standalone-$(date -u +%Y%m%d-%H%M%S)"
DLL_PATH="$REPO_ROOT/target/$TARGET/release/er_save_picker_dll.dll"
BUILD=1
LAUNCH=0

usage() {
  cat <<'USAGE'
Usage: scripts/smoke-save-picker-dll.sh [options]

Builds/prepares a standalone ME3 profile for er_save_picker_dll.dll. By default it builds the DLL
and writes artifacts only; pass --launch to run the approved ME3/offline Elden Ring path.
This is a surface/staging smoke: the standalone DLL validates/plans a picked save through
`er-save-redirect` and releases its own picker latch, but it does not install the product
save-redirect hooks or prove standalone autoload.

Options:
  --artifact-dir DIR   Artifact directory to create/use
  --dll PATH           Existing er_save_picker_dll.dll path, or build output path override
  --skip-build         Do not run cargo xwin build; --dll must point at an existing DLL
  --prepare-only       Build/write profile but do not launch (default)
  --launch             Launch Elden Ring through scripts/me3-launch-lib.sh after preflight
  -h, --help           Show this help
USAGE
}

require_value() {
  local flag="$1" value="${2:-}"
  [[ -n "$value" ]] || { echo "$flag requires a value" >&2; exit 2; }
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --artifact-dir) require_value "$1" "${2:-}"; ARTIFACT_DIR="$2"; shift 2 ;;
    --dll) require_value "$1" "${2:-}"; DLL_PATH="$2"; shift 2 ;;
    --skip-build) BUILD=0; shift ;;
    --prepare-only) LAUNCH=0; shift ;;
    --launch) LAUNCH=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

mkdir -p "$ARTIFACT_DIR"
SUMMARY="$ARTIFACT_DIR/summary.txt"
PROFILE_PATH="$ARTIFACT_DIR/$PROFILE_NAME"

if [[ "$BUILD" -eq 1 ]]; then
  cargo xwin build --release --target "$TARGET" -p er-save-picker-dll
fi

DLL_PATH="$(realpath "$DLL_PATH")"
[[ -f "$DLL_PATH" ]] || { echo "standalone save-picker DLL not found: $DLL_PATH" >&2; exit 2; }
[[ "$(basename "$DLL_PATH")" == "er_save_picker_dll.dll" ]] || {
  echo "expected er_save_picker_dll.dll, got: $DLL_PATH" >&2
  exit 2
}

cat > "$PROFILE_PATH" <<EOF_PROFILE
profileVersion = "v1"
start_online = false

[[supports]]
game = "eldenring"

[[natives]]
path = '$DLL_PATH'
EOF_PROFILE

{
  echo "artifact_dir=$ARTIFACT_DIR"
  echo "profile=$PROFILE_PATH"
  echo "dll=$DLL_PATH"
  echo "launch=$LAUNCH"
  echo "expected_log=Game/er-save-picker-dll.log"
} > "$SUMMARY"

if [[ "$LAUNCH" -eq 0 ]]; then
  echo "Prepared standalone save-picker DLL smoke artifacts: $SUMMARY"
  exit 0
fi

# shellcheck source=scripts/steam-running.sh
source "$REPO_ROOT/scripts/steam-running.sh"
if ! steam_running; then
  echo "Steam is not running; standalone runtime smoke blocked before launch." | tee -a "$SUMMARY" >&2
  exit 2
fi

# shellcheck source=scripts/me3-launch-lib.sh
source "$REPO_ROOT/scripts/me3-launch-lib.sh"
me3_preflight | tee -a "$SUMMARY"
GAME_DIR="$ME3_STEAM_DIR/steamapps/common/ELDEN RING/Game"
me3_require_no_lazyloader "$GAME_DIR"
cd "$GAME_DIR"

echo "Launching standalone er_save_picker_dll smoke via ME3; profile=$PROFILE_PATH" | tee -a "$SUMMARY"
me3_launch "$PROFILE_PATH"
