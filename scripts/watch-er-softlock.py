#!/usr/bin/env python3
"""Watch er-effects runtime telemetry and self-report soft locks while the DLL runs.

This is a sidecar for agent/runtime proofs: start it before launching ME3/Elden Ring.
It samples telemetry quickly enough to catch the first bad branch instead of waiting for
an agent/tool poll after the game is already wedged.
"""

from __future__ import annotations

import argparse
import atexit
import ctypes
import json
import os
import select
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

DEFAULT_GAME_DIR = Path.home() / ".local/share/Steam/steamapps/common/ELDEN RING/Game"
DEFAULT_TELEMETRY = DEFAULT_GAME_DIR / "er-effects-telemetry.json"
REPO_ROOT = Path(__file__).resolve().parents[1]
TRIAGE_SCRIPT = REPO_ROOT / "scripts/triage-er-softlock.sh"


class TelemetryChangeWaiter:
    """Wake on telemetry directory writes, with a bounded poll interval as the safety backstop."""

    _IN_MODIFY = 0x0000_0002
    _IN_CLOSE_WRITE = 0x0000_0008
    _IN_MOVED_TO = 0x0000_0080
    _IN_CREATE = 0x0000_0100

    def __init__(self, telemetry: Path) -> None:
        libc = ctypes.CDLL(None, use_errno=True)
        libc.inotify_init1.argtypes = [ctypes.c_int]
        libc.inotify_init1.restype = ctypes.c_int
        libc.inotify_add_watch.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32]
        libc.inotify_add_watch.restype = ctypes.c_int

        self.fd = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
        if self.fd < 0:
            error = ctypes.get_errno()
            raise OSError(error, os.strerror(error))
        mask = self._IN_MODIFY | self._IN_CLOSE_WRITE | self._IN_MOVED_TO | self._IN_CREATE
        watch = libc.inotify_add_watch(self.fd, os.fsencode(telemetry.parent), mask)
        if watch < 0:
            error = ctypes.get_errno()
            os.close(self.fd)
            raise OSError(error, os.strerror(error), telemetry.parent)
        atexit.register(os.close, self.fd)

        self.poller = select.poll()
        self.poller.register(self.fd, select.POLLIN)

    def wait(self, max_interval_seconds: float) -> None:
        timeout_ms = max(1, min(int(max_interval_seconds * 1000), 30_000))
        if not self.poller.poll(timeout_ms):
            return
        while True:
            try:
                os.read(self.fd, 65_536)
            except BlockingIOError:
                return


