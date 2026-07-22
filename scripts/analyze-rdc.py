#!/usr/bin/env python3
"""Summarize a RenderDoc .rdc frame: draw-call count + per-event GPU timing.

Answers GPU-vs-CPU for the reload-fps question (total GPU time vs the ~50ms frame) and ranks the
heaviest passes so a vanilla-reload capture can be diffed against a product-reload capture.

Run under WINDOWS qrenderdoc (which bundles the `renderdoc` python module). qrenderdoc's argv passing is
unreliable, so config comes from a JSON file at $RDC_ANALYZE_CONFIG (default C:/temp/rdc-analyze.json):
    {"rdc": "C:/.../er_cap_frame0.rdc", "log": "C:/.../rdc-summary.txt", "top": 20}

    qrenderdoc.exe --python scripts/analyze-rdc.py

Output (to the log path): total draws/dispatches, whether EventGPUDuration is available, total GPU ms,
and the top-N events by GPU time (eventId + name).
"""
import json
import os
import sys

import renderdoc as rd  # provided by qrenderdoc's embedded interpreter


def cfg():
    p = os.environ.get("RDC_ANALYZE_CONFIG", "C:/temp/rdc-analyze.json")
    c = json.load(open(p)) if os.path.exists(p) else {}
    return c.get("rdc"), c.get("log", "C:/temp/rdc-summary.txt"), int(c.get("top", 20))


def walk(actions):
    for a in actions:
        yield a
        yield from walk(a.children)


def run(rdc, top, out):
    def log(*a):
        print(*a)
        out.write(" ".join(str(x) for x in a) + "\n")
        out.flush()

    log(f"[analyze-rdc] opening {rdc}")
    cap = rd.OpenCaptureFile()
    if cap.OpenFile(rdc, "", None) != rd.ResultCode.Succeeded:
        log("OpenFile FAILED")
        os._exit(2)
    if not cap.LocalReplaySupport():
        log("no local replay support")
        os._exit(2)
    res, ctrl = cap.OpenCapture(rd.ReplayOptions(), None)
    if res != rd.ResultCode.Succeeded:
        log(f"OpenCapture FAILED: {res}")
        os._exit(2)

    sdf = ctrl.GetStructuredFile()
    actions = list(walk(ctrl.GetRootActions()))
    draws = [a for a in actions if a.flags & rd.ActionFlags.Drawcall]
    dispatches = [a for a in actions if a.flags & rd.ActionFlags.Dispatch]
    names = {a.eventId: a.GetName(sdf) for a in actions}
    log(f"total actions={len(actions)} drawcalls={len(draws)} dispatches={len(dispatches)}")

    counters = ctrl.EnumerateCounters()
    if rd.GPUCounter.EventGPUDuration not in counters:
        log("EventGPUDuration counter NOT available on this replay -> cannot get GPU time")
        ctrl.Shutdown()
        cap.Shutdown()
        os._exit(0)

    desc = ctrl.DescribeCounter(rd.GPUCounter.EventGPUDuration)
    results = ctrl.FetchCounters([rd.GPUCounter.EventGPUDuration])
    # value is a union; duration counters report a double in `.value.d` (seconds unless unit says else).
    to_ms = 1000.0 if desc.unit == rd.CounterUnit.Seconds else 1.0
    timed = []
    total = 0.0
    for r in results:
        v = r.value.d
        total += v
        timed.append((v * to_ms, r.eventId))
    log(f"GPU counter unit={desc.unit} events_timed={len(timed)} TOTAL_GPU_MS={total * to_ms:.2f}")
    log(f"--- top {top} events by GPU time ---")
    for ms, eid in sorted(timed, reverse=True)[:top]:
        log(f"  {ms:7.3f} ms  eid={eid:>6}  {names.get(eid, '?')[:80]}")

    ctrl.Shutdown()
    cap.Shutdown()
    os._exit(0)


def main():
    import threading

    rdc, logp, top = cfg()
    out = open(logp, "w", buffering=1)
    sys.stdout = sys.stderr = out
    if not rdc or not os.path.exists(rdc):
        out.write(f"ERROR: rdc missing ({rdc}); set $RDC_ANALYZE_CONFIG json {{rdc,log,top}}\n")
        os._exit(1)

    def hook(args):
        import traceback

        out.write("ANALYZE ERROR:\n" + "".join(traceback.format_exception(args.exc_type, args.exc_value, args.exc_traceback)))
        os._exit(3)

    threading.excepthook = hook
    # Run on a worker thread so qrenderdoc's main thread pumps its Qt event loop (replay calls need it).
    threading.Thread(target=run, args=(rdc, top, out), daemon=False).start()


main()
