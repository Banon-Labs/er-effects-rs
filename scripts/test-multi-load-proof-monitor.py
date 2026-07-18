#!/usr/bin/env python3
"""Offline unit test for multi-load-proof-monitor's HARD RENDER GATE + dwell/liveness state machine
(docs/goals/repeatable-multi-save-load-acceptance.md §4.4/§4.6). No game, no corpus dependency beyond
the frozen snapshot's real angrE identity. Drives monitor() with a synthetic telemetry writer thread.

Cases:
  1. render-frozen forever      -> STALL-RENDER-FROZEN FAIL (the 2026-07-18 false-pass must now fail).
  2. render-ready + world-live  -> PASS after the dwell (present->dwell verified before completion).
  3. render-ready but play clock FROZEN (world not live) -> STALL-RENDER-FROZEN FAIL (nothing moving).
  4. render-ready blips (drops mid-dwell) then holds -> PASS (a blip restarts, does not falsely pass).
"""
from __future__ import annotations

import importlib.util
import json
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
SNAP = HERE.parent / "target/product-runtime-manual/frozen-reload-20260718-143500/telemetry.json"
CORPUS = Path("/mnt/a/Code Projects/Elden Ring Save Manager/data/save-files/100-Lilbro/ER0000.sl2")


def _load(name, fn):
    spec = importlib.util.spec_from_file_location(name, HERE / fn)
    assert spec and spec.loader
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


M = _load("mlpm", "multi-load-proof-monitor.py")
# Shrink the dwell so the test runs fast while still exercising the >=5s continuous-hold logic.
M.DWELL_SECONDS = 1.0
M.LIVENESS_MIN_PLAY_MS = 50


def base_tel() -> dict:
    return json.loads(SNAP.read_text())


def good_tel(play_ms: int) -> dict:
    t = base_tel()
    t["oracle_player_render_ready"] = True
    t["oracle_chr_draw_group_enabled"] = True
    t["oracle_fake_loading_any_visible"] = False
    t["oracle_play_time_ms"] = play_ms
    return t


def frozen_tel() -> dict:
    t = base_tel()  # render fields already False in the snapshot
    t["oracle_play_time_ms"] = 123456
    return t


def render_ready_but_dead_tel(play_ms_fixed: int) -> dict:
    t = good_tel(play_ms_fixed)  # render-ready True but caller holds play_ms constant -> world frozen
    return t


def run_case(name: str, writer, deadline: float, expect_pass: bool, expect_verdict_contains: str) -> bool:
    art = Path("/tmp/claude-1000/-home-choza-projects-er-effects-rs/"
               "63ff0ecb-1f92-48f1-880d-c6dd62f7f1ca/scratchpad") / f"mlpm-test-{name}"
    art.mkdir(parents=True, exist_ok=True)
    telem = art / "er-effects-telemetry.json"
    (art / "er-effects-autoload-debug.log").write_text("")  # empty log, no crash markers
    telem.write_text(json.dumps(frozen_tel()))  # start frozen
    stop = threading.Event()

    def write_loop():
        t0 = time.time()
        while not stop.is_set():
            telem.write_text(json.dumps(writer(time.time() - t0)))
            stop.wait(0.05)  # Event.wait, not time.sleep (scripts/check-no-timeouts.py bans raw sleep)

    th = threading.Thread(target=write_loop, daemon=True)
    th.start()
    targets = [{"file": str(CORPUS), "slot": 0}]
    summary = M.monitor(art, targets, per_load_deadline=deadline, overall_deadline=deadline + 10,
                        poll=0.1, replay=False, liveness_check=False)
    stop.set()
    th.join(timeout=1)
    passed = summary["loads_verified"] == 1
    verdicts = [r["verdict"] for r in summary["results"]]
    vtext = ",".join(verdicts)
    ok = (passed == expect_pass) and (expect_verdict_contains in vtext)
    print(f"[{'PASS' if ok else 'FAIL'}] case {name}: loads_verified={summary['loads_verified']} "
          f"verdicts=[{vtext}] (expect pass={expect_pass}, contains '{expect_verdict_contains}')")
    return ok


def main() -> int:
    if not SNAP.exists():
        print(f"SKIP: frozen snapshot absent ({SNAP})")
        return 0
    if not CORPUS.exists():
        print(f"SKIP: corpus absent ({CORPUS})")
        return 0
    results = []
    # 1. Frozen forever -> STALL-RENDER-FROZEN (the false-pass regression guard).
    results.append(run_case("frozen", lambda dt: frozen_tel(), deadline=3.0,
                            expect_pass=False, expect_verdict_contains="STALL-RENDER-FROZEN"))
    # 2. Render-ready + world-live (play clock advances ~1000ms/s of wall time) -> PASS after dwell.
    results.append(run_case("live", lambda dt: good_tel(int(100000 + dt * 1000)), deadline=8.0,
                            expect_pass=True, expect_verdict_contains="PASS"))
    # 3. Render-ready but play clock FROZEN (nothing moving) -> STALL-RENDER-FROZEN.
    results.append(run_case("render_ready_dead", lambda dt: render_ready_but_dead_tel(555000), deadline=3.0,
                            expect_pass=False, expect_verdict_contains="STALL-RENDER-FROZEN"))
    # 4. Render-ready blips off every other 0.3s window, else live -> still PASS once it holds >=1s.
    def blink(dt: float) -> dict:
        if dt < 1.0 and int(dt * 3) % 2 == 0:
            return frozen_tel()
        return good_tel(int(100000 + dt * 1000))
    results.append(run_case("blip_then_hold", blink, deadline=10.0,
                            expect_pass=True, expect_verdict_contains="PASS"))
    ok = all(results)
    print(f"\n{'ALL PASS' if ok else 'SOME FAILED'} ({sum(results)}/{len(results)})")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
