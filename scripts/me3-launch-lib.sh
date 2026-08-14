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

# shellcheck source=scripts/user-runtime-gate.sh
# shellcheck disable=SC1091
source "${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}/scripts/user-runtime-gate.sh"

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
  local launch_scope="${1:-full-product}"
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
  me3_launch_gate "$launch_scope" || return 2
}

# Refuse a launch whose changed code path has never been shown to execute.
#
# A launch takes the user's screen and produces one recording. Spending it on a predicate that
# cannot fire is worse than not launching: it returns a clean-looking run that proves nothing, and
# the absence of the expected log line reads as "the fix did not work" rather than "the fix could
# never have run". That happened on 2026-08-04 -- a disarm gated on `requestCode` latching 2 shipped
# and did nothing, because the state it was scoped to is DEFINED by `requestCode` staying 1, and the
# previous run's telemetry already said so.
#
# `scripts/er-launch-gate.py` checks that offline, against recordings that already exist, in
# milliseconds. Fail-closed: a gate that cannot read its evidence refuses.
#
# ER_LAUNCH_GATE_SKIP=1 bypasses it. That is for a launch which is NOT validating a code path (the
# user just wants to play, or a launch whose only purpose is to PRODUCE a first recording). It is
# not a way to launch a fix you have not shown can run -- the skip is logged so a run that used it
# can never later be cited as proof.
me3_launch_gate() {
  local launch_scope="${1:-full-product}"
  if [[ "${ER_LAUNCH_GATE_SKIP:-0}" == "1" ]]; then
    echo "me3-launch-lib: launch gate SKIPPED by ER_LAUNCH_GATE_SKIP=1 -- this run is not validation evidence" >&2
    return 0
  fi
  local repo_root
  repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
  local gate="$repo_root/scripts/er-launch-gate.py"
  if [[ ! -f "$gate" ]]; then
    echo "me3-launch-lib: launch gate missing at $gate -- refusing to launch" >&2
    return 2
  fi
  python3 "$gate" --scope "$launch_scope" || return 2
}

# me3_write_profile PROFILE_PATH DLL_PATH [EXTRA_NATIVE_PATH]
# Writes a v1 ModProfile loading DLL_PATH as a native. DLL_PATH may be absolute
# (per-run artifact copies) or relative (resolved against the profile's directory --
# used by the relocatable release payload). EXTRA_NATIVE_PATH (optional) is emitted as
# an ADDITIONAL native BEFORE the DLL -- used by seamless-mode probes to load the
# user's installed SeamlessCoop/ersc.dll IN PLACE by absolute path. The referenced
# file is never copied, moved, or staged (Do-not-bundle-ersc rule): only this per-run
# profile TOML mentions it.
me3_write_profile() {
  local profile_path="$1" dll_path="$2" extra_native="${3:-}"
  cat > "$profile_path" <<EOF
profileVersion = "v1"

[[supports]]
game = "eldenring"
EOF
  if [[ -n "$extra_native" ]]; then
    cat >> "$profile_path" <<EOF

[[natives]]
path = '$extra_native'
EOF
  fi
  cat >> "$profile_path" <<EOF

[[natives]]
path = '$dll_path'
EOF
}

# me3_write_telemetry_only_profile PROFILE_PATH TELEMETRY_DLL_PATH
# Writes a v1 ModProfile that loads ONLY the standalone er_telemetry_dll.dll (the
# read-side telemetry-only DLL from crates/er-telemetry-dll). Runs alone -- no
# product DLL, no hooks -- so it emits er-telemetry-standalone.json from RAM/PE
# reads only. Build the DLL with:
#   cargo xwin build --release --target x86_64-pc-windows-msvc -p er-telemetry-dll
# -> target/x86_64-pc-windows-msvc/release/er_telemetry_dll.dll
me3_write_telemetry_only_profile() {
  local profile_path="$1" telemetry_dll="$2"
  cat > "$profile_path" <<EOF
profileVersion = "v1"

[[supports]]
game = "eldenring"

[[natives]]
path = '$telemetry_dll'
EOF
}

# me3_append_native PROFILE_PATH DLL_PATH
# Appends one additional [[natives]] block to an existing profile. Used to add the
# standalone er_telemetry_dll.dll as a 4th native alongside the product +
# reload-trace + input-harness DLLs (companion DLLs are enabled purely by presence
# in the profile; product must be listed FIRST so its er_effects_union_register
# export is mapped before companions resolve it).
me3_append_native() {
  local profile_path="$1" dll_path="$2"
  cat >> "$profile_path" <<EOF

[[natives]]
path = '$dll_path'
EOF
}

# me3_launch PROFILE_PATH -- runs me3 launch in the foreground of the caller's shell.
# The caller decides env, redirection, and backgrounding; the me3 CLI stays alive as
# the launch owner for the lifetime of the game.
me3_launch() {
  local profile_path="$1"
  require_user_runtime_go || return $?
  # Fail loudly on the CWD payload-hijack trap (bd me3-launch-cwd-must-lack-rust-target-dir):
  # me3 resolves me3-launcher.exe/me3_mod_host.dll from a CWD-relative
  # target/x86_64-pc-windows-msvc/release dir when one exists. Launching from a rust
  # checkout makes Proton exec a nonexistent (or stale) launcher and the run dies
  # silently inside the compat tool. Callers must cd to GAME_DIR (or any non-checkout
  # dir) first.
  if [[ -d "$PWD/target/x86_64-pc-windows-msvc/release" ]]; then
    echo "me3-launch-lib: refusing to launch from $PWD -- CWD contains target/x86_64-pc-windows-msvc/release, which hijacks me3's launcher payload resolution; cd to GAME_DIR first" >&2
    return 2
  fi
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
