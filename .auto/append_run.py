#!/usr/bin/env python3
"""Append one autoresearch run entry to .auto/log.jsonl.

Reads METRIC k=v lines from a measure stdout file and records a run with the
given run number, commit, status, description, and ASI hypothesis/observed.
Kept deliberately small and re-runnable so the loop can record runs without
hand-editing JSON.
"""
from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
LOG = REPO / ".auto/log.jsonl"


def parse_metrics(measure_out: Path) -> dict:
    metrics: dict = {}
    for line in measure_out.read_text(encoding="utf-8", errors="replace").splitlines():
        if not line.startswith("METRIC "):
            continue
        body = line[len("METRIC "):]
        key, _, value = body.partition("=")
        key = key.strip()
        value = value.strip()
        try:
            metrics[key] = int(value)
        except ValueError:
            try:
                metrics[key] = float(value)
            except ValueError:
                metrics[key] = value
    return metrics


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", type=int, required=True)
    ap.add_argument("--commit", required=True)
    ap.add_argument("--status", required=True)
    ap.add_argument("--description", required=True)
    ap.add_argument("--metric", type=int, required=True)
    ap.add_argument("--measure-out", required=True)
    ap.add_argument("--hypothesis", required=True)
    ap.add_argument("--observed", required=True)
    ap.add_argument("--segment", type=int, default=6)
    args = ap.parse_args()

    metrics = parse_metrics(Path(args.measure_out))
    evidence = REPO / ".auto/last-measure/static-re-evidence.json"
    entry = {
        "run": args.run,
        "commit": args.commit,
        "metric": args.metric,
        "metrics": metrics,
        "status": args.status,
        "description": args.description,
        "timestamp": int(time.time() * 1000),
        "segment": args.segment,
        "confidence": None,
        "asi": {
            "hypothesis": args.hypothesis,
            "static_re_evidence_path": str(evidence),
            "observed": args.observed,
        },
    }
    with LOG.open("a", encoding="utf-8") as fh:
        fh.write(json.dumps(entry) + "\n")
    print(f"appended run {args.run} status={args.status} metric={args.metric}")


if __name__ == "__main__":
    main()
