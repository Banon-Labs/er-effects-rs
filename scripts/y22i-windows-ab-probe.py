#!/usr/bin/env python3
"""y22i native-Windows A/B probe runner (WSL-side driver).

Runs ONE bounded me3 offline launch of Elden Ring with a given er_effects_rs.dll
(guard or control arm) on the native-Windows dual-boot machine, and scores it
from RAM/in-process telemetry semaphores only:

  - control arm success  = crash log line `access-violation rva=0xec95d1 ... fault_addr=0x20`
  - guard   arm success  = telemetry `oracle_scaleform_desc_guard_installed: true`
                           AND `oracle_scaleform_desc_provider_null_hits > 0`
                           AND no 0xec95d1 access violation

The runtime portion is hard-capped by the canonical cap in
`.auto/runtime_timeout_cap_seconds` (read through scripts/runtime_timeout_cap.py);
the game is force-killed at the cap. Stop/continue decisions come only from the
crash log, telemetry JSON, and process exit -- never from screenshots.

Usage:
  python3 scripts/y22i-windows-ab-probe.py --arm control --run 1 \
      --dll 'C:\\Users\\choza\\build\\y22i\\control\\er-effects-rs\\target\\x86_64-pc-windows-msvc\\release\\er_effects_rs.dll'

Emits a single JSON verdict line prefixed with `VERDICT: ` and copies all
evidence files into .artifacts/y22i-win/<arm>-run<run>/.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GAME_DIR = "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game"
ME3 = "/mnt/c/Users/choza/AppData/Local/garyttierney/me3/bin/me3.exe"
# me3 is launched with this WSL cwd so any CWD-relative DLL logs land on a real
# Windows path (not a \\wsl.localhost UNC the game cannot write to).
LAUNCH_CWD = "/mnt/c/Users/choza/build/y22i"

CRASH_RVA = "0xec95d1"
# Verdict oracle set: y22i guard fields + the display-path fields (Present hook / swapchain find /
# boot bar / portrait composite) added for the 2026-07-15 native-Windows visual-regression work.
ORACLE_KEYS = (
    "oracle_scaleform_desc_guard_installed",
    "oracle_scaleform_desc_provider_null_hits",
    "player_available", "player_seen", "runtime_mode", "oracle_char_name",
    "oracle_present_hook_hits", "oracle_present_find_tries", "oracle_present_find_stage",
    "oracle_present_find_vt_module_kind", "oracle_present_find_streak",
    "oracle_present_accept_path", "oracle_present_find_candidate",
    "oracle_present_find_candidate_vt", "oracle_present_find_got8", "oracle_present_find_got22",
    "oracle_boot_view_draw_hits", "oracle_boot_view_self_presents",
    "oracle_boot_view_pump_stop_reason", "oracle_boot_view_swapchain_found_ms",
    "oracle_overlay_draw_hits", "oracle_overlay_reuploads",
    "oracle_portrait_pump_draws", "oracle_portrait_pump_block_off_resource",
)
EVIDENCE_FILES = [
    "er-effects-crash-log.txt",
    "er-effects-telemetry.json",
    "er-effects-autoload-debug.log",
    "er-effects-bootstrap.json",
    "er-effects-bootstrap-state.json",
    "er-effects-fail-fast.txt",
    "er-effects-assert-nonfatal.txt",
]
# Dirs the game/DLL may use as CWD for relative log paths, checked in order.
EVIDENCE_DIRS = [GAME_DIR, LAUNCH_CWD]


def runtime_cap_seconds() -> int:
    out = subprocess.run(
        [sys.executable, os.path.join(REPO, "scripts", "runtime_timeout_cap.py")],
        capture_output=True, text=True, timeout=30, check=True,
    )
    return int(out.stdout.strip())


def tasklist(image: str) -> list[int]:
    """PIDs of a Windows image name, [] if none."""
    out = subprocess.run(
        ["/mnt/c/Windows/System32/tasklist.exe", "/FI", f"IMAGENAME eq {image}",
         "/FO", "CSV", "/NH"],
        capture_output=True, text=True, timeout=30,
    )
    pids = []
    for line in out.stdout.splitlines():
        parts = [p.strip('"') for p in line.split('","')]
        if len(parts) >= 2 and parts[0].lower() == image.lower():
            try:
                pids.append(int(parts[1]))
            except ValueError:
                pass
    return pids


def taskkill(image: str) -> None:
    subprocess.run(["/mnt/c/Windows/System32/taskkill.exe", "/IM", image, "/F"],
                   capture_output=True, text=True, timeout=30)


def read_text(path: str) -> str:
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            return fh.read()
    except OSError:
        return ""


def find_evidence(name: str) -> str:
    for d in EVIDENCE_DIRS:
        p = os.path.join(d, name)
        if os.path.exists(p):
            return p
    return ""


def parse_telemetry() -> dict:
    path = find_evidence("er-effects-telemetry.json")
    if not path:
        return {}
    raw = read_text(path)
    try:
        return json.loads(raw)
    except ValueError:
        # mid-write tolerance: regex-scavenge the fields we score on
        got = {}
        for key in ("oracle_scaleform_desc_guard_installed",
                    "oracle_scaleform_desc_provider_null_hits",
                    "player_available", "player_seen"):
            m = re.search(rf'"{key}"\s*:\s*([^,\n}}]+)', raw)
            if m:
                val = m.group(1).strip().strip('"')
                got[key] = {"true": True, "false": False}.get(val, val)
        return got


def evidence_sizes() -> tuple[int, ...]:
    sizes = []
    for name in ("er-effects-crash-log.txt", "er-effects-telemetry.json"):
        path = find_evidence(name)
        try:
            sizes.append(os.path.getsize(path) if path else -1)
        except OSError:
            sizes.append(-1)
    return tuple(sizes)


def wait_for_evidence_flush() -> None:
    """Deterministic flush wait: evidence file sizes stable across two consecutive
    samples, each paced by a real tasklist.exe spawn (~0.3-1s); hard-capped at 8."""
    prev = evidence_sizes()
    for _ in range(8):
        tasklist("eldenring.exe")
        cur = evidence_sizes()
        if cur == prev:
            return
        prev = cur


def crash_lines() -> list[str]:
    path = find_evidence("er-effects-crash-log.txt")
    if not path:
        return []
    return [l for l in read_text(path).splitlines() if "access-violation" in l]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--arm", required=True, choices=["guard", "control", "fix"])
    ap.add_argument("--run", required=True, type=int)
    ap.add_argument("--dll", required=True, help="Windows path to er_effects_rs.dll")
    args = ap.parse_args()

    cap = runtime_cap_seconds()
    artifact_dir = os.path.join(REPO, ".artifacts", "y22i-win", f"{args.arm}-run{args.run}")
    os.makedirs(artifact_dir, exist_ok=True)

    # --- preflight (fail closed) ---
    if not tasklist("steam.exe"):
        print("PREFLIGHT-FAIL: steam.exe is not running; start Steam first")
        return 2
    for image in ("eldenring.exe", "start_protected_game.exe"):
        if tasklist(image):
            print(f"PREFLIGHT-FAIL: {image} already running; refusing to launch a second instance")
            return 2
    if not os.path.exists(ME3):
        print("PREFLIGHT-FAIL: me3.exe not found")
        return 2
    dll_wsl = subprocess.run(["wslpath", "-u", args.dll], capture_output=True,
                             text=True, check=True, timeout=15).stdout.strip()
    if not os.path.exists(dll_wsl):
        print(f"PREFLIGHT-FAIL: DLL not found: {args.dll}")
        return 2
    for name in EVIDENCE_FILES:
        for d in EVIDENCE_DIRS:
            p = os.path.join(d, name)
            if os.path.exists(p):
                os.remove(p)

    print(f"[{args.arm} run {args.run}] launching me3 (cap {cap}s runtime) dll={args.dll}", flush=True)
    me3_log = open(os.path.join(artifact_dir, "me3.log"), "w", encoding="utf-8")
    me3_proc = subprocess.Popen(
        [ME3, "launch", "-g", "eldenring", "-n", args.dll],
        cwd=LAUNCH_CWD, stdout=me3_log, stderr=subprocess.STDOUT,
    )

    verdict = {
        "arm": args.arm, "run": args.run, "cap_seconds": cap,
        "game_seen": False, "runtime_seconds": None, "process_exited": False,
        "killed_at_cap": False, "crash_rva_hit": False,
        "other_access_violation": False, "crash_lines": [],
        "oracles": {}, "stop_reason": None,
    }

    # --- wait for eldenring.exe to appear (launcher grace, not runtime) ---
    # Poll cadence comes from the tasklist.exe readiness check itself (~0.3-1s per
    # WSL-interop spawn); no artificial sleep pacing anywhere in this script.
    launch_deadline = time.monotonic() + 180
    while time.monotonic() < launch_deadline:
        if tasklist("eldenring.exe"):
            verdict["game_seen"] = True
            break
        if me3_proc.poll() is not None:
            break
    if not verdict["game_seen"]:
        verdict["stop_reason"] = "game-never-appeared"
        print(f"VERDICT: {json.dumps(verdict)}", flush=True)
        me3_log.close()
        return 1

    # --- runtime watch loop, hard-capped ---
    t0 = time.monotonic()
    stop = None
    while True:
        elapsed = time.monotonic() - t0
        if elapsed >= cap:
            stop = "cap"
            break
        lines = crash_lines()
        if lines:
            verdict["crash_lines"] = lines[:5]
            if any(f"rva={CRASH_RVA}" in l for l in lines):
                verdict["crash_rva_hit"] = True
                stop = "crash-rva-hit"
                break
            verdict["other_access_violation"] = True
            stop = "other-access-violation"
            break
        tele = parse_telemetry()
        if tele:
            verdict["oracles"] = {
                k: tele.get(k) for k in ORACLE_KEYS if k in tele
            }
            o = verdict["oracles"]
            null_hits = o.get("oracle_scaleform_desc_provider_null_hits")
            if (args.arm == "guard"
                    and o.get("oracle_scaleform_desc_guard_installed") in (True, "true", 1)
                    and isinstance(null_hits, int) and null_hits > 0
                    and o.get("player_available") in (True, "true")):
                stop = "guard-proven-world-reached"
                break
        if not tasklist("eldenring.exe"):
            verdict["process_exited"] = True
            stop = "process-exited"
            break
    verdict["runtime_seconds"] = round(time.monotonic() - t0, 1)
    verdict["stop_reason"] = stop

    # --- teardown: always leave no game running ---
    if stop in ("cap", "crash-rva-hit", "other-access-violation", "guard-proven-world-reached"):
        if stop != "cap":
            wait_for_evidence_flush()
        if tasklist("eldenring.exe"):
            verdict["killed_at_cap"] = stop == "cap"
            taskkill("eldenring.exe")
    me3_proc.poll()
    if me3_proc.returncode is None:
        me3_proc.terminate()
    me3_log.close()
    wait_for_evidence_flush()

    # --- final evidence snapshot (post-death flushes included) ---
    lines = crash_lines()
    if lines:
        verdict["crash_lines"] = lines[:5]
        verdict["crash_rva_hit"] = any(f"rva={CRASH_RVA}" in l for l in lines)
        verdict["other_access_violation"] = bool(lines) and not verdict["crash_rva_hit"]
    tele = parse_telemetry()
    for k in ORACLE_KEYS:
        if k in tele:
            verdict["oracles"][k] = tele[k]
    for name in EVIDENCE_FILES:
        src = find_evidence(name)
        if src:
            shutil.copyfile(src, os.path.join(artifact_dir, name))

    print(f"VERDICT: {json.dumps(verdict)}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
