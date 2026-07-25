#!/usr/bin/env python3
"""Windows-side Frida attach helper for WSL-driven ER probes.

Run this with Windows Python (`py -3`), not WSL Python. It attaches to a Windows
process, loads a Frida JavaScript file, and writes all messages to JSONL.
"""

from __future__ import annotations

import argparse
import importlib
import json
import time
from collections.abc import Callable
from contextlib import suppress
from pathlib import Path
from threading import Event
from typing import Protocol, TextIO, cast


class FridaProcess(Protocol):
    name: str
    pid: int


class FridaScript(Protocol):
    def on(self, signal: str, callback: Callable[..., None]) -> None: ...

    def load(self) -> None: ...

    def unload(self) -> None: ...


class FridaSession(Protocol):
    def on(self, signal: str, callback: Callable[..., None]) -> None: ...

    def create_script(self, source: str) -> FridaScript: ...

    def detach(self) -> None: ...


class FridaDevice(Protocol):
    def enumerate_processes(self) -> list[FridaProcess]: ...

    def attach(self, target: int) -> FridaSession: ...


class FridaModule(Protocol):
    def get_local_device(self) -> FridaDevice: ...


FridaMessage = dict[str, object]
JsonEvent = dict[str, object]


def find_pid(device: FridaDevice, process_name: str) -> int | None:
    want = process_name.casefold()
    for proc in device.enumerate_processes():
        if proc.name.casefold() == want:
            return int(proc.pid)
    return None


def write_event(out: TextIO, event: JsonEvent) -> None:
    if "ts" not in event:
        event["ts"] = time.time()
    out.write(json.dumps(event, sort_keys=True) + "\n")
    out.flush()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--process", default="eldenring.exe")
    parser.add_argument("--script", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--timeout", type=float, default=120.0)
    parser.add_argument("--wait", type=float, default=45.0)
    parser.add_argument(
        "--ready-marker",
        type=Path,
        help="Optional marker file to write after the Frida script is loaded.",
    )
    args = parser.parse_args()
    script_path = cast(Path, args.script)
    out_path = cast(Path, args.out)
    ready_marker = cast(Path | None, args.ready_marker)
    timeout_seconds = cast(float, args.timeout)
    wait_seconds = cast(float, args.wait)
    process_name = cast(str, args.process)

    started = time.monotonic()
    out_path.parent.mkdir(parents=True, exist_ok=True)
    script_source = script_path.read_text(encoding="utf-8")

    with out_path.open("w", encoding="utf-8") as out:
        try:
            frida = cast(FridaModule, importlib.import_module("frida"))
            device = frida.get_local_device()
            pid = None
            process_poll_tick = Event()
            while time.monotonic() - started < wait_seconds:
                pid = find_pid(device, process_name)
                if pid is not None:
                    break
                remaining = max(0.0, wait_seconds - (time.monotonic() - started))
                process_poll_tick.wait(min(0.25, remaining))
            if pid is None:
                write_event(
                    out,
                    {
                        "type": "error",
                        "error": "process_not_found",
                        "process": process_name,
                    },
                )
                return 2

            write_event(out, {"type": "attach", "process": process_name, "pid": pid})
            session = device.attach(pid)
            detached = Event()

            def on_detached(reason: str, crash: object) -> None:
                write_event(
                    out, {"type": "detached", "reason": reason, "crash": str(crash)}
                )
                detached.set()

            session.on("detached", on_detached)

            script = session.create_script(script_source)
            script_error = Event()

            def on_message(message: FridaMessage, data: bytes | None) -> None:
                event: JsonEvent = {"type": "message", "message": message}
                if data is not None:
                    event["data_len"] = len(data)
                if message.get("type") == "error":
                    script_error.set()
                write_event(out, event)

            script.on("message", on_message)
            script.load()
            if script_error.is_set():
                write_event(out, {"type": "error", "error": "frida_script_error"})
                with suppress(Exception):
                    script.unload()
                with suppress(Exception):
                    session.detach()
                return 3
            if ready_marker is not None:
                ready_marker.write_text("loaded\n", encoding="utf-8")
            write_event(
                out,
                {
                    "type": "loaded",
                    "ready_marker": str(ready_marker) if ready_marker else None,
                },
            )

            detached.wait(timeout_seconds)

            write_event(out, {"type": "timeout_or_done", "detached": detached.is_set()})
            with suppress(Exception):
                script.unload()
            with suppress(Exception):
                session.detach()
            return 0
        except Exception as exc:  # noqa: BLE001 - this helper must preserve any attach/setup failure.
            write_event(
                out, {"type": "error", "error": type(exc).__name__, "detail": str(exc)}
            )
            return 1


if __name__ == "__main__":
    raise SystemExit(main())
