# shellcheck shell=bash
# Shared me3 launch helpers. me3 is the ONLY supported loader for er_effects_rs.dll:
# the LazyLoader dinput8 proxy + lazyLoad.ini chainload delivery was removed 2026-07-04
# (branch feat/me3-launch-smoketest) after the me3 production smoke passed end-to-end
# (run me3-product-smoke-20260704-110507: DLL attach, env propagation, flag files,
# zero-input autoload to world-stable all proven through me3).
#
# Source this file; do not execute it. Callers own artifact dirs, env, backgrounding,
# and teardown. me3 launches Game/eldenring.exe directly through the Steam compat tool
# (waitforexitandrun verb) -- never a Steam AppID/URL form, never the EAC launcher.

ME3_BIN="${ME3_BIN:-$HOME/.local/bin/me3}"
# me3 auto-detects the flatpak Steam on this machine but Elden Ring lives in the host
# library -- always pin the host Steam dir.
ME3_STEAM_DIR="${ME3_STEAM_DIR:-$HOME/.local/share/Steam}"
ME3_WINDOWS_BIN_DIR="${ME3_WINDOWS_BIN_DIR:-$HOME/.local/share/me3/windows-bin}"
ME3_LOG_DIR="${ME3_LOG_DIR:-$HOME/.local/share/me3/logs}"

# Validate the me3 installation and that me3 can resolve a Proton compat tool for
# Elden Ring. me3 resolves strictly: per-app CompatToolMapping (config.vdf) -> global
# "0" mapping -> its hardcoded per-game default (proton_8 for Elden Ring, me3 0.11.0
# crates/mod-protocol/src/game.rs); there is NO me3-side override. Returns non-zero
# with guidance on stderr instead of burning a launch.
me3_preflight() {
  [[ -x "$ME3_BIN" ]] || { echo "me3-launch-lib: missing me3 binary: $ME3_BIN" >&2; return 2; }
  [[ -f "$ME3_WINDOWS_BIN_DIR/me3-launcher.exe" ]] || { echo "me3-launch-lib: missing $ME3_WINDOWS_BIN_DIR/me3-launcher.exe" >&2; return 2; }
  [[ -f "$ME3_WINDOWS_BIN_DIR/me3_mod_host.dll" ]] || { echo "me3-launch-lib: missing $ME3_WINDOWS_BIN_DIR/me3_mod_host.dll" >&2; return 2; }
  python3 - "$ME3_STEAM_DIR" <<'PY'
import os
import re
import sys

steam = sys.argv[1]
cfg_path = os.path.join(steam, "config/config.vdf")
try:
    cfg = open(cfg_path, encoding="utf-8", errors="replace").read()
except OSError:
    print(f"me3-launch-lib: cannot read {cfg_path}", file=sys.stderr)
    sys.exit(1)
i = cfg.find('"CompatToolMapping"')
seg = cfg[i:i + 20000] if i >= 0 else ""
tools = dict(re.findall(r'"(\d+)"\s*\{[^{}]*?"name"\s*"([^"]*)"', seg))
name = tools.get("1245620") or tools.get("0")
if name:
    print(f"me3-launch-lib: compat tool resolves -> {name}")
    sys.exit(0)
if os.path.exists(os.path.join(steam, "steamapps/appmanifest_2348590.acf")):
    print("me3-launch-lib: no Steam mapping; me3's hardcoded proton_8 fallback is installed")
    sys.exit(0)
print(
    "me3-launch-lib: me3 cannot resolve a Proton compat tool for Elden Ring "
    "(no CompatToolMapping for 1245620 or global default; Proton 8 fallback not installed). "
    "Fix once in the running Steam client: ELDEN RING -> Properties -> Compatibility -> "
    "force 'Proton Experimental'.",
    file=sys.stderr,
)
sys.exit(1)
PY
}

# me3_write_profile PROFILE_PATH DLL_PATH
# Writes a v1 ModProfile loading DLL_PATH as the sole native. DLL_PATH may be absolute
# (per-run artifact copies) or relative (resolved against the profile's directory --
# used by the relocatable release payload).
me3_write_profile() {
  local profile_path="$1" dll_path="$2"
  cat > "$profile_path" <<EOF
profileVersion = "v1"

[[supports]]
game = "eldenring"

[[natives]]
path = '$dll_path'
EOF
}

# me3_launch PROFILE_PATH -- runs me3 launch in the foreground of the caller's shell.
# The caller decides env, redirection, and backgrounding; the me3 CLI stays alive as
# the launch owner for the lifetime of the game.
me3_launch() {
  local profile_path="$1"
  "$ME3_BIN" --steam-dir "$ME3_STEAM_DIR" launch -g eldenring -p "$profile_path"
}

# Fail closed if a leftover LazyLoader proxy is still active in GAME_DIR: an me3 native
# plus a dinput8 chainload would DOUBLE-LOAD the DLL (two modules, two DllMains).
me3_require_no_lazyloader() {
  local game_dir="$1"
  if [[ -f "$game_dir/dinput8.dll" ]]; then
    echo "me3-launch-lib: $game_dir/dinput8.dll is present (LazyLoader proxy) -- refusing the double-load run; LazyLoader was removed 2026-07-04, delete or stage away the proxy" >&2
    return 2
  fi
  return 0
}
