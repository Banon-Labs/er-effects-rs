#!/usr/bin/env python3
"""LEAN, DETERMINISTIC prompt-teardown watcher for the warm-title-rebuild A/B.

Replaces the `capture-samechar-3x.py --observe-only --observe-seconds 300` ride (which tears down 45s
AFTER settled world, so each arm is 4-5 min and lingers -- bd teardown-must-be-prompt-scoped-to-decisive-
oracle-no-long-waits-2026-07-24). This watcher RETURNS PROMPTLY the instant THIS test's DECISIVE oracle is
captured, letting run-vanilla-reload-agentdriven.sh's PID-scoped `cleanup()` EXIT trap do the kill. It
NEVER kills the game itself and NEVER blanket-kills.

THE DECISIVE ORACLE (bd STEP4-BLOCKER / decoupled-diagnostics-architecture-buildplan-2026-07-24):
`dialog+0xb78` SceneObjProxy binding at the WARM switch title, right after the first quit-to-title -- BEFORE
load2-to-world. So teardown fires there, not after reaching settled world.

WARM-title vs BOOT-title (the correctness crux -- must NOT tear down on boot-title samples):
Two RE'd, independently-derived quit signals are REQUIRED together before any title verdict fires (defense
in depth against a boot-title false-positive; either alone is sufficient in principle, but a title verdict
never fires unless BOTH agree):
  1. TELEMETRY quit signal -- a title-binding line with `epoch >= 1`. `standalone_tick`
     (er-telemetry/src/lib.rs:525) feeds `play_time_ms = -1` while GameDataMan is null (boot title, no
     character loaded), so the load-epoch counter (read/epoch.rs) stays 0 at the boot title and only
     increments once the world has simulated. A title reached with `epoch >= 1` therefore had a prior
     in-world session + a quit -- i.e. it is the WARM title. The boot title is always epoch 0.
  2. HARNESS quit signal -- a `native_quit` / `quit` / `quit_teardown` phase reaching `outcome:"advanced"`
     in `er-input-harness-phases.jsonl` (or the matching `... ADVANCED` line in `er-input-harness.log`).
     drive.rs advances those phases only when `world_sim` goes false (the native return-to-title teardown).
     The harness force-drives the quit in both the armed and disarmed arms, so this is present in both.

DECISIVE CONDITIONS (first to hold -> prompt return; polled ~every --poll-seconds):
  (a) GAME_EXITED       -- eldenring.exe seen alive then gone (debounced --exit-debounce checks).
  (b) BOUND             -- at the WARM title, title-binding `proxy_handle_nonnull=true` held for
                           --settle-seconds (the native reload bound the proxy). ALSO BOUND: stream-overlap
                           reaches a reload epoch (> warm epoch) with `player_movable=true` held for
                           --settle-seconds (the reload reached world).
  (c) DEADLOCK_NOT_BOUND-- at the WARM title, title-binding `proxy_handle_nonnull=false` SUSTAINED for
                           --deadlock-seconds with NO progress to a reload epoch (the hypothesized armed
                           product hold that keeps the warm title unbound).
  (d) CAP               -- --max-seconds backstop (a SMALL net, e.g. 180; NOT the 300s runtime cap).

On return: copy er-oracle-title-binding.jsonl + er-oracle-stream-overlap.jsonl into --artifact-dir and print
a one-line VERDICT. Exit 0 for every real verdict (so an A/B `|| true` caller keeps going); exit 2 only on
a usage/setup error. stdlib only; py_compile-clean.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
import threading

TITLE_FILE = "er-oracle-title-binding.jsonl"
STREAM_FILE = "er-oracle-stream-overlap.jsonl"
DIALOG_FILE = "er-oracle-dialog-active.jsonl"
HARNESS_PHASES_FILE = "er-input-harness-phases.jsonl"
# Epoch 0 = boot load1; a reload is epoch >= 1 (the DIP feature only cares about reload epochs).
FIRST_RELOAD_EPOCH = 1
HARNESS_LOG_FILE = "er-input-harness.log"

# drive.rs quit-to-title phases: advancing any of these means the native world teardown completed.
QUIT_PHASE_NAMES = ("quit_teardown", "native_quit", "quit")



def bounded_poll_wait(seconds: float) -> None:
    """Bounded loop pacing; loop predicates still own readiness/stop decisions."""
    threading.Event().wait(min(max(float(seconds), 0.0), 30.0))

def read_jsonl(path: str) -> list[dict]:
    """Parse a JSONL file, skipping blank/partial lines. Missing file -> []."""
    rows: list[dict] = []
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    rows.append(json.loads(line))
                except json.JSONDecodeError:
                    continue  # a line the DLL is mid-append on; picked up next poll
    except FileNotFoundError:
        return []
    except OSError:
        return []
    return rows


def harness_quit_seen(game_dir: str) -> bool:
    """True once the harness has driven a quit-to-title to completion (RE signal #2).

    Primary source: the self-describing per-phase JSONL. Fallback: the human log. Both are truncated on
    harness DLL attach, so a hit is always from THIS run.
    """
    for row in read_jsonl(os.path.join(game_dir, HARNESS_PHASES_FILE)):
        if row.get("phase") in QUIT_PHASE_NAMES and row.get("outcome") == "advanced":
            return True
    log_path = os.path.join(game_dir, HARNESS_LOG_FILE)
    try:
        with open(log_path, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                if "ADVANCED" not in line:
                    continue
                # log shape: "[NNNNNN +Tms] phase[idx] <name> ADVANCED after ..."
                if (
                    "native_quit" in line
                    or "quit_teardown" in line
                    or "] quit ADVANCED" in line
                ):
                    return True
    except (FileNotFoundError, OSError):
        pass
    return False


def game_running() -> bool | None:
    """True/False if eldenring.exe is/ isn't running; None if it cannot be determined (no tasklist.exe)."""
    try:
        out = subprocess.run(
            ["tasklist.exe", "/FI", "IMAGENAME eq eldenring.exe", "/FO", "CSV", "/NH"],
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (FileNotFoundError, OSError, subprocess.TimeoutExpired):
        return None
    return "eldenring.exe" in out.stdout.lower()


def _int_epoch(row: dict):
    e = row.get("epoch")
    return e if isinstance(e, int) else None


def copy_artifacts(game_dir: str, artifact_dir: str) -> None:
    os.makedirs(artifact_dir, exist_ok=True)
    for name in (TITLE_FILE, STREAM_FILE, DIALOG_FILE):
        src = os.path.join(game_dir, name)
        if os.path.exists(src):
            try:
                shutil.copy2(src, os.path.join(artifact_dir, name))
            except OSError:
                pass


def main() -> int:
    ap = argparse.ArgumentParser(description="Prompt-teardown watcher for the warm-title-rebuild A/B.")
    ap.add_argument("--game-dir", required=True, help="ELDEN RING Game dir (where the oracle/harness files land).")
    ap.add_argument("--artifact-dir", required=True, help="Where to copy the oracle streams + write the verdict.")
    ap.add_argument("--max-seconds", type=float, default=180.0, help="SMALL backstop (NOT the 300s runtime cap).")
    ap.add_argument("--settle-seconds", type=float, default=3.0, help="How long the BOUND condition must hold.")
    ap.add_argument("--deadlock-seconds", type=float, default=8.0, help="How long an unbound warm title = deadlock.")
    ap.add_argument("--poll-seconds", type=float, default=0.75, help="Poll cadence.")
    ap.add_argument("--exit-debounce", type=int, default=4, help="Consecutive 'gone' checks before GAME_EXITED.")
    ap.add_argument("--no-process-check", action="store_true", help="Skip tasklist.exe (dry-run/fixture mode).")
    ap.add_argument("--msgbox-stuck-seconds", type=float, default=15.0, help="A blocking dialog on screen this long with no title/reload progress = MSGBOX_BLOCKED (fast teardown, not the 180s backstop).")
    ap.add_argument("--no-dialog-check", action="store_true", help="Skip the dialog_active msgbox-stuck check (dry-run/fixture mode).")
    ap.add_argument(
        "--feature",
        choices=("title-binding", "dip"),
        default="title-binding",
        help=(
            "Which feature's capture is the ONLY auto-teardown (besides the backstop + game-already-exited). "
            "'dip' = tear down when a RELOAD epoch reaches player_movable and capture the stream-overlap "
            "result; leave EVERY other state (autoload stall, boot dialog) ALIVE for the user to inspect + "
            "kill (bd teardown-model-only-feature-under-test-plus-backstop-user-kills-rest)."
        ),
    )
    args = ap.parse_args()

    if not os.path.isdir(args.game_dir):
        print(f"wait-title-rebuild-teardown: --game-dir not a directory: {args.game_dir}", file=sys.stderr)
        return 2

    title_path = os.path.join(args.game_dir, TITLE_FILE)
    stream_path = os.path.join(args.game_dir, STREAM_FILE)

    start = time.monotonic()
    seen_alive = False
    gone_count = 0

    warm_confirmed_at: float | None = None  # wall time the WARM title was first confirmed (both signals)
    warm_epoch: int | None = None           # epoch of the warm-title samples (>=1)
    proxy_true_since: float | None = None    # wall time proxy became (and stayed) bound
    proxy_ever_true = False                   # has the warm-title proxy bound at any point?
    reload_epoch_since: float | None = None  # wall time a reload epoch (> warm) first appeared
    reload_movable_ever = False              # did the player become movable at a reload epoch?
    msgbox_active_since: float | None = None # wall time a blocking dialog first appeared (and stayed) up
    dip_movable_since: float | None = None   # wall time a RELOAD epoch first reached player_movable (dip)

    verdict: str | None = None
    reason = ""

    while True:
        now = time.monotonic()
        elapsed = now - start

        if elapsed >= args.max_seconds:
            verdict = "CAP"
            reason = (
                f"backstop {args.max_seconds:.0f}s reached "
                f"(warm_title_confirmed={warm_confirmed_at is not None})"
            )
            break

        # (a) PROCESS EXIT -- deterministic, arm-independent.
        if not args.no_process_check:
            alive = game_running()
            if alive is True:
                seen_alive = True
                gone_count = 0
            elif alive is False and seen_alive:
                gone_count += 1
                if gone_count >= args.exit_debounce:
                    verdict = "GAME_EXITED"
                    reason = f"eldenring.exe seen alive then gone ({gone_count} consecutive checks)"
                    break
            # alive is None -> can't determine; do not count toward exit.

        # DIP feature-capture (bd teardown-model-only-feature-under-test-plus-backstop-user-kills-rest): the
        # ONLY auto-teardown besides the backstop + game-exit. Tear down when a RELOAD epoch reaches
        # player_movable (the moment the WorldResWait fix targets), held --settle-seconds, capturing the
        # overlap RESULT there (overlap->0 = fix settled streaming before movable; overlap present = it did
        # not). EVERY other state (autoload stall, boot dialog, wrong state) is left ALIVE for the user.
        if args.feature == "dip":
            srows = read_jsonl(stream_path)
            reload_movable = [
                r
                for r in srows
                if isinstance(r.get("epoch"), int)
                and r["epoch"] >= FIRST_RELOAD_EPOCH
                and r.get("player_movable") is True
            ]
            if reload_movable:
                if dip_movable_since is None:
                    dip_movable_since = now
                if (now - dip_movable_since) >= args.settle_seconds:
                    ov = sum(1 for r in reload_movable if r.get("overlap") is True)
                    ep = min(r["epoch"] for r in reload_movable)
                    worked = ov == 0
                    verdict = "DIP_CAPTURED"
                    reason = (
                        f"reload epoch {ep} player_movable held {args.settle_seconds:.1f}s; overlap "
                        f"{ov}/{len(reload_movable)} movable samples ("
                        + ("FIX WORKED: settled before movable" if worked else "FIX DID NOT settle: still overlapping")
                        + ")"
                    )
                    break
            else:
                dip_movable_since = None
            bounded_poll_wait(args.poll_seconds)
            continue

        # (a2) MSGBOX BLOCKED -- a boot/title dialog (session-warning / save-retry) stuck on screen that the
        # harness never dismissed. The dialog_active oracle reads the live dialog regardless of build timing
        # (bd msgbox-oracle-false-negative-boot-session-dialog-masked-by-cover); a sustained msgbox_active
        # with no title/reload progress = a blocked boot -> tear down FAST, do not ride the 180s backstop.
        if not args.no_dialog_check:
            dialog_rows = read_jsonl(os.path.join(args.game_dir, DIALOG_FILE))
            msgbox_now = bool(dialog_rows) and dialog_rows[-1].get("msgbox_active") is True
            if msgbox_now:
                if msgbox_active_since is None:
                    msgbox_active_since = now
            else:
                msgbox_active_since = None
            if (
                msgbox_active_since is not None
                and (now - msgbox_active_since) >= args.msgbox_stuck_seconds
                and not proxy_ever_true
                and reload_epoch_since is None
            ):
                verdict = "MSGBOX_BLOCKED"
                reason = (
                    f"a dialog (msgbox_active) stuck on screen {args.msgbox_stuck_seconds:.0f}s with no "
                    "title/reload progress -- harness never dismissed it"
                )
                break

        # WARM-title gate: BOTH RE'd quit signals must agree before any title verdict.
        title_rows = read_jsonl(title_path)
        warm_rows = [r for r in title_rows if (_int_epoch(r) or 0) >= 1]
        quit_seen = harness_quit_seen(args.game_dir)

        if warm_rows and quit_seen:
            if warm_confirmed_at is None:
                warm_confirmed_at = now
                warm_epoch = min(e for e in (_int_epoch(r) for r in warm_rows) if e is not None)

            # Latest warm-title proxy state (the decisive dialog+0xb78 binding).
            latest = warm_rows[-1]
            proxy_bound = latest.get("proxy_handle_nonnull") is True
            if proxy_bound:
                proxy_ever_true = True
                if proxy_true_since is None:
                    proxy_true_since = now
            else:
                proxy_true_since = None  # a transient during rebuild resets the hold

            # Reload progress / reached-world (the alternate BOUND + the deadlock canceller).
            stream_rows = read_jsonl(stream_path)
            reload_epoch_present = any(
                (_int_epoch(r) or 0) > (warm_epoch or 0) for r in stream_rows
            )
            if reload_epoch_present and reload_epoch_since is None:
                reload_epoch_since = now
            if any(
                (_int_epoch(r) or 0) > (warm_epoch or 0) and r.get("player_movable") is True
                for r in stream_rows
            ):
                reload_movable_ever = True

            # (b) BOUND via proxy: dialog+0xb78 bound and held.
            if proxy_true_since is not None and (now - proxy_true_since) >= args.settle_seconds:
                verdict = "BOUND"
                reason = (
                    f"warm-title proxy_handle_nonnull=true held {args.settle_seconds:.1f}s "
                    f"(epoch {warm_epoch})"
                )
                break

            # (b) BOUND via reached-world: the reload progressed past the warm title into a movable world.
            if (
                reload_epoch_since is not None
                and reload_movable_ever
                and (now - reload_epoch_since) >= args.settle_seconds
            ):
                verdict = "BOUND"
                reason = (
                    f"reload reached world: stream-overlap player_movable=true at epoch>{warm_epoch} "
                    f"held {args.settle_seconds:.1f}s"
                )
                break

            # (c) DEADLOCK: warm title never bound and no reload progress for the deadlock window.
            if (
                not proxy_ever_true
                and not reload_epoch_present
                and (now - warm_confirmed_at) >= args.deadlock_seconds
            ):
                verdict = "DEADLOCK_NOT_BOUND"
                reason = (
                    f"warm-title proxy_handle_nonnull=false sustained {args.deadlock_seconds:.1f}s, "
                    f"no reload epoch progress (epoch {warm_epoch})"
                )
                break

        bounded_poll_wait(args.poll_seconds)

    copy_artifacts(args.game_dir, args.artifact_dir)
    elapsed = time.monotonic() - start
    print(f"VERDICT={verdict} reason=\"{reason}\" elapsed={elapsed:.1f}s")
    # Persist the verdict alongside the artifacts for the A/B analyzer / next agent.
    try:
        os.makedirs(args.artifact_dir, exist_ok=True)
        with open(os.path.join(args.artifact_dir, "title-rebuild-teardown-verdict.txt"), "w", encoding="utf-8") as fh:
            fh.write(f"VERDICT={verdict}\nreason={reason}\nelapsed_seconds={elapsed:.1f}\n")
    except OSError:
        pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
