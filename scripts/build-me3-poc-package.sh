#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
TARGET_TRIPLE="x86_64-pc-windows-msvc"
DLL_PATH="$REPO_ROOT/target/$TARGET_TRIPLE/release/er_effects_rs.dll"
OUT_DIR="$REPO_ROOT/target/deliverables"
PACKAGE_NAME="er-effects-me3-poc"
DO_BUILD=0

usage() {
  cat <<'USAGE'
Usage: scripts/build-me3-poc-package.sh [--build] [--dll PATH] [--out-dir DIR] [--name NAME]

Build a minimal ME3 POC zip containing:
  er_effects_rs.dll
  er-effects-poc.me3
  run-er-effects-poc.ps1
  run-er-effects-poc.sh
  autoload-request.txt

The launchers set ER_EFFECTS_* env vars, generate an absolute-path ME3 profile next
to themselves, then call ME3 with that profile. They require the user to pass a save
file path at launch time; the save file is intentionally not bundled.
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

cp -f "$DLL_PATH" "$STAGE_DIR/er_effects_rs.dll"

cat > "$STAGE_DIR/er-effects-poc.me3" <<'EOF_PROFILE'
profileVersion = "v1"

[[supports]]
game = "eldenring"

[[natives]]
# The launch scripts generate er-effects-poc.generated.me3 with an absolute DLL path.
# This static config is kept as the smallest human-readable ME3 profile for the bundle.
path = 'er_effects_rs.dll'
EOF_PROFILE

cat > "$STAGE_DIR/autoload-request.txt" <<'EOF_AUTOLOAD'
slot=0
EOF_AUTOLOAD

cat > "$STAGE_DIR/run-er-effects-poc.ps1" <<'EOF_PS'
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
$DllPath = (Resolve-Path (Join-Path $Root "er_effects_rs.dll")).Path
$SavePath = (Resolve-Path $SaveFile).Path
$LogDir = Join-Path $Root "logs"
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

function Convert-ToTomlBasicString([string]$Value) {
    return ($Value -replace '\\', '\\' -replace '"', '\"')
}

$ProfilePath = Join-Path $Root "er-effects-poc.generated.me3"
$DllToml = Convert-ToTomlBasicString $DllPath
@"
profileVersion = "v1"

[[supports]]
game = "$Game"

[[natives]]
path = "$DllToml"
"@ | Set-Content -Encoding UTF8 -Path $ProfilePath

$AutoloadPath = Join-Path $Root "autoload-request.txt"
"slot=$Slot" | Set-Content -Encoding ASCII -Path $AutoloadPath

$env:ER_EFFECTS_SAVE_FILE = $SavePath
$env:ER_EFFECTS_AUTOLOAD_PATH = $AutoloadPath
$env:ER_EFFECTS_AUTOLOAD_SLOT = [string]$Slot
$env:ER_EFFECTS_TELEMETRY_PATH = Join-Path $LogDir "er-effects-telemetry.json"
$env:ER_EFFECTS_BOOTSTRAP_PATH = Join-Path $LogDir "bootstrap.jsonl"
$env:ER_EFFECTS_BOOTSTRAP_STATE_PATH = Join-Path $LogDir "bootstrap-state.json"
$env:ER_EFFECTS_CRASH_LOG = "1"
$env:ER_EFFECTS_CRASH_LOG_PATH = Join-Path $LogDir "er-effects-crash-log.txt"
$env:ER_EFFECTS_AUTOLOAD_DEBUG_PATH = Join-Path $LogDir "er-effects-autoload-debug.log"

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

cat > "$STAGE_DIR/run-er-effects-poc.sh" <<'EOF_SH'
#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ME3_PATH="${ME3_PATH:-me3}"
GAME="${GAME:-eldenring}"
SLOT="${ER_EFFECTS_AUTOLOAD_SLOT:-0}"
STEAM_DIR="${ME3_STEAM_DIR:-}"

usage() {
  cat <<'USAGE'
Usage: ./run-er-effects-poc.sh /path/to/ER0000.sl2

Optional env:
  ME3_PATH=/path/to/me3
  ME3_STEAM_DIR=/path/to/Steam
  GAME=eldenring
  ER_EFFECTS_AUTOLOAD_SLOT=0
USAGE
}

[[ $# -eq 1 ]] || { usage >&2; exit 2; }
SAVE_FILE="$(realpath "$1")"
DLL_PATH="$ROOT/er_effects_rs.dll"
PROFILE_PATH="$ROOT/er-effects-poc.generated.me3"
AUTOLOAD_PATH="$ROOT/autoload-request.txt"
LOG_DIR="$ROOT/logs"
mkdir -p "$LOG_DIR"
[[ -f "$DLL_PATH" ]] || { echo "missing DLL: $DLL_PATH" >&2; exit 2; }
[[ -f "$SAVE_FILE" ]] || { echo "missing save file: $SAVE_FILE" >&2; exit 2; }

python3 - "$PROFILE_PATH" "$DLL_PATH" "$GAME" <<'PY'
from pathlib import Path
import json
import sys
profile = Path(sys.argv[1])
dll = sys.argv[2]
game = sys.argv[3]
profile.write_text(
    'profileVersion = "v1"\n\n'
    '[[supports]]\n'
    f'game = {json.dumps(game)}\n\n'
    '[[natives]]\n'
    f'path = {json.dumps(dll)}\n',
    encoding='utf-8',
)
PY
printf 'slot=%s\n' "$SLOT" > "$AUTOLOAD_PATH"

export ER_EFFECTS_SAVE_FILE="$SAVE_FILE"
export ER_EFFECTS_AUTOLOAD_PATH="$AUTOLOAD_PATH"
export ER_EFFECTS_AUTOLOAD_SLOT="$SLOT"
export ER_EFFECTS_TELEMETRY_PATH="$LOG_DIR/er-effects-telemetry.json"
export ER_EFFECTS_BOOTSTRAP_PATH="$LOG_DIR/bootstrap.jsonl"
export ER_EFFECTS_BOOTSTRAP_STATE_PATH="$LOG_DIR/bootstrap-state.json"
export ER_EFFECTS_CRASH_LOG=1
export ER_EFFECTS_CRASH_LOG_PATH="$LOG_DIR/er-effects-crash-log.txt"
export ER_EFFECTS_AUTOLOAD_DEBUG_PATH="$LOG_DIR/er-effects-autoload-debug.log"

args=()
if [[ -n "$STEAM_DIR" ]]; then
  args+=(--steam-dir "$STEAM_DIR")
fi
args+=(launch -g "$GAME" -p "$PROFILE_PATH")

echo "ME3 profile: $PROFILE_PATH"
echo "DLL: $DLL_PATH"
echo "Save: $SAVE_FILE"
echo "Logs: $LOG_DIR"
exec "$ME3_PATH" "${args[@]}"
EOF_SH
chmod +x "$STAGE_DIR/run-er-effects-poc.sh"

cat > "$STAGE_DIR/README.txt" <<'EOF_README'
Minimal er-effects-rs + ME3 POC

Windows PowerShell:
  .\run-er-effects-poc.ps1 -SaveFile "C:\path\to\ER0000.sl2" -Me3Path "C:\path\to\me3.exe"

Linux:
  ME3_PATH=/path/to/me3 ME3_STEAM_DIR="$HOME/.local/share/Steam" ./run-er-effects-poc.sh /path/to/ER0000.sl2

The launchers set ER_EFFECTS_SAVE_FILE plus telemetry/log env vars, generate an
absolute-path ME3 profile, then run ME3 with er_effects_rs.dll as a native.
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
