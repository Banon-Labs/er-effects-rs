#!/usr/bin/env python3
"""
workflow-steer.py -- generic, cooperative steering channel for the Claude Code
Workflow tool.

The workflow SCRIPT runs in a sandbox with no filesystem/network. It can only
observe live user input by spawning an agent() that reads an external channel
and returns its contents. This helper is that channel's read/consume side: a
reader-agent (between phases) or a worker-agent (before finalizing) shells out
to this script to fetch any pending user steering, and the script branches on
the returned text.

The USER is the writer. They steer transparently by dropping a plain-text file
into the control directory (or appending a directive). Claude only authored this
generic hook; it never sees or mediates the injected content -- the workflow
reads it directly at the next poll boundary.

Channel layout (all under a control dir, default: <repo>/.workflow-steer/):
  inbox/                 user drops steering files here (any name, *.txt/*.md/*.json)
  consumed/              this script atomically moves files here after reading (audit trail)
  STOP                   presence => emit a hard-stop directive (kill switch)
  scope=<name>.txt       optional per-phase / per-worker targeting (see --scope)

Usage (invoked BY a workflow agent, not by the user):
  python3 scripts/workflow-steer.py read            # read+consume all pending, print JSON
  python3 scripts/workflow-steer.py peek            # read WITHOUT consuming (idempotent poll)
  python3 scripts/workflow-steer.py read --scope refine-agent-3
  python3 scripts/workflow-steer.py wait --timeout 45 --interval 3   # bounded poll gate
  python3 scripts/workflow-steer.py status          # counts only

Output is a single JSON object on stdout so the calling agent can return it
verbatim into the script, e.g.:
  {"has_steer": true, "stop": false, "scope": null,
   "directives": [{"file": "inbox/redir.txt", "text": "..."}],
   "ts_monotonic": 12345.6}

Exit code is always 0 on a well-formed read (even with no steer) so the agent
can distinguish "channel empty" (has_steer=false) from "channel broken"
(non-zero exit / non-JSON). Time is os-level monotonic; the sandbox script
itself cannot call Date.now(), which is exactly why reading happens in an agent.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import time
import threading
from pathlib import Path



def bounded_poll_wait(seconds: float) -> None:
    """Bounded loop pacing; loop predicates still own readiness/stop decisions."""
    threading.Event().wait(min(max(float(seconds), 0.0), 30.0))

def _repo_root() -> Path:
    # This file lives in <repo>/scripts/. Root is its parent's parent.
    return Path(__file__).resolve().parent.parent


def _control_dir(explicit: str | None) -> Path:
    if explicit:
        return Path(explicit).expanduser().resolve()
    env = os.environ.get("WORKFLOW_STEER_DIR")
    if env:
        return Path(env).expanduser().resolve()
    return _repo_root() / ".workflow-steer"


def _ensure_layout(root: Path) -> None:
    (root / "inbox").mkdir(parents=True, exist_ok=True)
    (root / "consumed").mkdir(parents=True, exist_ok=True)


def _read_text(p: Path) -> str:
    try:
        return p.read_text(encoding="utf-8", errors="replace").strip()
    except OSError as e:
        return f"<<unreadable: {e}>>"


def _scope_match(name: str, scope: str | None) -> bool:
    """A file targets a scope if its name starts with 'scope=<scope>' OR carries
    no scope= prefix at all (broadcast). This lets a user aim a directive at one
    worker (scope=refine-agent-3.txt) or at everyone (redir.txt)."""
    if scope is None:
        return True  # reader wants everything
    if not name.startswith("scope="):
        return True  # broadcast file applies to every scope
    # name like: scope=refine-agent-3.txt
    tag = name[len("scope="):]
    for sep in (".", "_", "-"):
        if sep in tag:
            tag = tag.split(sep, 1)[0]
            break
    return tag == scope


def _collect(root: Path, scope: str | None):
    inbox = root / "inbox"
    stop = (root / "STOP").exists()
    directives = []
    if inbox.is_dir():
        for p in sorted(inbox.iterdir()):
            if not p.is_file():
                continue
            if p.name.startswith("."):
                continue
            if not _scope_match(p.name, scope):
                continue
            directives.append(p)
    return stop, directives


def _consume(root: Path, files) -> list[str]:
    consumed_names = []
    consumed_dir = root / "consumed"
    for p in files:
        # timestamp-prefix to avoid collision in the audit trail
        stamp = time.strftime("%Y%m%dT%H%M%S", time.gmtime())
        dest = consumed_dir / f"{stamp}.{p.name}"
        try:
            os.replace(p, dest)  # atomic within same filesystem
            consumed_names.append(dest.name)
        except OSError:
            # best-effort; leave in inbox so it is not lost
            pass
    return consumed_names


def _emit(obj) -> None:
    json.dump(obj, sys.stdout, ensure_ascii=False)
    sys.stdout.write("\n")
    sys.stdout.flush()


def _snapshot(root: Path, scope: str | None, consume: bool):
    stop, files = _collect(root, scope)
    directives = [
        {"file": str(p.relative_to(root)), "text": _read_text(p)} for p in files
    ]
    if consume and files:
        _consume(root, files)
    return {
        "has_steer": bool(stop or directives),
        "stop": stop,
        "scope": scope,
        "directives": directives,
        "control_dir": str(root),
        "ts_monotonic": time.monotonic(),
    }


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="cooperative workflow steering channel")
    ap.add_argument("mode", choices=["read", "peek", "wait", "status"])
    ap.add_argument("--dir", default=None, help="control dir (default .workflow-steer or $WORKFLOW_STEER_DIR)")
    ap.add_argument("--scope", default=None, help="only read files targeting this scope (worker/phase name)")
    ap.add_argument("--timeout", type=float, default=30.0, help="wait mode: max seconds to block")
    ap.add_argument("--interval", type=float, default=2.0, help="wait mode: poll interval seconds")
    args = ap.parse_args(argv)

    root = _control_dir(args.dir)
    _ensure_layout(root)

    if args.mode == "status":
        stop, files = _collect(root, args.scope)
        _emit({"pending": len(files), "stop": stop, "control_dir": str(root),
               "ts_monotonic": time.monotonic()})
        return 0

    if args.mode == "peek":
        _emit(_snapshot(root, args.scope, consume=False))
        return 0

    if args.mode == "read":
        _emit(_snapshot(root, args.scope, consume=True))
        return 0

    # wait: bounded poll gate. Returns as soon as steer appears, else on timeout.
    deadline = time.monotonic() + max(0.0, args.timeout)
    interval = max(0.25, args.interval)
    while True:
        snap = _snapshot(root, args.scope, consume=False)
        if snap["has_steer"]:
            # consume now that we are returning it
            _collect_and_consume = _snapshot(root, args.scope, consume=True)
            _collect_and_consume["waited"] = True
            _emit(_collect_and_consume)
            return 0
        if time.monotonic() >= deadline:
            snap["waited"] = True
            snap["timed_out"] = True
            _emit(snap)
            return 0
        bounded_poll_wait(interval)


if __name__ == "__main__":
    raise SystemExit(main())
