#!/usr/bin/env python3
"""Offline load1-vs-load2(+) semaphore comparator for er-effects-input-trace.jsonl.

Reuses capture-samechar-3x.py's epoch/checkpoint helpers, but adds the SIDE-BY-SIDE
ordered MoveMapStep/checkpoint sequences per load epoch -- the view that actually shows
*where* a reload's bootup sequence diverges from the boot baseline (the built-in
write_semaphore_diff only reports the first-divergent checkpoint + timing deltas).

Usage:
  python3 scripts/loadcmp-diff.py <trace.jsonl> [--full-diff]

`<trace.jsonl>` is an er-effects-input-trace.jsonl (or load-semaphore-trace.jsonl) with
{"t":"sem",...} rows carrying load_epoch. --full-diff also writes the standard
write_semaphore_diff artifacts next to the trace.

This is the reusable form of the 2026-07-19 analysis (bd er-effects-rs-9fmm). It NEVER
launches or reads the game; pure offline analysis of an existing trace.
"""
from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

_CAP = Path(__file__).with_name("capture-samechar-3x.py")


def _load_helpers():
    spec = importlib.util.spec_from_file_location("cap3x", _CAP)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load helpers from {_CAP}")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _fmt_cp(cp: dict) -> str:
    return (
        f"+{cp['rel_shared_ms']:>7}ms seq{cp['seq']:>4} {cp['name']:<28} "
        f"ig={cp['ig_pstep']}/{cp['ig_pnext']} d8={cp['ig_d8']} mms={cp['mms_step']} "
        f"gate={cp['mms_gate_lo']}/{cp['mms_gate_hi']} bar={cp['bar_frame']}/{cp['bar_progress_permille']} "
        f"player={cp['player']} wcm={cp['world_chr_man']} mp={cp['main_player']} can_move={cp['can_move']}"
    )


def main(argv: list[str]) -> int:
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__)
        return 2
    trace = Path(argv[0])
    full_diff = "--full-diff" in argv[1:]
    if not trace.exists():
        print(f"loadcmp-diff: trace not found: {trace}", file=sys.stderr)
        return 2

    m = _load_helpers()
    # capture-samechar reads either artifact_dir/load-semaphore-trace.jsonl or
    # game_dir/er-effects-input-trace.jsonl; point both at the trace's own directory
    # after normalizing its name so _load_semaphore_rows finds it.
    art = trace.parent
    if trace.name not in ("load-semaphore-trace.jsonl", "er-effects-input-trace.jsonl"):
        alias = art / "load-semaphore-trace.jsonl"
        if not alias.exists():
            alias.write_bytes(trace.read_bytes())
    rows = m._load_semaphore_rows(art, art)
    if not rows:
        print("loadcmp-diff: no {'t':'sem'} rows with load_epoch found", file=sys.stderr)
        return 1

    epochs = sorted({m.as_int(r.get("load_epoch"), -1) for r in rows if m.as_int(r.get("load_epoch"), -1) >= 0})
    names = {0: "load1 (boot autoload)", 1: "load2 (reload)", 2: "load3", 3: "load4"}
    print(f"# load comparison — {trace}")
    print(f"epochs: {epochs}\n")

    boot_names: list[str] = []
    for ep in epochs:
        ev = m._epoch_shared_rows(rows, ep)
        cps = m._ordered_checkpoints(ev)
        mms_seq = [f"{c['mms_step']}:{c['name'].split(':', 2)[-1]}" for c in cps if c["name"].startswith("mms_step:")]
        print(f"===== epoch {ep}: {names.get(ep, 'load')} — {len(ev)} shared rows, {len(cps)} checkpoints =====")
        print(f"  MoveMapStep sequence: {' -> '.join(mms_seq) or '(none)'}")
        if ep == 0:
            boot_names = [c["name"] for c in cps]
        elif boot_names:
            reload_names = [c["name"] for c in cps]
            missing = [n for n in boot_names if n not in reload_names]
            extra = [n for n in reload_names if n not in boot_names]
            print(f"  vs boot -> missing: {missing[:12]}")
            print(f"  vs boot -> extra  : {extra[:12]}")
        for cp in cps:
            print("  " + _fmt_cp(cp))
        print()

    if full_diff:
        jp, mdp = m.write_semaphore_diff(art, art)
        print(f"[--full-diff] wrote {jp} and {mdp}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
