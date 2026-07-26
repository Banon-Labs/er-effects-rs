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


def _winpath(p: str) -> str:
    """/mnt/<d>/rest -> <D>:\\rest (the Windows path the game opens); pass-through otherwise."""
    if p.startswith('/mnt/') and len(p) > 6 and p[6] == '/':
        return p[5].upper() + ':\\' + p[7:].replace('/', '\\')
    return p
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


# ---- HARD RENDER GATE (docs/goals/repeatable-multi-save-load-acceptance.md §4.4, revised 2026-07-18) ----
# The 2026-07-18 render-freeze false-pass proved "present + controllable-by-model" is NOT enough: the
# character can be present yet invisible (draw group off) in a frozen world. These gate on the DLL's
# render-readiness oracles + a WORLD-LIVE liveness clock, held continuously for a dwell window.
DWELL_SECONDS = 5.0            # §4.4: render-ready must hold CONTINUOUSLY this long (no one-frame blips)
LIVENESS_MIN_PLAY_MS = 250     # over a >=5s dwell a live world advances play_time ~5000ms; 250 is a safe floor
HAVOK_EPS = 1e-3               # fallback liveness (no play_time): world-position movement threshold


def render_ready(tel: dict) -> bool:
    """The HARD render gate: the character is actually rendering and the loading cover is dismissed.
    - oracle_player_render_ready == true (DLL ANDs: chr-model+ctrl present, draw_group_enabled,
      is_render_group_enabled, enable_render) -- the exact combination that was FALSE in the freeze.
    - oracle_chr_draw_group_enabled == true -- kept explicit; it was THE failing field.
    - oracle_fake_loading_any_visible == false -- the loading cover is actually gone.
    NOTE (evidence-based deviation from §4.4's literal 'oracle_now_loading cleared'): oracle_now_loading
    is CSNowLoadingHelperImp::load_done, a load-COMPLETE latch that reads TRUE and LINGERS into normal
    gameplay (per the DLL RE comment in write_game_module_oracles.rs), and across captured frozen
    snapshots it was observed as BOTH 0 and 1 -- so it does NOT discriminate frozen vs live and
    requiring ==0 would false-fail good loads (or false-pass a now_loading==0 freeze). The real
    cover-dismissed signal is oracle_fake_loading_any_visible==false, which was TRUE in every frozen
    snapshot. now_loading is reported for diagnostics only, never gated on."""
    return (
        tel.get("oracle_player_render_ready") is True
        and tel.get("oracle_chr_draw_group_enabled") is True
        and tel.get("oracle_fake_loading_any_visible") is False
    )


def liveness_of(tel: dict) -> dict:
    """A snapshot of world-liveness signals for dwell comparison. Primary = play_time_ms (the game's
    own in-game play clock, which advances ONLY while the world sim steps and is paused during
    loads/menus/frozen states). Secondary (fallback if play_time is unavailable) = world position."""
    pt = tel.get("oracle_play_time_ms")
    pos = tel.get("oracle_havok_pos")
    return {
        "play_ms": pt if isinstance(pt, (int, float)) and pt >= 0 else None,
        "pos": tuple(pos) if isinstance(pos, (list, tuple)) and len(pos) == 3 else None,
    }


def liveness_advanced(a: dict, b: dict) -> bool:
    """True iff the world genuinely advanced between two liveness samples. If play_time is available at
    both ends it is AUTHORITATIVE (a frozen world's play clock does not tick, so havok jitter cannot
    rescue it -- matching §4.4's 'nothing moving must FAIL'). Only when play_time is absent do we fall
    back to world-position movement."""
    pa, pb = a.get("play_ms"), b.get("play_ms")
    if pa is not None and pb is not None:
        return (pb - pa) >= LIVENESS_MIN_PLAY_MS
    qa, qb = a.get("pos"), b.get("pos")
    if qa is not None and qb is not None:
        d2 = sum((x - y) ** 2 for x, y in zip(qa, qb))
        return d2 > (HAVOK_EPS ** 2)
    return False


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


def world_present(tel: dict) -> bool:
    """A real character is resident in a loaded world -- WITHOUT the now_loading==0 requirement that
    ORACLE.stable_world_loaded imposes. Deliberately excludes oracle_now_loading: it is an unreliable
    load-DONE latch (lingers true in gameplay; observed both 0 and 1 in frozen snapshots), so keying
    presence on it both false-fails good loads and false-passes now_loading==0 freezes (the actual
    2026-07-18 blind spot). The HARD RENDER GATE (render_ready + cover-dismissed), held for the dwell,
    is what proves the world is finished, rendered, and live -- not now_loading."""
    player = tel.get("oracle_player_present") is True or tel.get("player_available") is True
    loaded = (
        tel.get("oracle_block_id_valid") is True
        or isinstance(tel.get("oracle_havok_pos"), list)
        or ORACLE._as_int(tel.get("oracle_saved_map_c30"), -1) not in (-1, 0)
    )
    real = (not ORACLE._name_empty_like(tel.get("oracle_char_name"))
            and ORACLE._as_int(tel.get("oracle_char_level"), 0) > 0)
    return bool(player and loaded and real)