KEYS = [
    "dll_hash_tag",
    "product_autoload_armed",
    "product_core_autoload_ticks",
    "product_core_ready_successes",
    "product_core_ready_blocker",
    "product_core_last_branch",
    "product_core_last_phase",
    "product_core_last_menu_opened_latch",
    "autoload_attempts",
    "autoload_commits",
    "oracle_load_game_fallback_calls",
    "oracle_load_game_fallback_last_item",
    "oracle_load_game_fallback_last_docall",
    "oracle_load_game_fallback_last_blocker",
    "oracle_load_game_fallback_stack_first_external_kind",
    "oracle_load_game_fallback_stack_first_external_label",
    "oracle_load_game_fallback_stack_first_external_name",
    "oracle_load_game_fallback_stack_first_external_base",
    "oracle_load_game_fallback_stack_first_external_offset",
    "oracle_load_game_fallback_stack_self_frames",
    "oracle_load_game_fallback_stack_ersc_frames",
    "oracle_load_game_fallback_stack_me3_frames",
    "oracle_load_game_fallback_stack_other_user_frames",
    "oracle_load_game_fallback_stack_game_frames",
    "oracle_own_stepper_s2_invoke_calls",
    "oracle_own_stepper_s2_invoke_last_item",
    "oracle_own_stepper_s2_invoke_last_functor",
    "oracle_own_stepper_s2_invoke_last_ctx10",
    "oracle_own_stepper_s2_invoke_last_pre130",
    "oracle_own_stepper_s2_invoke_last_update_ret",
    "oracle_own_stepper_s2_invoke_last_candidate",
    "oracle_own_stepper_s2_invoke_last_blocker",
    "oracle_native_submit_hits",
    "oracle_continue_phase",
    "oracle_player_present",
    "oracle_can_move",
    "oracle_msgbox_total_builds",
    "oracle_msgbox_any_seen",
    "oracle_boot_view_draw_hits",
    "oracle_boot_view_last_permille",
    "oracle_boot_view_milestone_idx",
    "oracle_boot_view_milestone_mask",
    "oracle_boot_view_self_presents",
    "oracle_boot_view_swapchain_found_ms",
    "oracle_boot_view_pump_stop_reason",
    "oracle_boot_view_pump_stop_ms",
    "oracle_present_hook_hits",
    "oracle_present_find_tries",
    "oracle_present_find_stage",
    "oracle_title_now_loading_helper_hooks_installed",
    "oracle_loading_bar_hook_installed",
    "oracle_loading_bar_update_hits",
    "oracle_loading_bar_progress_permille",
    "oracle_now_loading",
    "oracle_load_in_progress_b80",
    "oracle_menu_continue_candidate_stack_first_external_kind",
    "oracle_menu_continue_candidate_stack_first_external_label",
    "oracle_menu_continue_candidate_stack_first_external_name",
    "oracle_menu_continue_candidate_stack_first_external_base",
    "oracle_menu_continue_candidate_stack_first_external_offset",
    "oracle_menu_continue_candidate_stack_self_frames",
    "oracle_menu_continue_candidate_stack_ersc_frames",
    "oracle_menu_continue_candidate_stack_me3_frames",
    "oracle_menu_continue_candidate_stack_other_user_frames",
    "oracle_menu_continue_candidate_stack_game_frames",
]

BRANCH_LABELS = {
    0: "unseen",
    1: "wait_accept_byte",
    2: "wait_native_rows",
    3: "menu_rows_accepted",
    4: "profile_select_flow",
    5: "portrait_hold",
    6: "switch_old_world",
    7: "switch_handoff",
    8: "load_game_fallback_returned",
    9: "wait_continue_ready",
    10: "commit_continue",
}

