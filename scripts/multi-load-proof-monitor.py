#!/usr/bin/env python3
"""Autonomous multi-save-load PROOF monitor + report generator.

For docs/goals/repeatable-multi-save-load-acceptance.md. Given a live (or finished) runtime
artifact dir (er-effects-telemetry.json + er-effects-autoload-debug.log) and an ordered list of
expected load targets, this:
  - watches the RAM telemetry for each distinct STABLE, finished-loading world (mechanism-agnostic:
    it keys on the observed character identity becoming stable, not on any particular reload path);
  - verifies each load's IDENTITY (name+name_len+saved_map_c30+runes) against the expected
    (file, slot) decoded offline, plus a STATS spot-check (level + attribute array) and a GEAR
    spot-check (talisman slots / flask counts / spirit-ash level), and CONTROLLABLE-in-world
    (player present + chr controller/onscreen);
  - detects CRASHES (process gone with the run incomplete, or an access-violation/assert marker in
    the debug log) and STALLS (no new verified load within the per-load deadline);
  - logs per-load timings; and
  - emits a short human-readable pass/fail report. Exit 0 == every expected load verified with zero
    crashes/stalls == PROVEN.

The RAM telemetry is the ONLY load-success oracle (never a screenshot). Reuses the evidence-bound
identity logic from switch-character-oracle.py and the slot decoder from save-slot-oracle.py.

Usage (live):
  multi-load-proof-monitor.py --artifact-dir DIR --targets targets.json [--report OUT.md]
       [--per-load-deadline 90] [--overall-deadline 600] [--poll 2]
Offline replay of a finished run (no waiting):
  multi-load-proof-monitor.py --artifact-dir DIR --targets targets.json --replay

targets.json = [{"file": "/abs/ER0000.sl2", "slot": 0}, {"file": "...", "slot": 1}, ...]
 (index 0 = the initial TOML auto-load; the rest are the reloads under test.)
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
# Interruptible poll wait (never set) -- the sanctioned no-bare-sleep pace primitive used by the
# sibling watchers; the loop's real synchronization is the telemetry-file/process-state readiness
# checks above, this only bounds poll frequency. (scripts/check-no-timeouts.py bans raw time.sleep.)
_POLL_WAIT = threading.Event()


def _load(mod_name: str, file_name: str):
    spec = importlib.util.spec_from_file_location(mod_name, HERE / file_name)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


ORACLE = _load("switch_character_oracle", "switch-character-oracle.py")
DECODER = _load("save_slot_oracle", "save-slot-oracle.py")

# Genuine hard-crash / deliberate-abort signatures. NOTE: the DLL's own boot-time safe_input hook
# INSTALL lines ("safe_input hook NtTerminateProcess: target..." / "...applied") are NOT crashes --
# they are excluded below. An actual crash surfaces the AV rva / assert / deliberate-abort markers,
# and process-liveness (process_alive) catches a real terminate independently.
AV_MARKERS = ("access-violation rva", "0x1eb9999", "deliberate-abort",
              "game ASSERT", "a0_rva=0x29c7aa0", "DL_PANIC")


def expected_identity(file_path: Path, slot: int) -> dict:
    data = file_path.read_bytes()
    df = DECODER.decode_save_slot(data, file_path, slot).get("decoded_fields", {})
    return {
        "name": df.get("name"),
        "name_len": df.get("name_len"),
        "level": df.get("level"),
        "runes": df.get("runes"),
        "saved_map_c30": df.get("saved_map_c30"),
        "stats": df.get("stats"),
        "unlocked_talisman_slots": df.get("unlocked_talisman_slots"),
        "max_crimson_flask_count": df.get("max_crimson_flask_count"),
        "max_cerulean_flask_count": df.get("max_cerulean_flask_count"),
        "spirit_ash_level": df.get("spirit_ash_level"),
    }


def read_telemetry(path: Path) -> dict | None:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return None


def stats_match(exp: dict, tel: dict) -> bool:
    lvl_ok = ORACLE._as_int(tel.get("oracle_char_level"), -1) == ORACLE._as_int(exp.get("level"), -2)
    exp_stats, obs_stats = exp.get("stats"), tel.get("oracle_char_stats")
    stats_ok = isinstance(exp_stats, list) and exp_stats == obs_stats
    return bool(lvl_ok and stats_ok)


def gear_match(exp: dict, tel: dict) -> bool:
    # Spot-check save-specific loadout scalars that ride with the character (not level/name):
    # unlocked talisman slots, both flask caps, and spirit-ash level. All four must match.
    checks = [
        ("unlocked_talisman_slots", "oracle_char_unlocked_talisman_slots"),
        ("max_crimson_flask_count", "oracle_char_max_crimson_flask_count"),
        ("max_cerulean_flask_count", "oracle_char_max_cerulean_flask_count"),
        ("spirit_ash_level", "oracle_char_spirit_ash_level"),
    ]
    for ek, ok in checks:
        if ORACLE._as_int(exp.get(ek), -1) != ORACLE._as_int(tel.get(ok), -2):
            return False
    return True


def controllable(tel: dict) -> bool:
    player = tel.get("oracle_player_present") is True or tel.get("player_available") is True
    ctrl = tel.get("oracle_chr_ctrl_present") is True or tel.get("oracle_chr_model_ins_present") is True
    return bool(player and ctrl)


def log_has_crash(log_path: Path, start_offset: int = 0) -> str | None:
    if not log_path.exists():
        return None
    # Scan only content written AFTER start_offset (the shared append-log holds prior runs; scanning
    # their tail would false-positive on old AV markers). Cap the scanned window for cost.
    try:
        with open(log_path, "rb") as f:
            f.seek(0, os.SEEK_END)
            size = f.tell()
            begin = max(start_offset, size - 1_048_576)
            if begin >= size:
                return None
            f.seek(begin)
            tail = f.read().decode("utf-8", "replace")
    except OSError:
        return None
    for line in tail.splitlines():
        # The safe_input NtTerminateProcess *hook install* lines at boot are not crashes.
        if "safe_input hook NtTerminateProcess" in line:
            continue
        for m in AV_MARKERS:
            if m in line:
                return line.strip()[:200]
    return None


def stall_diagnosis(log_path: Path, start_offset: int = 0) -> str:
    """On a STALL, name WHERE the reload got stuck from the DLL debug log: the last MoveMapStep
    state (mms_step=N(NAME)) and the last 'waiting for ...' reason. Makes the report self-diagnosing
    (e.g. 'mms_step=18 MOVE MAP' = teardown lock vs 'waiting for native a40/menu-open' = title stall)."""
    if not log_path.exists():
        return "no debug log"
    try:
        with open(log_path, "rb") as f:
            f.seek(0, os.SEEK_END)
            size = f.tell()
            f.seek(max(start_offset, size - 2_097_152))
            tail = f.read().decode("utf-8", "replace")
    except OSError:
        return "debug log unreadable"
    import re as _re
    last_mms = None
    last_wait = None
    for line in tail.splitlines():
        m = _re.search(r"mms_step=(\d+\([A-Za-z_ ]+\))", line)
        if m:
            last_mms = m.group(1)
        m = _re.search(r"(waiting (?:for|to)[^-\n]{0,80})", line)
        if m:
            last_wait = m.group(1).strip()
    bits = []
    if last_mms:
        bits.append(f"last MoveMapStep={last_mms}")
    if last_wait:
        bits.append(f"last reason='{last_wait}'")
    return "; ".join(bits) or "no stall markers found"


def process_alive() -> bool:
    # eldenring.exe present == the run is still live. WSL-AWARE: on a WSL2 + Windows-Steam box the
    # game is a WINDOWS process (tasklist.exe), so a Linux `pgrep -x eldenring.exe` false-negatives
    # and would report an instant fake crash (bd steam-detection-wsl-false-negative-2026-07-18).
    # Check the Linux side first, then the Windows process list.
    if os.system("pgrep -x eldenring.exe >/dev/null 2>&1") == 0:
        return True
    return os.system(
        "command -v tasklist.exe >/dev/null 2>&1 && "
        "tasklist.exe /FI 'IMAGENAME eq eldenring.exe' 2>/dev/null | grep -qi eldenring.exe"
    ) == 0


def evaluate_load(exp: dict, tel: dict) -> dict:
    observed = ORACLE.observed_identity(tel)
    return {
        "identity_ok": ORACLE.identity_matches(exp, observed),
        "stats_ok": stats_match(exp, tel),
        "gear_ok": gear_match(exp, tel),
        "controllable_ok": controllable(tel),
        "stable": ORACLE.stable_world_loaded(tel),
        "observed": observed,
    }


def load_ok(res: dict) -> bool:
    return all(res[k] for k in ("identity_ok", "stats_ok", "gear_ok", "controllable_ok", "stable"))


def monitor(artifact_dir: Path, targets: list[dict], per_load_deadline: float,
            overall_deadline: float, poll: float, replay: bool, liveness_check: bool = True,
            debug_log_offset: int = 0) -> dict:
    telem = artifact_dir / "er-effects-telemetry.json"
    log = artifact_dir / "er-effects-autoload-debug.log"
    expected = [
        {**t, "expected": expected_identity(Path(t["file"]), int(t["slot"]))}
        for t in targets
    ]
    results: list[dict] = []
    idx = 0
    start = time.time()
    last_progress = start
    last_verified_identity = None
    crash = None

    def snapshot_result(i, verdict, res, tel_time):
        exp = expected[i]
        e = exp["expected"]
        return {
            "index": i,
            "role": "initial-autoload" if i == 0 else f"reload-{i}",
            "file": os.path.basename(os.path.dirname(exp["file"])) + "/" + os.path.basename(exp["file"]),
            "slot": exp["slot"],
            "expected_name": e.get("name"),
            "expected_level": e.get("level"),
            "verdict": verdict,
            "checks": {k: res.get(k) for k in ("identity_ok", "stats_ok", "gear_ok", "controllable_ok", "stable")} if res else None,
            "observed": res.get("observed") if res else None,
            "seconds_since_prev": round(tel_time, 1),
        }

    while idx < len(expected):
        now = time.time()
        if now - start > overall_deadline:
            break
        tel = read_telemetry(telem)
        crash = log_has_crash(log, debug_log_offset)
        if crash:
            break
        if tel is not None:
            res = evaluate_load(expected[idx]["expected"], tel)
            obs_name = res["observed"].get("name")
            # A distinct, stable, finished-loading world with the EXPECTED identity == this load done.
            if res["stable"] and obs_name and obs_name != last_verified_identity:
                if load_ok(res):
                    results.append(snapshot_result(idx, "PASS", res, now - last_progress))
                    last_verified_identity = obs_name
                    last_progress = now
                    idx += 1
                    continue
                # stable but WRONG identity/stats/gear against the expected target for this step
                elif res["identity_ok"] is False and obs_name != (results[-1]["observed"]["name"] if results else None):
                    # a different-but-unexpected character stabilized -> mismatch for this step
                    results.append(snapshot_result(idx, "FAIL-MISMATCH", res, now - last_progress))
                    last_progress = now
                    idx += 1
                    continue
        # stall check
        if not replay and now - last_progress > per_load_deadline:
            r = snapshot_result(idx, "STALL", None, now - last_progress)
            r["diagnosis"] = stall_diagnosis(log, debug_log_offset)
            results.append(r)
            break
        if replay:
            # one pass over the final telemetry only
            if tel is not None:
                res = evaluate_load(expected[idx]["expected"], tel)
                verdict = "PASS" if load_ok(res) else ("FAIL" if res["observed"].get("name") else "NO-DATA")
                results.append(snapshot_result(idx, verdict + "(replay-final)", res, now - last_progress))
            idx += 1
            continue
        if not replay and liveness_check and not process_alive() and idx < len(expected):
            crash = crash or "eldenring.exe exited before all loads completed"
            break
        _POLL_WAIT.wait(poll)

    return {
        "artifact_dir": str(artifact_dir),
        "targets_total": len(expected),
        "loads_verified": sum(1 for r in results if r["verdict"].startswith("PASS")),
        "results": results,
        "crash": crash,
        "elapsed_s": round(time.time() - start, 1),
    }


def render_report(summary: dict) -> str:
    lines = []
    lines.append("# Repeatable Multi-Save Load -- Proof Report")
    lines.append("")
    passed = summary["loads_verified"]
    total = summary["targets_total"]
    crash = summary["crash"]
    files = sorted({r["file"] for r in summary["results"]})
    proven = passed == total and total > 0 and not crash
    lines.append(f"**Verdict: {'PROVEN (exit 0)' if proven else 'NOT PROVEN'}** -- {passed}/{total} loads verified; "
                 f"crash/stall: {crash or 'none'}; files covered: {len(files)}")
    lines.append(f"Artifact dir: `{summary['artifact_dir']}`  (elapsed {summary['elapsed_s']}s)")
    lines.append("")
    lines.append("| # | role | file | slot | expect | observed | id | stats | gear | ctrl | stable | Δt(s) | verdict |")
    lines.append("|---|------|------|------|--------|----------|----|-------|------|------|--------|-------|---------|")
    for r in summary["results"]:
        c = r["checks"] or {}
        obs = (r["observed"] or {}).get("name")
        def mark(v):
            return "ok" if v is True else ("--" if v is None else "X")
        lines.append(
            f"| {r['index']} | {r['role']} | {r['file']} | {r['slot']} | "
            f"{r['expected_name']}/L{r['expected_level']} | {obs} | "
            f"{mark(c.get('identity_ok'))} | {mark(c.get('stats_ok'))} | {mark(c.get('gear_ok'))} | "
            f"{mark(c.get('controllable_ok'))} | {mark(c.get('stable'))} | {r['seconds_since_prev']} | {r['verdict']} |"
        )
    lines.append("")
    for r in summary["results"]:
        if r.get("diagnosis"):
            lines.append(f"> STALL diagnosis (load {r['index']} {r['role']}): {r['diagnosis']}")
    if crash:
        lines.append(f"> CRASH/STALL evidence: `{crash}`")
    lines.append("")
    lines.append("Load-success oracle = RAM telemetry only (identity+stats+gear+controllable). "
                 "Timings are Δt between consecutive verified loads (not gated on speed this round).")
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--artifact-dir", required=True, type=Path)
    ap.add_argument("--targets", required=True, type=Path, help="ordered JSON list of {file, slot}")
    ap.add_argument("--report", type=Path, default=None)
    ap.add_argument("--per-load-deadline", type=float, default=90.0)
    ap.add_argument("--overall-deadline", type=float, default=600.0)
    ap.add_argument("--poll", type=float, default=2.0)
    ap.add_argument("--replay", action="store_true", help="single pass over the finished run's final telemetry")
    ap.add_argument("--no-liveness-check", action="store_true", help="do not treat a missing eldenring.exe as a crash (offline loop testing)")
    ap.add_argument("--debug-log-offset", type=int, default=0, help="only scan the debug log for crash markers past this byte offset (shared append-log)")
    args = ap.parse_args()

    targets = json.loads(args.targets.read_text())
    summary = monitor(args.artifact_dir, targets, args.per_load_deadline,
                      args.overall_deadline, args.poll, args.replay, liveness_check=not args.no_liveness_check,
                      debug_log_offset=args.debug_log_offset)
    report = render_report(summary)
    if args.report:
        args.report.write_text(report)
    print(report)
    print()
    print(json.dumps(summary["counts"] if "counts" in summary else
                     {"loads_verified": summary["loads_verified"], "targets_total": summary["targets_total"],
                      "crash": summary["crash"]}, indent=1))
    proven = summary["loads_verified"] == summary["targets_total"] and summary["targets_total"] > 0 and not summary["crash"]
    return 0 if proven else 1


if __name__ == "__main__":
    sys.exit(main())
