#!/usr/bin/env python3
"""Summarize er-reload-trace-dll logs for same-character reload diagnosis."""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path
from typing import TypedDict

EVENT_RE = re.compile(
    r"^\[(?P<seq>\d+) \+(?P<tick>\d+)ms\] (?P<label>\S+)(?: (?P<phase>ENTER|LEAVE))?(?P<rest>.*)$"
)
FIELD_RE = re.compile(
    r"\b(?P<key>base|gm|df0|gdm|pgd|mounted_registry)=0x(?P<hex>[0-9a-fA-F]+)|\b(?P<ikey>b78|b80|ac0)=(?P<int>-?\d+|<unreadable>)|\bc30=(?P<c30>0x[0-9a-fA-F]+|<unreadable>)"
)
IMPORTANT_LABELS = (
    "menu_continue_wrapper_82bac0",
    "menu_new_or_load_wrapper_82ba80",
    "menu_other_load_wrapper_82bb00",
    "native_submit_7ac890",
    "result_event_handler_746e80",
    "result_action_builder_746a00",
    "result_event_wrapper_builder_744a60",
    "task_enqueue_7a7b60",
    "set_save_slot_67a810",
    "save_request_profile_67a420",
    "request_save_67a520",
    "current_slot_load_67b570",
    "continue_load_67b750",
    "combined_load_67b940",
    "map_load_67bc10",
    "save_load_state_init_67b030",
    "b80_preview_67b4e0",
    "title_confirm_b0e180",
    "request_load_slot_67b200",
    "request_profile_read_67b1a0",
    "b80_poll_679180",
    "slot_deser_67b290",
    "movemap_dispatcher2_afb880",
    "movemap_do_save_stuff_afbad0",
    "map_request_do_836f30",
    "map_work_82faf0",
    "cap_setstate_b0d960",
    "cap_load_activate_9a4670",
    "cap_load_activate2_9ac760",
    "cap_builder_826510",
    "cap_selector_tick_826d50",
    "cap_menu_deser_82c240",
    "cap_dialog_factory_81ead0",
    "menu_window_job_ctor_7ac8c0",
    "menu_window_job_native_ctor_b_7acb00",
    "menu_window_job_idle_ctor_7acf80",
    "title_native_ready_733150",
)


class Event(TypedDict, total=False):
    line: int
    raw: str
    parse_error: bool
    seq: int
    tick_ms: int
    label: str
    phase: str
    fields: dict[str, str]


class Summary(TypedDict):
    event_count: int
    counts: dict[str, int]
    enter_counts: dict[str, int]
    first_last: dict[str, dict[str, int]]
    observed_field_values: dict[str, list[str]]
    missing_important_labels: list[str]


def parse_fields(rest: str) -> dict[str, str]:
    fields: dict[str, str] = {}
    for match in FIELD_RE.finditer(rest):
        if match.group("key"):
            fields[match.group("key")] = "0x" + match.group("hex").lower()
        elif match.group("ikey"):
            fields[match.group("ikey")] = match.group("int")
        else:
            fields["c30"] = match.group("c30")
    return fields


def parse_log(path: Path) -> list[Event]:
    events: list[Event] = []
    for line_no, line in enumerate(
        path.read_text(encoding="utf-8", errors="replace").splitlines(), 1
    ):
        match = EVENT_RE.match(line)
        if not match:
            events.append({"line": line_no, "raw": line, "parse_error": True})
            continue
        rest = match.group("rest") or ""
        events.append(
            {
                "line": line_no,
                "seq": int(match.group("seq")),
                "tick_ms": int(match.group("tick")),
                "label": match.group("label"),
                "phase": match.group("phase") or "INFO",
                "fields": parse_fields(rest),
                "raw": line,
            }
        )
    return events


def summarize(events: list[Event]) -> Summary:
    counts: Counter[str] = Counter()
    phase_counts: Counter[str] = Counter()
    first_last: dict[str, dict[str, int]] = {}
    field_values: dict[str, set[str]] = defaultdict(set)
    for event in events:
        label = event.get("label", "<parse_error>")
        phase = event.get("phase", "INFO")
        counts[label] += 1
        phase_counts[f"{label}:{phase}"] += 1
        seq = event.get("seq")
        if seq is not None:
            slot = first_last.setdefault(label, {"first_seq": seq, "last_seq": seq})
            slot["first_seq"] = min(slot["first_seq"], seq)
            slot["last_seq"] = max(slot["last_seq"], seq)
        fields = event.get("fields")
        if fields is not None:
            for key in ("b78", "b80", "ac0", "c30", "df0", "pgd", "mounted_registry"):
                value = fields.get(key)
                if isinstance(value, str):
                    field_values[key].add(value)
    missing_enter = [
        label
        for label in IMPORTANT_LABELS
        if phase_counts.get(f"{label}:ENTER", 0) == 0 and counts.get(label, 0) == 0
    ]
    return {
        "event_count": len(events),
        "counts": dict(sorted(counts.items())),
        "enter_counts": {
            label: phase_counts.get(f"{label}:ENTER", 0) for label in IMPORTANT_LABELS
        },
        "first_last": first_last,
        "observed_field_values": {
            key: sorted(values) for key, values in sorted(field_values.items())
        },
        "missing_important_labels": missing_enter,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "log", nargs="?", default="er-reload-trace.log", help="Trace log path."
    )
    parser.add_argument("--json", action="store_true", help="Emit JSON summary.")
    parser.add_argument(
        "--events",
        type=int,
        default=30,
        help="Number of tail events to show in text mode.",
    )
    args = parser.parse_args()

    path = Path(args.log)
    events = parse_log(path)
    summary = summarize(events)
    if args.json:
        json.dump(
            {"path": str(path), "summary": summary},
            sys.stdout,
            indent=2,
            sort_keys=True,
        )
        print()
        return 0

    print(f"reload trace log: {path}")
    print(f"events: {summary['event_count']}")
    print("enter counts:")
    for label, count in summary["enter_counts"].items():
        print(f"  {label}: {count}")
    if summary["missing_important_labels"]:
        print("missing important labels:")
        for label in summary["missing_important_labels"]:
            print(f"  {label}")
    print("observed field values:")
    for key, values in summary["observed_field_values"].items():
        print(f"  {key}: {', '.join(values)}")
    print(f"tail events (last {args.events}):")
    for event in events[-args.events :]:
        print(f"  {event.get('raw')}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