def evaluate_load(exp: dict, tel: dict) -> dict:
    observed = ORACLE.observed_identity(tel)
    return {
        "identity_ok": ORACLE.identity_matches(exp, observed),
        "stats_ok": stats_match(exp, tel),
        "gear_ok": gear_match(exp, tel),
        "controllable_ok": controllable(tel),
        "render_ready_ok": render_ready(tel),
        "world_present": world_present(tel),
        "stable": ORACLE.stable_world_loaded(tel),  # reported for diagnostics (now_loading-gated)
        "observed": observed,
    }


def identity_ok_full(res: dict) -> bool:
    """The right character is loaded (identity+stats+gear+a real loaded world). Uses world_present
    (NOT the now_loading-gated `stable`) so the render gate/dwell -- not the unreliable now_loading
    latch -- decides finished-and-rendered."""
    return all(res[k] for k in ("identity_ok", "stats_ok", "gear_ok", "world_present"))


def load_ok(res: dict) -> bool:
    """A single-frame snapshot passes ALL gates (used by --replay; the live path additionally
    enforces the >=5s render-ready dwell + world-live liveness in the monitor loop)."""
    return identity_ok_full(res) and res.get("render_ready_ok") is True


def monitor(artifact_dir: Path, targets: list[dict], per_load_deadline: float,
            overall_deadline: float, poll: float, replay: bool, liveness_check: bool = True,
            debug_log_offset: int = 0, drive_slot_file: Path | None = None,
            drive_file_override: Path | None = None) -> dict:
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
    # Per-load dwell state (§4.4 hard render gate + §4.6 sequencing gate). Reset whenever the current
    # load fails the render gate (a blip restarts the continuous dwell) or a new load index begins.
    dwell_start_t: float | None = None       # when render-ready+identity first held continuously
    dwell_start_liveness: dict | None = None # world-liveness sample at dwell start
    present_since: float | None = None       # when the right identity first became present (for time-to-stable)

    def snapshot_result(i, verdict, res, tel_time, extra=None):
        exp = expected[i]
        e = exp["expected"]
        r = {
            "index": i,
            "role": "initial-autoload" if i == 0 else f"reload-{i}",
            "file": os.path.basename(os.path.dirname(exp["file"])) + "/" + os.path.basename(exp["file"]),
            "slot": exp["slot"],
            "expected_name": e.get("name"),
            "expected_level": e.get("level"),
            "verdict": verdict,
            "checks": {k: res.get(k) for k in ("identity_ok", "stats_ok", "gear_ok", "controllable_ok", "render_ready_ok", "stable")} if res else None,
            "observed": res.get("observed") if res else None,
            "seconds_since_prev": round(tel_time, 1),
        }
        if extra:
            r.update(extra)
        return r

    def drive_next(next_idx: int):
        # PROGRAMMATIC DRIVE (§6): trigger the next reload by writing its (file,)slot to the DLL
        # control files. targets[0] is the boot autoload (TOML), so we only drive reloads (idx>=1).
        if drive_slot_file is None or next_idx >= len(expected):
            return
        try:
            if drive_file_override is not None:
                drive_file_override.write_text(_winpath(str(expected[next_idx]["file"])) + "\n")
            drive_slot_file.write_text(f"{int(expected[next_idx]['slot'])}\n")
        except OSError:
            pass

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
            id_match = identity_ok_full(res) and obs_name and obs_name != last_verified_identity
            # ---- §4.6 SEQUENCING GATE: the right character must be render-ready AND hold that,
            # world-live, for a >=5s dwell BEFORE we count the load and trigger the next one. ----
            if id_match and res["render_ready_ok"]:
                if present_since is None:
                    present_since = now
                cur_live = liveness_of(tel)
                if dwell_start_t is None:
                    # render gate first satisfied -> begin the continuous dwell
                    dwell_start_t = now
                    dwell_start_liveness = cur_live
                else:
                    held = (now - dwell_start_t) >= DWELL_SECONDS
                    live = liveness_advanced(dwell_start_liveness or {}, cur_live)
                    if held and live:
                        tts = round(now - present_since, 1) if present_since else None
                        results.append(snapshot_result(
                            idx, "PASS", res, now - last_progress,
                            {"time_to_stable_s": tts, "dwell_s": round(now - dwell_start_t, 1),
                             "play_ms": cur_live.get("play_ms")}))
                        last_verified_identity = obs_name
                        last_progress = now
                        dwell_start_t = None
                        dwell_start_liveness = None
                        present_since = None
                        idx += 1
                        drive_next(idx)
                        continue
            else:
                # Render gate NOT satisfied for the target this frame.
                if id_match and present_since is None:
                    present_since = now  # right char is present (logically) but not yet render-ready
                # A blip during dwell (render-ready dropped, or identity changed) breaks continuity:
                # restart the dwell; the per-load deadline keeps ticking (a never-stabilizing load stalls).
                dwell_start_t = None
                dwell_start_liveness = None
                # a real loaded world with the WRONG identity for this step -> mismatch
                if res["world_present"] and obs_name and res["identity_ok"] is False \
                        and obs_name != (results[-1]["observed"]["name"] if results else None):
                    results.append(snapshot_result(idx, "FAIL-MISMATCH", res, now - last_progress))
                    last_progress = now
                    idx += 1
                    present_since = None
                    continue
        # stall check -- INCLUDES the logically-loaded-but-render-frozen state (§4.5): if the right
        # character became present but never passed the render-ready dwell within the deadline, that is
        # a STALL/FAIL and the run stops (does NOT advance to the next load, §4.6).
        if not replay and now - last_progress > per_load_deadline:
            frozen = present_since is not None
            r = snapshot_result(idx, "STALL-RENDER-FROZEN" if frozen else "STALL", None, now - last_progress)
            r["diagnosis"] = stall_diagnosis(log, debug_log_offset)
            if frozen:
                r["diagnosis"] = ("present but never render-ready/world-live for the >=5s dwell "
                                  "(render-handoff freeze); " + r["diagnosis"])
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
    lines.append("| # | role | file | slot | expect | observed | id | stats | gear | render | stable | Δt(s) | t2stable(s) | verdict |")
    lines.append("|---|------|------|------|--------|----------|----|-------|------|--------|--------|-------|-------------|---------|")
    for r in summary["results"]:
        c = r["checks"] or {}
        obs = (r["observed"] or {}).get("name")
        def mark(v):
            return "ok" if v is True else ("--" if v is None else "X")
        tts = r.get("time_to_stable_s")
        lines.append(
            f"| {r['index']} | {r['role']} | {r['file']} | {r['slot']} | "
            f"{r['expected_name']}/L{r['expected_level']} | {obs} | "
            f"{mark(c.get('identity_ok'))} | {mark(c.get('stats_ok'))} | {mark(c.get('gear_ok'))} | "
            f"{mark(c.get('render_ready_ok'))} | {mark(c.get('stable'))} | {r['seconds_since_prev']} | "
            f"{'--' if tts is None else tts} | {r['verdict']} |"
        )
    lines.append("")
    for r in summary["results"]:
        if r.get("diagnosis"):
            lines.append(f"> STALL diagnosis (load {r['index']} {r['role']}): {r['diagnosis']}")
    if crash:
        lines.append(f"> CRASH/STALL evidence: `{crash}`")
    lines.append("")
    lines.append("Load-success oracle = RAM telemetry only. PASS requires the HARD RENDER GATE "
                 "(player_render_ready + chr_draw_group_enabled + loading cover dismissed) held "
                 f">= {DWELL_SECONDS:.0f}s with the world-live play clock advancing, verified BEFORE the "
                 "next load is triggered (§4.4/§4.6). 'render'=render gate; t2stable=present->dwell-passed. "
                 "A present-but-frozen load is a STALL-RENDER-FROZEN FAIL, not a pass.")
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
    ap.add_argument("--drive-slot-file", type=Path, default=None, help="programmatic drive: after each verified load, write the next target slot here (the DLL control file) to trigger the reload")
    ap.add_argument("--drive-file-override", type=Path, default=None, help="cross-file drive: write the next target save FILE (as a Windows path) here before the slot")
    args = ap.parse_args()

    targets = json.loads(args.targets.read_text())
    summary = monitor(args.artifact_dir, targets, args.per_load_deadline,
                      args.overall_deadline, args.poll, args.replay, liveness_check=not args.no_liveness_check,
                      debug_log_offset=args.debug_log_offset, drive_slot_file=args.drive_slot_file,
                      drive_file_override=args.drive_file_override)
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
