#!/usr/bin/env bash
# er-artifact-redirect: this script GENERATES the two user-facing launchers below, and their
# redirect lines are the ones a user's run actually uses. The `me3 launch` commands live inside
# the heredocs that write them, which the audit's shape detector treats as data rather than a
# command (correctly -- see stage-autoload-release.sh, whose usage() text is not a launch). This
# marker opts the generator in so both halves are checked against the live knob table.
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
TARGET_TRIPLE="x86_64-pc-windows-msvc"
DLL_PATH="$REPO_ROOT/target/$TARGET_TRIPLE/release/er_quickload.dll"
OUT_DIR="$REPO_ROOT/target/deliverables"
PACKAGE_NAME="er-quickload-me3-poc"
DO_BUILD=0

usage() {
  cat <<'USAGE'
Usage: scripts/build-me3-poc-package.sh [--build] [--dll PATH] [--out-dir DIR] [--name NAME]

Build a minimal ME3 POC zip containing:
  er_quickload.dll
  er-quickload.toml
  er-quickload-poc.me3
  run-er-quickload-poc.ps1
  run-er-quickload-poc.sh

The launchers write the required DLL-adjacent er-quickload.toml, set telemetry/log
env vars, generate an absolute-path ME3 profile next to themselves, then call ME3
with that profile. They require the user to pass a save file path at launch time;
the save file is intentionally not bundled.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --build) DO_BUILD=1; shift ;;
    --dll) DLL_PATH="$2"; shift 2 ;;
    --out-dir) OUT_DIR="$2"; shift 2 ;;
    --name) PACKAGE_NAME="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

require_file() { [[ -f "$1" ]] || { echo "missing file: $1" >&2; exit 2; }; }
require_cmd() { command -v "$1" >/dev/null 2>&1 || { echo "missing command: $1" >&2; exit 127; }; }

if [[ "$DO_BUILD" == "1" ]]; then
  (cd "$REPO_ROOT" && cargo xwin build --release --target "$TARGET_TRIPLE")
fi

require_file "$DLL_PATH"
require_cmd python3

COMMIT="$(cd "$REPO_ROOT" && git rev-parse --short HEAD 2>/dev/null || echo unknown)"
STAGE_DIR="$OUT_DIR/$PACKAGE_NAME-$COMMIT"
ZIP_PATH="$OUT_DIR/$PACKAGE_NAME-$COMMIT.zip"
rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR"

cp -f "$DLL_PATH" "$STAGE_DIR/er_quickload.dll"

cat > "$STAGE_DIR/er-quickload-poc.me3" <<'EOF_PROFILE'
profileVersion = "v1"

[[supports]]
game = "eldenring"

[[natives]]
# The launch scripts generate er-quickload-poc.generated.me3 with an absolute DLL path.
# This static config is kept as the smallest human-readable ME3 profile for the bundle.
path = 'er_quickload.dll'
EOF_PROFILE

cat > "$STAGE_DIR/er-quickload.toml" <<'EOF_CONFIG'
# Required: this file must live next to er_quickload.dll.
# The launch scripts overwrite save_file/slot before launching.
save_file = "CHANGE_ME_TO_A_COPY_OF_ER0000.sl2"
slot = 0
EOF_CONFIG

