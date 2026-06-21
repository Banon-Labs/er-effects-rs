#!/usr/bin/env python3
"""Watch an Elden Ring launch until DLL telemetry is ready or a structured failure is known.

This helper uses process/window/bootstrap/telemetry observations first, but every
runtime watch is also hard-bounded by --max-runtime-seconds, capped at 60 seconds,
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

DEFAULT_RUNTIME_PROCESS_PATTERN = r"(?:^|[/\\])(eldenring\.exe|start_protected_game\.exe)(?:\s|$)"
DEFAULT_WINDOW_CLASS = "steam_app_1245620"
DEFAULT_SPAWN_POLL_BUDGET = 4096
DEFAULT_READINESS_POLL_BUDGET = 8192
DEFAULT_WINDOW_STALE_POLL_BUDGET = 4096
DEFAULT_AUTOLOAD_ATTEMPT_BUDGET = 300
DEFAULT_POST_REQUEST_TICK_BUDGET = 300
DEFAULT_MAX_RUNTIME_SECONDS = 60.0
DEFAULT_WORLD_STABLE_DWELL_SECONDS = 5.0
MAX_ALLOWED_RUNTIME_SECONDS = 60.0
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


def telemetry_native_continue_chain_stage(telemetry: dict[str, Any]) -> str:
    phase = as_int(telemetry.get("oracle_continue_phase"), -1)
    result_chain_ready = telemetry_native_result_chain_ready(telemetry)
    action_inserted = telemetry_result_action_inserted(telemetry)
    deser_fired = as_int(telemetry.get("oracle_continue_deser_fired"), 0)
    confirmed = as_int(telemetry.get("oracle_continue_confirmed"), 0)
    if telemetry_world_loaded(telemetry):
        return "world_loaded"
    if confirmed > 0 and phase >= 3:
        return "confirmed_waiting_world"
    if deser_fired == 2:
        return "deserialized_waiting_confirm"
    if phase >= 3 and result_chain_ready and action_inserted:
        return "action_insert_waiting_continue_load"
    if phase >= 3 and result_chain_ready:
        return "result_chain_waiting_action_insert"
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
        "result_action_insert_hits": telemetry.get("oracle_result_action_insert_hits"),
        "result_action_last_insert_arg0": telemetry.get("oracle_result_action_last_insert_arg0"),
        "result_action_last_insert_arg1": telemetry.get("oracle_result_action_last_insert_arg1"),
        "result_action_last_insert_ret": telemetry.get("oracle_result_action_last_insert_ret"),
        "result_action_last_insert_arg1_update_rva": telemetry.get("oracle_result_action_last_insert_arg1_update_rva"),
        "result_action_last_insert_ret_update_rva": telemetry.get("oracle_result_action_last_insert_ret_update_rva"),
        "continue_phase": telemetry.get("oracle_continue_phase"),
        "continue_expected_slot": telemetry.get("oracle_continue_expected_slot"),
        "continue_deser_fired": telemetry.get("oracle_continue_deser_fired"),
        "continue_confirmed": telemetry.get("oracle_continue_confirmed"),
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
        "result_action_inserted": telemetry_result_action_inserted(telemetry),
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
    if telemetry is None or telemetry.get("game_man_available") is not True:
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
    if telemetry is not None and telemetry.get("game_man_available") is True:
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
        if args.target == TARGET_WORLD_STABLE:
            if process_running and telemetry_world_loaded(telemetry, expected_save_oracle, args.expected_animation_id):
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
            else:
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
        "--max-runtime-seconds",
        type=float,
        default=DEFAULT_MAX_RUNTIME_SECONDS,
        help="Hard wall-clock cap for the readiness watch; must be >0 and <=60.",
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
        raise SystemExit("--max-runtime-seconds must be greater than 0 and no more than 60")
    if args.world_stable_samples <= 0:
        raise SystemExit("--world-stable-samples must be greater than 0")
    if args.world_stable_dwell_seconds < 0:
        raise SystemExit("--world-stable-dwell-seconds must be greater than or equal to 0")
    if args.visual_legal_popup_check_interval_seconds <= 0:
        raise SystemExit("--visual-legal-popup-check-interval-seconds must be greater than 0")
    if args.visual_save_data_popup_check_interval_seconds <= 0:
        raise SystemExit("--visual-save-data-popup-check-interval-seconds must be greater than 0")
    if args.expected_save_oracle and read_json(args.expected_save_oracle) is None:
        raise SystemExit(f"--expected-save-oracle is not readable JSON: {args.expected_save_oracle}")
    args.artifact_dir.mkdir(parents=True, exist_ok=True)
    result = with_runtime_mode_info(wait_readiness(args), args.expected_runtime_mode)
    output = args.artifact_dir / "readiness-result.json"
    write_result(output, result)
    print(json.dumps(result.to_json(), sort_keys=True))
    return SUCCESS_RC if result.ready else FAILURE_RC


if __name__ == "__main__":
    raise SystemExit(main())