S2_BLOCKER_LABELS = {
    0: "none",
    1: "no_item",
    2: "no_dialog_after_update",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--telemetry", type=Path, default=DEFAULT_TELEMETRY)
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--me3-log", type=Path)
    parser.add_argument("--pidfile", type=Path, help="Optional ME3 pidfile; watcher stops if PID exits")
    parser.add_argument("--sample-interval-ms", type=int, default=250)
    parser.add_argument("--stable-softlock-ms", type=int, default=3000)
    parser.add_argument("--max-seconds", type=float, default=90.0)
    parser.add_argument("--close-on-softlock", action="store_true")
    parser.add_argument("--once", action="store_true", help="Read one sample, classify, and exit")
    return parser.parse_args()


def read_pid(pidfile: Path | None) -> int | None:
    if not pidfile:
        return None
    try:
        text = pidfile.read_text(encoding="utf-8", errors="replace").strip()
    except FileNotFoundError:
        return None
    try:
        return int(text)
    except ValueError:
        return None


def pid_alive(pid: int | None) -> bool:
    if pid is None:
        return True
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def load_json(path: Path, min_mtime: float | None = None) -> dict[str, Any] | None:
    try:
        if min_mtime is not None and path.stat().st_mtime < min_mtime:
            return None
        return json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except (FileNotFoundError, json.JSONDecodeError, OSError):
        return None


def compact_sample(raw: dict[str, Any] | None, now_ms: int) -> dict[str, Any]:
    if raw is None:
        return {"t_ms": now_ms, "telemetry_missing": True}
    sample = {"t_ms": now_ms}
    for key in KEYS:
        if key in raw:
            sample[key] = raw[key]
    branch = sample.get("product_core_last_branch")
    if isinstance(branch, int):
        sample["product_core_last_branch_label"] = BRANCH_LABELS.get(branch, f"unknown_{branch}")
    s2 = sample.get("oracle_own_stepper_s2_invoke_last_blocker")
    if isinstance(s2, int):
        sample["oracle_own_stepper_s2_invoke_last_blocker_label"] = S2_BLOCKER_LABELS.get(
            s2, f"unknown_{s2}"
        )
    return sample


def terminal_success(sample: dict[str, Any]) -> bool:
    return bool(sample.get("oracle_player_present")) and bool(sample.get("oracle_can_move"))


def softlock_signature(sample: dict[str, Any]) -> tuple[Any, ...] | None:
    if sample.get("telemetry_missing"):
        return None
    if sample.get("oracle_msgbox_total_builds", 0) not in (0, None):
        return ("message_box", sample.get("oracle_msgbox_total_builds"))
    if terminal_success(sample):
        return None
    if not sample.get("product_autoload_armed", False):
        return None

    branch = sample.get("product_core_last_branch")
    phase = sample.get("product_core_last_phase")
    attempts = sample.get("autoload_attempts", 0)
    s2_blocker = sample.get("oracle_own_stepper_s2_invoke_last_blocker")
    fallback_calls = sample.get("oracle_load_game_fallback_calls", 0)

    if branch == 8 and phase in (2, 6) and fallback_calls and s2_blocker:
        return (
            "stage2_invoke_blocked",
            phase,
            s2_blocker,
            sample.get("oracle_own_stepper_s2_invoke_last_pre130"),
            sample.get("oracle_own_stepper_s2_invoke_last_ctx10"),
            sample.get("oracle_load_game_fallback_stack_first_external_label"),
            sample.get("oracle_load_game_fallback_stack_first_external_name"),
            sample.get("oracle_load_game_fallback_stack_ersc_frames"),
            sample.get("oracle_load_game_fallback_stack_other_user_frames"),
        )
    if branch in (1, 2, 9) and attempts == 0:
        return ("product_branch_wait", branch, sample.get("product_core_ready_blocker"), sample.get("product_core_last_menu_opened_latch"))
    if phase == 2 and s2_blocker:
        return ("stage2_invoke_blocked", phase, s2_blocker)
    return None


def write_report(path: Path, status: str, sample: dict[str, Any], signature: tuple[Any, ...] | None) -> None:
    lines = [
        f"status={status}",
        f"signature={signature!r}",
        f"product_core_last_branch={sample.get('product_core_last_branch')!r} ({sample.get('product_core_last_branch_label')})",
        f"product_core_last_phase={sample.get('product_core_last_phase')!r}",
        f"oracle_own_stepper_s2_invoke_last_blocker={sample.get('oracle_own_stepper_s2_invoke_last_blocker')!r} ({sample.get('oracle_own_stepper_s2_invoke_last_blocker_label')})",
        f"oracle_load_game_fallback_calls={sample.get('oracle_load_game_fallback_calls')!r}",
        f"oracle_load_game_fallback_stack_first_external={sample.get('oracle_load_game_fallback_stack_first_external_label')!r} name={sample.get('oracle_load_game_fallback_stack_first_external_name')!r} base={sample.get('oracle_load_game_fallback_stack_first_external_base')!r} offset={sample.get('oracle_load_game_fallback_stack_first_external_offset')!r}",
        f"oracle_load_game_fallback_stack_frames self={sample.get('oracle_load_game_fallback_stack_self_frames')!r} ersc={sample.get('oracle_load_game_fallback_stack_ersc_frames')!r} me3={sample.get('oracle_load_game_fallback_stack_me3_frames')!r} other_user={sample.get('oracle_load_game_fallback_stack_other_user_frames')!r} game={sample.get('oracle_load_game_fallback_stack_game_frames')!r}",
        f"oracle_menu_continue_candidate_stack_first_external={sample.get('oracle_menu_continue_candidate_stack_first_external_label')!r} name={sample.get('oracle_menu_continue_candidate_stack_first_external_name')!r} base={sample.get('oracle_menu_continue_candidate_stack_first_external_base')!r} offset={sample.get('oracle_menu_continue_candidate_stack_first_external_offset')!r}",
        f"oracle_menu_continue_candidate_stack_frames self={sample.get('oracle_menu_continue_candidate_stack_self_frames')!r} ersc={sample.get('oracle_menu_continue_candidate_stack_ersc_frames')!r} me3={sample.get('oracle_menu_continue_candidate_stack_me3_frames')!r} other_user={sample.get('oracle_menu_continue_candidate_stack_other_user_frames')!r} game={sample.get('oracle_menu_continue_candidate_stack_game_frames')!r}",
        f"oracle_player_present={sample.get('oracle_player_present')!r}",
        f"oracle_can_move={sample.get('oracle_can_move')!r}",
        f"oracle_boot_view_draw_hits={sample.get('oracle_boot_view_draw_hits')!r}",
        f"oracle_boot_view_pump_stop_reason={sample.get('oracle_boot_view_pump_stop_reason')!r}",
        f"oracle_present_find_tries={sample.get('oracle_present_find_tries')!r}",
    ]
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def run_triage(args: argparse.Namespace) -> None:
    cmd = [str(TRIAGE_SCRIPT), "--close", "--artifact-dir", str(args.artifact_dir / "triage-close")]
    if args.me3_log:
        cmd.extend(["--me3-log", str(args.me3_log)])
    cmd.extend(["--telemetry", str(args.telemetry)])
    with (args.artifact_dir / "triage-close.log").open("w", encoding="utf-8", errors="replace") as out:
        subprocess.run(
            cmd,
            cwd=REPO_ROOT,
            stdout=out,
            stderr=subprocess.STDOUT,
            check=False,
            timeout=30,
        )


def main() -> int:
    args = parse_args()
    args.artifact_dir.mkdir(parents=True, exist_ok=True)
    timeline = args.artifact_dir / "softlock-watch.jsonl"
    report = args.artifact_dir / "softlock-watch-report.txt"

    start_wall = time.time()
    start = time.monotonic()
    stable_sig: tuple[Any, ...] | None = None
    stable_since: float | None = None
    last_sample: dict[str, Any] = {"telemetry_missing": True}
    interval = max(args.sample_interval_ms, 50) / 1000.0
    pid = read_pid(args.pidfile)
    telemetry_changes = TelemetryChangeWaiter(args.telemetry)

    with timeline.open("a", encoding="utf-8") as out:
        while True:
            now = time.monotonic()
            now_ms = int((now - start) * 1000)
            if pid is None:
                pid = read_pid(args.pidfile)
            raw = load_json(args.telemetry, min_mtime=start_wall)
            sample = compact_sample(raw, now_ms)
            last_sample = sample
            out.write(json.dumps(sample, sort_keys=True) + "\n")
            out.flush()

            sig = softlock_signature(sample)
            if terminal_success(sample):
                write_report(report, "success", sample, None)
                return 0
            if sig is not None and sig == stable_sig:
                if stable_since is not None and (now - stable_since) * 1000 >= args.stable_softlock_ms:
                    write_report(report, "softlock", sample, sig)
                    if args.close_on_softlock:
                        run_triage(args)
                    return 2
            else:
                stable_sig = sig
                stable_since = now if sig is not None else None

            if args.once:
                write_report(report, "once", sample, sig)
                return 0
            if (now - start) >= args.max_seconds:
                write_report(report, "timeout", sample, sig)
                return 3
            if pid is not None and not pid_alive(pid):
                write_report(report, "process-exited", sample, sig)
                return 4
            telemetry_changes.wait(interval)


if __name__ == "__main__":
    raise SystemExit(main())
