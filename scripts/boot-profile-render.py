#!/usr/bin/env python3
"""Render a boot-sequence profile from a runtime-probe artifact dir.

Inputs (in the run dir):
  * er-effects-autoload-debug.log  -- `[+<ms>ms] <tag>: ...` phase markers
  * er-effects-profile.jsonl       -- per-sample, per-thread CPU counters (from the DLL profiler)
  * readiness-result.json          -- optional; runtime_module_base fallback, launch timing

Outputs (written into the run dir):
  * boot-profile-report.md         -- phase waterfall table + per-phase CPU verdict + hot threads
  * boot-profile-timeline.svg      -- waterfall + utilization + active-thread-count + per-thread heat

The key signal for "missed parallelization": a phase with large wall-clock but ~1 active thread and
low total CPU utilization across N cores is serialized -- a candidate to parallelize.

Usage: boot-profile-render.py <run_dir>
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

# Ordered boot milestones: (regex on the log message, short label). First match's ms is the marker.
MILESTONES = [
    (r"^safe_input hook ExitProcess", "dll_attach"),
    (r"^boot-phase: cstask_instance_ready", "cstask_instance_ready"),
    (r"^boot-phase: first_game_frame", "first_game_frame"),
    (r"^force-offline: cleared", "force_offline_cleared"),
    (r"^pab-advance: press-any-button job READY", "press_any_button_ready"),
    (r"^pab-advance: \*\*\* SET", "press_any_button_dismissed"),
    (r"^native_title_job: captured title owner", "title_owner_captured"),
    (r"^title-accept-byte: set", "menu_open"),
    (r"^network-check-shortcircuit: forced", "network_check"),
    (r"^show-progress: PASS-THROUGH", "save_data_read"),
    (r"^own-load-stream: frame=0\b", "world_stream_begin"),
    (r"^EVENT T_controllable", "in_world_controllable"),
]

CPU_ACTIVE_MS_THRESH = 0.2  # per-interval per-thread CPU ms above which a thread counts as "active"


def parse_log(path: Path) -> list[tuple[int, str]]:
    """Return [(ms, message)] for every `[+<ms>ms] <message>` line."""
    out = []
    if not path.exists():
        return out
    rx = re.compile(r"^\[\+(\d+)ms\]\s+(.*)$")
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        m = rx.match(line)
        if m:
            out.append((int(m.group(1)), m.group(2)))
    return out


def extract_milestones(log: list[tuple[int, str]]) -> list[tuple[str, int]]:
    found: dict[str, int] = {}
    for ms, msg in log:
        for rx, label in MILESTONES:
            if label not in found and re.search(rx, msg):
                found[label] = ms
    # Sort by actual time: some markers (e.g. the early own-stream observer) interleave, so
    # MILESTONES declaration order is not the wall-clock order. Time order = correct waterfall.
    return sorted(((label, found[label]) for _, label in MILESTONES if label in found), key=lambda kv: kv[1])


def parse_profile(path: Path):
    """Return (header, samples) where samples = [{'ms':int,'t':{tid:(cy,k,u,rip,name)}}]."""
    header = {}
    samples = []
    if not path.exists():
        return header, samples
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        if obj.get("kind") == "header":
            header = obj
            continue
        if "ms" not in obj:
            continue
        threads = {}
        for t in obj.get("t", []):
            threads[t["id"]] = (
                t.get("cy", 0), t.get("k", 0), t.get("u", 0), t.get("rip"), t.get("n"),
            )
        samples.append({"ms": obj["ms"], "t": threads})
    return header, samples


def cpu_intervals(samples):
    """Per consecutive-sample interval: dt_ms and per-tid CPU-ms (kernel+user delta, 100ns->ms)."""
    intervals = []
    for a, b in zip(samples, samples[1:]):
        dt = b["ms"] - a["ms"]
        if dt <= 0:
            continue
        per_tid = {}
        for tid, (_, k2, u2, _, _) in b["t"].items():
            if tid in a["t"]:
                _, k1, u1, _, _ = a["t"][tid]
                d = (k2 + u2) - (k1 + u1)  # 100ns units
                if d > 0:
                    per_tid[tid] = d / 1e4  # -> ms
        intervals.append({"t0": a["ms"], "t1": b["ms"], "dt": dt, "cpu": per_tid})
    return intervals


def thread_names(samples):
    names = {}
    for s in samples:
        for tid, (_, _, _, _, n) in s["t"].items():
            if n and tid not in names:
                names[tid] = n
    return names


def fmt_ms(ms):
    return f"{ms/1000:.2f}s"


def phase_windows(milestones, end_ms):
    """[(label, t0, t1)] from consecutive milestones; last extends to end_ms."""
    wins = []
    for i, (label, t0) in enumerate(milestones):
        t1 = milestones[i + 1][1] if i + 1 < len(milestones) else end_ms
        wins.append((f"{label} → {milestones[i+1][0]}" if i + 1 < len(milestones) else f"{label} → end", t0, t1))
    return wins


def verdict(util, max_active, wall_ms, busiest_frac):
    """Classify a phase.

    `busiest_frac` = (CPU ms of the single busiest thread) / wall_ms. A value near 1.0 means ONE
    thread was pegged for the whole phase. Combined with low all-core `util`, that is the missed-
    parallelism signature: one core saturated while the other N-1 idle. This is checked BEFORE the
    wait-bound test, because a single pegged thread on a 16-core box still yields low overall util
    yet is emphatically NOT wait-bound -- it is serialized CPU work that could be parallelized.
    """
    if wall_ms < 150:
        return "trivial"
    if busiest_frac >= 0.65 and util < 0.5:
        return f"SERIALIZED 1-thread ({busiest_frac*100:.0f}% of one core, rest idle) — PARALLELIZE"
    if util > 0.5:
        return "CPU-parallel (good)"
    if util < 0.15 and busiest_frac < 0.3:
        return "WAIT-BOUND (I/O / sleep / network)"
    return "mixed CPU/wait"


def build_report(run_dir, header, milestones, intervals, names, ncpu, end_ms):
    lines = []
    lines.append(f"# Boot profile — {run_dir.name}\n")
    lines.append(f"- cores (ncpu): **{ncpu}**")
    lines.append(f"- profiler samples interval: {header.get('interval_ms','?')}ms, RIP={header.get('rip')}")
    lines.append(f"- total observed boot window: **{fmt_ms(end_ms)}**\n")

    lines.append("## Milestone timeline\n")
    lines.append("| milestone | t (+ms) | wall | Δ from prev |")
    lines.append("|---|--:|--:|--:|")
    prev = None
    for label, ms in milestones:
        d = "" if prev is None else fmt_ms(ms - prev)
        lines.append(f"| {label} | {ms} | {fmt_ms(ms)} | {d} |")
        prev = ms
    lines.append("")

    lines.append("## Per-phase CPU attribution\n")
    lines.append("| phase | wall | util (all cores) | busiest 1 thread | peak active | verdict | top threads (CPU ms) |")
    lines.append("|---|--:|--:|--:|--:|---|---|")
    wins = phase_windows(milestones, end_ms)
    phase_rows = []
    for label, t0, t1 in wins:
        wall = t1 - t0
        segs = [iv for iv in intervals if iv["t1"] > t0 and iv["t0"] < t1]
        if not segs:
            continue
        total_cpu = 0.0
        total_dt = 0.0
        per_tid = {}
        peak_active = 0
        for iv in segs:
            total_dt += iv["dt"]
            active = 0
            for tid, cms in iv["cpu"].items():
                total_cpu += cms
                per_tid[tid] = per_tid.get(tid, 0.0) + cms
                if cms >= CPU_ACTIVE_MS_THRESH:
                    active += 1
            peak_active = max(peak_active, active)
        util = total_cpu / (total_dt * ncpu) if total_dt and ncpu else 0.0
        top = sorted(per_tid.items(), key=lambda kv: -kv[1])[:4]
        top_s = ", ".join(f"{names.get(tid, tid)}={v:.0f}" for tid, v in top)
        busiest_frac = (top[0][1] / wall) if (top and wall) else 0.0
        v = verdict(util, peak_active, wall, busiest_frac)
        busiest_disp = "—" if wall < 150 else f"{busiest_frac*100:.0f}%"
        lines.append(f"| {label} | {fmt_ms(wall)} | {util*100:.0f}% | {busiest_disp} | {peak_active} | {v} | {top_s} |")
        phase_rows.append((label, t0, t1, util, peak_active, v))
    lines.append("")

    # RIP hot spots (raw; symbolize offline via the Ghidra dump).
    rip_hist = {}
    base = header.get("module_base") or 0
    for s_idx in []:
        pass
    return "\n".join(lines), phase_rows


def svg_timeline(path, milestones, intervals, names, ncpu, end_ms):
    W, H = 1400, 720
    left, right, top = 220, 40, 40
    plot_w = W - left - right
    def x(ms):
        return left + plot_w * (ms / end_ms if end_ms else 0)

    parts = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" font-family="monospace" font-size="11">']
    parts.append(f'<rect width="{W}" height="{H}" fill="#0c0c12"/>')

    # Time axis ticks every 5s
    sec = 5000
    t = 0
    while t <= end_ms:
        xx = x(t)
        parts.append(f'<line x1="{xx:.1f}" y1="{top}" x2="{xx:.1f}" y2="{H-30}" stroke="#222"/>')
        parts.append(f'<text x="{xx:.1f}" y="{H-14}" fill="#888" text-anchor="middle">{t//1000}s</text>')
        t += sec

    # --- Waterfall band (phases) ---
    wf_y, wf_h = top, 150
    colors = ["#4e79a7", "#f28e2b", "#59a14f", "#e15759", "#b07aa1", "#76b7b2", "#edc948", "#ff9da7", "#9c755f", "#bab0ac"]
    wins = []
    for i, (label, ms) in enumerate(milestones):
        t1 = milestones[i + 1][1] if i + 1 < len(milestones) else end_ms
        wins.append((label, ms, t1))
    row_h = max(12, wf_h // max(1, len(wins)))
    for i, (label, t0, t1) in enumerate(wins):
        yy = wf_y + i * row_h
        c = colors[i % len(colors)]
        parts.append(f'<rect x="{x(t0):.1f}" y="{yy}" width="{max(1,x(t1)-x(t0)):.1f}" height="{row_h-2}" fill="{c}" opacity="0.85"/>')
        parts.append(f'<text x="6" y="{yy+row_h-5}" fill="#ddd">{label[:30]}</text>')
        parts.append(f'<text x="{x(t0)+3:.1f}" y="{yy+row_h-5}" fill="#000">{(t1-t0)/1000:.1f}s</text>')

    # --- Utilization + active-thread lines ---
    ut_y, ut_h = wf_y + wf_h + 30, 180
    parts.append(f'<text x="6" y="{ut_y-6}" fill="#aaa">CPU util (all {ncpu} cores) = white, active-thread count = green</text>')
    parts.append(f'<rect x="{left}" y="{ut_y}" width="{plot_w}" height="{ut_h}" fill="#111" stroke="#333"/>')
    # util polyline (0..1)
    pu = []
    pa = []
    max_active = max((sum(1 for c in iv["cpu"].values() if c >= CPU_ACTIVE_MS_THRESH) for iv in intervals), default=1)
    for iv in intervals:
        mid = (iv["t0"] + iv["t1"]) / 2
        cpu = sum(iv["cpu"].values())
        util = cpu / (iv["dt"] * ncpu) if iv["dt"] and ncpu else 0
        active = sum(1 for c in iv["cpu"].values() if c >= CPU_ACTIVE_MS_THRESH)
        pu.append(f"{x(mid):.1f},{ut_y+ut_h-util*ut_h:.1f}")
        pa.append(f"{x(mid):.1f},{ut_y+ut_h-(active/max_active)*ut_h:.1f}")
    if pu:
        parts.append(f'<polyline points="{" ".join(pu)}" fill="none" stroke="#fff" stroke-width="1"/>')
        parts.append(f'<polyline points="{" ".join(pa)}" fill="none" stroke="#59f759" stroke-width="1"/>')
    parts.append(f'<text x="{left-4}" y="{ut_y+10}" fill="#fff" text-anchor="end">100%</text>')
    parts.append(f'<text x="{left-4}" y="{ut_y+ut_h}" fill="#fff" text-anchor="end">0</text>')
    parts.append(f'<text x="{left-4}" y="{ut_y+12}" fill="#59f759" text-anchor="end" dy="12">{max_active}thr</text>')

    # --- Per-thread heat (top K by total CPU) ---
    totals = {}
    for iv in intervals:
        for tid, c in iv["cpu"].items():
            totals[tid] = totals.get(tid, 0.0) + c
    topk = [tid for tid, _ in sorted(totals.items(), key=lambda kv: -kv[1])[:14]]
    ht_y = ut_y + ut_h + 30
    rh = 18
    parts.append(f'<text x="6" y="{ht_y-6}" fill="#aaa">per-thread CPU heat (top {len(topk)} threads)</text>')
    for r, tid in enumerate(topk):
        yy = ht_y + r * rh
        label = str(names.get(tid, tid))[:26]
        parts.append(f'<text x="6" y="{yy+rh-5}" fill="#ccc">{label}</text>')
        for iv in intervals:
            c = iv["cpu"].get(tid, 0.0)
            frac = min(1.0, c / max(1e-6, iv["dt"]))  # 1.0 == thread pegged the whole interval
            if frac <= 0.01:
                continue
            inten = int(40 + frac * 215)
            parts.append(f'<rect x="{x(iv["t0"]):.1f}" y="{yy}" width="{max(1,x(iv["t1"])-x(iv["t0"])):.1f}" height="{rh-2}" fill="rgb({inten},{int(inten*0.55)},40)"/>')
    parts.append("</svg>")
    Path(path).write_text("\n".join(parts), encoding="utf-8")


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: boot-profile-render.py <run_dir>", file=sys.stderr)
        return 2
    run_dir = Path(sys.argv[1])
    log = parse_log(run_dir / "er-effects-autoload-debug.log")
    header, samples = parse_profile(run_dir / "er-effects-profile.jsonl")
    if not samples:
        print(f"no profiler samples in {run_dir}/er-effects-profile.jsonl", file=sys.stderr)
    milestones = extract_milestones(log)
    ncpu = int(header.get("ncpu") or 1)
    end_ms = max(
        (samples[-1]["ms"] if samples else 0),
        (milestones[-1][1] if milestones else 0),
        (log[-1][0] if log else 0),
    )
    intervals = cpu_intervals(samples)
    names = thread_names(samples)

    report, _ = build_report(run_dir, header, milestones, intervals, names, ncpu, end_ms)
    (run_dir / "boot-profile-report.md").write_text(report, encoding="utf-8")
    if intervals or milestones:
        svg_timeline(run_dir / "boot-profile-timeline.svg", milestones, intervals, names, ncpu, end_ms)
    print(report)
    print(f"\nwrote: {run_dir/'boot-profile-report.md'}")
    print(f"wrote: {run_dir/'boot-profile-timeline.svg'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
