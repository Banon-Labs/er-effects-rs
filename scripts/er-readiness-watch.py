#!/usr/bin/env python3
"""Watch an Elden Ring launch until DLL telemetry is ready or a structured failure is known.

This helper uses process/window/bootstrap/telemetry observations first, but every
runtime watch is also hard-bounded by --max-runtime-seconds, capped at the canonical
runtime-probe cap (the single source of truth in .auto/runtime_timeout_cap_seconds),
so a missing DLL telemetry stream cannot strand Elden Ring on-screen indefinitely.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

from runtime_timeout_cap import runtime_timeout_cap_seconds

DEFAULT_RUNTIME_PROCESS_PATTERN = r"(?:^|[/\\])(eldenring\.exe|start_protected_game\.exe)(?:\s|$)"
DEFAULT_WINDOW_CLASS = "steam_app_1245620"
DEFAULT_SPAWN_POLL_BUDGET = 4096
# 16384, not 8192: the fast readiness poll loop (~hundreds/s) exhausted 8192 polls at ~29s -- BEFORE
# the 45s time deadline -- so a world that finished loading at ~27s could not accumulate its 5s
# world-stable dwell before the budget ran out (ready=False with a fully-loaded character). The poll
# budget is only a runaway backstop; the real bound is --max-runtime-seconds (the time deadline still
# binds first at 45s). Raising the COUNT lets the time cap bind -- it does NOT raise the runtime cap.
DEFAULT_READINESS_POLL_BUDGET = 16384
DEFAULT_WINDOW_STALE_POLL_BUDGET = 4096
DEFAULT_AUTOLOAD_ATTEMPT_BUDGET = 300
DEFAULT_POST_REQUEST_TICK_BUDGET = 300
# Single source of truth for the runtime-probe wall-clock cap: .auto/runtime_timeout_cap_seconds,
# read through the shared scripts/runtime_timeout_cap.py helper (same reader the contract checker
# uses). bash (run-product-continue-direct-probe.sh, runtime_probe.sh) reads the same file and
# passes the value through as --max-runtime-seconds; the rego policy is kept in sync by the checker.
MAX_ALLOWED_RUNTIME_SECONDS = float(runtime_timeout_cap_seconds())
DEFAULT_MAX_RUNTIME_SECONDS = MAX_ALLOWED_RUNTIME_SECONDS
DEFAULT_WORLD_STABLE_DWELL_SECONDS = 5.0
OBSERVATION_SUBPROCESS_TIMEOUT_SECONDS = 5.0
SUCCESS_RC = 0
FAILURE_RC = 1
TARGET_GAME_MAN = "game-man"
TARGET_MODULE_BASE = "module-base"
TARGET_AUTOLOAD_REQUEST = "autoload-request"
TARGET_REQUEST_CONSUMPTION = "request-consumption"
TARGET_PLAYER_LOAD = "player-load"
TARGET_WORLD_STABLE = "world-stable"
READY_REASON = "game_man_telemetry_ready"
COLD_CHAR_MOUNT_COMPLETE = "cold_char_mount_complete"
COLD_CHAR_MOUNT_PHASE_DONE = 5  # cold_char_mount_drive MOUNT_PHASE PHASE_DONE, published as phase+1
MODULE_BASE_READY = "runtime_module_base_observed"
WORLD_STABLE = "world_stable"
RUNTIME_EXE_NAME = "eldenring.exe"
WINDOW_WITHOUT_BOOTSTRAP = "window_without_bootstrap_marker"
WINDOW_WITHOUT_TASK = "window_without_game_task_ready"
WINDOW_WITHOUT_TELEMETRY = "window_without_valid_telemetry"
TELEMETRY_WITHOUT_GAME_MAN = "telemetry_without_game_man"
AUTOLOAD_REQUESTED = "autoload_requested"
TITLE_HANDOFF_COMPLETE = "title_handoff_complete"
PLAYER_AVAILABLE = "player_available"
AUTOLOAD_ATTEMPT_BUDGET_REACHED = "autoload_attempt_budget_reached"
POST_REQUEST_TICK_BUDGET_REACHED = "post_request_tick_budget_reached"
PLAYER_LOAD_TICK_BUDGET_REACHED = "player_load_tick_budget_reached"
AUTOLOAD_SLOT_MISSING = "autoload_slot_missing"
PROCESS_EXITED = "process_exited_before_ready"
SPAWN_BUDGET_EXHAUSTED = "runtime_process_not_observed_within_spawn_poll_budget"
READINESS_BUDGET_EXHAUSTED = "readiness_poll_budget_exhausted"
TIMEOUT_BUDGET_EXHAUSTED = "timeout_seconds_budget_exhausted"
GAME_FPS_BELOW_MIN = "game_fps_below_min"
WORLD_STREAM_STALLED = "world_stream_stalled"
TITLE_STALLED = "title_stalled"
CONTINUE_STALLED = "continue_stalled"
WORLD_LOAD_DEADLINE_EXCEEDED = "world_load_deadline_exceeded"
# Fail-fast world-load deadline: once the first telemetry read lands, the world-loaded semaphore
# (player present / world-stable) must be reached within this many seconds of the TRUE bash launch
# epoch. A GOLDEN run where the user navigated the native menu SLOWLY reached the map-mount in ~24s
# from bash launch, so any menu-free automated run that cannot hit world-loaded comfortably under
# that is failing and should bail well below the 120s runtime cap. Default sits above the 24s golden
# baseline but far below the cap; defeatable with --no-world-load-deadline. Complementary to the 6s
# world_stream_stalled semaphore (which catches a flat post-map-load stall even earlier).
DEFAULT_WORLD_LOAD_DEADLINE_SECONDS = 30.0
# Milestone names (each a delta-from-launch in the timing dict). Detected from these oracle/telemetry
# fields, recorded only on the first transition:
#   t_launch          -> 0.0 by construction (the bash launch epoch)
#   t_first_telemetry -> first poll the telemetry JSON is readable (telemetry_present)
#   t_continue_fired  -> telemetry["oracle_own_load_continue_fired"] is True
#   t_player_present  -> telemetry["oracle_player_present"] is True
#   t_world_stable    -> the existing world-stable target reached (telemetry_world_loaded + dwell)
#   t_teardown        -> the watch exits (any reason)
TIMING_MILESTONES = (
    "t_launch",
    "t_first_telemetry",
    "t_continue_fired",
    "t_player_present",
    "t_world_stable",
    "t_teardown",
)
# World-stream stall semaphore: on the menu-free OWN-LOAD path the front half (continue fired +
# player map block registered) can succeed while the player map block never actually streams --
# per-block phase pinned at 2, zero IO inflight, player never present -- held flat to the wall-clock
# cap. A healthy stream advances within ~2-5s (io_inflight goes non-zero, wbr_max_phase climbs past
# 2, the m28 dispatch counter increments, player_present flips true). We track a monotonic progress
# watermark and, once map-load has begun, fail fast if it does not improve within this window. 3s of
# NO movement (flat watermark) is sufficient to call it stalled -- just above a healthy ~2s golden
# stream, and the watermark resets on ANY forward progress so a working-but-slightly-plateauing stream
# is not false-tripped. Tunable via --world-stream-stall-seconds.
DEFAULT_WORLD_STREAM_STALL_SECONDS = 3.0
# PER-PHASE PROGRESS WATCHDOG: generalizes the tail-stage world-stream stall detector into a
# per-phase rule -- every phase of the menu-free load pipeline (boot->title, title->continue,
# continue->map-load, map-load->world) must expose a monotonic progress signal, and the ACTIVE
# phase must advance that signal within this many seconds or the watchdog fails fast NAMING which
# phase wedged. Distinguishes "inherent slow but PROGRESSING" (e.g. the ~15s ER boot-to-title and
# the ~10.7s title_boot_ready wait both keep game_task_ticks incrementing every frame, so the
# watermark resets and never trips) from "wedged" (a true freeze: ticks flat AND no scan/state/
# milestone advance for >3s). Same flat-watermark + reset-on-progress contract as
# world_stream_stall_step, applied per phase. Tunable via --phase-stall-seconds; off with
# --no-phase-watchdog. The world_stream phase keeps its own --world-stream-stall-seconds window.
DEFAULT_PHASE_STALL_SECONDS = 3.0
# FPS semaphore: the game-task tick counter advances ~once per rendered frame, so its rate is an
# fps proxy. Elden Ring hard-throttles an UNFOCUSED window to a few fps; at that rate the title
# never boots within the runtime cap and the probe silently burns the whole budget producing a
# non-representative run. Treat a sustained sub-30-fps game as a crash-class failure and fail fast.
MIN_GAME_FPS = 30.0
FPS_STALL_WINDOW_SECONDS = 6.0
FPS_JUDGE_MIN_TICKS = 120
VISUAL_LOADING_SCREEN_DETECTED = "visual_loading_screen_detected"
LEGAL_POPUP_DETECTED = "visual_legal_popup_detected"
NATIVE_LEGAL_POPUP_DETECTED = "native_legal_popup_detected"
SAVE_DATA_POPUP_DETECTED = "visual_save_data_popup_detected"
MESSAGEBOX_DIALOG_DETECTED = "native_messagebox_dialog_detected"
SERVER_STATUS_SEMAPHORE_DETECTED = "native_server_status_semaphore_detected"
TARGET_WINDOW_CAPTURE_UNSAFE = "target_window_capture_unsafe"
VISUAL_CHECK_SUBPROCESS_TIMEOUT_SECONDS = 10.0
VISUAL_OCR_PREVIEW_CHARS = 1000
LEGAL_POPUP_CHECK_INTERVAL_SECONDS = 0.75
SAVE_DATA_POPUP_CHECK_INTERVAL_SECONDS = 0.75
RUNTIME_MODE_ANY = "any"
RUNTIME_MODE_VANILLA = "vanilla"
RUNTIME_MODE_SEAMLESS = "seamless"
RUNTIME_MODE_UNKNOWN = "unknown"
RUNTIME_MODE_MISMATCH = "runtime_mode_mismatch"
SEAMLESS_MODULE_MARKERS = ("ersc.dll", "/seamlesscoop/", "\\seamlesscoop\\")
LOADING_SCREEN_OCR_PATTERNS = [
    re.compile(r"\bcritical hits?\b", re.I),
    re.compile(r"\bcritical hit\b", re.I),
    re.compile(r"\bwhen near\b", re.I),
    re.compile(r"\busing torches\b", re.I),
    re.compile(r"\braise your torch\b", re.I),
    re.compile(r"\bnext\b", re.I),
]
VISUAL_OCR_CROPS = [
    ("whole", "640x360+0+0"),
    ("lower_left_tips", "520x220+0+110"),
    ("tip_text", "420x180+0+140"),
]
LEGAL_POPUP_TEXT_PATTERNS = [
    r"\bend[- ]user license\b",
    r"\bsoftware license\b",
    r"\blicen[cs]e agreement\b",
    r"\beula\b",
    r"\bterms of (?:use|service)\b",
    r"\bprivacy policy\b",
    r"\buser agreement\b",
]
LEGAL_POPUP_OCR_PATTERNS = [re.compile(pattern, re.I) for pattern in LEGAL_POPUP_TEXT_PATTERNS]
# Asset-backed from msg/engus/menu.msgbnd.dcx -> ToS_win64.fmg. These are the
# English EULA/privacy/data-consent ranges in the packed FMG, not OCR strings.
NATIVE_LEGAL_TEXT_ID_RANGES = [
    range(607100, 607133),
    range(607200, 607213),
    range(607300, 607302),
]
# Asset-backed from msg/engus/menu.msgbnd.dcx -> GR_System_Message_win64.fmg.
# Native records at 0x142acbe40 route these IDs through the title/network login status UI.
SERVER_STATUS_TEXT_IDS = {
    401120,  # Checking network connection status...
    401150,  # Logging in to the ELDEN RING game server...
    401160,  # Retrieving data from the ELDEN RING game server...
    401165,  # Saving data to the ELDEN RING game server...
}
SAVE_DATA_POPUP_OCR_PATTERNS = [
    re.compile(r"\bfailed to load save data\b", re.I),
    re.compile(r"\bsave data (?:could not|cannot|can't) be loaded\b", re.I),
    re.compile(r"\bunable to load save data\b", re.I),
    re.compile(r"\bload save data failed\b", re.I),
]
VISUAL_LEGAL_OCR_CROPS = [
    ("whole", "640x360+0+0"),
    ("center_modal", "520x260+60+45"),
    ("dialog_text", "500x210+70+60"),
    ("buttons", "360x90+140+260"),
]
MIN_REAL_CHARACTER_LEVEL = 1
MIN_REAL_CHARACTER_HP = 1
MIN_REAL_CHARACTER_NAME_LEN = 1
MIN_EXPECTED_STAT_COUNT = 8
DEFAULT_EXPECTED_ANIMATION_ID = 4050

TASK_READY_STAGES = {
    "game_task_recurring_registered",
    "telemetry_write",
}
TELEMETRY_READY_STAGES = {
    "telemetry_write",
}


@dataclass(frozen=True)
class ProcessRow:
    pid: int
    args: str


class TimingTracker:
    """Wall-clock milestone tracker, every delta measured from the TRUE bash launch epoch.

    The headline metric for the unified harness is (world-loaded time) - (bash launch time). The
    launch epoch is captured in bash at the moment eldenring.exe is fired and passed in via
    --launch-epoch / ER_PROBE_LAUNCH_EPOCH; if absent we fall back to watcher-start (time.time()).
    Each milestone is recorded only on its FIRST transition; deltas are seconds from launch_epoch.
    """

    def __init__(self, launch_epoch: float) -> None:
        self.launch_epoch = float(launch_epoch)
        self.deltas: dict[str, float | None] = {name: None for name in TIMING_MILESTONES}
        # t_launch is 0 by construction (the bash launch epoch itself).
        self.deltas["t_launch"] = 0.0

    def _now_delta(self) -> float:
        return max(time.time() - self.launch_epoch, 0.0)

    def mark(self, milestone: str) -> None:
        """Record `milestone` on its first occurrence and emit a clean greppable TIMING line."""
        if milestone not in self.deltas or self.deltas.get(milestone) is not None:
            return
        delta = self._now_delta()
        self.deltas[milestone] = delta
        # Greppable: `TIMING +<delta>s <milestone>` (strip the t_ prefix for the human label).
        label = milestone[2:] if milestone.startswith("t_") else milestone
        print(f"TIMING +{delta:.1f}s {label}", file=sys.stderr, flush=True)

    def observe(self, telemetry: dict[str, Any] | None, world_stable: bool = False) -> None:
        """Update milestone transitions from the latest telemetry read."""
        if telemetry is not None:
            self.mark("t_first_telemetry")
            if telemetry.get("oracle_own_load_continue_fired") is True:
                self.mark("t_continue_fired")
            if telemetry.get("oracle_player_present") is True:
                self.mark("t_player_present")
        if world_stable:
            self.mark("t_world_stable")

    def world_load_deadline_exceeded(self, deadline_seconds: float) -> bool:
        """True once the load has had `deadline_seconds` to reach the world-loaded semaphore and
        hasn't. ANCHORED to continue_fired (the load actually starting), NOT bash launch: our
        boot+title latency (~24s) varies and must not eat the load budget, and a launch-anchored
        deadline preempts the precise 6s world_stream_stalled semaphore. So the budget is measured
        from continue_fired when it has happened; before continue it falls back to launch-anchored
        (which then also catches a continue-never-fires hang). Independent of world_stream_stalled."""
        if self.deltas.get("t_first_telemetry") is None:
            return False
        if self.deltas.get("t_player_present") is not None or self.deltas.get("t_world_stable") is not None:
            return False
        continue_delta = self.deltas.get("t_continue_fired")
        anchor = continue_delta if continue_delta is not None else 0.0
        return (self._now_delta() - anchor) >= float(deadline_seconds)

    def snapshot(self) -> dict[str, Any]:
        payload: dict[str, Any] = {"launch_epoch": self.launch_epoch}
        for name in TIMING_MILESTONES:
            value = self.deltas.get(name)
            payload[name] = round(value, 3) if value is not None else None
        return payload

    def summary_line(self, reason: str) -> str:
        def fmt(name: str) -> str:
            value = self.deltas.get(name)
            return f"{value:.1f}" if value is not None else "n/a"

        return (
            "TIMING SUMMARY "
            f"launch->title={fmt('t_first_telemetry')}s "
            f"->continue={fmt('t_continue_fired')}s "
            f"->player={fmt('t_player_present')}s "
            f"->world={fmt('t_world_stable')}s "
            f"reason={reason}"
        )


def resolve_launch_epoch(args: argparse.Namespace) -> float:
    """The TRUE bash launch epoch: --launch-epoch, else ER_PROBE_LAUNCH_EPOCH, else watcher-start.

    The bash probe captures `date +%s.%N` at the moment eldenring.exe is fired (the closest bash
    timestamp to process start) and threads it here so deltas are from the true launch, not
    watcher-start. Unparseable/<=0 values fall back to time.time() so the harness never crashes.
    """
    candidate: float | None = None
    if getattr(args, "launch_epoch", None) is not None:
        candidate = args.launch_epoch
    else:
        env_value = os.environ.get("ER_PROBE_LAUNCH_EPOCH")
        if env_value:
            try:
                candidate = float(env_value)
            except ValueError:
                candidate = None
    if candidate is None or candidate <= 0:
        return time.time()
    return float(candidate)


@dataclass(frozen=True)
class ReadinessResult:
    ready: bool
    reason: str
    pid: int | None
    bootstrap: dict[str, Any] | None
    telemetry: dict[str, Any] | None
    windows: list[dict[str, Any]]
    polls: int
    timeout_seconds: float | None = None
    runtime_module_base: str | None = None
    runtime_module_mappings: list[dict[str, Any]] | None = None
    world_stable_samples: int = 0
    expected_save_oracle: dict[str, Any] | None = None
    expected_animation_id: int | None = None
    runtime_mode_expected: str | None = None
    runtime_mode_actual: str | None = None
    runtime_mode_match: bool | None = None
    seamless_module_mappings: list[dict[str, Any]] | None = None
    window_class: str = DEFAULT_WINDOW_CLASS
    world_stream_stall: dict[str, Any] | None = None
    phase_progress_stall: dict[str, Any] | None = None
    timing: dict[str, Any] | None = None

    def to_json(self) -> dict[str, Any]:
        payload = {
            "ready": self.ready,
            "reason": self.reason,
            "pid": self.pid,
            "bootstrap": self.bootstrap,
            "telemetry": self.telemetry,
            "windows": self.windows,
            "polls": self.polls,
            "timeout_seconds": self.timeout_seconds,
            "runtime_module_base": self.runtime_module_base,
            "runtime_module_mappings": self.runtime_module_mappings or [],
            "world_stable_samples": self.world_stable_samples,
            "runtime_mode_expected": self.runtime_mode_expected,
            "runtime_mode_actual": self.runtime_mode_actual,
            "runtime_mode_match": self.runtime_mode_match,
            "seamless_module_mappings": self.seamless_module_mappings or [],
            "target_window_capture": target_window_capture_diagnostics(self.windows, self.window_class),
            "autoload_progress": autoload_progress_summary(self.telemetry),
            "world_stream_stall": self.world_stream_stall,
            "phase_progress_stall": self.phase_progress_stall,
            "timing": self.timing,
        }
        oracle = oracle_summary(self.telemetry, self.expected_save_oracle, self.expected_animation_id)
        if oracle:
            payload["oracle"] = oracle
        return payload


def write_result(path: Path, result: ReadinessResult) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(result.to_json(), indent=2, sort_keys=True) + "\n", encoding="utf-8")


def read_json(path: Path) -> dict[str, Any] | None:
    if not path.exists() or path.stat().st_size == 0:
        return None
    try:
        payload = json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except json.JSONDecodeError:
        return None
    return payload if isinstance(payload, dict) else None


def read_last_json_line(path: Path) -> dict[str, Any] | None:
    if not path.exists() or path.stat().st_size == 0:
        return None
    for line in reversed(path.read_text(encoding="utf-8", errors="replace").splitlines()):
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(payload, dict):
            return payload
    return None


def read_bootstrap(event_path: Path, state_path: Path) -> dict[str, Any] | None:
    return read_json(state_path) or read_last_json_line(event_path)


def runtime_process_rows(pattern: re.Pattern[str]) -> list[ProcessRow]:
    output = subprocess.check_output(
        ["ps", "-eo", "pid=,args="],
        text=True,
        timeout=OBSERVATION_SUBPROCESS_TIMEOUT_SECONDS,
    )
    rows: list[ProcessRow] = []
    for line in output.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        pid_text, _, args = stripped.partition(" ")
        if pid_text.isdigit() and pattern.search(args):
            rows.append(ProcessRow(int(pid_text), args))
    return rows


def pid_running(pid: int) -> bool:
    try:
        os.kill(pid, os.O_SIGNAL if hasattr(os, "O_SIGNAL") else 0)
    except AttributeError:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        except PermissionError:
            return True
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def pid_file_value(path: Path) -> int | None:
    if not path.exists():
        return None
    text = path.read_text(encoding="utf-8", errors="replace").strip()
    return int(text) if text.isdigit() else None


def runtime_module_mappings(pid: int) -> list[dict[str, Any]]:
    maps_path = Path("/proc", str(pid), "maps")
    if not maps_path.exists():
        return []
    mappings: list[dict[str, Any]] = []
    for line in maps_path.read_text(encoding="utf-8", errors="replace").splitlines():
        if RUNTIME_EXE_NAME not in line.lower():
            continue
        fields = line.split(maxsplit=5)
        if len(fields) < 5:
            continue
        start_text, _, end_text = fields[0].partition("-")
        path = fields[5] if len(fields) >= 6 else ""
        mappings.append(
            {
                "start": f"0x{int(start_text, 16):x}",
                "end": f"0x{int(end_text, 16):x}",
                "permissions": fields[1],
                "offset": f"0x{int(fields[2], 16):x}",
                "device": fields[3],
                "inode": fields[4],
                "path": path,
            }
        )
    return mappings


def seamless_module_mappings(pid: int | None) -> list[dict[str, Any]]:
    if pid is None:
        return []
    maps_path = Path("/proc", str(pid), "maps")
    if not maps_path.exists():
        return []
    matches: list[dict[str, Any]] = []
    for line in maps_path.read_text(encoding="utf-8", errors="replace").splitlines():
        if not any(marker in line.lower() for marker in SEAMLESS_MODULE_MARKERS):
            continue
        fields = line.split(maxsplit=5)
        if len(fields) < 5:
            continue
        start_text, _, end_text = fields[0].partition("-")
        path = fields[5] if len(fields) >= 6 else ""
        matches.append(
            {
                "start": f"0x{int(start_text, 16):x}",
                "end": f"0x{int(end_text, 16):x}",
                "permissions": fields[1],
                "offset": f"0x{int(fields[2], 16):x}",
                "device": fields[3],
                "inode": fields[4],
                "path": path,
            }
        )
    return matches


def telemetry_seamless_loaded(telemetry: dict[str, Any] | None) -> bool | None:
    if not isinstance(telemetry, dict):
        return None
    if telemetry.get("seamless_coop_loaded") is True or telemetry.get("runtime_mode") == RUNTIME_MODE_SEAMLESS:
        return True
    if telemetry.get("seamless_coop_loaded") is False:
        return False
    return None


def observed_runtime_mode(pid: int | None, telemetry: dict[str, Any] | None) -> tuple[str, list[dict[str, Any]]]:
    mappings = seamless_module_mappings(pid)
    telemetry_loaded = telemetry_seamless_loaded(telemetry)
    if mappings or telemetry_loaded is True:
        return RUNTIME_MODE_SEAMLESS, mappings
    if telemetry_loaded is False:
        return RUNTIME_MODE_VANILLA, mappings
    return RUNTIME_MODE_UNKNOWN, mappings


def runtime_mode_matches(expected: str, actual: str) -> bool:
    if expected == RUNTIME_MODE_ANY:
        return True
    if expected == RUNTIME_MODE_VANILLA:
        return actual != RUNTIME_MODE_SEAMLESS
    if expected == RUNTIME_MODE_SEAMLESS:
        return actual == RUNTIME_MODE_SEAMLESS
    return False


def runtime_mode_definite_mismatch(expected: str, actual: str) -> bool:
    if expected == RUNTIME_MODE_ANY or actual == RUNTIME_MODE_UNKNOWN:
        return False
    return not runtime_mode_matches(expected, actual)


def with_runtime_mode_info(result: ReadinessResult, expected: str) -> ReadinessResult:
    actual, mappings = observed_runtime_mode(result.pid, result.telemetry)
    return replace(
        result,
        runtime_mode_expected=expected,
        runtime_mode_actual=actual,
        runtime_mode_match=runtime_mode_matches(expected, actual),
        seamless_module_mappings=mappings,
    )


def runtime_module_base(pid: int) -> tuple[str | None, list[dict[str, Any]]]:
    mappings = runtime_module_mappings(pid)
    zero_offset_starts = [
        int(mapping["start"], 16)
        for mapping in mappings
        if mapping.get("offset") == "0x0"
    ]
    if zero_offset_starts:
        return f"0x{min(zero_offset_starts):x}", mappings
    if mappings:
        return f"0x{min(int(mapping['start'], 16) for mapping in mappings):x}", mappings
    return None, mappings


def with_runtime_module_info(result: ReadinessResult) -> ReadinessResult:
    if result.pid is None:
        return result
    base, mappings = runtime_module_base(result.pid)
    return ReadinessResult(
        result.ready,
        result.reason,
        result.pid,
        result.bootstrap,
        result.telemetry,
        result.windows,
        result.polls,
        result.timeout_seconds,
        base,
        mappings,
        result.world_stable_samples,
        result.expected_save_oracle,
        result.expected_animation_id,
    )


def select_runtime_pid(
    pattern: re.Pattern[str],
    pid_file: Path,
    poll_budget: int,
    deadline: float,
    allow_async_launcher_exit: bool = False,
) -> tuple[int | None, str, int]:
    launcher_pid = pid_file_value(pid_file)
    preexisting_runtime_pids = {
        row.pid
        for row in runtime_process_rows(pattern)
        if RUNTIME_EXE_NAME in row.args.lower()
    }
    launcher_exited = False
    for poll in range(poll_budget):
        if time.monotonic() >= deadline:
            return None, TIMEOUT_BUDGET_EXHAUSTED, poll
        rows = runtime_process_rows(pattern)
        for row in rows:
            if RUNTIME_EXE_NAME in row.args.lower() and row.pid not in preexisting_runtime_pids:
                return row.pid, READY_REASON, poll
        if launcher_pid is not None and not launcher_exited and not pid_running(launcher_pid):
            launcher_exited = True
            rows = runtime_process_rows(pattern)
            for row in rows:
                if RUNTIME_EXE_NAME in row.args.lower() and row.pid not in preexisting_runtime_pids:
                    return row.pid, READY_REASON, poll
            if not allow_async_launcher_exit:
                return None, PROCESS_EXITED, poll
        os.sched_yield()
    return None, SPAWN_BUDGET_EXHAUSTED, poll_budget


def process_exists(pid: int) -> bool:
    proc = Path("/proc", str(pid))
    if not proc.exists():
        return False
    try:
        cmd = proc.joinpath("cmdline").read_bytes().replace(b"\0", b" ").decode("utf-8", "replace").lower()
        comm = proc.joinpath("comm").read_text(errors="replace").strip().lower()
    except Exception:
        return False
    return RUNTIME_EXE_NAME in cmd or comm == RUNTIME_EXE_NAME


def client_is_game_window(client: dict[str, Any], window_class: str) -> bool:
    # Exact class only. Title fallback can match unrelated browser/tab titles and caused
    # wrong-window OCR during a runtime probe; fail closed if the real ER window is absent.
    klass = str(client.get("class") or "")
    return klass == window_class


def target_window_capture_problems(window: dict[str, Any], window_class: str) -> list[str]:
    """Return reasons this Hyprland client is unsafe to capture by screen geometry.

    grim -g captures the current desktop region, not a window backing store. If the
    Elden Ring client is not the focused/top window, the same geometry can contain
    an unrelated browser/terminal and produce a false OCR result. Therefore visual
    checks only capture an exact-class, mapped, unhidden, focused target window with
    sane geometry; otherwise they fail closed without taking a screenshot.
    """
    problems: list[str] = []
    if not client_is_game_window(window, window_class):
        problems.append("target_window_class_mismatch")
    if window.get("mapped") is False:
        problems.append("target_window_unmapped")
    if window.get("hidden") is True:
        problems.append("target_window_hidden")
    if "focusHistoryID" not in window:
        problems.append("target_window_focus_unknown")
    elif as_int(window.get("focusHistoryID"), -1) != 0:
        problems.append("target_window_not_focused")
    at = window.get("at") or []
    size = window.get("size") or []
    if len(at) != 2 or len(size) != 2:
        problems.append("target_window_bad_geometry")
    elif as_int(size[0], 0) <= 0 or as_int(size[1], 0) <= 0:
        problems.append("target_window_empty_geometry")
    return problems


def window_capture_safe(window: dict[str, Any], window_class: str) -> bool:
    return not target_window_capture_problems(window, window_class)


def target_window_capture_diagnostics(windows: list[dict[str, Any]], window_class: str) -> dict[str, Any]:
    if not windows:
        return {
            "window_class": window_class,
            "window_present": False,
            "capture_safe": False,
            "problems": ["no_target_window"],
        }
    selected = windows[0]
    problems = target_window_capture_problems(selected, window_class)
    return {
        "window_class": window_class,
        "window_present": True,
        "capture_safe": not problems,
        "problems": problems,
        "selected": {
            key: selected.get(key)
            for key in (
                "class",
                "workspace",
                "at",
                "size",
                "pid",
                "mapped",
                "hidden",
                "focusHistoryID",
                "fullscreen",
                "address",
            )
        },
        "candidate_count": len(windows),
    }


def as_int(value: Any, default: int = -1) -> int:
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, (int, float)):
        return int(value)
    if isinstance(value, str):
        try:
            return int(value, 0)
        except ValueError:
            return default
    return default


def world_stream_armed(telemetry: dict[str, Any] | None) -> bool:
    """True once the OWN-LOAD map-load has actually begun.

    The stall semaphore must never arm before our continue has fired AND the player map block has
    registered (target block present), so a slow boot / title screen can never trip it. Both gates
    come from in-process OWN-LOAD telemetry; missing/None reads count as not-yet-armed.
    """
    if not isinstance(telemetry, dict):
        return False
    if telemetry.get("oracle_own_load_continue_fired") is not True:
        return False
    return as_int(telemetry.get("oracle_own_load_target_block_present"), 0) == 1


def world_stream_progress_watermark(telemetry: dict[str, Any] | None) -> tuple[int, int, int, int, int] | None:
    """Comparable monotonic progress watermark for the streaming player map block.

    Returns None when the map-load has not begun (predicate disarmed). Otherwise a tuple that
    increases strictly on any forward streaming progress, parsed from the OWN-LOAD oracle fields:
      - oracle_own_load_wbr_max_phase     (per-block phase; hex string, stuck at 0x2 when stalled)
      - oracle_own_load_stream_io_inflight (hex string; 0 -> no IO dispatched; 1 once it goes non-zero)
      - oracle_own_load_stream_mms_state   (3 when stalled; advances past 3 when progressing)
      - oracle_own_m28_dispatch_fired      (dispatch counter; increments on a working stream)
      - oracle_player_present              (terminal: 1 once the player exists)
    Null/missing/unparseable fields contribute 0 (treated as "no progress"), never a crash.
    """
    if not world_stream_armed(telemetry):
        return None
    assert isinstance(telemetry, dict)
    wbr_max_phase = max(as_int(telemetry.get("oracle_own_load_wbr_max_phase"), 0), 0)
    io_inflight = 1 if as_int(telemetry.get("oracle_own_load_stream_io_inflight"), 0) > 0 else 0
    mms_state = max(as_int(telemetry.get("oracle_own_load_stream_mms_state"), 0), 0)
    m28_dispatch = max(as_int(telemetry.get("oracle_own_m28_dispatch_fired"), 0), 0)
    player_present = 1 if telemetry.get("oracle_player_present") is True else 0
    return (player_present, m28_dispatch, mms_state, io_inflight, wbr_max_phase)


def world_stream_stall_step(
    telemetry: dict[str, Any] | None,
    prev_watermark: tuple[int, int, int, int, int] | None,
    progress_since: float | None,
    now: float,
    stall_seconds: float,
) -> tuple[tuple[int, int, int, int, int] | None, float | None, bool]:
    """One poll's worth of world-stream stall bookkeeping.

    Pure so the loop and the tests share the exact same decision. Returns the carried-forward
    (watermark, progress_since, stalled). `stalled` is True only once the watermark has been flat
    (no strict improvement) for >= stall_seconds since the map-load armed. Any forward progress --
    or a disarmed (None watermark) state -- resets the timer so a healthy stream never trips it.
    """
    watermark = world_stream_progress_watermark(telemetry)
    if watermark is None:
        return None, None, False
    if prev_watermark is None or watermark > prev_watermark:
        return watermark, now, False
    if progress_since is not None and (now - progress_since >= stall_seconds):
        return watermark, progress_since, True
    return watermark, progress_since, False


def world_stream_stall_snapshot(telemetry: dict[str, Any] | None, stuck_seconds: float) -> dict[str, Any]:
    """Small diagnostic snapshot describing why the world-stream stall semaphore fired."""
    fields: dict[str, Any] = {}
    if isinstance(telemetry, dict):
        for key in (
            "oracle_own_load_continue_fired",
            "oracle_own_load_target_block_present",
            "oracle_own_load_wbr_max_phase",
            "oracle_own_load_stream_io_inflight",
            "oracle_own_load_stream_mms_state",
            "oracle_own_m28_dispatch_fired",
            "oracle_player_present",
            "oracle_now_loading",
        ):
            fields[key] = telemetry.get(key)
    return {
        "stuck_seconds": round(stuck_seconds, 3),
        "watermark": list(world_stream_progress_watermark(telemetry) or ()),
        "fields": fields,
    }


# --- per-phase progress watchdog ------------------------------------------------------------------
# An ORDERED phase model over the menu-free load pipeline. Each phase exposes:
#   - name:            the reason-string stem (f"{name}_stalled")
#   - active(t):       True when this phase is the one currently in flight
#   - progress(t):     a comparable monotonic tuple that STRICTLY increases on forward progress
#                      within the phase, or None when the phase is not active / not measurable
# The watchdog tracks last-progress-value + last-progress-time for the active phase and fails fast
# when the active phase's progress stays flat (no strict increase) for >= its window. Any increase
# -- or a phase transition -- resets the timer, so an inherently-slow-but-MOVING phase never trips.
#
# Phases, in order of the real run (timing this session, deltas from bash launch):
#   boot          first_telemetry=3.0s     (covered by the existing spawn/no-telemetry handling;
#                                            the watchdog STARTS at telemetry_present so it never
#                                            double-covers the pre-telemetry boot wait)
#   title         ~3.0s -> ~14.9s          (game boot + title-owner scan; ~12s but PROGRESSING:
#                                            game_task_ticks + title_owner_scan_attempts climb)
#   continue      ~14.9s -> ~25.7s         (own_stepper "waiting for title_boot_ready" loop; ~10.7s
#                                            but ALIVE: game_task_ticks keep incrementing)
#   world_stream  ~25.7s -> world          (the existing world_stream_stalled tail detector)
#
# title and continue are gated on game_task_ticks (advances every rendered frame while the game is
# alive) so a slow-but-healthy boot/title/wait never false-fails -- only a true freeze trips.


def phase_boot_active(telemetry: dict[str, Any] | None) -> bool:
    """boot: before any telemetry is readable. Not watched here (spawn/no-telemetry handling owns
    the pre-telemetry wait); kept in the model only so the ordered set is complete."""
    return telemetry is None


def phase_title_active(telemetry: dict[str, Any] | None) -> bool:
    """title: telemetry present, continue not yet fired, and the title owner not yet captured.

    The own_stepper captures the title owner (title_owner_scan_last_state == 10 at the title screen)
    before it begins the continue handshake. While the title owner is not yet cached we are still in
    the boot+title flow.
    """
    if not isinstance(telemetry, dict):
        return False
    if telemetry.get("oracle_own_load_continue_fired") is True:
        return False
    return not phase_title_owner_captured(telemetry)


def phase_title_owner_captured(telemetry: dict[str, Any] | None) -> bool:
    """True once the title-owner scan has locked the title screen (last_state == 10) or cached an
    owner. This is the boundary between the title phase and the continue phase."""
    if not isinstance(telemetry, dict):
        return False
    if as_int(telemetry.get("title_owner_scan_last_state"), -1) == 10:
        return True
    cached = telemetry.get("title_owner_scan_cached_owner")
    if isinstance(cached, str) and cached not in ("", "0x0", "0", "null"):
        return True
    if cached not in (None, 0, "", "0x0", "0", "null") and as_int(cached, 0) != 0:
        return True
    return telemetry.get("title_handoff_complete") is True


def phase_continue_active(telemetry: dict[str, Any] | None) -> bool:
    """continue: title owner captured, continue not yet fired. The ~10.7s title_boot_ready wait."""
    if not isinstance(telemetry, dict):
        return False
    if telemetry.get("oracle_own_load_continue_fired") is True:
        return False
    return phase_title_owner_captured(telemetry)


def phase_title_progress(telemetry: dict[str, Any] | None) -> tuple[int, int, int, int, int] | None:
    """Monotonic title-phase progress watermark, or None when the title phase is not active.

    Built from signals that climb during a healthy boot/title:
      game_task_ticks         -- increments every rendered frame while the game is alive
      title_owner_scan_attempts -- the owner scan runs every tick, so this climbs
      title_owner_scan_vtable_hits -- climbs as candidate owners are inspected
      title_owner_scan_last_state  -- advances toward 10 (title-ready)
      title_handoff_complete as 0/1  -- flips to 1 when the title bootstrap is observed
    A flatline of ALL of these for >window means the title flow is truly wedged.
    """
    if not phase_title_active(telemetry):
        return None
    assert isinstance(telemetry, dict)
    ticks = max(as_int(telemetry.get("game_task_ticks"), 0), 0)
    scan_attempts = max(as_int(telemetry.get("title_owner_scan_attempts"), 0), 0)
    vtable_hits = max(as_int(telemetry.get("title_owner_scan_vtable_hits"), 0), 0)
    last_state = max(as_int(telemetry.get("title_owner_scan_last_state"), 0), 0)
    bootstrap_seen = 1 if telemetry.get("title_handoff_complete") is True else 0
    return (bootstrap_seen, last_state, scan_attempts, vtable_hits, ticks)


def phase_continue_progress(telemetry: dict[str, Any] | None) -> tuple[int] | None:
    """Monotonic continue-phase progress watermark, or None when the continue phase is not active.

    Gated ONLY on game_task_ticks: the title_boot_ready wait keeps the game alive (ticks advancing
    every frame) even though continue has not fired yet -- that is PROGRESSING, not wedged, so it must
    not trip on "continue is slow". Continue-is-too-slow is handled by the continue-anchored
    world_load_deadline backstop, not this watchdog. The watchdog fires here ONLY on a true freeze:
    game_task_ticks flat for >window (the game itself stopped advancing frames).
    """
    if not phase_continue_active(telemetry):
        return None
    assert isinstance(telemetry, dict)
    return (max(as_int(telemetry.get("game_task_ticks"), 0), 0),)


# Ordered phase registry: (name, reason, active_predicate, progress_fn). The world_stream phase is
# integrated via its own world_stream_stall_step (different, richer watermark + its own window), so
# it is intentionally NOT in this table -- this table covers the previously-BLIND gaps (title,
# continue). boot is listed for completeness but its progress_fn returns None (unwatched here).
PHASE_WATCHDOG_MODEL: tuple[tuple[str, str, Any, Any], ...] = (
    ("title", TITLE_STALLED, phase_title_active, phase_title_progress),
    ("continue", CONTINUE_STALLED, phase_continue_active, phase_continue_progress),
)


def active_watchdog_phase(telemetry: dict[str, Any] | None) -> tuple[str, str, Any] | None:
    """Return (name, reason, progress_value) for the first active phase whose progress is measurable,
    else None. Only one phase is active at a time by construction (the predicates are mutually
    exclusive on continue_fired + title-owner-captured)."""
    for name, reason, active, progress in PHASE_WATCHDOG_MODEL:
        if active(telemetry):
            value = progress(telemetry)
            if value is not None:
                return name, reason, value
    return None


def phase_progress_stall_step(
    telemetry: dict[str, Any] | None,
    state: dict[str, Any],
    now: float,
    stall_seconds: float,
) -> tuple[bool, str | None]:
    """One poll of per-phase watchdog bookkeeping. Pure so the loop and tests share the decision.

    `state` is a mutable carry-dict the caller owns across polls with keys:
      "phase"          -- name of the phase last seen active (or None)
      "value"          -- last progress watermark for that phase
      "since"          -- monotonic time the watermark last strictly improved
    Returns (stalled, reason). `reason` is the f"{phase}_stalled" reason string when stalled, else
    None. The timer resets on ANY of: a phase transition, a strict increase in the active phase's
    watermark, or the phase becoming inactive -- so an inherently-slow-but-MOVING phase never trips.
    Stalls only when the SAME active phase's watermark is flat for >= stall_seconds.
    """
    active = active_watchdog_phase(telemetry)
    if active is None:
        # No watched phase active (pre-telemetry boot, or continue fired -> world_stream owns it).
        state["phase"] = None
        state["value"] = None
        state["since"] = None
        return False, None
    name, reason, value = active
    if state.get("phase") != name or state.get("value") is None:
        # Entered (or re-entered) this phase: arm a fresh timer.
        state["phase"] = name
        state["value"] = value
        state["since"] = now
        return False, None
    if value > state["value"]:
        # Forward progress within the phase: reset the timer.
        state["value"] = value
        state["since"] = now
        return False, None
    since = state.get("since")
    if since is not None and (now - since) >= stall_seconds:
        return True, reason
    return False, None


def phase_progress_stall_snapshot(
    telemetry: dict[str, Any] | None,
    phase_name: str,
    stuck_seconds: float,
) -> dict[str, Any]:
    """Diagnostic snapshot describing which phase wedged and the signals that stayed flat."""
    fields: dict[str, Any] = {}
    if isinstance(telemetry, dict):
        for key in (
            "game_task_ticks",
            "title_owner_scan_attempts",
            "title_owner_scan_vtable_hits",
            "title_owner_scan_last_state",
            "title_owner_scan_cached_owner",
            "title_handoff_complete",
            "oracle_own_load_continue_fired",
        ):
            fields[key] = telemetry.get(key)
    value = active_watchdog_phase(telemetry)
    return {
        "phase": phase_name,
        "stuck_seconds": round(stuck_seconds, 3),
        "watermark": list(value[2]) if value is not None else [],
        "fields": fields,
    }


def expected_save_fields(expected_save_oracle: dict[str, Any] | None) -> dict[str, Any]:
    if not isinstance(expected_save_oracle, dict):
        return {}
    decoded = expected_save_oracle.get("decoded_fields")
    return decoded if isinstance(decoded, dict) else {}


def name_empty_like(value: Any) -> bool:
    if not isinstance(value, str):
        return True
    stripped = value.strip()
    return stripped == "" or stripped == "_"


def telemetry_expected_save_match(telemetry: dict[str, Any], expected_save_oracle: dict[str, Any] | None) -> bool:
    fields = expected_save_fields(expected_save_oracle)
    if not fields:
        return True
    expected_map = as_int(fields.get("saved_map_c30"), -1)
    observed_map = as_int(telemetry.get("oracle_saved_map_c30"), -1)
    expected_slot = as_int(expected_save_oracle.get("slot") if isinstance(expected_save_oracle, dict) else None, -1)
    observed_slot = as_int(telemetry.get("game_save_slot"), -2)
    return bool(
        observed_slot == expected_slot
        and telemetry.get("oracle_char_name") == fields.get("name")
        and as_int(telemetry.get("oracle_char_name_len"), -1) == as_int(fields.get("name_len"), -2)
        and as_int(telemetry.get("oracle_char_level"), -1) == as_int(fields.get("level"), -2)
        and as_int(telemetry.get("oracle_char_current_hp"), -1) == as_int(fields.get("health"), -2)
        and telemetry.get("oracle_char_stats") == fields.get("stats")
        and expected_map == observed_map
    )


def telemetry_expected_animation_match(telemetry: dict[str, Any], expected_animation_id: int | None) -> bool:
    return expected_animation_id is None or as_int(telemetry.get("current_animation_id"), -1) == expected_animation_id


def telemetry_messagebox_dialog_detected(telemetry: dict[str, Any] | None) -> bool:
    if not isinstance(telemetry, dict):
        return False
    return bool(
        telemetry.get("oracle_msgbox_any_seen") is True
        or as_int(telemetry.get("oracle_msgbox_total_builds"), 0) > 0
        or telemetry.get("oracle_blocking_modal_present") is True
        or telemetry.get("oracle_postload_modal_seen") is True
        or as_int(telemetry.get("oracle_msgbox_postload_builds"), 0) > 0
    )


def native_legal_text_id(value: Any) -> int | None:
    text_id = as_int(value, -1)
    if any(text_id in text_id_range for text_id_range in NATIVE_LEGAL_TEXT_ID_RANGES):
        return text_id
    return None


def telemetry_native_legal_popup_detected(telemetry: dict[str, Any] | None) -> bool:
    if not isinstance(telemetry, dict):
        return False
    # CS::MessageBoxDialog path: legal/privacy FMG IDs captured from native builder args.
    args = telemetry.get("oracle_msgbox_builder_args")
    msgbox_legal = isinstance(args, list) and any(native_legal_text_id(arg) is not None for arg in args)
    # TosTitle path: the Privacy/ToS policy surface is not a MessageBoxDialog; constructor hits are
    # native/asset-backed evidence that the policy UI was built from the ToS_win64-backed layout.
    policy_window = (
        telemetry.get("oracle_policy_window_any_seen") is True
        or as_int(telemetry.get("oracle_policy_window_total_builds"), 0) > 0
    )
    return msgbox_legal or policy_window


def telemetry_cold_char_mount_complete(telemetry: dict[str, Any] | None) -> bool:
    """True once cold_char_mount_drive reaches its terminal phase (success or timeout). Used for
    evidence-driven teardown of a no-write cold-mount probe, which never reaches world-stable.
    Mirrors the codebase pattern: guard a possibly-None telemetry centrally so the caller cannot
    deref None (the bug that previously crashed the watcher and burned a runtime launch)."""
    if not isinstance(telemetry, dict):
        return False
    return as_int(telemetry.get("oracle_cold_char_mount_phase"), 0) >= COLD_CHAR_MOUNT_PHASE_DONE


def telemetry_server_status_semaphore_detected(telemetry: dict[str, Any] | None) -> bool:
    if not isinstance(telemetry, dict):
        return False
    text_id = as_int(telemetry.get("oracle_server_status_text_id"), -1)
    return bool(
        telemetry.get("oracle_server_status_any_seen") is True
        or as_int(telemetry.get("oracle_server_status_total_seen"), 0) > 0
        or text_id in SERVER_STATUS_TEXT_IDS
    )


def telemetry_no_postload_popup(telemetry: dict[str, Any]) -> bool:
    return not telemetry_messagebox_dialog_detected(telemetry)


def telemetry_native_submit_entered(telemetry: dict[str, Any]) -> bool:
    return as_int(telemetry.get("oracle_native_submit_hits"), 0) > 0


def telemetry_native_result_chain_same_result(telemetry: dict[str, Any]) -> bool:
    submit_result = telemetry.get("oracle_native_submit_last_result")
    event_result = telemetry.get("oracle_result_event_last_result")
    action_result = telemetry.get("oracle_result_action_last_result")
    return bool(
        isinstance(submit_result, str)
        and submit_result.startswith("0x")
        and submit_result == event_result
        and submit_result == action_result
    )


def telemetry_native_submit_fd4_event_match(telemetry: dict[str, Any]) -> bool:
    # Static RE pins native submit 0x1407ac890 to construct FD4 event {code=3,arg=0}
    # before dispatching result.vtable+0x60. Runtime result-handler telemetry must agree.
    return bool(
        as_int(telemetry.get("oracle_result_event_last_fd4_code"), -1) == 3
        and as_int(telemetry.get("oracle_result_event_last_fd4_arg"), -1) == 0
    )


def telemetry_native_result_chain_ready(telemetry: dict[str, Any]) -> bool:
    return bool(
        telemetry_native_submit_entered(telemetry)
        and as_int(telemetry.get("oracle_result_event_handler_hits"), 0) > 0
        and as_int(telemetry.get("oracle_result_action_builder_hits"), 0) > 0
        and telemetry_native_result_chain_same_result(telemetry)
        and telemetry_native_submit_fd4_event_match(telemetry)
    )


def telemetry_result_action_inserted(telemetry: dict[str, Any]) -> bool:
    return as_int(telemetry.get("oracle_result_action_insert_hits"), 0) > 0


def telemetry_result_action_wrapper_built(telemetry: dict[str, Any]) -> bool:
    return as_int(telemetry.get("oracle_result_action_wrapper_builder_hits"), 0) > 0


def telemetry_result_action_wrapper_has_update_rva(telemetry: dict[str, Any]) -> bool:
    value = telemetry.get("oracle_result_action_last_wrapper_builder_ret_update_rva")
    return isinstance(value, str) and value.startswith("0x")


def telemetry_result_action_insert_has_update_rva(telemetry: dict[str, Any]) -> bool:
    for key in (
        "oracle_result_action_last_insert_arg1_update_rva",
        "oracle_result_action_last_insert_ret_update_rva",
    ):
        value = telemetry.get(key)
        if isinstance(value, str) and value.startswith("0x"):
            return True
    return False


def telemetry_native_continue_chain_stage(telemetry: dict[str, Any]) -> str:
    phase = as_int(telemetry.get("oracle_continue_phase"), -1)
    result_chain_ready = telemetry_native_result_chain_ready(telemetry)
    action_wrapper_built = telemetry_result_action_wrapper_built(telemetry)
    action_wrapper_has_update_rva = telemetry_result_action_wrapper_has_update_rva(telemetry)
    action_inserted = telemetry_result_action_inserted(telemetry)
    action_insert_has_update_rva = telemetry_result_action_insert_has_update_rva(telemetry)
    # oracle_continue_deser_fired / oracle_continue_confirmed were REMOVED (2026-06-24): they
    # tracked the own_stepper confirm-FIRE chain, not the load. world_loaded below is the real
    # load semaphore and wins first; the intermediate "confirmed_waiting_world" /
    # "deserialized_waiting_confirm" stall labels are dropped with them.
    if telemetry_world_loaded(telemetry):
        return "world_loaded"
    if phase >= 3 and result_chain_ready and action_inserted and action_insert_has_update_rva:
        return "action_insert_waiting_continue_load"
    if phase >= 3 and result_chain_ready and action_inserted:
        return "action_insert_without_update_rva"
    if phase >= 3 and result_chain_ready and action_wrapper_built and action_wrapper_has_update_rva:
        return "wrapper_builder_waiting_action_insert"
    if phase >= 3 and result_chain_ready and action_wrapper_built:
        return "wrapper_builder_without_update_rva"
    if phase >= 3 and result_chain_ready:
        return "result_chain_waiting_wrapper_builder"
    if phase >= 3 and telemetry_native_submit_entered(telemetry):
        return "submitted_without_result_chain"
    if phase >= 3:
        return "guard_without_native_submit"
    if result_chain_ready:
        return "result_chain_before_guard"
    if telemetry_native_submit_entered(telemetry):
        return "native_submit_before_guard"
    if phase == 0:
        return "pre_submit"
    if phase > 0:
        return "intermediate_without_result_chain"
    return "unknown"


def autoload_progress_summary(telemetry: dict[str, Any] | None) -> dict[str, Any]:
    if not isinstance(telemetry, dict):
        return {"telemetry_present": False, "blocker": "waiting_for_telemetry"}
    attempts = as_int(telemetry.get("autoload_attempts"), 0)
    product_core_ticks = as_int(telemetry.get("product_core_autoload_ticks"), 0)
    product_core_blocker = str(telemetry.get("product_core_ready_blocker") or "unseen")
    phase = as_int(telemetry.get("oracle_continue_phase"), -1)
    title_handoff_complete = telemetry.get("title_handoff_complete") is True
    native_stage = telemetry_native_continue_chain_stage(telemetry)
    if telemetry.get("game_man_instance_resolved") is not True:
        blocker = "waiting_for_game_man"
    elif telemetry.get("product_autoload_armed") is True and product_core_ticks > 0 and attempts <= 0:
        blocker = f"product_core_{product_core_blocker}"
    elif attempts <= 0 and not title_handoff_complete:
        blocker = "autoload_not_attempted_waiting_title_bootstrap"
    elif attempts <= 0:
        blocker = "autoload_not_attempted"
    elif phase < 3:
        blocker = "autoload_attempted_before_native_submit_guard"
    elif native_stage != "world_loaded":
        blocker = native_stage
    else:
        blocker = "world_loaded"
    return {
        "telemetry_present": True,
        "blocker": blocker,
        "game_man_instance_resolved": telemetry.get("game_man_instance_resolved"),
        "game_task_ticks": telemetry.get("game_task_ticks"),
        "player_available": telemetry.get("player_available"),
        "player_seen": telemetry.get("player_seen"),
        "autoload_slot": telemetry.get("autoload_slot"),
        "autoload_method": telemetry.get("autoload_method"),
        "autoload_require_title_bootstrap": telemetry.get("autoload_require_title_bootstrap"),
        "product_autoload_armed": telemetry.get("product_autoload_armed"),
        "product_core_autoload_ticks": telemetry.get("product_core_autoload_ticks"),
        "product_core_ready_blocks": telemetry.get("product_core_ready_blocks"),
        "product_core_ready_successes": telemetry.get("product_core_ready_successes"),
        "product_core_owner_ticks": telemetry.get("product_core_owner_ticks"),
        "product_core_last_owner": telemetry.get("product_core_last_owner"),
        "product_core_last_title_dialog": telemetry.get("product_core_last_title_dialog"),
        "product_core_last_title_dialog_vt": telemetry.get("product_core_last_title_dialog_vt"),
        "product_core_last_title_in_loop": telemetry.get("product_core_last_title_in_loop"),
        "product_core_last_title_in_textfadeout": telemetry.get("product_core_last_title_in_textfadeout"),
        "product_core_last_menu_opened_latch": telemetry.get("product_core_last_menu_opened_latch"),
        "product_core_last_press_start_proxy": telemetry.get("product_core_last_press_start_proxy"),
        "product_core_last_press_start_vt": telemetry.get("product_core_last_press_start_vt"),
        "product_core_last_press_start_context": telemetry.get("product_core_last_press_start_context"),
        "product_core_last_phase": telemetry.get("product_core_last_phase"),
        "product_core_ready_blocker": telemetry.get("product_core_ready_blocker"),
        "title_owner_scan_attempts": telemetry.get("title_owner_scan_attempts"),
        "title_owner_scan_vtable_hits": telemetry.get("title_owner_scan_vtable_hits"),
        "title_owner_scan_table_rejects": telemetry.get("title_owner_scan_table_rejects"),
        "title_owner_scan_state_rejects": telemetry.get("title_owner_scan_state_rejects"),
        "title_owner_scan_cached_owner": telemetry.get("title_owner_scan_cached_owner"),
        "title_owner_scan_last_candidate": telemetry.get("title_owner_scan_last_candidate"),
        "title_owner_scan_last_table": telemetry.get("title_owner_scan_last_table"),
        "title_owner_scan_last_state": telemetry.get("title_owner_scan_last_state"),
        "autoload_attempts": telemetry.get("autoload_attempts"),
        "autoload_last_status": telemetry.get("autoload_last_status"),
        "title_handoff_complete": telemetry.get("title_handoff_complete"),
        "continue_phase": telemetry.get("oracle_continue_phase"),
        "continue_member_node": telemetry.get("oracle_continue_member_node"),
        "continue_task_node": telemetry.get("oracle_continue_task_node"),
        "menu_window_ctor_hits": telemetry.get("oracle_menu_window_ctor_hits"),
        "menu_window_ctor_semantic_hits": telemetry.get("oracle_menu_window_ctor_semantic_hits"),
        "menu_window_ctor_last_item": telemetry.get("oracle_menu_window_ctor_last_item"),
        "menu_window_ctor_last_vt": telemetry.get("oracle_menu_window_ctor_last_vt"),
        "menu_window_ctor_last_docall": telemetry.get("oracle_menu_window_ctor_last_docall"),
        "menu_window_ctor_last_accept": telemetry.get("oracle_menu_window_ctor_last_accept"),
        "menu_window_native_ctor_b_hits": telemetry.get("oracle_menu_window_native_ctor_b_hits"),
        "menu_window_native_ctor_b_continue_hits": telemetry.get("oracle_menu_window_native_ctor_b_continue_hits"),
        "menu_window_native_ctor_b_last_caller_rva": telemetry.get("oracle_menu_window_native_ctor_b_last_caller_rva"),
        "menu_window_native_ctor_b_last_item": telemetry.get("oracle_menu_window_native_ctor_b_last_item"),
        "menu_window_native_ctor_b_last_out_slot": telemetry.get("oracle_menu_window_native_ctor_b_last_out_slot"),
        "menu_window_native_ctor_b_last_vt": telemetry.get("oracle_menu_window_native_ctor_b_last_vt"),
        "menu_window_native_ctor_b_last_docall": telemetry.get("oracle_menu_window_native_ctor_b_last_docall"),
        "menu_window_native_ctor_b_last_accept": telemetry.get("oracle_menu_window_native_ctor_b_last_accept"),
        "menu_window_idle_ctor_hits": telemetry.get("oracle_menu_window_idle_ctor_hits"),
        "menu_window_idle_ctor_continue_hits": telemetry.get("oracle_menu_window_idle_ctor_continue_hits"),
        "menu_window_idle_ctor_continue_last_caller_rva": telemetry.get("oracle_menu_window_idle_ctor_continue_last_caller_rva"),
        "menu_window_idle_ctor_continue_last_item": telemetry.get("oracle_menu_window_idle_ctor_continue_last_item"),
        "menu_window_idle_ctor_continue_last_out_slot": telemetry.get("oracle_menu_window_idle_ctor_continue_last_out_slot"),
        "menu_window_idle_ctor_continue_last_docall": telemetry.get("oracle_menu_window_idle_ctor_continue_last_docall"),
        "menu_window_idle_ctor_continue_last_accept": telemetry.get("oracle_menu_window_idle_ctor_continue_last_accept"),
        "menu_continue_idle_insert_hits": telemetry.get("oracle_menu_continue_idle_insert_hits"),
        "menu_continue_idle_insert_last_caller_rva": telemetry.get("oracle_menu_continue_idle_insert_last_caller_rva"),
        "menu_continue_idle_insert_last_arg0": telemetry.get("oracle_menu_continue_idle_insert_last_arg0"),
        "menu_continue_idle_insert_last_arg1": telemetry.get("oracle_menu_continue_idle_insert_last_arg1"),
        "menu_continue_idle_insert_last_ret": telemetry.get("oracle_menu_continue_idle_insert_last_ret"),
        "menu_continue_idle_insert_last_arg1_update_rva": telemetry.get("oracle_menu_continue_idle_insert_last_arg1_update_rva"),
        "menu_continue_idle_insert_last_ret_update_rva": telemetry.get("oracle_menu_continue_idle_insert_last_ret_update_rva"),
        "task_enqueue_generic_hits": telemetry.get("oracle_task_enqueue_generic_hits"),
        "task_enqueue_generic_last_caller_rva": telemetry.get("oracle_task_enqueue_generic_last_caller_rva"),
        "task_enqueue_generic_last_arg0": telemetry.get("oracle_task_enqueue_generic_last_arg0"),
        "task_enqueue_generic_last_arg0_pointee": telemetry.get("oracle_task_enqueue_generic_last_arg0_pointee"),
        "task_enqueue_generic_last_arg1": telemetry.get("oracle_task_enqueue_generic_last_arg1"),
        "task_enqueue_generic_last_ret": telemetry.get("oracle_task_enqueue_generic_last_ret"),
        "task_enqueue_generic_sample0_caller_rva": telemetry.get("oracle_task_enqueue_generic_sample0_caller_rva"),
        "task_enqueue_generic_sample0_arg0": telemetry.get("oracle_task_enqueue_generic_sample0_arg0"),
        "task_enqueue_generic_sample0_arg0_pointee": telemetry.get("oracle_task_enqueue_generic_sample0_arg0_pointee"),
        "task_enqueue_generic_sample0_arg1": telemetry.get("oracle_task_enqueue_generic_sample0_arg1"),
        "task_enqueue_generic_sample0_ret": telemetry.get("oracle_task_enqueue_generic_sample0_ret"),
        "task_enqueue_generic_sample1_caller_rva": telemetry.get("oracle_task_enqueue_generic_sample1_caller_rva"),
        "task_enqueue_generic_sample1_arg0": telemetry.get("oracle_task_enqueue_generic_sample1_arg0"),
        "task_enqueue_generic_sample1_arg0_pointee": telemetry.get("oracle_task_enqueue_generic_sample1_arg0_pointee"),
        "task_enqueue_generic_sample1_arg1": telemetry.get("oracle_task_enqueue_generic_sample1_arg1"),
        "task_enqueue_generic_sample1_ret": telemetry.get("oracle_task_enqueue_generic_sample1_ret"),
        "task_enqueue_generic_idle_item_match_hits": telemetry.get("oracle_task_enqueue_generic_idle_item_match_hits"),
        "task_enqueue_generic_idle_item_last_match_kind": telemetry.get("oracle_task_enqueue_generic_idle_item_last_match_kind"),
        "menu_window_idle_ctor_last_caller_rva": telemetry.get("oracle_menu_window_idle_ctor_last_caller_rva"),
        "menu_window_idle_ctor_last_item": telemetry.get("oracle_menu_window_idle_ctor_last_item"),
        "menu_window_idle_ctor_last_vt": telemetry.get("oracle_menu_window_idle_ctor_last_vt"),
        "menu_window_idle_ctor_last_docall": telemetry.get("oracle_menu_window_idle_ctor_last_docall"),
        "menu_window_idle_ctor_last_accept": telemetry.get("oracle_menu_window_idle_ctor_last_accept"),
        "menu_item_update_hits": telemetry.get("oracle_menu_item_update_hits"),
        "menu_item_update_semantic_hits": telemetry.get("oracle_menu_item_update_semantic_hits"),
        "menu_item_update_last_item": telemetry.get("oracle_menu_item_update_last_item"),
        "menu_item_update_last_vt": telemetry.get("oracle_menu_item_update_last_vt"),
        "menu_item_update_last_docall": telemetry.get("oracle_menu_item_update_last_docall"),
        "menu_item_update_last_accept": telemetry.get("oracle_menu_item_update_last_accept"),
        "menu_continue_candidate_item": telemetry.get("oracle_menu_continue_candidate_item"),
        "menu_continue_candidate_hits": telemetry.get("oracle_menu_continue_candidate_hits"),
        "menu_continue_candidate_idle_accept_hits": telemetry.get("oracle_menu_continue_candidate_idle_accept_hits"),
        "menu_continue_candidate_native_accept_hits": telemetry.get("oracle_menu_continue_candidate_native_accept_hits"),
        "menu_continue_candidate_other_accept_hits": telemetry.get("oracle_menu_continue_candidate_other_accept_hits"),
        "menu_continue_candidate_accept_changes": telemetry.get("oracle_menu_continue_candidate_accept_changes"),
        "menu_continue_candidate_last_accept": telemetry.get("oracle_menu_continue_candidate_last_accept"),
        "title_native_ready_hits": telemetry.get("oracle_title_native_ready_hits"),
        "title_native_ready_last_caller_rva": telemetry.get("oracle_title_native_ready_last_caller_rva"),
        "title_native_ready_last_this": telemetry.get("oracle_title_native_ready_last_this"),
        "title_native_ready_last_object": telemetry.get("oracle_title_native_ready_last_object"),
        "title_native_ready_last_flags": telemetry.get("oracle_title_native_ready_last_flags"),
        "title_native_ready_last_masked": telemetry.get("oracle_title_native_ready_last_masked"),
        "title_native_ready_last_ret": telemetry.get("oracle_title_native_ready_last_ret"),
        "title_langselect_ready_last_object": telemetry.get("oracle_title_langselect_ready_last_object"),
        "title_langselect_ready_last_masked": telemetry.get("oracle_title_langselect_ready_last_masked"),
        "title_langselect_ready_last_ret": telemetry.get("oracle_title_langselect_ready_last_ret"),
        "native_submit_entered": telemetry_native_submit_entered(telemetry),
        "native_result_chain_ready": telemetry_native_result_chain_ready(telemetry),
        "native_continue_chain_stage": native_stage,
        "result_event_handler_hits": telemetry.get("oracle_result_event_handler_hits"),
        "result_action_builder_hits": telemetry.get("oracle_result_action_builder_hits"),
        "result_action_wrapper_builder_hits": telemetry.get("oracle_result_action_wrapper_builder_hits"),
        "result_action_insert_hits": telemetry.get("oracle_result_action_insert_hits"),
    }


def oracle_summary(
    telemetry: dict[str, Any] | None,
    expected_save_oracle: dict[str, Any] | None = None,
    expected_animation_id: int | None = None,
) -> dict[str, Any]:
    if telemetry is None:
        return {}
    fields = expected_save_fields(expected_save_oracle)
    expected = {
        "save_source_path": expected_save_oracle.get("source_path") if isinstance(expected_save_oracle, dict) else None,
        "save_slot": expected_save_oracle.get("slot") if isinstance(expected_save_oracle, dict) else None,
        "character_name": fields.get("name"),
        "character_name_len": fields.get("name_len"),
        "character_level": fields.get("level"),
        "character_hp": fields.get("health"),
        "character_stats": fields.get("stats"),
        "saved_map_c30": fields.get("saved_map_c30"),
        "animation_id": expected_animation_id,
    }
    expected["character_name_empty_like"] = name_empty_like(expected["character_name"])
    observed = {
        "character_name": telemetry.get("oracle_char_name"),
        "character_name_len": telemetry.get("oracle_char_name_len"),
        "character_level": telemetry.get("oracle_char_level"),
        "character_hp": telemetry.get("oracle_char_current_hp"),
        "character_stats": telemetry.get("oracle_char_stats"),
        "saved_map_c30": telemetry.get("oracle_saved_map_c30"),
        "save_slot": telemetry.get("game_save_slot"),
        "animation_id": telemetry.get("current_animation_id"),
        "postload_popup_seen": telemetry.get("oracle_postload_modal_seen"),
        "postload_popup_builds": telemetry.get("oracle_msgbox_postload_builds"),
        "blocking_modal_present": telemetry.get("oracle_blocking_modal_present"),
        "simulated_button_presses_total": telemetry.get("simulated_button_presses_total"),
        "policy_window_backing_flag_ptr": telemetry.get("oracle_policy_window_backing_flag_ptr"),
        "policy_window_stored_backing_flag_ptr": telemetry.get("oracle_policy_window_stored_backing_flag_ptr"),
        "policy_window_backing_flag_value": telemetry.get("oracle_policy_window_backing_flag_value"),
        "policy_window_requested_flag_value": telemetry.get("oracle_policy_window_requested_flag_value"),
        "policy_window_caller_rva": telemetry.get("oracle_policy_window_caller_rva"),
        "policy_ctor_wrapper_hits": telemetry.get("oracle_policy_ctor_wrapper_hits"),
        "policy_ctor_wrapper_original_this": telemetry.get("oracle_policy_ctor_wrapper_original_this"),
        "policy_ctor_wrapper_original_vtable": telemetry.get("oracle_policy_ctor_wrapper_original_vtable"),
        "policy_ctor_wrapper_record_id": telemetry.get("oracle_policy_ctor_wrapper_record_id"),
        "policy_ctor_wrapper_backing_flag_ptr": telemetry.get("oracle_policy_ctor_wrapper_backing_flag_ptr"),
        "policy_ctor_wrapper_caller_rva": telemetry.get("oracle_policy_ctor_wrapper_caller_rva"),
        "policy_selector_wrapper_hits": telemetry.get("oracle_policy_selector_wrapper_hits"),
        "policy_selector_wrapper_original_this": telemetry.get("oracle_policy_selector_wrapper_original_this"),
        "policy_selector_wrapper_original_vtable": telemetry.get("oracle_policy_selector_wrapper_original_vtable"),
        "policy_selector_wrapper_owner": telemetry.get("oracle_policy_selector_wrapper_owner"),
        "policy_selector_wrapper_requested_flag": telemetry.get("oracle_policy_selector_wrapper_requested_flag"),
        "policy_selector_wrapper_selector_arg": telemetry.get("oracle_policy_selector_wrapper_selector_arg"),
        "policy_selector_wrapper_caller_rva": telemetry.get("oracle_policy_selector_wrapper_caller_rva"),
        "policy_selector_ctor_hits": telemetry.get("oracle_policy_selector_ctor_hits"),
        "policy_selector_ctor_owner": telemetry.get("oracle_policy_selector_ctor_owner"),
        "policy_selector_ctor_requested_flag_ptr": telemetry.get("oracle_policy_selector_ctor_requested_flag_ptr"),
        "policy_selector_ctor_requested_flag_value": telemetry.get("oracle_policy_selector_ctor_requested_flag_value"),
        "policy_selector_ctor_selector_arg": telemetry.get("oracle_policy_selector_ctor_selector_arg"),
        "policy_selector_ctor_stored_selector_arg": telemetry.get("oracle_policy_selector_ctor_stored_selector_arg"),
        "policy_selector_ctor_stored_requested_flag_ptr": telemetry.get("oracle_policy_selector_ctor_stored_requested_flag_ptr"),
        "policy_selector_ctor_caller_rva": telemetry.get("oracle_policy_selector_ctor_caller_rva"),
        "native_submit_hits": telemetry.get("oracle_native_submit_hits"),
        "native_submit_last_result": telemetry.get("oracle_native_submit_last_result"),
        "result_event_handler_hits": telemetry.get("oracle_result_event_handler_hits"),
        "result_action_builder_hits": telemetry.get("oracle_result_action_builder_hits"),
        "result_event_last_event": telemetry.get("oracle_result_event_last_event"),
        "result_event_last_raw_qword0": telemetry.get("oracle_result_event_last_raw_qword0"),
        "result_event_last_fd4_code": telemetry.get("oracle_result_event_last_fd4_code"),
        "result_event_last_fd4_arg": telemetry.get("oracle_result_event_last_fd4_arg"),
        "result_action_last_event": telemetry.get("oracle_result_action_last_event"),
        "result_action_last_word0": telemetry.get("oracle_result_action_last_word0"),
        "result_action_last_word1": telemetry.get("oracle_result_action_last_word1"),
        "result_action_wrapper_builder_hits": telemetry.get("oracle_result_action_wrapper_builder_hits"),
        "result_action_last_wrapper_builder_rcx": telemetry.get("oracle_result_action_last_wrapper_builder_rcx"),
        "result_action_last_wrapper_builder_rdx": telemetry.get("oracle_result_action_last_wrapper_builder_rdx"),
        "result_action_last_wrapper_builder_r8": telemetry.get("oracle_result_action_last_wrapper_builder_r8"),
        "result_action_last_wrapper_builder_ret": telemetry.get("oracle_result_action_last_wrapper_builder_ret"),
        "result_action_last_wrapper_builder_ret_update_rva": telemetry.get("oracle_result_action_last_wrapper_builder_ret_update_rva"),
        "result_action_insert_hits": telemetry.get("oracle_result_action_insert_hits"),
        "result_action_last_insert_arg0": telemetry.get("oracle_result_action_last_insert_arg0"),
        "result_action_last_insert_arg1": telemetry.get("oracle_result_action_last_insert_arg1"),
        "result_action_last_insert_ret": telemetry.get("oracle_result_action_last_insert_ret"),
        "result_action_last_insert_arg1_update_rva": telemetry.get("oracle_result_action_last_insert_arg1_update_rva"),
        "result_action_last_insert_ret_update_rva": telemetry.get("oracle_result_action_last_insert_ret_update_rva"),
        "policy_status_predicate_hits": telemetry.get("oracle_policy_status_predicate_hits"),
        "policy_status_predicate_flag_value": telemetry.get("oracle_policy_status_predicate_flag_value"),
        "policy_status_predicate_ret": telemetry.get("oracle_policy_status_predicate_ret"),
        "policy_status_predicate_caller_rva": telemetry.get("oracle_policy_status_predicate_caller_rva"),
        "policy_flag_setter_hits": telemetry.get("oracle_policy_flag_setter_hits"),
        "policy_flag_setter_value": telemetry.get("oracle_policy_flag_setter_value"),
        "policy_flag_setter_force": telemetry.get("oracle_policy_flag_setter_force"),
        "policy_flag_setter_before": telemetry.get("oracle_policy_flag_setter_before"),
        "policy_flag_setter_after": telemetry.get("oracle_policy_flag_setter_after"),
        "policy_flag_setter_caller_rva": telemetry.get("oracle_policy_flag_setter_caller_rva"),
        "title_native_menu_visual_suppress_installed": telemetry.get("oracle_title_native_menu_visual_suppress_installed"),
        "title_native_menu_visual_suppressed_builds": telemetry.get("oracle_title_native_menu_visual_suppressed_builds"),
        "title_native_menu_visual_any_suppressed": telemetry.get("oracle_title_native_menu_visual_any_suppressed"),
        "title_native_menu_visual_last_caller_rva": telemetry.get("oracle_title_native_menu_visual_last_caller_rva"),
        "title_custom_cover_profile_select_builds": telemetry.get("oracle_title_custom_cover_profile_select_builds"),
        "title_custom_cover_profile_select_any_built": telemetry.get("oracle_title_custom_cover_profile_select_any_built"),
        "title_custom_cover_profile_select_last_job": telemetry.get("oracle_title_custom_cover_profile_select_last_job"),
        "title_custom_cover_profile_select_last_caller_rva": telemetry.get("oracle_title_custom_cover_profile_select_last_caller_rva"),
        "continue_phase": telemetry.get("oracle_continue_phase"),
        "continue_expected_slot": telemetry.get("oracle_continue_expected_slot"),
        "continue_mount_c30": telemetry.get("oracle_continue_mount_c30"),
        "continue_guard_waits": telemetry.get("oracle_continue_guard_waits"),
    }
    observed["character_name_empty_like"] = name_empty_like(observed["character_name"])
    return {
        "character_name": observed["character_name"],
        "observed": observed,
        "expected": expected,
        "expected_save_match": telemetry_expected_save_match(telemetry, expected_save_oracle),
        "expected_animation_match": telemetry_expected_animation_match(telemetry, expected_animation_id),
        "native_submit_entered": telemetry_native_submit_entered(telemetry),
        "native_result_chain_same_result": telemetry_native_result_chain_same_result(telemetry),
        "native_submit_fd4_event_match": telemetry_native_submit_fd4_event_match(telemetry),
        "native_result_chain_ready": telemetry_native_result_chain_ready(telemetry),
        "result_action_wrapper_built": telemetry_result_action_wrapper_built(telemetry),
        "result_action_wrapper_has_update_rva": telemetry_result_action_wrapper_has_update_rva(telemetry),
        "result_action_inserted": telemetry_result_action_inserted(telemetry),
        "result_action_insert_has_update_rva": telemetry_result_action_insert_has_update_rva(telemetry),
        "native_continue_chain_stage": telemetry_native_continue_chain_stage(telemetry),
        "no_postload_popup": telemetry_no_postload_popup(telemetry),
    }


def telemetry_real_character_loaded(telemetry: dict[str, Any]) -> bool:
    stats = telemetry.get("oracle_char_stats")
    name = telemetry.get("oracle_char_name")
    name_len = as_int(
        telemetry.get("oracle_char_name_len"),
        len(name) if isinstance(name, str) else 0,
    )
    return bool(
        isinstance(name, str)
        and not name_empty_like(name)
        and name_len >= MIN_REAL_CHARACTER_NAME_LEN
        and len(name) >= MIN_REAL_CHARACTER_NAME_LEN
        and as_int(telemetry.get("oracle_char_level"), 0) >= MIN_REAL_CHARACTER_LEVEL
        and as_int(telemetry.get("oracle_char_current_hp"), 0) >= MIN_REAL_CHARACTER_HP
        and isinstance(stats, list)
        and len(stats) >= MIN_EXPECTED_STAT_COUNT
    )


def telemetry_render_semantic_ready(telemetry: dict[str, Any]) -> bool:
    return bool(
        telemetry.get("oracle_player_render_ready") is True
        or (
            telemetry.get("oracle_chr_model_ins_present") is True
            and telemetry.get("oracle_chr_ctrl_present") is True
            and telemetry.get("oracle_chr_onscreen") is True
            and telemetry.get("oracle_chr_render_group_enabled") is True
            and telemetry.get("oracle_chr_enable_render") is True
        )
    )


def telemetry_world_loaded(
    telemetry: dict[str, Any] | None,
    expected_save_oracle: dict[str, Any] | None = None,
    expected_animation_id: int | None = None,
) -> bool:
    if telemetry is None or telemetry.get("game_man_instance_resolved") is not True:
        return False
    player_seen = telemetry.get("player_available") is True or telemetry.get("player_seen") is True
    load_in_progress_clear = as_int(telemetry.get("oracle_load_in_progress_b80"), -1) == 0
    saved_map_present = as_int(telemetry.get("oracle_saved_map_c30"), -1) != -1
    canonical_world_clear = telemetry.get("oracle_grounded") is True or as_int(telemetry.get("oracle_now_loading"), -1) == 0
    real_character_loaded = telemetry_real_character_loaded(telemetry)
    semantic_world_clear = real_character_loaded and telemetry_render_semantic_ready(telemetry)
    return bool(
        player_seen
        and telemetry.get("oracle_player_present") is True
        and telemetry.get("oracle_block_id_valid") is True
        and load_in_progress_clear
        and saved_map_present
        and telemetry_no_postload_popup(telemetry)
        and real_character_loaded
        and telemetry_expected_save_match(telemetry, expected_save_oracle)
        and telemetry_expected_animation_match(telemetry, expected_animation_id)
        and (canonical_world_clear or semantic_world_clear)
    )


def telemetry_world_tick(telemetry: dict[str, Any], fallback: int) -> int:
    return as_int(telemetry.get("game_task_ticks"), fallback)


def visual_loading_screen_visible(artifact_dir: Path, windows: list[dict[str, Any]], sample: int, window_class: str = DEFAULT_WINDOW_CLASS) -> bool:
    check_path = artifact_dir / f"world-stable-visual-check-{sample:03d}.json"
    result: dict[str, Any] = {"sample": sample, "loading_screen_detected": True}
    try:
        if not windows:
            result["error"] = "no_target_window"
            return True
        grim = shutil.which("grim")
        tesseract = shutil.which("tesseract")
        magick = shutil.which("magick") or shutil.which("convert")
        if not grim or not tesseract or not magick:
            result["error"] = "missing_visual_check_tool"
            result["grim"] = grim
            result["tesseract"] = tesseract
            result["magick"] = magick
            return True
        win = windows[0]
        result["window"] = win
        capture_problems = target_window_capture_problems(win, window_class)
        result["target_window_capture_valid"] = not capture_problems
        result["target_window_capture_problems"] = capture_problems
        if capture_problems:
            result["error"] = "target_window_not_capture_safe"
            return True
        at = win.get("at") or []
        size = win.get("size") or []
        geom = f"{int(at[0])},{int(at[1])} {int(size[0])}x{int(size[1])}"
        png = artifact_dir / f"world-stable-visual-check-{sample:03d}.png"
        grim_run = subprocess.run([grim, "-g", geom, str(png)], capture_output=True, text=True, timeout=VISUAL_CHECK_SUBPROCESS_TIMEOUT_SECONDS)
        result["geom"] = geom
        result["grim_rc"] = grim_run.returncode
        result["grim_stderr"] = grim_run.stderr.strip()
        if grim_run.returncode != 0 or not png.exists():
            result["error"] = "grim_failed"
            return True
        ocr_results = []
        text_parts = []
        for crop_name, crop in VISUAL_OCR_CROPS:
            ocr_png = artifact_dir / f"world-stable-visual-check-{sample:03d}.{crop_name}.ocr.png"
            magick_run = subprocess.run(
                [magick, str(png), "-crop", crop, "-resize", "300%", "-colorspace", "Gray", "-normalize", "-level", "20%,85%", "-sharpen", "0x1", str(ocr_png)],
                capture_output=True,
                text=True,
                timeout=VISUAL_CHECK_SUBPROCESS_TIMEOUT_SECONDS,
            )
            ocr_input = ocr_png if ocr_png.exists() else png
            ocr_run = subprocess.run(
                [tesseract, str(ocr_input), "stdout", "--psm", "6"],
                capture_output=True,
                text=True,
                timeout=VISUAL_CHECK_SUBPROCESS_TIMEOUT_SECONDS,
            )
            text_parts.append(ocr_run.stdout)
            ocr_results.append(
                {
                    "crop": crop_name,
                    "geometry": crop,
                    "magick_rc": magick_run.returncode,
                    "magick_stderr": magick_run.stderr.strip(),
                    "ocr_rc": ocr_run.returncode,
                    "ocr_stderr": ocr_run.stderr.strip(),
                    "ocr_preview": ocr_run.stdout[:VISUAL_OCR_PREVIEW_CHARS],
                }
            )
        text = "\n".join(text_parts)
        matches = [pattern.pattern for pattern in LOADING_SCREEN_OCR_PATTERNS if pattern.search(text)]
        loading = bool(matches)
        result.update(
            {
                "loading_screen_detected": loading,
                "ocr_results": ocr_results,
                "ocr_matches": matches,
                "ocr_preview": text[:VISUAL_OCR_PREVIEW_CHARS],
            }
        )
        return loading
    except Exception as exc:
        result["error"] = repr(exc)
        return True
    finally:
        check_path.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def legal_popup_ocr_matches(text: str) -> list[str]:
    return [pattern.pattern for pattern in LEGAL_POPUP_OCR_PATTERNS if pattern.search(text)]


def save_data_popup_ocr_matches(text: str) -> list[str]:
    return [pattern.pattern for pattern in SAVE_DATA_POPUP_OCR_PATTERNS if pattern.search(text)]


def visual_legal_popup_visible(artifact_dir: Path, windows: list[dict[str, Any]], sample: int, window_class: str = DEFAULT_WINDOW_CLASS) -> bool:
    check_path = artifact_dir / f"legal-popup-check-{sample:03d}.json"
    result: dict[str, Any] = {"sample": sample, "legal_popup_detected": False}
    try:
        if not windows:
            result["error"] = "no_target_window"
            return False
        grim = shutil.which("grim")
        tesseract = shutil.which("tesseract")
        magick = shutil.which("magick") or shutil.which("convert")
        result["capture_tools"] = {"grim": grim, "tesseract": tesseract, "magick": magick}
        if not grim or not tesseract or not magick:
            result["error"] = "missing_visual_check_tool"
            return False
        win = windows[0]
        result["window"] = win
        capture_problems = target_window_capture_problems(win, window_class)
        result["target_window_capture_valid"] = not capture_problems
        result["target_window_capture_problems"] = capture_problems
        if capture_problems:
            result["error"] = "target_window_not_capture_safe"
            return False
        at = win.get("at") or []
        size = win.get("size") or []
        geom = f"{int(at[0])},{int(at[1])} {int(size[0])}x{int(size[1])}"
        png = artifact_dir / f"legal-popup-check-{sample:03d}.png"
        grim_run = subprocess.run(
            [grim, "-g", geom, str(png)],
            capture_output=True,
            text=True,
            timeout=VISUAL_CHECK_SUBPROCESS_TIMEOUT_SECONDS,
        )
        result["geom"] = geom
        result["png"] = str(png)
        result["grim_rc"] = grim_run.returncode
        result["grim_stderr"] = grim_run.stderr.strip()
        if grim_run.returncode != 0 or not png.exists():
            result["error"] = "grim_failed"
            return False
        ocr_results = []
        text_parts = []
        for crop_name, crop in VISUAL_LEGAL_OCR_CROPS:
            ocr_png = artifact_dir / f"legal-popup-check-{sample:03d}.{crop_name}.ocr.png"
            magick_run = subprocess.run(
                [
                    magick,
                    str(png),
                    "-crop",
                    crop,
                    "-resize",
                    "300%",
                    "-colorspace",
                    "Gray",
                    "-normalize",
                    "-level",
                    "20%,85%",
                    "-sharpen",
                    "0x1",
                    str(ocr_png),
                ],
                capture_output=True,
                text=True,
                timeout=VISUAL_CHECK_SUBPROCESS_TIMEOUT_SECONDS,
            )
            ocr_input = ocr_png if ocr_png.exists() else png
            ocr_run = subprocess.run(
                [tesseract, str(ocr_input), "stdout", "--psm", "6"],
                capture_output=True,
                text=True,
                timeout=VISUAL_CHECK_SUBPROCESS_TIMEOUT_SECONDS,
            )
            text_parts.append(ocr_run.stdout)
            ocr_results.append(
                {
                    "crop": crop_name,
                    "geometry": crop,
                    "magick_rc": magick_run.returncode,
                    "magick_stderr": magick_run.stderr.strip(),
                    "ocr_rc": ocr_run.returncode,
                    "ocr_stderr": ocr_run.stderr.strip(),
                    "ocr_preview": ocr_run.stdout[:VISUAL_OCR_PREVIEW_CHARS],
                }
            )
        text = "\n".join(text_parts)
        matches = legal_popup_ocr_matches(text)
        detected = bool(matches)
        result.update(
            {
                "legal_popup_detected": detected,
                "ocr_results": ocr_results,
                "ocr_matches": matches,
                "ocr_preview": text[:VISUAL_OCR_PREVIEW_CHARS],
            }
        )
        return detected
    except Exception as exc:
        result["error"] = repr(exc)
        return False
    finally:
        check_path.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def visual_save_data_popup_visible(artifact_dir: Path, windows: list[dict[str, Any]], sample: int, window_class: str = DEFAULT_WINDOW_CLASS) -> bool:
    check_path = artifact_dir / f"save-data-popup-check-{sample:03d}.json"
    result: dict[str, Any] = {"sample": sample, "save_data_popup_detected": False}
    try:
        if not windows:
            result["error"] = "no_target_window"
            return False
        grim = shutil.which("grim")
        tesseract = shutil.which("tesseract")
        magick = shutil.which("magick") or shutil.which("convert")
        result["capture_tools"] = {"grim": grim, "tesseract": tesseract, "magick": magick}
        if not grim or not tesseract or not magick:
            result["error"] = "missing_visual_check_tool"
            return False
        win = windows[0]
        result["window"] = win
        capture_problems = target_window_capture_problems(win, window_class)
        result["target_window_capture_valid"] = not capture_problems
        result["target_window_capture_problems"] = capture_problems
        if capture_problems:
            result["error"] = "target_window_not_capture_safe"
            return False
        at = win.get("at") or []
        size = win.get("size") or []
        geom = f"{int(at[0])},{int(at[1])} {int(size[0])}x{int(size[1])}"
        png = artifact_dir / f"save-data-popup-check-{sample:03d}.png"
        grim_run = subprocess.run(
            [grim, "-g", geom, str(png)],
            capture_output=True,
            text=True,
            timeout=VISUAL_CHECK_SUBPROCESS_TIMEOUT_SECONDS,
        )
        result["geom"] = geom
        result["png"] = str(png)
        result["window"] = win
        result["grim_rc"] = grim_run.returncode
        result["grim_stderr"] = grim_run.stderr.strip()
        if grim_run.returncode != 0 or not png.exists():
            result["error"] = "grim_failed"
            return False
        ocr_results = []
        text_parts = []
        for crop_name, crop in VISUAL_LEGAL_OCR_CROPS:
            ocr_png = artifact_dir / f"save-data-popup-check-{sample:03d}.{crop_name}.ocr.png"
            magick_run = subprocess.run(
                [magick, str(png), "-crop", crop, "-resize", "300%", "-colorspace", "Gray", "-normalize", "-level", "20%,85%", "-sharpen", "0x1", str(ocr_png)],
                capture_output=True,
                text=True,
                timeout=VISUAL_CHECK_SUBPROCESS_TIMEOUT_SECONDS,
            )
            ocr_input = ocr_png if ocr_png.exists() else png
            ocr_run = subprocess.run(
                [tesseract, str(ocr_input), "stdout", "--psm", "6"],
                capture_output=True,
                text=True,
                timeout=VISUAL_CHECK_SUBPROCESS_TIMEOUT_SECONDS,
            )
            text_parts.append(ocr_run.stdout)
            ocr_results.append(
                {
                    "crop": crop_name,
                    "geometry": crop,
                    "magick_rc": magick_run.returncode,
                    "magick_stderr": magick_run.stderr.strip(),
                    "ocr_rc": ocr_run.returncode,
                    "ocr_stderr": ocr_run.stderr.strip(),
                    "ocr_preview": ocr_run.stdout[:VISUAL_OCR_PREVIEW_CHARS],
                }
            )
        text = "\n".join(text_parts)
        matches = save_data_popup_ocr_matches(text)
        detected = bool(matches)
        result.update({"save_data_popup_detected": detected, "ocr_results": ocr_results, "ocr_matches": matches, "ocr_preview": text[:VISUAL_OCR_PREVIEW_CHARS]})
        return detected
    except Exception as exc:
        result["error"] = repr(exc)
        return False
    finally:
        check_path.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def focus_target_window(window_class: str) -> None:
    hyprctl = shutil.which("hyprctl")
    if not hyprctl:
        return
    selector = f"class:^{re.escape(window_class)}$"
    subprocess.run(
        [hyprctl, "dispatch", "focuswindow", selector],
        capture_output=True,
        text=True,
        timeout=OBSERVATION_SUBPROCESS_TIMEOUT_SECONDS,
        check=False,
    )


def hypr_windows(window_class: str) -> list[dict[str, Any]]:
    try:
        clients = json.loads(
            subprocess.check_output(
                ["hyprctl", "clients", "-j"],
                text=True,
                timeout=OBSERVATION_SUBPROCESS_TIMEOUT_SECONDS,
            )
        )
    except Exception:
        return []
    if not isinstance(clients, list):
        return []
    matches: list[dict[str, Any]] = []
    for client in clients:
        if not isinstance(client, dict):
            continue
        title = str(client.get("title") or "")
        klass = str(client.get("class") or "")
        if client_is_game_window(client, window_class):
            matches.append(
                {
                    "class": klass,
                    "title": title,
                    "workspace": (client.get("workspace") or {}).get("name"),
                    "at": client.get("at"),
                    "size": client.get("size"),
                    "pid": client.get("pid"),
                    "mapped": client.get("mapped"),
                    "hidden": client.get("hidden"),
                    "focusHistoryID": client.get("focusHistoryID"),
                    "fullscreen": client.get("fullscreen"),
                    "address": client.get("address"),
                }
            )
    return matches


def classify_snapshot(
    *,
    pid: int,
    process_running: bool,
    telemetry: dict[str, Any] | None,
    bootstrap: dict[str, Any] | None,
    windows: list[dict[str, Any]],
    window_stale_polls: int,
    window_stale_poll_budget: int,
    polls: int,
    target: str = TARGET_GAME_MAN,
    autoload_attempt_budget: int = DEFAULT_AUTOLOAD_ATTEMPT_BUDGET,
    post_request_tick_budget: int = DEFAULT_POST_REQUEST_TICK_BUDGET,
) -> ReadinessResult | None:
    if not process_running:
        return ReadinessResult(False, PROCESS_EXITED, pid, bootstrap, telemetry, windows, polls)
    if telemetry is not None and telemetry.get("game_man_instance_resolved") is True:
        if target == TARGET_GAME_MAN:
            return ReadinessResult(True, READY_REASON, pid, bootstrap, telemetry, windows, polls)
        if target == TARGET_WORLD_STABLE:
            return None
        if telemetry.get("autoload_slot") is None:
            return ReadinessResult(False, AUTOLOAD_SLOT_MISSING, pid, bootstrap, telemetry, windows, polls)
        player_seen = telemetry.get("player_available") is True or telemetry.get("player_seen") is True
        player_playable = player_seen and (
            "oracle_block_id_valid" not in telemetry or telemetry.get("oracle_block_id_valid") is True
        )
        if target == TARGET_PLAYER_LOAD and player_playable:
            return ReadinessResult(True, PLAYER_AVAILABLE, pid, bootstrap, telemetry, windows, polls)
        status = str(telemetry.get("autoload_last_status") or "")
        if (
            status.startswith("direct continue sequence requested")
            or status.startswith("direct map load requested")
            or status.startswith("direct combined load requested")
            or status.startswith("direct combined-only load requested")
            or status.startswith("direct bootstrap combined load requested")
            or status.startswith("direct bootstrap pump requested")
            or status.startswith("direct trace sequence requested")
            or status.startswith("direct menu wrapper requested")
        ):
            if target == TARGET_AUTOLOAD_REQUEST:
                return ReadinessResult(True, AUTOLOAD_REQUESTED, pid, bootstrap, telemetry, windows, polls)
            if player_playable:
                return ReadinessResult(True, PLAYER_AVAILABLE, pid, bootstrap, telemetry, windows, polls)
            game_task_ticks = int(telemetry.get("game_task_ticks") or 0)
            if target == TARGET_REQUEST_CONSUMPTION and telemetry.get("title_handoff_complete") is True:
                return ReadinessResult(True, TITLE_HANDOFF_COMPLETE, pid, bootstrap, telemetry, windows, polls)
            if game_task_ticks >= post_request_tick_budget:
                reason = (
                    PLAYER_LOAD_TICK_BUDGET_REACHED
                    if target == TARGET_PLAYER_LOAD
                    else POST_REQUEST_TICK_BUDGET_REACHED
                )
                return ReadinessResult(
                    False,
                    reason,
                    pid,
                    bootstrap,
                    telemetry,
                    windows,
                    polls,
                )
        attempts = int(telemetry.get("autoload_attempts") or 0)
        if attempts >= autoload_attempt_budget:
            return ReadinessResult(
                False,
                AUTOLOAD_ATTEMPT_BUDGET_REACHED,
                pid,
                bootstrap,
                telemetry,
                windows,
                polls,
            )
    if (
        telemetry is not None
        and telemetry.get("game_man_instance_resolved") is not True
        and windows
        and window_stale_polls >= window_stale_poll_budget
    ):
        return ReadinessResult(
            False,
            TELEMETRY_WITHOUT_GAME_MAN,
            pid,
            bootstrap,
            telemetry,
            windows,
            polls,
        )
    if not windows:
        return None
    if bootstrap is None:
        if window_stale_polls >= window_stale_poll_budget:
            return ReadinessResult(False, WINDOW_WITHOUT_BOOTSTRAP, pid, bootstrap, telemetry, windows, polls)
        return None
    stage = str(bootstrap.get("stage") or "")
    if telemetry is None and stage in TELEMETRY_READY_STAGES and window_stale_polls >= window_stale_poll_budget:
        return ReadinessResult(False, WINDOW_WITHOUT_TELEMETRY, pid, bootstrap, telemetry, windows, polls)
    if stage not in TASK_READY_STAGES and window_stale_polls >= window_stale_poll_budget:
        return ReadinessResult(False, WINDOW_WITHOUT_TASK, pid, bootstrap, telemetry, windows, polls)
    return None


def wait_readiness(args: argparse.Namespace, timing: TimingTracker) -> ReadinessResult:
    pattern = re.compile(args.process_pattern, re.I)
    expected_save_oracle = read_json(args.expected_save_oracle) if args.expected_save_oracle else None
    deadline = time.monotonic() + float(args.max_runtime_seconds)
    pid, reason, spawn_polls = select_runtime_pid(
        pattern,
        args.pid_file,
        args.spawn_poll_budget,
        deadline,
        allow_async_launcher_exit=args.allow_async_launcher_exit,
    )
    if pid is None:
        return ReadinessResult(False, reason, None, None, None, [], spawn_polls, float(args.max_runtime_seconds))

    window_stale_polls = 0
    world_stable_samples = 0
    legal_popup_samples = 0
    next_legal_popup_check_at = 0.0
    save_data_popup_samples = 0
    next_save_data_popup_check_at = 0.0
    last_world_stable_tick: int | None = None
    world_stable_since: float | None = None
    # The expected-animation oracle confirms a GENUINE fresh load by the appear/spawn animation (e.g.
    # 4050). That is a TRANSIENT signal: the character plays it for a moment at spawn, then idles, so
    # current_animation_id stops matching. Requiring it on every world-stable dwell poll would flip the
    # world-loaded oracle false the instant the spawn animation ends, resetting the 5s dwell forever
    # (observed: world reached + grounded + stable mms_state for 12s yet world_stable_samples stayed 0).
    # Latch that the appear animation WAS observed once (correctness satisfied), then stop requiring it
    # for the dwell -- a player standing idle in the loaded world IS the playable world.
    appear_animation_seen = args.expected_animation_id is None
    # World-stream stall semaphore state: a monotonic progress watermark and the monotonic time it
    # last improved. Both stay None until the OWN-LOAD map-load arms (continue fired + block present).
    world_stream_watermark: tuple[int, int, int, int, int] | None = None
    world_stream_progress_since: float | None = None
    # Per-phase progress watchdog carry-state: which watched phase (title/continue) was last active,
    # its last progress watermark, and the monotonic time that watermark last improved. Reset on any
    # phase transition / forward progress so an inherently-slow-but-moving phase never trips.
    phase_watchdog_state: dict[str, Any] = {"phase": None, "value": None, "since": None}
    fps_samples: list[tuple[float, int]] = []  # (monotonic, game_task_ticks) for the fps semaphore
    for poll in range(args.readiness_poll_budget):
        if time.monotonic() >= deadline:
            return with_runtime_module_info(
                ReadinessResult(
                    False,
                    TIMEOUT_BUDGET_EXHAUSTED,
                    pid,
                    read_bootstrap(args.bootstrap, args.bootstrap_state),
                    read_json(args.telemetry),
                    hypr_windows(args.window_class),
                    spawn_polls + poll,
                    float(args.max_runtime_seconds),
                    expected_save_oracle=expected_save_oracle,
                    expected_animation_id=args.expected_animation_id,
                )
            )
        module_base, module_mappings = runtime_module_base(pid)
        process_running = process_exists(pid)
        if args.target == TARGET_MODULE_BASE and process_running and module_base is not None:
            return ReadinessResult(
                True,
                MODULE_BASE_READY,
                pid,
                read_bootstrap(args.bootstrap, args.bootstrap_state),
                read_json(args.telemetry),
                [],
                spawn_polls + poll,
                float(args.max_runtime_seconds),
                module_base,
                module_mappings,
            )
        telemetry = read_json(args.telemetry)
        bootstrap = read_bootstrap(args.bootstrap, args.bootstrap_state)
        # Milestone timing (deltas from the TRUE bash launch epoch). Record first telemetry / continue
        # fired / player present transitions; world-stable is marked at its dedicated success below.
        timing.observe(telemetry)
        # FAIL-FAST WORLD-LOAD DEADLINE: the world-loaded semaphore (player present / world-stable)
        # must be reached within --world-load-deadline-seconds of CONTINUE_FIRED (the load starting),
        # not bash launch -- so our ~24s boot+title latency doesn't eat the load budget and the
        # deadline can't preempt the precise 6s world_stream_stalled semaphore. Before continue fires
        # it's launch-anchored, so it also catches a continue-never-fires hang. Default 30s (room for a
        # real ER load after continue). Complementary to world_stream_stalled. Off: --no-world-load-deadline.
        if (
            args.world_load_deadline_exit
            and args.target == TARGET_WORLD_STABLE
            and process_exists(pid)
            and timing.world_load_deadline_exceeded(args.world_load_deadline_seconds)
        ):
            print(
                "er-readiness-watch: world-loaded semaphore not reached within "
                f"{args.world_load_deadline_seconds:g}s of continue_fired (load start; "
                "launch-anchored before continue); failing fast",
                file=sys.stderr,
            )
            return with_runtime_module_info(
                ReadinessResult(
                    False,
                    WORLD_LOAD_DEADLINE_EXCEEDED,
                    pid,
                    bootstrap,
                    telemetry,
                    hypr_windows(args.window_class),
                    spawn_polls + poll,
                    float(args.max_runtime_seconds),
                    expected_save_oracle=expected_save_oracle,
                    expected_animation_id=args.expected_animation_id,
                )
            )
        # FPS SEMAPHORE (fail fast on a sub-30-fps / throttled-or-hung game). The game-task tick
        # counter is an fps proxy; once it is clearly running (>= FPS_JUDGE_MIN_TICKS) we require a
        # sustained >= MIN_GAME_FPS over FPS_STALL_WINDOW_SECONDS, else treat it as a crash-class
        # failure (e.g. an unfocused window the engine throttles to a few fps). This avoids silently
        # spending the whole runtime budget on a non-representative run.
        ticks_now = as_int(telemetry.get("game_task_ticks"), 0) if telemetry is not None else 0
        now_fps = time.monotonic()
        if ticks_now >= FPS_JUDGE_MIN_TICKS:
            fps_samples.append((now_fps, ticks_now))
            fps_samples = [(t, k) for (t, k) in fps_samples if now_fps - t <= FPS_STALL_WINDOW_SECONDS]
            if len(fps_samples) >= 2:
                t0, k0 = fps_samples[0]
                dt = now_fps - t0
                if dt >= FPS_STALL_WINDOW_SECONDS:
                    fps = (ticks_now - k0) / dt if dt > 0 else 0.0
                    if fps < MIN_GAME_FPS:
                        print(
                            f"er-readiness-watch: game fps {fps:.1f} < {MIN_GAME_FPS} "
                            f"sustained over {dt:.1f}s (ticks {k0}->{ticks_now}); failing fast "
                            f"(likely unfocused-window throttle or a hang)",
                            file=sys.stderr,
                        )
                        return with_runtime_module_info(
                            ReadinessResult(
                                False,
                                GAME_FPS_BELOW_MIN,
                                pid,
                                bootstrap,
                                telemetry,
                                hypr_windows(args.window_class),
                                spawn_polls + poll,
                                float(args.max_runtime_seconds),
                                expected_save_oracle=expected_save_oracle,
                                expected_animation_id=args.expected_animation_id,
                            )
                        )
        actual_runtime_mode, seamless_mappings = observed_runtime_mode(pid, telemetry)
        if runtime_mode_definite_mismatch(args.expected_runtime_mode, actual_runtime_mode):
            return with_runtime_module_info(
                replace(
                    ReadinessResult(
                        False,
                        RUNTIME_MODE_MISMATCH,
                        pid,
                        bootstrap,
                        telemetry,
                        [],
                        spawn_polls + poll,
                        float(args.max_runtime_seconds),
                        expected_save_oracle=expected_save_oracle,
                        expected_animation_id=args.expected_animation_id,
                    ),
                    runtime_mode_expected=args.expected_runtime_mode,
                    runtime_mode_actual=actual_runtime_mode,
                    runtime_mode_match=False,
                    seamless_module_mappings=seamless_mappings,
                )
            )
        if args.fail_on_native_legal_popup and telemetry_native_legal_popup_detected(telemetry):
            return with_runtime_module_info(
                ReadinessResult(
                    False,
                    NATIVE_LEGAL_POPUP_DETECTED,
                    pid,
                    bootstrap,
                    telemetry,
                    [],
                    spawn_polls + poll,
                    float(args.max_runtime_seconds),
                    expected_save_oracle=expected_save_oracle,
                    expected_animation_id=args.expected_animation_id,
                )
            )
        # Evidence-driven teardown: a no-write cold_char_mount probe never reaches world-stable, so
        # exit the instant the mount reaches its terminal phase (success or timeout) rather than
        # idling to the wall-clock cap. The field is 0 on every non-cold-mount run, so this is inert
        # for those. ready=True so the run is not scored as a failure -- it is a completed diagnostic.
        if telemetry_cold_char_mount_complete(telemetry):
            return with_runtime_module_info(
                ReadinessResult(
                    True,
                    COLD_CHAR_MOUNT_COMPLETE,
                    pid,
                    bootstrap,
                    telemetry,
                    [],
                    spawn_polls + poll,
                    float(args.max_runtime_seconds),
                    expected_save_oracle=expected_save_oracle,
                    expected_animation_id=args.expected_animation_id,
                )
            )
        if args.fail_on_messagebox_dialog and telemetry_messagebox_dialog_detected(telemetry):
            return with_runtime_module_info(
                ReadinessResult(
                    False,
                    MESSAGEBOX_DIALOG_DETECTED,
                    pid,
                    bootstrap,
                    telemetry,
                    [],
                    spawn_polls + poll,
                    float(args.max_runtime_seconds),
                    expected_save_oracle=expected_save_oracle,
                    expected_animation_id=args.expected_animation_id,
                )
            )
        if args.fail_on_server_status_semaphore and telemetry_server_status_semaphore_detected(telemetry):
            return with_runtime_module_info(
                ReadinessResult(
                    False,
                    SERVER_STATUS_SEMAPHORE_DETECTED,
                    pid,
                    bootstrap,
                    telemetry,
                    [],
                    spawn_polls + poll,
                    float(args.max_runtime_seconds),
                    expected_save_oracle=expected_save_oracle,
                    expected_animation_id=args.expected_animation_id,
                )
            )
        windows = hypr_windows(args.window_class) if process_running else []
        if process_running and windows and (args.visual_legal_popup_check or args.visual_save_data_popup_check or args.visual_world_check):
            if not window_capture_safe(windows[0], args.window_class):
                focus_target_window(args.window_class)
                windows = hypr_windows(args.window_class)
            if not windows or not window_capture_safe(windows[0], args.window_class):
                if args.defer_unsafe_visual_capture_until_telemetry and telemetry is None:
                    os.sched_yield()
                    continue
                return with_runtime_module_info(
                    ReadinessResult(
                        False,
                        TARGET_WINDOW_CAPTURE_UNSAFE,
                        pid,
                        bootstrap,
                        telemetry,
                        windows,
                        spawn_polls + poll,
                        float(args.max_runtime_seconds),
                        expected_save_oracle=expected_save_oracle,
                        expected_animation_id=args.expected_animation_id,
                    )
                )
        now = time.monotonic()
        if process_running and args.visual_legal_popup_check and windows and now >= next_legal_popup_check_at:
            legal_popup_samples += 1
            next_legal_popup_check_at = now + args.visual_legal_popup_check_interval_seconds
            if visual_legal_popup_visible(args.artifact_dir, windows, legal_popup_samples, args.window_class):
                return with_runtime_module_info(
                    ReadinessResult(
                        False,
                        LEGAL_POPUP_DETECTED,
                        pid,
                        bootstrap,
                        telemetry,
                        windows,
                        spawn_polls + poll,
                        float(args.max_runtime_seconds),
                        expected_save_oracle=expected_save_oracle,
                        expected_animation_id=args.expected_animation_id,
                    )
                )
        if process_running and args.visual_save_data_popup_check and windows and now >= next_save_data_popup_check_at:
            save_data_popup_samples += 1
            next_save_data_popup_check_at = now + args.visual_save_data_popup_check_interval_seconds
            if visual_save_data_popup_visible(args.artifact_dir, windows, save_data_popup_samples, args.window_class):
                return with_runtime_module_info(
                    ReadinessResult(
                        False,
                        SAVE_DATA_POPUP_DETECTED,
                        pid,
                        bootstrap,
                        telemetry,
                        windows,
                        spawn_polls + poll,
                        float(args.max_runtime_seconds),
                        expected_save_oracle=expected_save_oracle,
                        expected_animation_id=args.expected_animation_id,
                    )
                )
        # PER-PHASE PROGRESS WATCHDOG: enforce "<=N s between any two consecutive progress semaphores"
        # for the previously-BLIND gaps (boot->title and the title_boot_ready continue wait). The
        # active phase (title or continue) must advance its monotonic watermark within
        # --phase-stall-seconds or we fail fast NAMING which phase wedged (title_stalled /
        # continue_stalled). Both phases ride game_task_ticks (advances every frame while alive), so a
        # slow-but-healthy boot/title/wait resets the timer every poll and never false-fails; only a
        # true freeze (no tick/scan/state advance for >window) trips. The map-load->world tail is
        # owned by the separate world_stream_stall_step below (its own richer watermark + window).
        if args.phase_watchdog and process_running:
            now_phase = time.monotonic()
            # Capture the active phase name BEFORE the step (the step's snapshot of which phase wedged).
            active_phase_now = active_watchdog_phase(telemetry)
            phase_stalled, phase_reason = phase_progress_stall_step(
                telemetry,
                phase_watchdog_state,
                now_phase,
                args.phase_stall_seconds,
            )
            if phase_stalled and phase_reason is not None:
                since = phase_watchdog_state.get("since")
                stuck_seconds = (now_phase - since) if since is not None else args.phase_stall_seconds
                stalled_phase = active_phase_now[0] if active_phase_now is not None else phase_reason
                print(
                    f"er-readiness-watch: phase '{stalled_phase}' progress watermark flat for "
                    f"{stuck_seconds:.1f}s >= {args.phase_stall_seconds:g}s (no game_task_ticks / "
                    "title-scan / state advance); failing fast",
                    file=sys.stderr,
                )
                return with_runtime_module_info(
                    replace(
                        ReadinessResult(
                            False,
                            phase_reason,
                            pid,
                            bootstrap,
                            telemetry,
                            hypr_windows(args.window_class),
                            spawn_polls + poll,
                            float(args.max_runtime_seconds),
                            expected_save_oracle=expected_save_oracle,
                            expected_animation_id=args.expected_animation_id,
                        ),
                        phase_progress_stall=phase_progress_stall_snapshot(
                            telemetry, stalled_phase, stuck_seconds
                        ),
                    )
                )
        # World-stream stall semaphore: once the OWN-LOAD map-load has begun, track a monotonic
        # progress watermark; if it does not strictly improve within --world-stream-stall-seconds
        # (and the world is not yet stable), the player map block is wedged (phase stuck at 2, zero
        # IO inflight, player never present) -- tear down fast instead of burning the runtime cap.
        # Resetting the timer on ANY forward progress means a healthy ~2-5s stream never trips it.
        if args.world_stream_stall_exit and args.target == TARGET_WORLD_STABLE and process_running:
            now_stream = time.monotonic()
            world_stream_watermark, world_stream_progress_since, world_stream_stalled = world_stream_stall_step(
                telemetry,
                world_stream_watermark,
                world_stream_progress_since,
                now_stream,
                args.world_stream_stall_seconds,
            )
            if world_stream_stalled and world_stream_progress_since is not None:
                stuck_seconds = now_stream - world_stream_progress_since
                print(
                    "er-readiness-watch: world-stream progress watermark flat for "
                    f"{stuck_seconds:.1f}s >= {args.world_stream_stall_seconds:g}s since map-load "
                    "began (player block wedged); failing fast",
                    file=sys.stderr,
                )
                return with_runtime_module_info(
                    replace(
                        ReadinessResult(
                            False,
                            WORLD_STREAM_STALLED,
                            pid,
                            bootstrap,
                            telemetry,
                            hypr_windows(args.window_class),
                            spawn_polls + poll,
                            float(args.max_runtime_seconds),
                            expected_save_oracle=expected_save_oracle,
                            expected_animation_id=args.expected_animation_id,
                        ),
                        world_stream_stall=world_stream_stall_snapshot(telemetry, stuck_seconds),
                    )
                )
        if args.target == TARGET_WORLD_STABLE:
            if (
                not appear_animation_seen
                and telemetry is not None
                and telemetry_expected_animation_match(telemetry, args.expected_animation_id)
            ):
                appear_animation_seen = True
            # Once the appear animation has been confirmed once, drop it from the per-poll world-loaded
            # gate so the dwell measures sustained in-world stability, not the transient spawn animation.
            effective_expected_animation_id = None if appear_animation_seen else args.expected_animation_id
            if process_running and telemetry_world_loaded(telemetry, expected_save_oracle, effective_expected_animation_id):
                # The world-loaded semaphore is reached HERE (first true), so this is the headline
                # launch->world delta -- recorded before the dwell/visual confirmation wait.
                timing.mark("t_world_stable")
                tick = telemetry_world_tick(telemetry or {}, poll)
                if tick != last_world_stable_tick:
                    world_stable_samples += 1
                    last_world_stable_tick = tick
                if world_stable_samples >= args.world_stable_samples:
                    if world_stable_since is None:
                        world_stable_since = time.monotonic()
                        os.sched_yield()
                        continue
                    if time.monotonic() - world_stable_since < args.world_stable_dwell_seconds:
                        os.sched_yield()
                        continue
                    if args.visual_world_check and visual_loading_screen_visible(args.artifact_dir, windows, world_stable_samples, args.window_class):
                        world_stable_samples = 0
                        last_world_stable_tick = None
                        world_stable_since = None
                        os.sched_yield()
                        continue
                    return with_runtime_module_info(
                        ReadinessResult(
                            True,
                            WORLD_STABLE,
                            pid,
                            bootstrap,
                            telemetry,
                            windows,
                            spawn_polls + poll,
                            float(args.max_runtime_seconds),
                            world_stable_samples=world_stable_samples,
                            expected_save_oracle=expected_save_oracle,
                            expected_animation_id=args.expected_animation_id,
                        )
                    )
            elif telemetry is not None:
                # Reset the world-stable dwell ONLY on a GENUINE not-loaded read (telemetry present but
                # the world-loaded oracle is false). A transient None telemetry -- the fast poll loop
                # (~hundreds/s) caught a partial write while the DLL flushed er-effects-telemetry.json,
                # so read_json returned None -- is NOT evidence the world unloaded. Resetting on it
                # prevented headless world-stable from ever accumulating its 5s dwell even though the
                # in-process oracles showed a stable in-world character the whole time. The oracles are
                # the primary detector (gamescope headless cannot be screenshotted); a failed file read
                # must not masquerade as a world-unload. bd accept-byte-gold-load-world-reached-2026-06-23.
                world_stable_samples = 0
                last_world_stable_tick = None
                world_stable_since = None
        if windows:
            window_stale_polls += 1
        else:
            window_stale_polls = 0
        result = classify_snapshot(
            pid=pid,
            process_running=process_running,
            telemetry=telemetry,
            bootstrap=bootstrap,
            windows=windows,
            window_stale_polls=window_stale_polls,
            window_stale_poll_budget=args.window_stale_poll_budget,
            polls=spawn_polls + poll,
            target=args.target,
            autoload_attempt_budget=args.autoload_attempt_budget,
            post_request_tick_budget=args.post_request_tick_budget,
        )
        if result is not None:
            return with_runtime_module_info(
                ReadinessResult(
                    result.ready,
                    result.reason,
                    result.pid,
                    result.bootstrap,
                    result.telemetry,
                    result.windows,
                    result.polls,
                    float(args.max_runtime_seconds),
                    expected_save_oracle=expected_save_oracle,
                    expected_animation_id=args.expected_animation_id,
                )
            )
        os.sched_yield()

    return with_runtime_module_info(
        ReadinessResult(
            False,
            READINESS_BUDGET_EXHAUSTED,
            pid,
            read_bootstrap(args.bootstrap, args.bootstrap_state),
            read_json(args.telemetry),
            hypr_windows(args.window_class),
            spawn_polls + args.readiness_poll_budget,
            float(args.max_runtime_seconds),
            expected_save_oracle=expected_save_oracle,
            expected_animation_id=args.expected_animation_id,
        )
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--pid-file", type=Path, required=True)
    parser.add_argument("--telemetry", type=Path, required=True)
    parser.add_argument("--bootstrap", type=Path, required=True)
    parser.add_argument("--bootstrap-state", type=Path, required=True)
    parser.add_argument("--process-pattern", default=DEFAULT_RUNTIME_PROCESS_PATTERN)
    parser.add_argument("--window-class", default=DEFAULT_WINDOW_CLASS)
    parser.add_argument("--spawn-poll-budget", type=int, default=DEFAULT_SPAWN_POLL_BUDGET)
    parser.add_argument("--readiness-poll-budget", type=int, default=DEFAULT_READINESS_POLL_BUDGET)
    parser.add_argument(
        "--target",
        choices=[
            TARGET_GAME_MAN,
            TARGET_MODULE_BASE,
            TARGET_AUTOLOAD_REQUEST,
            TARGET_REQUEST_CONSUMPTION,
            TARGET_PLAYER_LOAD,
            TARGET_WORLD_STABLE,
        ],
        default=TARGET_GAME_MAN,
    )
    parser.add_argument("--autoload-attempt-budget", type=int, default=DEFAULT_AUTOLOAD_ATTEMPT_BUDGET)
    parser.add_argument("--post-request-tick-budget", type=int, default=DEFAULT_POST_REQUEST_TICK_BUDGET)
    parser.add_argument("--world-stable-samples", type=int, default=3)
    parser.add_argument("--world-stable-dwell-seconds", type=float, default=DEFAULT_WORLD_STABLE_DWELL_SECONDS)
    parser.add_argument(
        "--world-stream-stall-seconds",
        type=float,
        default=DEFAULT_WORLD_STREAM_STALL_SECONDS,
        help=(
            "Once the OWN-LOAD map-load has begun (continue fired + player block present), fail fast "
            "with reason world_stream_stalled if the streaming progress watermark does not improve "
            "within this window. Sits above a healthy ~2-5s stream but well below --max-runtime-seconds."
        ),
    )
    parser.add_argument(
        "--no-world-stream-stall-exit",
        dest="world_stream_stall_exit",
        action="store_false",
        default=True,
        help="Disable the world-stream stall early-exit semaphore (restores old behavior: burn the full runtime cap on a wedged map-load).",
    )
    parser.add_argument(
        "--phase-stall-seconds",
        type=float,
        default=DEFAULT_PHASE_STALL_SECONDS,
        help=(
            "Per-phase progress watchdog window. The active load phase (title, continue) must advance "
            "its monotonic progress watermark within this many seconds or the watcher fails fast with "
            "reason <phase>_stalled. Both phases ride game_task_ticks (advances every frame while alive) "
            "so a slow-but-healthy boot/title/wait never trips; only a true freeze does."
        ),
    )
    parser.add_argument(
        "--no-phase-watchdog",
        dest="phase_watchdog",
        action="store_false",
        default=True,
        help="Disable the per-phase progress watchdog (title_stalled / continue_stalled). The tail-stage world_stream_stalled semaphore is unaffected.",
    )
    parser.add_argument(
        "--launch-epoch",
        type=float,
        default=None,
        help=(
            "TRUE bash launch epoch (float seconds, e.g. `date +%%s.%%N`), captured at the moment "
            "eldenring.exe is fired. All timing deltas are measured from this. Falls back to "
            "ER_PROBE_LAUNCH_EPOCH, then watcher-start if neither is given."
        ),
    )
    parser.add_argument(
        "--world-load-deadline-seconds",
        type=float,
        default=DEFAULT_WORLD_LOAD_DEADLINE_SECONDS,
        help=(
            "Fail fast with reason world_load_deadline_exceeded if the world-loaded semaphore "
            "(player present / world-stable) is not reached within this many seconds of CONTINUE_FIRED "
            "(the load starting; launch-anchored before continue fires). Default 30s -- room for a real "
            "ER load after continue, while the 6s world_stream_stalled semaphore does the precise fast-fail."
        ),
    )
    parser.add_argument(
        "--no-world-load-deadline",
        dest="world_load_deadline_exit",
        action="store_false",
        default=True,
        help="Disable the world-load fail-fast deadline (let the run go to the full runtime cap).",
    )
    parser.add_argument(
        "--expected-save-oracle",
        type=Path,
        help="Expected save-slot oracle JSON generated from the vanilla ER0000.sl2 file.",
    )
    parser.add_argument(
        "--expected-animation-id",
        type=int,
        default=DEFAULT_EXPECTED_ANIMATION_ID,
        help="Expected in-world player animation ID for the gold-start oracle.",
    )
    parser.add_argument(
        "--expected-runtime-mode",
        choices=[RUNTIME_MODE_VANILLA, RUNTIME_MODE_SEAMLESS, RUNTIME_MODE_ANY],
        default=RUNTIME_MODE_VANILLA,
        help="Fail early when loaded modules/telemetry prove a mismatched Elden Ring mode.",
    )
    parser.add_argument(
        "--visual-world-check",
        action="store_true",
        help="Before accepting world-stable telemetry, OCR a target-window screenshot and reject loading-tip screens.",
    )
    parser.add_argument(
        "--fail-on-messagebox-dialog",
        action="store_true",
        default=True,
        help="Fail immediately if in-process telemetry sees any native CS::MessageBoxDialog build; ideal product runtime has zero.",
    )
    parser.add_argument(
        "--fail-on-native-legal-popup",
        action="store_true",
        default=True,
        help="Fail immediately if in-process telemetry sees a MessageBoxDialog builder arg matching packed ToS_win64.fmg EULA/privacy text IDs.",
    )
    parser.add_argument(
        "--fail-on-server-status-semaphore",
        action="store_true",
        default=True,
        help="Fail immediately if in-process telemetry sees title/network login server status text IDs from GR_System_Message_win64.fmg.",
    )
    parser.add_argument(
        "--visual-legal-popup-check",
        action="store_true",
        help="Fail immediately when target-window OCR detects EULA/terms/license/legal-popup text.",
    )
    parser.add_argument(
        "--visual-legal-popup-check-interval-seconds",
        type=float,
        default=LEGAL_POPUP_CHECK_INTERVAL_SECONDS,
        help="Minimum seconds between target-window legal-popup OCR samples.",
    )
    parser.add_argument(
        "--visual-save-data-popup-check",
        action="store_true",
        help="Fail immediately when target-window OCR detects the in-game 'failed to load save data' popup.",
    )
    parser.add_argument(
        "--visual-save-data-popup-check-interval-seconds",
        type=float,
        default=SAVE_DATA_POPUP_CHECK_INTERVAL_SECONDS,
        help="Minimum seconds between target-window save-data-popup OCR samples.",
    )
    parser.add_argument(
        "--defer-unsafe-visual-capture-until-telemetry",
        action="store_true",
        help="Do not abort solely because the target window is unsafe to screenshot until native telemetry has had a chance to arrive; screenshots still require a safe exact target window.",
    )
    parser.add_argument(
        "--skip-visual-capture",
        action="store_true",
        help="Telemetry-only validation mode: rely solely on in-process native telemetry for popup/world detection and never require or perform target-window screenshots, removing the window-focus dependency (no target_window_capture_unsafe bail). The native fail-closed checks (--fail-on-native-legal-popup / --fail-on-messagebox-dialog / --fail-on-server-status-semaphore) stay active. For focus-independent runtime validation, not product proof.",
    )
    parser.add_argument(
        "--max-runtime-seconds",
        type=float,
        default=DEFAULT_MAX_RUNTIME_SECONDS,
        help="Hard wall-clock cap for the readiness watch; must be >0 and <= the canonical runtime-probe cap (.auto/runtime_timeout_cap_seconds).",
    )
    parser.add_argument(
        "--allow-async-launcher-exit",
        action="store_true",
        help="Keep observing for a late Steam/Proton game process after the initial launcher command exits.",
    )
    parser.add_argument(
        "--window-stale-poll-budget",
        type=int,
        default=DEFAULT_WINDOW_STALE_POLL_BUDGET,
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.max_runtime_seconds <= 0 or args.max_runtime_seconds > MAX_ALLOWED_RUNTIME_SECONDS:
        raise SystemExit(
            f"--max-runtime-seconds must be greater than 0 and no more than {MAX_ALLOWED_RUNTIME_SECONDS:g}"
        )
    if args.world_stable_samples <= 0:
        raise SystemExit("--world-stable-samples must be greater than 0")
    if args.world_stable_dwell_seconds < 0:
        raise SystemExit("--world-stable-dwell-seconds must be greater than or equal to 0")
    if args.world_stream_stall_seconds <= 0:
        raise SystemExit("--world-stream-stall-seconds must be greater than 0")
    if args.phase_stall_seconds <= 0:
        raise SystemExit("--phase-stall-seconds must be greater than 0")
    if args.world_load_deadline_seconds <= 0:
        raise SystemExit("--world-load-deadline-seconds must be greater than 0")
    if args.visual_legal_popup_check_interval_seconds <= 0:
        raise SystemExit("--visual-legal-popup-check-interval-seconds must be greater than 0")
    if args.visual_save_data_popup_check_interval_seconds <= 0:
        raise SystemExit("--visual-save-data-popup-check-interval-seconds must be greater than 0")
    if args.expected_save_oracle and read_json(args.expected_save_oracle) is None:
        raise SystemExit(f"--expected-save-oracle is not readable JSON: {args.expected_save_oracle}")
    if args.skip_visual_capture:
        # Telemetry-only mode: disable every target-window screenshot path so the watch has
        # no window-focus dependency. The capture-safe bail and the visual popup/world checks
        # are all gated on these flags; the native telemetry fail-closed checks run earlier
        # and are unaffected.
        args.visual_legal_popup_check = False
        args.visual_save_data_popup_check = False
        args.visual_world_check = False
    args.artifact_dir.mkdir(parents=True, exist_ok=True)
    timing = TimingTracker(resolve_launch_epoch(args))
    timing.mark("t_launch")
    result = replace(
        with_runtime_mode_info(wait_readiness(args, timing), args.expected_runtime_mode),
        window_class=args.window_class,
    )
    # Teardown is the watch exit; mark it before snapshotting so the timing dict is complete.
    timing.mark("t_teardown")
    result = replace(result, timing=timing.snapshot())
    output = args.artifact_dir / "readiness-result.json"
    write_result(output, result)
    # One greppable line gives the whole launch->title->continue->player->world picture + reason.
    print(timing.summary_line(result.reason), file=sys.stderr, flush=True)
    print(json.dumps(result.to_json(), sort_keys=True))
    return SUCCESS_RC if result.ready else FAILURE_RC


if __name__ == "__main__":
    raise SystemExit(main())
