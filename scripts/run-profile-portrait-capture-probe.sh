#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/profile-portrait-capture-$(date +%Y%m%d-%H%M%S)}"
TELEMETRY_PATH="${TELEMETRY_PATH:-$ARTIFACT_DIR/er-effects-telemetry.json}"
RUNTIME_TIMEOUT_CAP_SECONDS="$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 45)"
RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-$RUNTIME_TIMEOUT_CAP_SECONDS}"
GOLD_SLOT="${ER_EFFECTS_GOLD_SLOT:-0}"
DRY_RUN=0

usage() {
  cat <<EOF
Usage: $0 [--dry-run]

Runs one bounded diagnostic probe for the native Load Game / ProfileLoadDialog path.
The DLL is present only as a driver/passive observer: this mode disables product
autoload Continue and title-cover/custom-cover hooks, opens the native title menu via
the existing zero-host-input title gate, fires the native Load-Game menu row, then
captures the exact Elden Ring window when native ProfileSelect/profile-renderer
provenance is visible in telemetry.

Outputs:
  $ARTIFACT_DIR/slot0-profile-portrait.png
  $ARTIFACT_DIR/profile-portrait-provenance.json

This is diagnostic evidence only. It is not final oracle_title_portrait_pixels_visible product proof.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

fatal() { echo "run-profile-portrait-capture-probe: $*" >&2; exit 2; }

[[ "$RUNTIME_TIMEOUT_SECONDS" =~ ^[0-9]+$ ]] || fatal "RUNTIME_TIMEOUT_SECONDS must be an integer"
(( RUNTIME_TIMEOUT_SECONDS > 0 && RUNTIME_TIMEOUT_SECONDS <= RUNTIME_TIMEOUT_CAP_SECONDS )) || fatal "RUNTIME_TIMEOUT_SECONDS must be 1..$RUNTIME_TIMEOUT_CAP_SECONDS"

ARTIFACT_DIR="$(realpath -m "$ARTIFACT_DIR")"
TELEMETRY_PATH="$(realpath -m "$TELEMETRY_PATH")"
mkdir -p "$ARTIFACT_DIR"
AUTOLOAD_REQUEST="$ARTIFACT_DIR/profile-portrait-autoload-request.txt"
cat > "$AUTOLOAD_REQUEST" <<EOF
method=both
slot=$GOLD_SLOT
require_title_bootstrap=false
EOF

if (( DRY_RUN )); then
  cat > "$ARTIFACT_DIR/dry-run-summary.json" <<EOF
{"artifact_dir":"$ARTIFACT_DIR","telemetry":"$TELEMETRY_PATH","autoload_request":"$AUTOLOAD_REQUEST","slot":$GOLD_SLOT,"timeout_seconds":$RUNTIME_TIMEOUT_SECONDS,"mode":"native-profile-capture","launch_script":"scripts/run-product-continue-direct-probe.sh"}
EOF
  echo "dry-run ok: would launch native profile capture probe into $ARTIFACT_DIR"
  exit 0
fi

if [[ "${PROFILE_CAPTURE_SKIP_BUILD:-0}" != "1" ]]; then
  (cd "$REPO_ROOT" && cargo xwin build --release --target x86_64-pc-windows-msvc) \
    > "$ARTIFACT_DIR/profile-capture-build.out" \
    2> "$ARTIFACT_DIR/profile-capture-build.err" \
    || { tail -80 "$ARTIFACT_DIR/profile-capture-build.err" >&2; exit 1; }
fi

# Start the existing approved direct/offline launcher. It owns save staging, exact-process teardown,
# Steam/environment preflight, input blocking, and watcher cleanup. The env below deliberately avoids
# ER_EFFECTS_EXPERIMENTAL_DIRECT_MENU_LOAD/direct_menu_load so PRODUCT_AUTOLOAD_ARMED stays false.
(
  cd "$REPO_ROOT"
  ARTIFACT_DIR="$ARTIFACT_DIR" \
  TELEMETRY_PATH="$TELEMETRY_PATH" \
  RUNTIME_TIMEOUT_SECONDS="$RUNTIME_TIMEOUT_SECONDS" \
  ER_EFFECTS_AUTHORIZED_DIRECT_RUNTIME=1 \
  AUTO_ALLOW_MANUAL_RUNTIME_PROBE=1 \
  ER_EFFECTS_PROFILE_CAPTURE_NATIVE=1 \
  ER_EFFECTS_NATIVE_LOAD=1 \
  ER_EFFECTS_GOLD_SLOT="$GOLD_SLOT" \
  RUNTIME_EXTRA_WATCH_ARGS="${RUNTIME_EXTRA_WATCH_ARGS:---no-phase-watchdog --no-world-load-deadline}" \
  scripts/run-product-continue-direct-probe.sh --autoload-request "$AUTOLOAD_REQUEST"
) > "$ARTIFACT_DIR/profile-capture-launcher.out" 2> "$ARTIFACT_DIR/profile-capture-launcher.err" &
launcher_pid=$!
echo "$launcher_pid" > "$ARTIFACT_DIR/profile-capture-launcher.pid"

set +e
python3 - "$ARTIFACT_DIR" "$TELEMETRY_PATH" "$launcher_pid" "$RUNTIME_TIMEOUT_SECONDS" <<'PY'
from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

artifact_dir = Path(sys.argv[1])
telemetry_path = Path(sys.argv[2])
launcher_pid = int(sys.argv[3])
deadline = time.monotonic() + float(sys.argv[4]) + 5.0
window_class = "steam_app_1245620"
full_png = artifact_dir / "profile-portrait-window.png"
portrait_png = artifact_dir / "slot0-profile-portrait.png"
provenance_path = artifact_dir / "profile-portrait-provenance.json"
failure_path = artifact_dir / "profile-portrait-failure.json"

NULL_SENTINELS = {0, 0x140000000}


def read_json(path: Path) -> dict | None:
    try:
        return json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except Exception:
        return None


def as_int(value, default=0) -> int:
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    if isinstance(value, str):
        try:
            return int(value, 0)
        except Exception:
            return default
    return default


def non_null_ptr(value) -> bool:
    v = as_int(value, 0)
    return v not in NULL_SENTINELS and v > 0x10000


def ready_reason(t: dict | None) -> tuple[bool, list[str], dict]:
    if not t:
        return False, ["telemetry_missing"], {}
    checks = {
        "native_profile_capture_enabled": t.get("oracle_native_profile_capture_enabled") is True,
        "native_load_game_fired": t.get("oracle_native_load_game_fired") is True,
        "product_autoload_not_armed": t.get("product_autoload_armed") is False,
        "no_custom_profile_select_built": t.get("oracle_title_custom_cover_profile_select_any_built") is False,
        "no_custom_cover_run": t.get("oracle_title_custom_cover_run_any") is False,
        "no_title_visual_suppression": as_int(t.get("oracle_title_native_menu_visual_suppressed_builds"), 0) == 0,
        "source_ready": t.get("oracle_native_profile_source_ready") is True,
        "source_slot0": as_int(t.get("oracle_title_custom_cover_profile_source_slot"), -1) == 0,
        "renderer_ptr": non_null_ptr(t.get("oracle_title_custom_cover_profile_source_renderer")),
        "offscreen_ptr": non_null_ptr(t.get("oracle_title_custom_cover_profile_source_offscreen_rend")),
        "tex_rescap_ptr": non_null_ptr(t.get("oracle_title_custom_cover_profile_source_tex_rescap")),
    }
    missing = [k for k, ok in checks.items() if not ok]
    snapshot = {
        "checks": checks,
        "autoload_method": t.get("autoload_method"),
        "product_autoload_armed": t.get("product_autoload_armed"),
        "custom_profile_select_built": t.get("oracle_title_custom_cover_profile_select_any_built"),
        "custom_cover_run": t.get("oracle_title_custom_cover_run_any"),
        "title_visual_suppressed_builds": t.get("oracle_title_native_menu_visual_suppressed_builds"),
        "native_load_game_last_node": t.get("oracle_native_load_game_last_node"),
        "native_load_game_last_node_vtable": t.get("oracle_native_load_game_last_node_vtable"),
        "native_load_game_last_member_dialog": t.get("oracle_native_load_game_last_member_dialog"),
        "native_load_game_last_member_fn": t.get("oracle_native_load_game_last_member_fn"),
        "native_load_game_last_member_adjust": t.get("oracle_native_load_game_last_member_adjust"),
        "renderer": t.get("oracle_title_custom_cover_profile_source_renderer"),
        "renderer_vtable": t.get("oracle_title_custom_cover_profile_source_renderer_vtable"),
        "offscreen_rend": t.get("oracle_title_custom_cover_profile_source_offscreen_rend"),
        "tex_rescap": t.get("oracle_title_custom_cover_profile_source_tex_rescap"),
        "tex_index": t.get("oracle_title_custom_cover_profile_source_tex_index"),
        "ready_754": t.get("oracle_title_custom_cover_profile_source_ready_754"),
        "ready_755": t.get("oracle_title_custom_cover_profile_source_ready_755"),
        "systex_bind_hits": t.get("oracle_title_scaleform_bind_observer_systex_hits"),
        "bind_observer_last_owner": t.get("oracle_title_scaleform_bind_observer_last_owner"),
        "bind_observer_last_pair": t.get("oracle_title_scaleform_bind_observer_last_pair"),
        "bind_observer_last_symbol_ptr": t.get("oracle_title_scaleform_bind_observer_last_symbol_ptr"),
        "bind_observer_last_target_ptr": t.get("oracle_title_scaleform_bind_observer_last_target_ptr"),
        "source_name": t.get("oracle_native_profile_source_name"),
        "renderer_class": t.get("oracle_native_profile_renderer_class"),
    }
    return not missing, missing, snapshot


def hypr_clients(hyprctl: str) -> list[dict]:
    try:
        out = subprocess.run([hyprctl, "clients", "-j"], text=True, capture_output=True, timeout=10).stdout
        data = json.loads(out)
        return [c for c in data if isinstance(c, dict)]
    except Exception:
        return []


def find_er(hyprctl: str) -> dict | None:
    for c in hypr_clients(hyprctl):
        if str(c.get("class") or "") == window_class:
            return c
    return None


def window_summary(w: dict) -> dict:
    ws = w.get("workspace")
    return {
        "class": w.get("class"),
        "at": w.get("at"),
        "size": w.get("size"),
        "mapped": w.get("mapped"),
        "hidden": w.get("hidden"),
        "focusHistoryID": w.get("focusHistoryID"),
        "fullscreen": w.get("fullscreen"),
        "workspace": ws.get("id") if isinstance(ws, dict) else ws,
    }


def capture_window() -> dict:
    hyprctl = shutil.which("hyprctl")
    grim = shutil.which("grim")
    if not hyprctl or not grim:
        raise RuntimeError(f"missing capture tools hyprctl={hyprctl} grim={grim}")
    w = find_er(hyprctl)
    if not w:
        raise RuntimeError(f"no exact target window class={window_class}")
    addr = w.get("address")
    ws = w.get("workspace")
    ws_id = ws.get("id") if isinstance(ws, dict) else ws
    for _ in range(24):
        try:
            if ws_id is not None:
                subprocess.run([hyprctl, "dispatch", "workspace", str(ws_id)], capture_output=True, timeout=10)
            if addr:
                subprocess.run([hyprctl, "dispatch", "focuswindow", f"address:{addr}"], capture_output=True, timeout=10)
                subprocess.run([hyprctl, "dispatch", "alterzorder", f"top,address:{addr}"], capture_output=True, timeout=10)
        except Exception:
            pass
        refreshed = find_er(hyprctl)
        if refreshed:
            w = refreshed
            if as_int(w.get("focusHistoryID"), -1) == 0:
                break
        os.sched_yield()
    at = w.get("at") or []
    size = w.get("size") or []
    if w.get("mapped") is False or w.get("hidden") is True or len(at) != 2 or len(size) != 2:
        raise RuntimeError(f"target window unsafe: {window_summary(w)}")
    geom = f"{int(at[0])},{int(at[1])} {int(size[0])}x{int(size[1])}"
    rc = subprocess.run([grim, "-g", geom, str(full_png)], text=True, capture_output=True, timeout=15)
    if rc.returncode != 0 or not full_png.exists():
        raise RuntimeError(f"grim failed rc={rc.returncode} stderr={rc.stderr.strip()}")
    crop_portrait(full_png, portrait_png)
    return {"window": window_summary(w), "geometry": geom, "full_window_png": str(full_png), "portrait_png": str(portrait_png)}


def crop_portrait(src: Path, dst: Path) -> None:
    magick = shutil.which("magick") or shutil.which("convert")
    if magick:
        r = subprocess.run([magick, str(src), "-gravity", "center", "-crop", "60%x75%+0+0", "+repage", str(dst)], capture_output=True, timeout=20)
        if r.returncode == 0 and dst.exists():
            return
    try:
        from PIL import Image  # type: ignore
        im = Image.open(src)
        w, h = im.size
        cw, ch = int(w * 0.60), int(h * 0.75)
        left, top = (w - cw) // 2, (h - ch) // 2
        im.crop((left, top, left + cw, top + ch)).save(dst)
        return
    except Exception as exc:
        raise RuntimeError(f"unable to create PNG crop (need ImageMagick or Pillow): {exc}")

last_missing: list[str] = []
last_snapshot: dict = {}
ready_since: float | None = None
ready_dwell_seconds = float(os.environ.get("PROFILE_CAPTURE_READY_DWELL_SECONDS", "2.5"))
while time.monotonic() < deadline:
    telemetry = read_json(telemetry_path)
    ready, missing, snapshot = ready_reason(telemetry)
    last_missing = missing
    last_snapshot = snapshot
    if ready:
        if ready_since is None:
            ready_since = time.monotonic()
            os.sched_yield()
            continue
        if time.monotonic() - ready_since < ready_dwell_seconds:
            os.sched_yield()
            continue
        try:
            capture = capture_window()
            provenance_path.write_text(json.dumps({
                "status": "captured",
                "reason": "native_LoadGame_row_fired_and_native_profile_renderer_source_ready_after_dwell",
                "telemetry_path": str(telemetry_path),
                "provenance": snapshot,
                "capture": capture,
                "ready_dwell_seconds": ready_dwell_seconds,
                "note": "diagnostic only: driver/passive-observer DLL present; product title-cover/custom-cover hooks were not armed",
            }, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            print(f"profile portrait capture: {portrait_png}")
            try:
                os.kill(launcher_pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            raise SystemExit(0)
        except Exception as exc:
            failure_path.write_text(json.dumps({
                "status": "capture_failed_after_ready",
                "error": str(exc),
                "telemetry_path": str(telemetry_path),
                "missing": missing,
                "provenance": snapshot,
            }, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            try:
                os.kill(launcher_pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            raise SystemExit(1)
    else:
        ready_since = None
    try:
        os.kill(launcher_pid, 0)
    except ProcessLookupError:
        break
    except PermissionError:
        pass
    os.sched_yield()

failure_path.write_text(json.dumps({
    "status": "not_captured",
    "reason": "native_profile_renderer_ready_condition_not_reached_before_probe_exit",
    "telemetry_path": str(telemetry_path),
    "missing": last_missing,
    "last_provenance": last_snapshot,
}, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print(f"profile portrait capture: not captured; see {failure_path}", file=sys.stderr)
try:
    os.kill(launcher_pid, signal.SIGTERM)
except ProcessLookupError:
    pass
raise SystemExit(1)
PY
capture_rc=$?
wait "$launcher_pid"
launcher_rc=$?
set -e

if (( capture_rc != 0 )); then
  echo "run-profile-portrait-capture-probe: capture failed; launcher_rc=$launcher_rc artifact_dir=$ARTIFACT_DIR" >&2
  exit "$capture_rc"
fi

# A successful capture intentionally SIGTERMs the owner launcher so its cleanup trap tears down ER.
# Accept normal success, SIGTERM-shaped exits, or shells reporting 128+TERM.
if (( launcher_rc != 0 && launcher_rc != 143 && launcher_rc != 130 )); then
  echo "run-profile-portrait-capture-probe: capture succeeded but launcher exited $launcher_rc; artifact_dir=$ARTIFACT_DIR" >&2
  exit "$launcher_rc"
fi

echo "profile portrait capture artifact: $ARTIFACT_DIR/slot0-profile-portrait.png"
echo "profile portrait provenance: $ARTIFACT_DIR/profile-portrait-provenance.json"