cat > "$STAGE_DIR/run-er-quickload-poc.ps1" <<'EOF_PS'
param(
    [Parameter(Mandatory=$true)]
    [string]$SaveFile,

    [string]$Me3Path = "me3",
    [string]$Game = "eldenring",
    [int]$Slot = 0,
    [string]$SteamDir = ""
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$DllPath = (Resolve-Path (Join-Path $Root "er_quickload.dll")).Path
$SavePath = (Resolve-Path $SaveFile).Path
$LogDir = Join-Path $Root "logs"
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

function Convert-ToTomlBasicString([string]$Value) {
    return '"' + (($Value -replace '\\', '\\\\') -replace '"', '\"') + '"'
}

$ProfilePath = Join-Path $Root "er-quickload-poc.generated.me3"
$ConfigPath = Join-Path $Root "er-quickload.toml"
$DllToml = Convert-ToTomlBasicString $DllPath
$SaveToml = Convert-ToTomlBasicString $SavePath
@"
profileVersion = "v1"

[[supports]]
game = "$Game"

[[natives]]
path = $DllToml
"@ | Set-Content -Encoding UTF8 -Path $ProfilePath
@"
# Required: this file must live next to er_quickload.dll.
# ER_QUICKLOAD_SAVE_FILE / ER_QUICKLOAD_AUTOLOAD_SLOT may override these values.
save_file = $SaveToml
slot = $Slot
"@ | Set-Content -Encoding UTF8 -Path $ConfigPath

# EVERY per-run artifact goes to $LogDir. Anything left beside the game exe is SINGLE-SLOT: the DLL
# rotates `<name>` to `<name>.prev` on its first write, so run N-2 is already gone. Add a line here
# (and to the bash launcher below) for any future log rather than copying it out afterwards.
# ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH also relocates the portrait-capture-slot*.bin dumps.
$env:ER_QUICKLOAD_TELEMETRY_PATH = Join-Path $LogDir "er-quickload-telemetry.json"
$env:ER_QUICKLOAD_BOOTSTRAP_PATH = Join-Path $LogDir "bootstrap.jsonl"
$env:ER_QUICKLOAD_BOOTSTRAP_STATE_PATH = Join-Path $LogDir "bootstrap-state.json"
$env:ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH = Join-Path $LogDir "er-crash-log.txt"
$env:ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH = Join-Path $LogDir "er-crash-latest.txt"
$env:ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH = Join-Path $LogDir "er-crash-breadcrumb-latest.txt"
$env:ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH = Join-Path $LogDir "er-crash-modules.txt"
$env:ER_QUICKLOAD_CRASH_LOG = "1"
$env:ER_QUICKLOAD_CRASH_LOG_PATH = Join-Path $LogDir "er-quickload-crash-log.txt"
$env:ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH = Join-Path $LogDir "er-quickload-autoload-debug.log"
$env:ER_QUICKLOAD_TRACE_CONTINUE_PATH = Join-Path $LogDir "er-quickload-continue-trace.log"
$env:ER_QUICKLOAD_INPUT_TRACE_PATH = Join-Path $LogDir "er-quickload-input-trace.jsonl"
$env:ER_QUICKLOAD_PROFILE_PATH = Join-Path $LogDir "er-quickload-profile.jsonl"
$env:ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH = Join-Path $LogDir "er-crash-log.txt"
$env:ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH = Join-Path $LogDir "er-crash-latest.txt"
$env:ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH = Join-Path $LogDir "er-crash-breadcrumb-latest.txt"
$env:ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH = Join-Path $LogDir "er-crash-modules.txt"
# The companion shells' artifacts. These five had no redirect knob at all until 2026-08-31, so no
# launcher could move them; the reload trace alone runs at ~655 MB/hour. Set even when this profile
# loads only the product, so adding a companion native later does not silently start leaking.
$env:ER_QUICKLOAD_RELOAD_TRACE_PATH = Join-Path $LogDir "er-reload-trace.log"
$env:ER_QUICKLOAD_INPUT_HARNESS_LOG_PATH = Join-Path $LogDir "er-input-harness.log"
$env:ER_QUICKLOAD_INPUT_HARNESS_PHASES_PATH = Join-Path $LogDir "er-input-harness-phases.jsonl"
$env:ER_QUICKLOAD_DIAG_HARNESS_PATH = Join-Path $LogDir "er-diag-harness.log"
$env:ER_QUICKLOAD_TIMESERIES_PATH = Join-Path $LogDir "er-telemetry-timeseries.jsonl"
$env:ER_QUICKLOAD_CPU_PROFILE_PATH = Join-Path $LogDir "er-cpu-profile.txt"
$env:ER_QUICKLOAD_ARMAMENT_ICONS_PATH = Join-Path $LogDir "er-armament-icons.log"
$env:ER_QUICKLOAD_SAVE_DISABLE_LOG_PATH = Join-Path $LogDir "er-save-disable.log"
$env:ER_QUICKLOAD_SAVE_DISABLE_TELEMETRY_PATH = Join-Path $LogDir "er-save-disable-telemetry.json"
$env:ER_QUICKLOAD_LOADING_PORTRAIT_PATH = Join-Path $LogDir "er-loading-portrait.log"
$env:ER_QUICKLOAD_LOADING_PORTRAIT_CRASH_LOG_PATH = Join-Path $LogDir "er-loading-portrait-crash-log.txt"
$env:ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH = Join-Path $LogDir "er-crash-log.txt"
$env:ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH = Join-Path $LogDir "er-crash-latest.txt"
$env:ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH = Join-Path $LogDir "er-crash-breadcrumb-latest.txt"
$env:ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH = Join-Path $LogDir "er-crash-modules.txt"

$Args = @()
if ($SteamDir -ne "") { $Args += @("--steam-dir", $SteamDir) }
$Args += @("launch", "-g", $Game, "-p", $ProfilePath)

Write-Host "ME3 profile: $ProfilePath"
Write-Host "DLL: $DllPath"
Write-Host "Save: $SavePath"
Write-Host "Logs: $LogDir"
& $Me3Path @Args
exit $LASTEXITCODE
EOF_PS

cat > "$STAGE_DIR/run-er-quickload-poc.sh" <<'EOF_SH'
#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ME3_PATH="${ME3_PATH:-me3}"
GAME="${GAME:-eldenring}"
SLOT="${ER_QUICKLOAD_AUTOLOAD_SLOT:-0}"
STEAM_DIR="${ME3_STEAM_DIR:-}"

usage() {
  cat <<'USAGE'
Usage: ./run-er-quickload-poc.sh /path/to/ER0000.sl2

Optional env:
  ME3_PATH=/path/to/me3
  ME3_STEAM_DIR=/path/to/Steam
  GAME=eldenring
  ER_QUICKLOAD_AUTOLOAD_SLOT=0  # optional env override; script also writes slot to er-quickload.toml
USAGE
}

[[ $# -eq 1 ]] || { usage >&2; exit 2; }
SAVE_FILE="$(realpath "$1")"
DLL_PATH="$ROOT/er_quickload.dll"
PROFILE_PATH="$ROOT/er-quickload-poc.generated.me3"
CONFIG_PATH="$ROOT/er-quickload.toml"
LOG_DIR="$ROOT/logs"
mkdir -p "$LOG_DIR"
[[ -f "$DLL_PATH" ]] || { echo "missing DLL: $DLL_PATH" >&2; exit 2; }
[[ -f "$SAVE_FILE" ]] || { echo "missing save file: $SAVE_FILE" >&2; exit 2; }

python3 - "$PROFILE_PATH" "$CONFIG_PATH" "$DLL_PATH" "$SAVE_FILE" "$GAME" "$SLOT" <<'PY'
from pathlib import Path
import json
import sys
profile = Path(sys.argv[1])
config = Path(sys.argv[2])
dll = sys.argv[3]
save = sys.argv[4]
game = sys.argv[5]
slot = int(sys.argv[6])
profile.write_text(
    'profileVersion = "v1"\n\n'
    '[[supports]]\n'
    f'game = {json.dumps(game)}\n\n'
    '[[natives]]\n'
    f'path = {json.dumps(dll)}\n',
    encoding='utf-8',
)
config.write_text(
    '# Required: this file must live next to er_quickload.dll.\n'
    '# ER_QUICKLOAD_SAVE_FILE / ER_QUICKLOAD_AUTOLOAD_SLOT may override these values.\n'
    f'save_file = {json.dumps(save)}\n'
    f'slot = {slot}\n',
    encoding='utf-8',
)
PY

# EVERY per-run artifact goes to $LOG_DIR. Anything left beside the game exe is SINGLE-SLOT: the DLL
# rotates `<name>` to `<name>.prev` on its first write, so run N-2 is already gone. Add a line here
# (and to the PowerShell launcher above) for any future log rather than copying it out afterwards.
# ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH also relocates the portrait-capture-slot*.bin dumps.
export ER_QUICKLOAD_TELEMETRY_PATH="$LOG_DIR/er-quickload-telemetry.json"
export ER_QUICKLOAD_BOOTSTRAP_PATH="$LOG_DIR/bootstrap.jsonl"
export ER_QUICKLOAD_BOOTSTRAP_STATE_PATH="$LOG_DIR/bootstrap-state.json"
export ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH="$LOG_DIR/er-crash-log.txt"
export ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH="$LOG_DIR/er-crash-latest.txt"
export ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH="$LOG_DIR/er-crash-breadcrumb-latest.txt"
export ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH="$LOG_DIR/er-crash-modules.txt"
export ER_QUICKLOAD_CRASH_LOG=1
export ER_QUICKLOAD_CRASH_LOG_PATH="$LOG_DIR/er-quickload-crash-log.txt"
export ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH="$LOG_DIR/er-quickload-autoload-debug.log"
export ER_QUICKLOAD_TRACE_CONTINUE_PATH="$LOG_DIR/er-quickload-continue-trace.log"
export ER_QUICKLOAD_INPUT_TRACE_PATH="$LOG_DIR/er-quickload-input-trace.jsonl"
export ER_QUICKLOAD_PROFILE_PATH="$LOG_DIR/er-quickload-profile.jsonl"
export ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH="$LOG_DIR/er-crash-log.txt"
export ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH="$LOG_DIR/er-crash-latest.txt"
export ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH="$LOG_DIR/er-crash-breadcrumb-latest.txt"
export ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH="$LOG_DIR/er-crash-modules.txt"
# The companion shells' artifacts. These five had no redirect knob at all until 2026-08-31, so no
# launcher could move them; the reload trace alone runs at ~655 MB/hour. Set even when this profile
# loads only the product, so adding a companion native later does not silently start leaking.
export ER_QUICKLOAD_RELOAD_TRACE_PATH="$LOG_DIR/er-reload-trace.log"
export ER_QUICKLOAD_INPUT_HARNESS_LOG_PATH="$LOG_DIR/er-input-harness.log"
export ER_QUICKLOAD_INPUT_HARNESS_PHASES_PATH="$LOG_DIR/er-input-harness-phases.jsonl"
export ER_QUICKLOAD_DIAG_HARNESS_PATH="$LOG_DIR/er-diag-harness.log"
export ER_QUICKLOAD_TIMESERIES_PATH="$LOG_DIR/er-telemetry-timeseries.jsonl"
export ER_QUICKLOAD_CPU_PROFILE_PATH="$LOG_DIR/er-cpu-profile.txt"
export ER_QUICKLOAD_ARMAMENT_ICONS_PATH="$LOG_DIR/er-armament-icons.log"
export ER_QUICKLOAD_SAVE_DISABLE_LOG_PATH="$LOG_DIR/er-save-disable.log"
export ER_QUICKLOAD_SAVE_DISABLE_TELEMETRY_PATH="$LOG_DIR/er-save-disable-telemetry.json"
export ER_QUICKLOAD_LOADING_PORTRAIT_PATH="$LOG_DIR/er-loading-portrait.log"
export ER_QUICKLOAD_LOADING_PORTRAIT_CRASH_LOG_PATH="$LOG_DIR/er-loading-portrait-crash-log.txt"
export ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH="$LOG_DIR/er-crash-log.txt"
export ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH="$LOG_DIR/er-crash-latest.txt"
export ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH="$LOG_DIR/er-crash-breadcrumb-latest.txt"
export ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH="$LOG_DIR/er-crash-modules.txt"

args=()
if [[ -n "$STEAM_DIR" ]]; then
  args+=(--steam-dir "$STEAM_DIR")
fi
args+=(launch -g "$GAME" -p "$PROFILE_PATH")

echo "ME3 profile: $PROFILE_PATH"
echo "DLL: $DLL_PATH"
echo "Config: $CONFIG_PATH"
echo "Save: $SAVE_FILE"
echo "Logs: $LOG_DIR"
exec "$ME3_PATH" "${args[@]}"
EOF_SH
chmod +x "$STAGE_DIR/run-er-quickload-poc.sh"

cat > "$STAGE_DIR/README.txt" <<'EOF_README'
Minimal er-quickload + ME3 POC

Windows PowerShell:
  .\run-er-quickload-poc.ps1 -SaveFile "C:\path\to\ER0000.sl2" -Me3Path "C:\path\to\me3.exe"

Linux:
  ME3_PATH=/path/to/me3 ME3_STEAM_DIR="$HOME/.local/share/Steam" ./run-er-quickload-poc.sh /path/to/ER0000.sl2

The launchers write er-quickload.toml next to er_quickload.dll, set telemetry/log
env vars, generate an absolute-path ME3 profile, then run ME3 with er_quickload.dll
as a native. ER_QUICKLOAD_SAVE_FILE and ER_QUICKLOAD_AUTOLOAD_SLOT remain optional
overrides for the TOML values.
EOF_README

python3 - "$STAGE_DIR" "$ZIP_PATH" <<'PY'
from pathlib import Path
import sys
import zipfile
stage = Path(sys.argv[1])
zip_path = Path(sys.argv[2])
zip_path.parent.mkdir(parents=True, exist_ok=True)
with zipfile.ZipFile(zip_path, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
    for path in sorted(stage.rglob('*')):
        if path.is_file():
            zf.write(path, path.relative_to(stage).as_posix())
print(f'stage_dir={stage}')
print(f'zip_path={zip_path}')
with zipfile.ZipFile(zip_path) as zf:
    for info in zf.infolist():
        print(f'{info.file_size:9d} {info.filename}')
PY
