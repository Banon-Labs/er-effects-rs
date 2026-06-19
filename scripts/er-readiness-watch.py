#!/usr/bin/env python3
"""Watch an Elden Ring launch until DLL telemetry is ready or a structured failure is known.

This helper uses process/window/bootstrap/telemetry observations first, but every
runtime watch is also hard-bounded by --max-runtime-seconds, capped at 120 seconds,
so a missing DLL telemetry stream cannot strand Elden Ring on-screen indefinitely.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

DEFAULT_RUNTIME_PROCESS_PATTERN = r"(?:^|[/\\])(eldenring\.exe|start_protected_game\.exe)(?:\s|$)"
DEFAULT_WINDOW_CLASS = "steam_app_1245620"
DEFAULT_SPAWN_POLL_BUDGET = 4096
DEFAULT_READINESS_POLL_BUDGET = 8192
DEFAULT_WINDOW_STALE_POLL_BUDGET = 4096
DEFAULT_AUTOLOAD_ATTEMPT_BUDGET = 300
DEFAULT_POST_REQUEST_TICK_BUDGET = 300
DEFAULT_MAX_RUNTIME_SECONDS = 120.0
MAX_ALLOWED_RUNTIME_SECONDS = 120.0
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
MODULE_BASE_READY = "runtime_module_base_observed"
WORLD_STABLE = "world_stable"
RUNTIME_EXE_NAME = "eldenring.exe"
WINDOW_WITHOUT_BOOTSTRAP = "window_without_bootstrap_marker"
WINDOW_WITHOUT_TASK = "window_without_game_task_ready"
WINDOW_WITHOUT_TELEMETRY = "window_without_valid_telemetry"
TELEMETRY_WITHOUT_GAME_MAN = "telemetry_without_game_man"
AUTOLOAD_REQUESTED = "autoload_requested"
TITLE_BOOTSTRAP_SEEN = "title_bootstrap_seen"
PLAYER_AVAILABLE = "player_available"
AUTOLOAD_ATTEMPT_BUDGET_REACHED = "autoload_attempt_budget_reached"
POST_REQUEST_TICK_BUDGET_REACHED = "post_request_tick_budget_reached"
PLAYER_LOAD_TICK_BUDGET_REACHED = "player_load_tick_budget_reached"
AUTOLOAD_SLOT_MISSING = "autoload_slot_missing"
PROCESS_EXITED = "process_exited_before_ready"
SPAWN_BUDGET_EXHAUSTED = "runtime_process_not_observed_within_spawn_poll_budget"
READINESS_BUDGET_EXHAUSTED = "readiness_poll_budget_exhausted"
TIMEOUT_BUDGET_EXHAUSTED = "timeout_seconds_budget_exhausted"

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

    def to_json(self) -> dict[str, Any]:
        return {
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
        }


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
    )


def select_runtime_pid(
    pattern: re.Pattern[str],
    pid_file: Path,
    poll_budget: int,
    deadline: float,
    allow_async_launcher_exit: bool = False,
) -> tuple[int | None, str, int]:
    launcher_pid = pid_file_value(pid_file)
    launcher_exited = False
    for poll in range(poll_budget):
        if time.monotonic() >= deadline:
            return None, TIMEOUT_BUDGET_EXHAUSTED, poll
        rows = runtime_process_rows(pattern)
        for row in rows:
            if RUNTIME_EXE_NAME in row.args.lower():
                return row.pid, READY_REASON, poll
        if launcher_pid is not None and not launcher_exited and not pid_running(launcher_pid):
            launcher_exited = True
            rows = runtime_process_rows(pattern)
            for row in rows:
                if RUNTIME_EXE_NAME in row.args.lower():
                    return row.pid, READY_REASON, poll
            if not allow_async_launcher_exit:
                return None, PROCESS_EXITED, poll
        os.sched_yield()
    return None, SPAWN_BUDGET_EXHAUSTED, poll_budget


def process_exists(pid: int) -> bool:
    return Path("/proc", str(pid)).exists()


def client_is_game_window(client: dict[str, Any], window_class: str) -> bool:
    title = str(client.get("title") or "")
    klass = str(client.get("class") or "")
    return klass == window_class or title.lower().startswith("elden ring")


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


def telemetry_world_loaded(telemetry: dict[str, Any] | None) -> bool:
    if telemetry is None or telemetry.get("game_man_available") is not True:
        return False
    player_seen = telemetry.get("player_available") is True or telemetry.get("player_seen") is True
    return bool(
        player_seen
        and telemetry.get("oracle_player_present") is True
        and telemetry.get("oracle_block_id_valid") is True
        and as_int(telemetry.get("oracle_now_loading"), -1) == 0
        and as_int(telemetry.get("oracle_saved_map_c30"), -1) != -1
    )


def telemetry_world_tick(telemetry: dict[str, Any], fallback: int) -> int:
    return as_int(telemetry.get("game_task_ticks"), fallback)


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
    if telemetry is not None and telemetry.get("game_man_available") is True:
        if target == TARGET_GAME_MAN:
            return ReadinessResult(True, READY_REASON, pid, bootstrap, telemetry, windows, polls)
        if telemetry.get("autoload_slot") is None:
            return ReadinessResult(False, AUTOLOAD_SLOT_MISSING, pid, bootstrap, telemetry, windows, polls)
        player_seen = telemetry.get("player_available") is True or telemetry.get("player_seen") is True
        if target == TARGET_PLAYER_LOAD and player_seen:
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
            if player_seen:
                return ReadinessResult(True, PLAYER_AVAILABLE, pid, bootstrap, telemetry, windows, polls)
            game_task_ticks = int(telemetry.get("game_task_ticks") or 0)
            if target == TARGET_REQUEST_CONSUMPTION and telemetry.get("title_bootstrap_seen") is True:
                return ReadinessResult(True, TITLE_BOOTSTRAP_SEEN, pid, bootstrap, telemetry, windows, polls)
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
        and telemetry.get("game_man_available") is not True
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


def wait_readiness(args: argparse.Namespace) -> ReadinessResult:
    pattern = re.compile(args.process_pattern, re.I)
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
    last_world_stable_tick: int | None = None
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
        windows = hypr_windows(args.window_class)
        if args.target == TARGET_WORLD_STABLE:
            if process_running and telemetry_world_loaded(telemetry):
                tick = telemetry_world_tick(telemetry or {}, poll)
                if tick != last_world_stable_tick:
                    world_stable_samples += 1
                    last_world_stable_tick = tick
                if world_stable_samples >= args.world_stable_samples:
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
                        )
                    )
            else:
                world_stable_samples = 0
                last_world_stable_tick = None
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
    parser.add_argument(
        "--max-runtime-seconds",
        type=float,
        default=DEFAULT_MAX_RUNTIME_SECONDS,
        help="Hard wall-clock cap for the readiness watch; must be >0 and <=120.",
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
        raise SystemExit("--max-runtime-seconds must be greater than 0 and no more than 120")
    if args.world_stable_samples <= 0:
        raise SystemExit("--world-stable-samples must be greater than 0")
    args.artifact_dir.mkdir(parents=True, exist_ok=True)
    result = wait_readiness(args)
    output = args.artifact_dir / "readiness-result.json"
    write_result(output, result)
    print(json.dumps(result.to_json(), sort_keys=True))
    return SUCCESS_RC if result.ready else FAILURE_RC


if __name__ == "__main__":
    raise SystemExit(main())
