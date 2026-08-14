#!/usr/bin/env python3
"""Local visual authoring UI for 05_010_ProfileSelect layout/chrome.

This is an editor for the tracked schema and rebuild path. Its canvas is a
Scaleform-coordinate approximation; Elden Ring's runtime remains the final visual oracle.
It never launches or kills Elden Ring/ME3.
"""
from __future__ import annotations

import argparse
import html
import json
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
SCHEMA = REPO / "crates/er-gfx/profile_05_010_layout.toml"
REBUILD = REPO / "scripts/rebuild-profile-05-010-layout.sh"
EDITOR_DIR = REPO / "target/pi-local/profile-editor"
CONTROL_FILE = EDITOR_DIR / "control.txt"
STATUS_FILE = EDITOR_DIR / "status.txt"
PROTOCOL_VERSION = 1
FIELD_NAMES = [
    "PlayerName",
    "StaticText_110502",
    "Level",
    "Location",
    "PlayTime",
    "ErStats",
    "ErCharStats",
    "DriveCell_0",
    "DriveCell_1",
    "DriveCell_2",
    "CurrentPath",
]
# `hit_area` is the row's invisible mouse target, not visual chrome: it is baked alpha-0 and the
# Rust schema validator rejects any other opacity. It is listed here so the editor round-trips the
# section instead of dropping it (or raising KeyError on load).
CHROME_NAMES = ["backing", "hit_area", "cursor", "cursor_body", "drive_button", "path_button"]
MODES = ["load_character", "save_picker", "drive_row"]
MAX_TEXT_FIELD_WIDTH_PX = 1500
MAX_TEXT_FIELD_FONT_HEIGHT_PX = 80
MAX_ABS_POSITION_PX = 2000
VALID_ALIGNMENTS = {"left", "center", "right"}

# Menu font vertical metrics, read from the game's own data0:/font/eu_std/font.gfx DefineFont3
# id=1 ("Agmena W1G For Bandai", 910 glyphs) layout block. Mirrors
# crates/er-gfx/src/profile_05_010_layout.rs -- keep the two in step.
#
# Scaleform lays one line out as (ascent + descent) / em_square * font_height px, and
# GFx::TextField reflow (FUN_14114bf10) then insets the usable area by 2 px on every edge.
# A box shorter than that sum cannot render its own text. This is not a style guideline:
# PlayerName shipped at clip_height = 30 against a 38.383 px requirement and rendered short.
MENU_FONT_ASCENT = 20800
MENU_FONT_DESCENT = 8540
MENU_FONT_EM_SQUARE = 20480  # SWF spec: DefineFont3 glyph coords are 1/20 twip = 1024 * 20
TEXT_DOC_INSET_PX_PER_EDGE = 2


def line_box_px(font_height) -> float:
    """Height of one laid-out line of the menu font at ``font_height``."""
    return (MENU_FONT_ASCENT + MENU_FONT_DESCENT) / MENU_FONT_EM_SQUARE * float(font_height)


def max_font_height_px(clip_height) -> int:
    """Largest font_height a box of ``clip_height`` can render one line of.

    Inverse of :func:`min_clip_height_px`. clip_height is baked into the movie and never
    applied live, so a font above this renders overflowed until a rebuild + screen reopen.
    """
    usable = int(clip_height) - 2 * TEXT_DOC_INSET_PX_PER_EDGE
    if usable <= 0:
        return 0
    return int(usable * MENU_FONT_EM_SQUARE / (MENU_FONT_ASCENT + MENU_FONT_DESCENT))


def min_clip_height_px(font_height) -> int:
    """Smallest clip_height that can render one line at ``font_height``, inset included.

    Rounds up: a fractional shortfall still clips, so 38.383 demands 39.
    """
    import math

    return math.ceil(line_box_px(font_height) + 2 * TEXT_DOC_INSET_PX_PER_EDGE)


def strip_comment(line: str) -> str:
    in_string = False
    escaped = False
    for i, ch in enumerate(line):
        if ch == '"' and not escaped:
            in_string = not in_string
        if ch == "#" and not in_string:
            return line[:i]
        escaped = ch == "\\" and not escaped
        if ch != "\\":
            escaped = False
    return line


def parse_value(value: str):
    value = value.strip()
    if value in {"true", "false"}:
        return value == "true"
    if value.startswith('"') and value.endswith('"'):
        return value[1:-1].replace('\\"', '"').replace('\\n', '\n').replace('\\t', '\t').replace('\\\\', '\\')
    if any(c in value for c in ".eE"):
        return float(value)
    return int(value)


def quote_value(value) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, str):
        return '"' + value.replace('\\', '\\\\').replace('"', '\\"').replace('\n', '\\n').replace('\t', '\\t') + '"'
    return str(value)


def default_data():
    return {
        "fields": {name: {} for name in FIELD_NAMES},
        "path_editor": {},
        "row_chrome": {name: {} for name in CHROME_NAMES},
        "list": {},
    }


def load_schema(path: Path = SCHEMA):
    data = default_data()
    section = None
    for raw in path.read_text().splitlines():
        line = strip_comment(raw).strip()
        if not line:
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1]
            continue
        if "=" not in line or section is None:
            continue
        key, value = [part.strip() for part in line.split("=", 1)]
        parsed = parse_value(value)
        if section.startswith("field."):
            data["fields"][section.removeprefix("field.")][key] = parsed
        elif section == "path_editor":
            data["path_editor"][key] = parsed
        elif section.startswith("row_chrome."):
            data["row_chrome"][section.removeprefix("row_chrome.")][key] = parsed
        elif section == "list":
            data["list"][key] = parsed
    return data


def validate_data(data, selected=None):
    errors = []
    if sorted(data.get("fields", {}).keys()) != sorted(FIELD_NAMES):
        errors.append("field set must exactly match the known 05_010 row text fields")
    if sorted(data.get("row_chrome", {}).keys()) != sorted(CHROME_NAMES):
        errors.append("row_chrome set must exactly match the known row chrome objects")
    for name in FIELD_NAMES:
        field = data.get("fields", {}).get(name, {})
        for key in ["x", "y", "width", "clip_height", "font_height", "align"]:
            if key not in field:
                errors.append(f"field.{name}.{key} is missing")
        try:
            x = float(field.get("x", 0))
            y = float(field.get("y", 0))
            width = int(field.get("width", 0))
            clip_height = int(field.get("clip_height", 0))
            font_height = int(field.get("font_height", 0))
        except (TypeError, ValueError):
            errors.append(f"field.{name} has non-numeric geometry")
            continue
        if abs(x) > MAX_ABS_POSITION_PX or abs(y) > MAX_ABS_POSITION_PX:
            errors.append(f"field.{name} position is outside the safe editor range (+/-{MAX_ABS_POSITION_PX}px)")
        if not (0 < width <= MAX_TEXT_FIELD_WIDTH_PX):
            errors.append(f"field.{name}.width must be 1..{MAX_TEXT_FIELD_WIDTH_PX}px")
        if not (0 < clip_height <= MAX_TEXT_FIELD_FONT_HEIGHT_PX * 2):
            errors.append(f"field.{name}.clip_height must be 1..{MAX_TEXT_FIELD_FONT_HEIGHT_PX * 2}px")
        if not (0 < font_height <= MAX_TEXT_FIELD_FONT_HEIGHT_PX):
            errors.append(f"field.{name}.font_height must be 1..{MAX_TEXT_FIELD_FONT_HEIGHT_PX}px")
        elif clip_height < min_clip_height_px(font_height):
            # Replaces the old `font_height > clip_height` rule, which was the hole that let
            # PlayerName ship at font_height 24 / clip_height 30: 24 <= 30 passed, but the real
            # requirement is 39. The floor below is the font's own line box plus the reflow inset.
            errors.append(
                f"field.{name}.clip_height {clip_height} cannot render one line of its own font: "
                f"font_height {font_height} needs a {line_box_px(font_height):.3f}px line box "
                f"plus {TEXT_DOC_INSET_PX_PER_EDGE}px/edge inset, so clip_height must be "
                f">= {min_clip_height_px(font_height)}"
            )
        if field.get("align") not in VALID_ALIGNMENTS:
            errors.append(f"field.{name}.align must be left, center, or right")
    path_editor = data.get("path_editor", {})
    for key in ["x", "y", "width", "clip_height", "font_height", "align"]:
        if key not in path_editor:
            errors.append(f"path_editor.{key} is missing")
    try:
        editor_x = float(path_editor.get("x", 0))
        editor_y = float(path_editor.get("y", 0))
        editor_width = int(path_editor.get("width", 0))
        editor_clip_height = int(path_editor.get("clip_height", 0))
        editor_font_height = int(path_editor.get("font_height", 0))
    except (TypeError, ValueError):
        errors.append("path_editor has non-numeric geometry")
    else:
        if abs(editor_x) > MAX_ABS_POSITION_PX or abs(editor_y) > MAX_ABS_POSITION_PX:
            errors.append(f"path_editor position is outside the safe editor range (+/-{MAX_ABS_POSITION_PX}px)")
        if not (0 < editor_width <= MAX_TEXT_FIELD_WIDTH_PX):
            errors.append(f"path_editor.width must be 1..{MAX_TEXT_FIELD_WIDTH_PX}px")
        if not (0 < editor_clip_height <= MAX_TEXT_FIELD_FONT_HEIGHT_PX * 2):
            errors.append(f"path_editor.clip_height must be 1..{MAX_TEXT_FIELD_FONT_HEIGHT_PX * 2}px")
        if not (0 < editor_font_height <= MAX_TEXT_FIELD_FONT_HEIGHT_PX):
            errors.append(f"path_editor.font_height must be 1..{MAX_TEXT_FIELD_FONT_HEIGHT_PX}px")
        elif editor_clip_height < min_clip_height_px(editor_font_height):
            errors.append(
                f"path_editor.clip_height {editor_clip_height} cannot render one line of its own font; "
                f"font_height {editor_font_height} requires >= {min_clip_height_px(editor_font_height)}"
            )
    if path_editor.get("align") not in VALID_ALIGNMENTS:
        errors.append("path_editor.align must be left, center, or right")
    for name in CHROME_NAMES:
        chrome = data.get("row_chrome", {}).get(name, {})
        for key in ["x", "y", "scale_x", "scale_y", "opacity", "editable"]:
            if key not in chrome:
                errors.append(f"row_chrome.{name}.{key} is missing")
        try:
            x = float(chrome.get("x", 0))
            y = float(chrome.get("y", 0))
            scale_x = float(chrome.get("scale_x", 0))
            scale_y = float(chrome.get("scale_y", 0))
            opacity = float(chrome.get("opacity", 0))
        except (TypeError, ValueError):
            errors.append(f"row_chrome.{name} has non-numeric geometry")
            continue
        if abs(x) > MAX_ABS_POSITION_PX or abs(y) > MAX_ABS_POSITION_PX:
            errors.append(f"row_chrome.{name} position is outside the safe editor range (+/-{MAX_ABS_POSITION_PX}px)")
        if not (0 < scale_x <= 40) or not (0 < scale_y <= 40):
            errors.append(f"row_chrome.{name} scale must be > 0 and <= 40")
        if not (0 <= opacity <= 1):
            errors.append(f"row_chrome.{name}.opacity must be 0..1")
    list_data = data.get("list", {})
    if list_data.get("visible_rows") != 10:
        errors.append("list.visible_rows must stay 10")
    for key in ["row_pitch", "top_recycle_rows", "bottom_recycle_rows", "mask_height", "scrollbar_x", "scrollbar_top_y", "scrollbar_track_height"]:
        if key not in list_data:
            errors.append(f"list.{key} is missing")
    for key in ["row_pitch", "mask_height", "scrollbar_track_height"]:
        try:
            if int(list_data.get(key, 0)) <= 0:
                errors.append(f"list.{key} must be positive")
        except (TypeError, ValueError):
            errors.append(f"list.{key} must be numeric")
    if selected is not None:
        kind = selected.get("kind", "field")
        name = selected.get("name", "PlayerName")
        if kind == "field" and name not in FIELD_NAMES:
            errors.append(f"selected field is unknown: {name}")
        elif kind == "chrome" and name not in CHROME_NAMES:
            errors.append(f"selected chrome object is unknown: {name}")
        elif kind == "path_editor" and name != "ActivePathEditor":
            errors.append(f"selected path editor is unknown: {name}")
        elif kind not in {"field", "path_editor", "chrome", "list"}:
            errors.append(f"selected kind is unknown: {kind}")
    return errors


def write_schema(data, path: Path = SCHEMA):
    errors = validate_data(data)
    if errors:
        raise ValueError("invalid 05_010 layout:\n" + "\n".join(f"- {e}" for e in errors))
    lines = [
        "# 05_010_ProfileSelect visual editor schema.",
        "# Edited by scripts/profile-05-010-editor.py or by hand.",
        "# The canvas is approximate; in-game Scaleform rendering is the final oracle.",
        "# Icon_0 stays placed and hidden, never unplaced.",
        "",
    ]
    for name in FIELD_NAMES:
        f = data["fields"][name]
        lines.append(f"[field.{name}]")
        for key in ["x", "y", "width", "clip_height", "font_height", "align", "sample_load_character", "sample_save_picker", "sample_drive_row"]:
            lines.append(f"{key} = {quote_value(f.get(key, ''))}")
        lines.append("")
    lines.append("[path_editor]")
    for key in ["x", "y", "width", "clip_height", "font_height", "align", "sample_load_character", "sample_save_picker", "sample_drive_row"]:
        lines.append(f"{key} = {quote_value(data['path_editor'].get(key, ''))}")
    lines.append("")
    for name in CHROME_NAMES:
        c = data["row_chrome"][name]
        lines.append(f"[row_chrome.{name}]")
        for key in ["x", "y", "scale_x", "scale_y", "opacity", "editable", "source"]:
            lines.append(f"{key} = {quote_value(c.get(key, ''))}")
        lines.append("")
    lines.append("[list]")
    for key in ["row_pitch", "visible_rows", "top_recycle_rows", "bottom_recycle_rows", "mask_height", "scrollbar_x", "scrollbar_top_y", "scrollbar_track_height"]:
        lines.append(f"{key} = {quote_value(data['list'][key])}")
    lines.append("")
    path.write_text("\n".join(lines))


rebuild_state = {"running": False, "returncode": None, "output": ""}


def quote_control(value) -> str:
    return '"' + str(value).replace('\\', '\\\\').replace('"', '\"').replace('\n', '\n').replace('\t', '\t') + '"'


def write_runtime_control(payload):
    data = payload["data"]
    selected = payload.get("selected", {"kind": "field", "name": "PlayerName"})
    sequence = int(payload.get("sequence", 0))
    render_mode = payload.get("render_mode", "offline_approximate")
    text_probe = bool(payload.get("text_probe", False))
    EDITOR_DIR.mkdir(parents=True, exist_ok=True)
    lines = [
        f"profile_editor_protocol = {quote_control(PROTOCOL_VERSION)}",
        f"sequence = {quote_control(sequence)}",
        f"render_mode = {quote_control(render_mode)}",
        f"selected_kind = {quote_control(selected.get('kind', 'field'))}",
        f"selected_name = {quote_control(selected.get('name', 'PlayerName'))}",
    ]
    if text_probe:
        lines.append(f"text_probe = {quote_control(True)}")
    for name in FIELD_NAMES:
        field = data["fields"][name]
        for key in ["x", "y", "width", "clip_height", "font_height", "align"]:
            lines.append(f"field.{name}.{key} = {quote_control(field[key])}")
    for key in ["x", "y", "width", "clip_height", "font_height", "align"]:
        lines.append(f"path_editor.{key} = {quote_control(data['path_editor'][key])}")
    for name in CHROME_NAMES:
        obj = data["row_chrome"][name]
        for key in ["x", "y", "scale_x", "scale_y", "opacity", "editable"]:
            lines.append(f"row_chrome.{name}.{key} = {quote_control(obj[key])}")
    for key in ["row_pitch", "visible_rows", "top_recycle_rows", "bottom_recycle_rows", "mask_height", "scrollbar_x", "scrollbar_top_y", "scrollbar_track_height"]:
        lines.append(f"list.{key} = {quote_control(data['list'][key])}")
    tmp = CONTROL_FILE.with_suffix(".txt.tmp")
    tmp.write_text("\n".join(lines) + "\n")
    tmp.replace(CONTROL_FILE)


def parse_status_text(text: str):
    out = {}
    for raw in text.splitlines():
        line = strip_comment(raw).strip()
        if not line or "=" not in line:
            continue
        key, value = [part.strip() for part in line.split("=", 1)]
        out[key] = parse_value(value)
    return out


def runtime_status():
    if not STATUS_FILE.exists():
        return {"connected": False, "status": "no status.txt yet", "path": str(STATUS_FILE)}
    try:
        status = parse_status_text(STATUS_FILE.read_text())
        status["path"] = str(STATUS_FILE)
        return status
    except Exception as e:
        return {"connected": False, "status": "status parse error", "error": str(e), "path": str(STATUS_FILE)}
protocol_sequence = 0


def protocol_quote(value) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    text = str(value)
    safe = text and all(ch.isalnum() or ch in "_-.:" for ch in text)
    if safe:
        return text
    return '"' + text.replace('\\', '\\\\').replace('"', '\\"') + '"'


def protocol_lines(data, sequence: int, render_mode: str, selected: dict, text_probe=False):
    lines = [
        f"profile_editor_protocol = {PROTOCOL_VERSION}",
        f"sequence = {sequence}",
        f"render_mode = {protocol_quote(render_mode)}",
        f"selected_kind = {protocol_quote(selected.get('kind', 'field'))}",
        f"selected_name = {protocol_quote(selected.get('name', 'PlayerName'))}",
    ]
    if text_probe:
        lines.append(f"text_probe = {protocol_quote(True)}")
    for name in FIELD_NAMES:
        f = data["fields"][name]
        for key in ["x", "y", "width", "clip_height", "font_height", "align"]:
            lines.append(f"field.{name}.{key} = {protocol_quote(f[key])}")
    for key in ["x", "y", "width", "clip_height", "font_height", "align"]:
        lines.append(f"path_editor.{key} = {protocol_quote(data['path_editor'][key])}")
    for name in CHROME_NAMES:
        c = data["row_chrome"][name]
        for key in ["x", "y", "scale_x", "scale_y", "opacity", "editable"]:
            lines.append(f"row_chrome.{name}.{key} = {protocol_quote(c[key])}")
    for key in ["row_pitch", "visible_rows", "top_recycle_rows", "bottom_recycle_rows", "mask_height", "scrollbar_x", "scrollbar_top_y", "scrollbar_track_height"]:
        lines.append(f"list.{key} = {protocol_quote(data['list'][key])}")
    return "\n".join(lines) + "\n"


def write_control(data, render_mode="offline_approximate", selected=None, text_probe=False):
    global protocol_sequence
    selected = selected or {"kind": "field", "name": "PlayerName"}
    protocol_sequence += 1
    EDITOR_DIR.mkdir(parents=True, exist_ok=True)
    CONTROL_FILE.write_text(protocol_lines(data, protocol_sequence, render_mode, selected, text_probe))
    return protocol_sequence


def parse_protocol_file(path: Path):
    if not path.exists():
        return None
    out = {}
    for raw in path.read_text(errors="replace").splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line or "=" not in line:
            continue
        key, value = [part.strip() for part in line.split("=", 1)]
        if value.startswith('"') and value.endswith('"'):
            value = value[1:-1].replace('\\"', '"').replace('\\\\', '\\')
        out[key] = value
    return out


# A status file this old means nobody is reading the control file any more. The DLL rewrites
# status.txt from its necromancy tick roughly once a second while it is alive, so the file's mtime is
# a heartbeat: a game that exited (or that was launched WITHOUT ER_PROFILE_05_010_EDITOR_DIR, which
# makes the whole editor path inert) leaves the last status frozen on disk with `connected = true`.
# Before this, the badge read that stale line and said "live runtime connected" to a dead process --
# two saves went into the void with the UI showing green (2026-08-07, sequence 62 vs ack 57).
STATUS_STALE_SECONDS = 6.0


def runtime_status(last_sequence: int = 0):
    status = parse_protocol_file(STATUS_FILE)
    if status is None:
        return {
            "profile_editor_protocol": PROTOCOL_VERSION,
            "ack_sequence": 0,
            "connected": False,
            "status": "disconnected",
            "active_surface": "none",
            "selected_kind": "",
            "selected_name": "",
            "applied_count": 0,
            "unsupported_count": 0,
            "error": "no runtime status file yet; live mode is not connected",
            "status_age_seconds": None,
            "sent_sequence": last_sequence,
        }
    try:
        age = max(0.0, time.time() - STATUS_FILE.stat().st_mtime)
    except OSError:
        age = None
    claims_connected = status.get("connected") == "true"
    stale = age is None or age > STATUS_STALE_SECONDS
    ack = int(status.get("ack_sequence", 0))
    error = status.get("error", "")
    if claims_connected and stale:
        error = (
            f"status.txt is {age:.0f}s old (stale over {STATUS_STALE_SECONDS:.0f}s): nothing is "
            f"reading {CONTROL_FILE}. The game is not running, or it was launched without "
            f"ER_PROFILE_05_010_EDITOR_DIR -- use scripts/run-quicksave-row-parity-live.sh, not "
            f"~/Elden/launch.sh, for live edits."
            + (f" Last live status: {error}" if error else "")
        )
    elif claims_connected and last_sequence and ack < last_sequence:
        error = (
            f"the game is alive but has not acked sequence {last_sequence} (last ack {ack}); "
            f"the edit has not been applied yet." + (f" {error}" if error else "")
        )
    return {
        "profile_editor_protocol": int(status.get("profile_editor_protocol", PROTOCOL_VERSION)),
        "ack_sequence": ack,
        "connected": claims_connected and not stale,
        "status": status.get("status", "unknown") if not stale else "stale",
        "active_surface": status.get("active_surface", "unknown"),
        "selected_kind": status.get("selected_kind", ""),
        "selected_name": status.get("selected_name", ""),
        "applied_count": int(status.get("applied_count", 0)),
        "unsupported_count": int(status.get("unsupported_count", 0)),
        "error": error,
        "status_age_seconds": None if age is None else round(age, 1),
        "sent_sequence": last_sequence,
    }



def start_rebuild():
    if rebuild_state["running"]:
        return
    # Publish RUNNING before the worker starts. When this happened inside the thread, a client could
    # POST /rebuild and immediately read the previous completed result, falsely declaring the new
    # rebuild done before it had begun.
    rebuild_state.update({"running": True, "returncode": None, "output": ""})
    def run():
        try:
            proc = subprocess.run(
                [str(REBUILD), "--hot-reload"],
                cwd=REPO,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                timeout=30,
            )
            rebuild_state.update(
                {"running": False, "returncode": proc.returncode, "output": proc.stdout[-20000:]}
            )
        except subprocess.TimeoutExpired as exc:
            output = exc.stdout or ""
            rebuild_state.update(
                {
                    "running": False,
                    "returncode": 124,
                    "output": (output + "\nhot reload timed out after 30s")[-20000:],
                }
            )

    threading.Thread(target=run, daemon=True).start()


HTML = r"""
<!doctype html>
<meta charset="utf-8">
<title>05_010 ProfileSelect Visual Editor</title>
<style>
:root { color-scheme: dark; font-family: system-ui, sans-serif; }
body { margin:0; display:grid; grid-template-columns: 330px 1fr 360px; height:100vh; background:#151515; color:#ddd; }
aside, section { overflow:auto; border-right:1px solid #333; padding:12px; }
button, input, select { background:#222; color:#eee; border:1px solid #555; border-radius:4px; padding:4px; }
button { cursor:pointer; } button.primary { border-color:#9d7; }
.warn { color:#f0c36a; } .bad { color:#ff7a7a; } .ok { color:#8f8; }
.tree button { display:block; width:100%; margin:3px 0; text-align:left; }
.tree button.sel { background:#38451f; border-color:#9d7; }
.grid { display:grid; grid-template-columns: 120px 1fr; gap:6px; align-items:center; }
.controls { display:flex; gap:6px; flex-wrap:wrap; margin:8px 0; }
pre { background:#0f0f0f; border:1px solid #333; padding:8px; white-space:pre-wrap; max-height:260px; overflow:auto; }
svg { width:100%; height:calc(100vh - 70px); background:#080808; border:1px solid #333; }
.field { fill:#2f72ff33; stroke:#6ca0ff; stroke-width:1; cursor:move; }
.chrome { fill:#d6922933; stroke:#ffbd5a; stroke-width:1; cursor:move; }
.readonly { fill:#7773; stroke:#aaa; stroke-dasharray:4 3; }
.ink { fill:#99ff6640; stroke:#99ff66; stroke-width:.6; pointer-events:none; }
.clip { fill:#ff2fb211; stroke:#ff5ccf; stroke-width:1.25; stroke-dasharray:6 3; pointer-events:none; }
.clip.selected { stroke:#fff; stroke-width:2; stroke-dasharray:none; }
.clipEdge { stroke:#ff5ccf; stroke-width:.8; stroke-dasharray:2 3; pointer-events:none; }
.label { fill:#ddd; font: 12px monospace; pointer-events:none; }
.clipLabel { fill:#ff9de3; font: 10px monospace; pointer-events:none; }
.rowline { stroke:#333; stroke-width:.5; }
</style>
<body>
<aside>
  <h2>05_010 ProfileSelect</h2>
  <p class="warn">Offline canvas is approximate. Live runtime calibration is authoritative only after Elden Ring/Scaleform writes matching acks.</p>
  <label>render mode <select id="renderMode"><option value="live_runtime" selected>live runtime calibration (authoritative when connected)</option><option value="offline_approximate">offline approximate canvas (not proof)</option></select></label>
  <div id="runtimeBadge" class="bad">runtime disconnected</div>
  <div class="controls">
    <button class="primary" id="saveBtn">save schema</button>
    <button id="rebuildBtn">hot rebuild GFX</button>
    <button id="probeTextBtn">probe selected text</button>
    <button id="charStatsSafeBtn">safe larger Character Stats</button>
    <button id="reloadBtn">reload</button>
  </div>
  <label>sample mode <select id="mode"><option>load_character</option><option>save_picker</option><option>drive_row</option></select></label>
  <label><input type="checkbox" id="showClips" checked> show text clipping masks</label>
  <label><input type="checkbox" id="clipPreview" checked> clip approximate text to masks</label>
  <label><input type="checkbox" id="watchSchema" checked> watch TOML file edits and hot rebuild automatically</label>
  <div id="schemaWatch" class="ok">schema watcher starting</div>
  <div class="tree" id="tree"></div>
</aside>
<section>
  <h3>Approximate layout canvas</h3>
  <svg id="canvas" viewBox="-620 -285 1260 570"></svg>
</section>
<aside>
  <h3 id="title">select object</h3>
  <div id="editor"></div>
  <h3>Layout health</h3>
  <pre id="layoutHealth">not checked</pre>
  <h3>Runtime bridge status</h3>
  <pre id="runtimeStatus">disconnected</pre>
  <h3>Rebuild output</h3>
  <pre id="log">not run</pre>
  <h3>Files</h3>
  <pre id="paths"></pre>
</aside>
<script>
let data=null, selected={kind:'field', name:'PlayerName'}, drag=null, runtimeStatus=null, lastSchemaMtimeNs=0, schemaWatchSuppressedUntil=0;
const fields = %FIELDS%;
const chromeObjects = %CHROME%;
const paths = %PATHS%;
const FIELD_CLIP_LOCAL = { ErCharStats:{x:-35,y:-2}, default:{x:-2,y:-2} };
const TEXT_LAYOUT_INSET_PX = 40;
// Menu font vertical metrics (data0:/font/eu_std/font.gfx DefineFont3 id=1 "Agmena W1G For
// Bandai"). Mirrors the Python + Rust copies; all three must agree. One laid-out line is
// (ascent+descent)/em * font_height px, and GFx::TextField reflow insets 2px per edge, so a
// box shorter than that sum clips its own text. clip_height 30 at font_height 24 is how
// PlayerName shipped broken -- the floor below makes that unreachable from the UI.
const MENU_FONT_ASCENT = 20800, MENU_FONT_DESCENT = 8540, MENU_FONT_EM_SQUARE = 20480;
const TEXT_DOC_INSET_PX_PER_EDGE = 2;
function lineBoxPx(fontHeight){ return (MENU_FONT_ASCENT+MENU_FONT_DESCENT)/MENU_FONT_EM_SQUARE*Number(fontHeight||16); }
function minClipHeightPx(fontHeight){ return Math.ceil(lineBoxPx(fontHeight) + 2*TEXT_DOC_INSET_PX_PER_EDGE); }
// Inverse of the floor. font_height hot-reloads live; clip_height does NOT (it is baked into the
// movie and only changes on rebuild + screen reopen), so a font raised past this overflows a box
// that cannot grow. The DLL clamps to the same number, so the render is safe either way -- this
// keeps the editor from silently authoring a value the live screen will not honour.
function maxFontHeightPx(clipHeight){ const usable=Number(clipHeight)-2*TEXT_DOC_INSET_PX_PER_EDGE;
  return usable<=0 ? 0 : Math.floor(usable*MENU_FONT_EM_SQUARE/(MENU_FONT_ASCENT+MENU_FONT_DESCENT)); }
document.getElementById('paths').textContent = JSON.stringify(paths, null, 2);
async function schemaMeta(){ return await (await fetch('/schema-meta',{cache:'no-store'})).json(); }
async function load(){ const r=await fetch('/schema',{cache:'no-store'}); if(!r.ok) throw new Error(await r.text()); data=await r.json(); const meta=await schemaMeta(); lastSchemaMtimeNs=Number(meta.mtime_ns||0); renderTree(); renderEditor(); draw(); document.getElementById('schemaWatch').textContent='watching '+paths.schema; }
async function watchSchema(){ try { if(document.getElementById('watchSchema').checked && Date.now()>=schemaWatchSuppressedUntil){ const meta=await schemaMeta(); const stamp=Number(meta.mtime_ns||0); if(lastSchemaMtimeNs && stamp!=lastSchemaMtimeNs){ const r=await fetch('/schema',{cache:'no-store'}); if(!r.ok) throw new Error(await r.text()); data=await r.json(); lastSchemaMtimeNs=stamp; renderTree(); renderEditor(); draw(); document.getElementById('schemaWatch').className='ok'; document.getElementById('schemaWatch').textContent='external TOML edit loaded; runtime control + hot rebuild started'; await postSchema({data,render_mode:renderMode(),selected:payloadSelected(),text_probe:false}); await fetch('/rebuild',{method:'POST'}); poll(); } else if(!lastSchemaMtimeNs) lastSchemaMtimeNs=stamp; } } catch(e){ document.getElementById('schemaWatch').className='bad'; document.getElementById('schemaWatch').textContent='schema watch failed: '+String(e); showError(e); } finally { setTimeout(watchSchema,500); } }
function showError(err){ const msg=(err&&err.stack)||String(err); document.getElementById('log').textContent='EDITOR JS ERROR\n'+msg; console.error(err); }
function objectList(){ return [['path_editor','ActivePathEditor'],['clip','EntryNameClip']].concat(fields.map(n=>['field',n]), chromeObjects.map(n=>['chrome',n]), [['list','list']]); }
function chromeLabel(name){ return ({backing:'normal row backing (Backing/char54)', hit_area:'invisible full-row mouse target (HitArea/char54)', cursor:'highlight wrapper (Cursor/char75)', cursor_body:'highlight inner body (Cursor/CursorBody/char73)', drive_button:'all drive button frames (DriveButton_*)', path_button:'full-path button frame (CurrentPathButton)'})[name] || name; }
function objectLabel(kind,name){ if(kind=='path_editor') return 'ACTIVE DRIVE-PATH EDITOR — 02_990'; if(kind=='clip') return 'PLAYER NAME ONLY — EntryNameClip'; if(kind=='field'&&name=='CurrentPath') return 'READ-ONLY DRIVE PATH LABEL — CurrentPath'; if(kind=='field') return 'text '+name; if(kind=='chrome') return 'chrome '+chromeLabel(name); return 'list'; }
function renderTree(){ const t=document.getElementById('tree'); t.innerHTML=''; for(const [kind,name] of objectList()){ const b=document.createElement('button'); b.className=(selected.kind==kind&&selected.name==name)?'sel':''; b.textContent=objectLabel(kind,name); b.onclick=()=>{selected={kind,name};renderTree();renderEditor();draw();}; t.appendChild(b);} }
function getObj(){ if(selected.kind=='path_editor') return data.path_editor; if(selected.kind=='clip') return data.fields.PlayerName; if(selected.kind=='field') return data.fields[selected.name]; if(selected.kind=='chrome') return data.row_chrome[selected.name]; return data.list; }
function payloadSelected(){ return selected.kind=='clip'?{kind:'field',name:'PlayerName'}:selected; }
function numberInput(obj,k,step=1){ return `<label>${k}</label><input type="number" step="${step}" value="${obj[k]}" onchange="setVal('${k}', this.value)">`; }
function driveCellNames(){ return fields.filter(name=>name.startsWith('DriveCell_')); }
function isDriveCellSelection(){ return selected.kind=='field' && selected.name.startsWith('DriveCell_'); }
function setDriveCellValue(k,v){ const value=Number(v); if(!Number.isFinite(value)) return; for(const name of driveCellNames()){ const f=data.fields[name]; f[k]=value; if(k=='clip_height'||k=='font_height') f.clip_height=Math.max(Number(f.clip_height),minClipHeightPx(f.font_height)); } renderEditor(); draw(); }
function padDriveCellValue(k,delta){ const base=Number(data.fields.DriveCell_0[k]); setDriveCellValue(k,k=='clip_height'?Math.max(minClipHeightPx(data.fields.DriveCell_0.font_height),Math.round(base+delta)):Math.max(1,Math.round(base+delta))); }
function setDriveButtonScaleY(v){ const value=Number(v); if(Number.isFinite(value)&&value>0){ data.row_chrome.drive_button.scale_y=value; renderEditor(); draw(); } }
function buttonChromeBaseRect(name=selected.name){ const f=name=='drive_button'?data.fields.DriveCell_0:data.fields.CurrentPath; return {width:Number(f.width),height:Number(f.clip_height)}; }
function buttonChromeWidthPx(name=selected.name){ const b=buttonChromeBaseRect(name), o=data.row_chrome[name]; return Math.round(b.width*Number(o.scale_x)*100)/100; }
function buttonChromeHeightPx(name=selected.name){ const b=buttonChromeBaseRect(name), o=data.row_chrome[name]; return Math.round(b.height*Number(o.scale_y)*100)/100; }
function setButtonChromeWidthPx(v){ const value=Number(v), b=buttonChromeBaseRect(); if(Number.isFinite(value)&&value>0&&b.width>0){ getObj().scale_x=value/b.width; renderEditor(); draw(); } }
function setButtonChromeHeightPx(v){ const value=Number(v), b=buttonChromeBaseRect(); if(Number.isFinite(value)&&value>0&&b.height>0){ getObj().scale_y=value/b.height; renderEditor(); draw(); } }
function matchButtonChromeToRow(){ const o=getObj(); o.scale_x=1; o.scale_y=1; renderEditor(); draw(); }
function textInput(obj,k){ return `<label>${k}</label><input value="${String(obj[k]??'').replaceAll('&','&amp;').replaceAll('"','&quot;')}" onchange="setText('${k}', this.value)">`; }
function fieldClipOffset(name){ return FIELD_CLIP_LOCAL[name] || FIELD_CLIP_LOCAL.default; }
function fieldClip(f,name=selected.name){ const off=fieldClipOffset(name); const left=Number(f.x)+off.x, top=Number(f.y)+off.y, width=Number(f.width), height=Number(f.clip_height||40); return {left,top,width,height,right:left+width,bottom:top+height,off}; }
function fieldTextArea(f,name=selected.name){ const clip=fieldClip(f,name); const inset=Math.min(TEXT_LAYOUT_INSET_PX, Math.max(0, clip.width/2-1)); return {...clip, left:clip.left+inset, right:clip.right-inset, width:Math.max(1, clip.width-inset*2), inset}; }
function syncCurrentPathChromeWidth(){ if(selected.kind=='field'&&selected.name=='CurrentPath') data.row_chrome.path_button.scale_x=1; }
function setClipRight(v){ const o=getObj(), name=selected.kind=='clip'?'PlayerName':selected.name; const clip=fieldClip(o,name); o.width=Math.max(1, Math.round((Number(v)-clip.left)*100)/100); syncCurrentPathChromeWidth(); renderEditor(); draw(); }
function setClipBottom(v){ const o=getObj(), name=selected.kind=='clip'?'PlayerName':selected.name; const clip=fieldClip(o,name); o.clip_height=Math.max(minClipHeightPx(o.font_height), Math.round((Number(v)-clip.top)*100)/100); renderEditor(); draw(); }
function renderEditor(){ const o=getObj(), e=document.getElementById('editor'); document.getElementById('title').textContent=selected.kind+': '+selected.name; let h='<div class="grid">'; if(selected.kind=='clip'){ const clip=fieldClip(o,'PlayerName'), right=Math.round(clip.right*100)/100, bottom=Math.round(clip.bottom*100)/100, sample=sampleFor(o)||'PlayerName', estimated=Math.ceil(estimateTextWidth(sample,o.font_height)); h+=`<div>ENTRY-NAME TEXT FIELD</div><div class="ok">EntryNameClip is PlayerName's real SWF DefineEditText field. It exposes the same position, bounds, font, alignment, sample, nudge, and fit handles as every other editable text field.</div>`; for(const k of ['x','y']) h+=numberInput(o,k,0.25); h+=`<label>actual clip left</label><output>${Math.round(clip.left*100)/100}</output><label>actual clip right</label><input type="number" step="0.25" value="${right}" onchange="setClipRight(this.value)"><label>actual clip top</label><output>${Math.round(clip.top*100)/100}</output><label>actual clip bottom</label><input type="number" step="0.25" value="${bottom}" onchange="setClipBottom(this.value)"><label>estimated sample width</label><output>${estimated}px</output><label>runtime usable width</label><output>${Math.max(1,Math.round((clip.width-TEXT_LAYOUT_INSET_PX*2)*100)/100)}px</output>`; for(const k of ['width','clip_height','font_height']) h+=numberInput(o,k,1); h+=`<label>align</label><select onchange="setText('align',this.value)">${['left','center','right'].map(a=>`<option ${o.align==a?'selected':''}>${a}</option>`).join('')}</select><div>clip width</div><div class="controls"><button onclick="padWidth(-20)">-20</button><button onclick="padWidth(-5)">-5</button><button onclick="padWidth(5)">+5</button><button onclick="padWidth(20)">+20</button><button onclick="fitWidthToSample(24)">fit sample +24</button><button onclick="fitWidthToSample(80)">fit current sample +80</button><button onclick="playerNameColumnFit()">fit browse-name column</button></div><div>clip height</div><div class="controls"><button onclick="padClipHeight(-4)">-4</button><button onclick="padClipHeight(4)">+4</button><button onclick="padClipHeight(10)">+10</button><button onclick="fitClipHeightToFont(2)">fit to font</button></div><div></div><div class="warn">The red right-edge and bottom-edge handles are draggable. Width and height are baked into the GFX; rebuild and reopen ProfileSelect to apply them.</div>`; for(const k of ['sample_load_character','sample_save_picker','sample_drive_row']) h+=textInput(o,k); }
else if(selected.kind=='field'||selected.kind=='path_editor'){ const clip=fieldClip(o, selected.name); const right=Math.round(clip.right*100)/100; const sample=sampleFor(o)||selected.name; const estimated=Math.ceil(estimateTextWidth(sample, o.font_height)); if(selected.kind=='path_editor') h+=`<div>ACTIVE 02_990 PATH EDITOR</div><div class="ok">Independent geometry for the native SoftwareKeyboard text-entry movie. Moving or resizing this does not move the read-only CurrentPath label.</div>`; if(selected.name=='CurrentPath') h+=`<div>FULL-PATH SUB-ROW CONTROL</div><div class="ok">Width drives the CurrentPath text field, CurrentPathButton frame, and focused native Cursor from one value. Changing width resets the button width multiplier to 1 so all three stay exact.</div>`; if(isDriveCellSelection()){ const d=data.fields.DriveCell_0, b=data.row_chrome.drive_button; h+=`<div>DRIVE CELLS</div><div class="ok">These shared controls resize every drive cell and its generated native button. They are the honest drive-strip geometry controls; the generic clip fields below show the same values for the selected sample cell.</div><label>drive cell width (all drives)</label><input type="number" min="1" step="1" value="${d.width}" onchange="setDriveCellValue('width',this.value)"><label>drive cell height (all drives)</label><input type="number" min="1" step="1" value="${d.clip_height}" onchange="setDriveCellValue('clip_height',this.value)"><div>drive cell height</div><div class="controls"><button onclick="padDriveCellValue('clip_height',-4)">-4</button><button onclick="padDriveCellValue('clip_height',-1)">-1</button><button onclick="padDriveCellValue('clip_height',1)">+1</button><button onclick="padDriveCellValue('clip_height',4)">+4</button></div><label>button chrome height multiplier</label><input type="number" min="0.001" max="40" step="0.01" value="${b.scale_y}" onchange="setDriveButtonScaleY(this.value)"><div></div><div class="warn">Cell height changes the text clip and the generated button height. The chrome multiplier changes only the button artwork around that cell.</div>`; } for(const k of ['x','y']) h+=numberInput(o,k,0.25); h+=`<label>actual clip right</label><input type="number" step="0.25" value="${right}" onchange="setClipRight(this.value)">`; h+=`<label>estimated sample width</label><output>${estimated}px</output>`; h+=`<label>runtime usable width</label><output>${Math.max(1, Math.round((clip.width-TEXT_LAYOUT_INSET_PX*2)*100)/100)}px</output>`; h+=`<div></div><div class="ok">The magenta mask uses the real SWF DefineEditText local bounds: ${clip.off.x}px/${clip.off.y}px from placement. The cyan box is Scaleform's observed TextDoc layout area, about ${TEXT_LAYOUT_INSET_PX}px inset on each side; text must fit cyan, not just magenta.</div>`; for(const k of ['width','clip_height','font_height']) h+=numberInput(o,k,1); h+=`<label>align</label><select onchange="setText('align',this.value)">${['left','center','right'].map(a=>`<option ${o.align==a?'selected':''}>${a}</option>`).join('')}</select>`; h+=`<div>clip width</div><div class="controls"><button onclick="padWidth(-20)">-20</button><button onclick="padWidth(-5)">-5</button><button onclick="padWidth(5)">+5</button><button onclick="padWidth(20)">+20</button><button onclick="fitWidthToSample(24)">fit sample +24</button></div>`; h+=`<div>clip height</div><div class="controls"><button onclick="padClipHeight(-4)">-4</button><button onclick="padClipHeight(4)">+4</button><button onclick="padClipHeight(10)">+10</button><button onclick="fitClipHeightToFont(2)" title="smallest box that renders one line of this font, plus 2px">fit to font</button></div>`; h+=`<div></div><div class="warn">Width is the GFX DefineEditText bounds. Live width is experimental; rebuild/reload is the honest path if status rejects the live probe.</div>`; h+=`<div></div><div class="warn">font_height applies LIVE; clip_height does NOT -- the box is baked into the movie and only changes on rebuild + screen reopen. This field\u0027s current box (${o.clip_height}px) renders at most font_height ${maxFontHeightPx(o.clip_height)}. Above that the DLL clamps the rendered size to ${maxFontHeightPx(o.clip_height)} so it cannot overflow; your value is still saved and takes effect once the rebuilt movie is loaded.</div>`; if(selected.name=='PlayerName') h+=`<div>Player name</div><div class="controls"><button onclick="playerNameWideFit()">open/fit entry-name clip</button><button onclick="fitWidthToSample(80)">fit current sample +80</button></div><div></div><div class="warn">If the in-game name still shows only a prefix after this mask is wide, the remaining bug is runtime text payload/source, not this field's GFX bounds.</div>`; if(selected.name=='ErCharStats') h+=`<div>Character Stats</div><div class="controls"><button onclick="charStatsSafeFit()">safe larger fit (17px)</button><button onclick="charStatsCompact16()">compact 16 fallback</button><button onclick="charStatsFont16()">font 16 only</button><button onclick="charStatsExperimental18()">experimental 18px</button><button onclick="charStatsWideProbe()">try width 600 live probe</button></div><div></div><div class="warn">Safe larger fit sets x=-185, y=-14, width=470, clip_height=42, font_height=17 and passed full offline overlap/clip tests. Width 600 is experimental live necromancy; if status says width_probe=false, it will not fix clipping until a rebuild/reload.</div>`; for(const k of ['sample_load_character','sample_save_picker','sample_drive_row']) h+=textInput(o,k); }
else if(selected.kind=='chrome'){ const buttonChrome=selected.name=='drive_button'||selected.name=='path_button'; if(buttonChrome){ for(const k of ['x','y']) h+=numberInput(o,k,0.1); h+=`<label>button chrome width (px)</label><input type="number" min="0.01" step="0.25" value="${buttonChromeWidthPx()}" onchange="setButtonChromeWidthPx(this.value)"><label>button chrome height (px)</label><input type="number" min="0.01" step="0.25" value="${buttonChromeHeightPx()}" onchange="setButtonChromeHeightPx(this.value)"><div>exact row/cell match</div><div class="controls"><button onclick="matchButtonChromeToRow()">match width + height</button></div><label>button chrome width multiplier</label><input type="number" min="0.001" max="40" step="0.01" value="${o.scale_x}" onchange="setVal('scale_x',this.value)"><label>button chrome height multiplier</label><input type="number" min="0.001" max="40" step="0.01" value="${o.scale_y}" onchange="setVal('scale_y',this.value)">`; } else for(const k of ['x','y','scale_x','scale_y']) h+=numberInput(o,k,k.startsWith('scale')?0.001:0.1); h+=`<label>art opacity (0 removes outline)</label><input type="number" min="0" max="1" step="0.05" value="${o.opacity}" onchange="setVal('opacity',this.value)"><label>editable</label><input type="checkbox" ${o.editable?'checked':''} onchange="setBool('editable',this.checked)">`; if(selected.name=='backing') h+=`<div></div><div class="ok">normal non-highlighted row rectangle: live transform via named Backing when connected</div>`; if(selected.name=='cursor') h+=`<div></div><div class="ok">outer highlighted-row wrapper: live transform when connected; use cursor_body for visible highlight-panel height</div>`; if(selected.name=='cursor_body') h+=`<div></div><div class="ok">inner highlighted-row body: live transform via Cursor/CursorBody when connected</div>`; if(selected.name=='drive_button') h+=`<div></div><div class="ok">shared RELATIVE transform and baked opacity for every DriveButton_* frame. Lower opacity softens the bold native outline without dimming drive text.</div>`; if(selected.name=='path_button') h+=`<div></div><div class="ok">independent CurrentPathButton transform applies live on the picker-owned drive row; opacity remains baked. CurrentPath width resets scale_x to 1 for exact text/button/Cursor sync.</div>`; h+=textInput(o,'source'); }
else { for(const k of ['row_pitch','visible_rows','top_recycle_rows','bottom_recycle_rows','mask_height','scrollbar_x','scrollbar_top_y','scrollbar_track_height']) h+=numberInput(o,k,1); }
 h+='</div><div class="controls"><button onclick="nudge(-1,0)">← 1</button><button onclick="nudge(1,0)">→ 1</button><button onclick="nudge(0,-1)">↑ 1</button><button onclick="nudge(0,1)">↓ 1</button><button onclick="nudge(-5,0)">← 5</button><button onclick="nudge(5,0)">→ 5</button><button onclick="nudge(0,-5)">↑ 5</button><button onclick="nudge(0,5)">↓ 5</button></div>'; e.innerHTML=h; }
// Typing a value directly is clamped the same way the nudge buttons are: clip_height cannot go
// under the font's floor, and raising font_height carries clip_height up with it. Without this,
// the only thing standing between a hand-typed 30 and a broken build was the save-time reject.
// List geometry that draw() divides or steps by must stay positive. Number("") is 0 and Number("-4")
// is negative, so clearing the row_pitch box (or typing a minus) made draw()'s
// `for(let y=-half; y<=half; y+=rowPitch)` never terminate -- it appended SVG lines until the tab
// hung, taking every unsaved edit with it. The hang happened before any POST, so the game was never
// involved; the cost was purely the user's in-progress work.
const LIST_MUST_BE_POSITIVE = ['row_pitch','visible_rows','mask_height','scrollbar_track_height'];
let pathEditorLiveSaveTimer=null;
function schedulePathEditorPositionSave(){
  if(selected.kind!='path_editor') return;
  clearTimeout(pathEditorLiveSaveTimer);
  pathEditorLiveSaveTimer=setTimeout(async()=>{
    try {
      const mode=runtimeStatus&&runtimeStatus.connected?'live_runtime':renderMode();
      if(mode=='live_runtime') document.getElementById('renderMode').value='live_runtime';
      await postSchema({data,render_mode:mode,selected:payloadSelected(),text_probe:false});
      pollStatus();
    } catch(e) { showError(e); }
  },80);
}
function setVal(k,v){ const o=getObj(); o[k]=Number(v);
  if(!Number.isFinite(Number(o[k]))) o[k]=0;
  if(selected.kind=='chrome' && k=='opacity') o[k]=Math.max(0,Math.min(1,Number(o[k])));
  if((selected.kind=='field'||selected.kind=='path_editor'||selected.kind=='clip') && (k=='clip_height'||k=='font_height')){
    const floor=minClipHeightPx(o.font_height);
    if(Number(o.clip_height)<floor) o.clip_height=floor;
  }
  if(isDriveCellSelection() && ['y','width','clip_height','font_height'].includes(k)) for(const name of driveCellNames()) data.fields[name][k]=o[k];
  if(selected.kind=='list' && LIST_MUST_BE_POSITIVE.includes(k) && !(Number(o[k])>0)) o[k]=1;
  if(k=='width') syncCurrentPathChromeWidth();
  renderEditor(); draw(); if(selected.kind=='path_editor'&&(k=='x'||k=='y')) schedulePathEditorPositionSave(); }
function setText(k,v){ getObj()[k]=v; if(isDriveCellSelection()&&k=='align') for(const name of driveCellNames()) data.fields[name][k]=v; renderEditor(); draw(); }
function setBool(k,v){ getObj()[k]=v; renderEditor(); draw(); }
function nudge(dx,dy){ const o=getObj(); if('x' in o) o.x+=dx; if(isDriveCellSelection()&&dy) for(const name of driveCellNames()) data.fields[name].y+=dy; else if('y' in o) o.y+=dy; renderEditor(); draw(); schedulePathEditorPositionSave(); }
function padWidth(delta){ const o=getObj(); if(isDriveCellSelection()) return padDriveCellValue('width',delta); if('width' in o) o.width=Math.max(1, Math.round(Number(o.width)+delta)); syncCurrentPathChromeWidth(); renderEditor(); draw(); }
// Nudging down stops at the font's floor instead of sailing past it into an unrenderable box.
function padClipHeight(delta){ const o=getObj(); if(isDriveCellSelection()) return padDriveCellValue('clip_height',delta); if('clip_height' in o) o.clip_height=Math.max(minClipHeightPx(o.font_height), Math.round(Number(o.clip_height)+delta)); renderEditor(); draw(); }
// pad is headroom ABOVE the real minimum, not above font_height. The old form was
// ceil(font_height + 10), which at font_height 24 gave 34 -- five px short of renderable.
function fitClipHeightToFont(pad=2){ const o=getObj(), value=minClipHeightPx(o.font_height)+Math.max(0,Math.round(pad)); if(isDriveCellSelection()) return setDriveCellValue('clip_height',value); if('clip_height' in o) o.clip_height=value; renderEditor(); draw(); }
function estimateTextWidth(text, fontHeight){ const plain=String(text||'').replace(/<[^>]*>/g,''); let w=0; for(const ch of plain){ w += /[ilI1|.,:;']/u.test(ch) ? 0.28 : /[MW@#]/u.test(ch) ? 0.85 : /\s/u.test(ch) ? 0.33 : 0.56; } return w * Number(fontHeight||16); }
function fitWidthToSample(pad=24){ const o=getObj(); o.width=Math.max(1, Math.ceil(estimateTextWidth(sampleFor(o)||selected.name, o.font_height)+pad+(TEXT_LAYOUT_INSET_PX*2))); syncCurrentPathChromeWidth(); renderEditor(); draw(); }
function selectCharStats(){ selected={kind:'field', name:'ErCharStats'}; renderTree(); renderEditor(); draw(); }
function selectPlayerName(){ selected={kind:'field', name:'PlayerName'}; renderTree(); renderEditor(); draw(); }
function selectEntryNameClip(){ selected={kind:'clip', name:'EntryNameClip'}; renderTree(); renderEditor(); draw(); }
function charStatsSafeFit(){ const f=data.fields.ErCharStats; Object.assign(f,{x:-185,y:-14,width:470,clip_height:42,font_height:17,align:'center'}); selectCharStats(); save(); }
function charStatsExperimental18(){ const f=data.fields.ErCharStats; Object.assign(f,{x:-240,y:-14,width:530,clip_height:46,font_height:18,align:'center'}); selectCharStats(); save(); }
function charStatsCompact16(){ const f=data.fields.ErCharStats; Object.assign(f,{x:-161,y:-14,width:425,clip_height:40,font_height:16,align:'center'}); selectCharStats(); save(); }
function charStatsFont16(){ data.fields.ErCharStats.font_height=16; selectCharStats(); save(); }
function charStatsWideProbe(){ Object.assign(data.fields.ErCharStats,{width:600,clip_height:50}); selectCharStats(); save(); }
function playerNameColumnFit(){ const f=data.fields.PlayerName; Object.assign(f,{x:-538,width:340}); selectEntryNameClip(); save(); }
function playerNameWideFit(){ playerNameColumnFit(); }
function sampleFor(f){ const m=document.getElementById('mode').value; return f['sample_'+m] || ''; }
function clipHeight(f){ return Number(f.clip_height||40); }
function clipTop(f){ return Number(f.y); }
function renderedTextRect(name,f){ const text=sampleFor(f); if(!text) return null; const clip=fieldClip(f,name), area=fieldTextArea(f,name); const w=Math.ceil(estimateTextWidth(text, f.font_height)); let left=area.left; if(f.align=='right') left=area.right-w; else if(f.align=='center') left=area.left+(area.width-w)/2; const top=clip.top+TEXT_DOC_INSET_PX_PER_EDGE; return {name,left,right:left+w,top,bottom:top+lineBoxPx(f.font_height),clipLeft:area.left,clipRight:area.right,clipTop:clip.top,clipBottom:clip.bottom,text}; }
function layoutHealth(){ const rects=Object.entries(data.fields).map(([n,f])=>renderedTextRect(n,f)).filter(Boolean); const notes=[]; for(const r of rects){ if(r.left<r.clipLeft || r.right>r.clipRight || r.top<r.clipTop || r.bottom>r.clipBottom) notes.push(`CLIP ${r.name}: text ${Math.round(r.left)}..${Math.round(r.right)} x ${Math.round(r.top)}..${Math.round(r.bottom)} outside mask ${Math.round(r.clipLeft)}..${Math.round(r.clipRight)} x ${Math.round(r.clipTop)}..${Math.round(r.clipBottom)}`); } for(let i=0;i<rects.length;i++) for(let j=i+1;j<rects.length;j++){ const a=rects[i], b=rects[j], gutter=4; const overlap=!(a.right+gutter<=b.left || b.right+gutter<=a.left || a.bottom+gutter<=b.top || b.bottom+gutter<=a.top); if(overlap) notes.push(`OVERLAP ${a.name} vs ${b.name}`); } document.getElementById('layoutHealth').textContent = notes.length ? notes.join('\n') : 'ok: approximate samples fit their masks and do not overlap'; }
function add(tag, attrs, parent=document.getElementById('canvas')){ const el=document.createElementNS('http://www.w3.org/2000/svg',tag); for(const [k,v] of Object.entries(attrs)) el.setAttribute(k,v); parent.appendChild(el); return el; }
// Second belt on the same hazard: even if a non-positive pitch reaches here (loaded file, hand-edited
// schema), refuse to enter the stepping loop rather than hanging the tab.
function draw(){ layoutHealth(); const svg=document.getElementById('canvas'); svg.innerHTML=''; if(!(Number(data.list.row_pitch)>0)) { document.getElementById('layoutHealth').textContent='list.row_pitch must be > 0; canvas not drawn'; return; } const rowPitch=data.list.row_pitch, half=data.list.mask_height/2, showClips=document.getElementById('showClips').checked, clipPreview=document.getElementById('clipPreview').checked; const defs=add('defs',{},svg); for(let y=-half;y<=half;y+=rowPitch) add('line',{x1:-620,x2:640,y1:y,y2:y,class:'rowline'}); add('rect',{x:-620,y:-half,width:1260,height:data.list.mask_height,fill:'none',stroke:'#777','stroke-dasharray':'6 4'}); add('rect',{x:data.list.scrollbar_x,y:data.list.scrollbar_top_y,width:10,height:data.list.scrollbar_track_height,fill:'#8884',stroke:'#aaa'});
 for(const [name,f] of Object.entries(data.fields)){ const sel=(selected.kind=='field'&&selected.name==name)||(selected.kind=='clip'&&name=='PlayerName'); const clip=fieldClip(f,name), clipId='clip_'+name.replace(/[^A-Za-z0-9_-]/g,'_'); const cp=add('clipPath',{id:clipId},defs); add('rect',{x:clip.left,y:clip.top,width:clip.width,height:clip.height},cp); const r=add('rect',{x:clip.left,y:clip.top,width:clip.width,height:clip.height,class:showClips?('clip '+(sel?'selected':'')):'field',stroke:sel?'#fff':'#6ca0ff','stroke-width':sel?2:1}); bindDrag(r,name=='PlayerName'&&selected.kind=='clip'?'clip':'field',name=='PlayerName'&&selected.kind=='clip'?'EntryNameClip':name); if(name=='PlayerName'){ const widthHandle=add('rect',{x:clip.right-4,y:clip.top,width:8,height:clip.height,fill:'#ff536a88',stroke:'#ff536a','stroke-width':1,cursor:'ew-resize'}); bindClipResize(widthHandle); const heightHandle=add('rect',{x:clip.left,y:clip.bottom-4,width:clip.width,height:8,fill:'#ff536a66',stroke:'#ff536a','stroke-width':1,cursor:'ns-resize'}); bindClipHeightResize(heightHandle); } const area=fieldTextArea(f,name); if(showClips){ add('rect',{x:area.left,y:area.top,width:area.width,height:area.height,fill:'#42e8ff10',stroke:'#42e8ff','stroke-width':.8,'stroke-dasharray':'3 2'}); add('line',{x1:clip.right,x2:clip.right,y1:-half,y2:half,class:'clipEdge'}); add('text',{x:clip.left,y:clip.top-5,class:'clipLabel'},svg).textContent=`${name} clip l=${Math.round(clip.left*100)/100} w=${f.width} r=${Math.round(clip.right*100)/100} usable=${Math.round(area.width*100)/100}`; }
 const g=add('g',clipPreview?{'clip-path':`url(#${clipId})`}:{}); const textX=f.align=='right'?area.right:f.align=='center'?area.left+area.width/2:area.left; add('text',{x:textX,y:clip.top+Number(f.font_height||16),class:'label','font-size':Math.max(10,f.font_height),'text-anchor':f.align=='right'?'end':f.align=='center'?'middle':'start'},g).textContent=sampleFor(f)||name; if(!showClips) add('text',{x:clip.left,y:clip.top-5,class:'label'},svg).textContent=name; }
 const driveMode=document.getElementById('mode').value=='drive_row'; let r; if(driveMode){ const pe=data.path_editor, clip=fieldClip(pe,'ActivePathEditor'), sel=selected.kind=='path_editor'; r=add('rect',{x:clip.left,y:clip.top,width:clip.width,height:clip.height,fill:'#43ff8b18',stroke:sel?'#fff':'#43ff8b','stroke-width':sel?2:1,'stroke-dasharray':'5 2'}); bindDrag(r,'path_editor','ActivePathEditor'); const widthHandle=add('rect',{x:clip.right-4,y:clip.top,width:8,height:clip.height,fill:'#43ff8b88',stroke:'#43ff8b','stroke-width':1,cursor:'ew-resize'}); bindPathEditorResize(widthHandle,'width'); const heightHandle=add('rect',{x:clip.left,y:clip.bottom-4,width:clip.width,height:8,fill:'#43ff8b66',stroke:'#43ff8b','stroke-width':1,cursor:'ns-resize'}); bindPathEditorResize(heightHandle,'height'); add('text',{x:clip.left,y:clip.top-7,class:'label'}).textContent='ACTIVE 02_990 EDITOR (independent)'; }
 if(!driveMode){ const backing=data.row_chrome.backing; const bx=backing.x-19.1*backing.scale_x, by=backing.y-19.4*backing.scale_y, bw=39*backing.scale_x, bh=37.5*backing.scale_y; r=add('rect',{x:bx,y:by,width:bw,height:bh,class:'chrome',stroke:selected.kind=='chrome'&&selected.name=='backing'?'#fff':'#ffbd5a','stroke-width':selected.kind=='chrome'&&selected.name=='backing'?2:1}); bindDrag(r,'chrome','backing'); add('text',{x:bx,y:by-4,class:'label'}).textContent='normal backing char54/shape53';
 const c=data.row_chrome.cursor; const cb=data.row_chrome.cursor_body; const cx=c.x+cb.x*c.scale_x, cy=c.y+cb.y*c.scale_y, cw=39*c.scale_x*cb.scale_x, ch=37.5*c.scale_y*cb.scale_y; r=add('rect',{x:cx,y:cy-19.4*c.scale_y*cb.scale_y,width:cw,height:ch,class:'chrome',stroke:selected.kind=='chrome'&&selected.name=='cursor'?'#fff':'#ffbd5a','stroke-width':selected.kind=='chrome'&&selected.name=='cursor'?2:1}); bindDrag(r,'chrome','cursor'); add('text',{x:cx,y:cy-25,class:'label'}).textContent='effective highlight body (cursor + inner body)';
 const body=data.row_chrome.cursor_body; add('rect',{x:body.x,y:body.y-19.4*body.scale_y,width:39*body.scale_x,height:37.5*body.scale_y,class:'readonly'}); add('text',{x:body.x,y:body.y-25,class:'label'}).textContent='raw inner highlight body only'; }
 if(driveMode){ const button=data.row_chrome.drive_button; const selectedCell=selected.kind=='field'&&selected.name.startsWith('DriveCell_')?Math.max(0,Math.min(2,Number(selected.name.slice(10))||0)):0; for(let i=0;i<3;i++){ const f=data.fields['DriveCell_'+i]; const sx=Number(f.width)/39*Number(button.scale_x), sy=Number(f.clip_height)/37.5*Number(button.scale_y); const centerX=Number(f.x)-2+Number(f.width)/2+Number(button.x), centerY=Number(f.y)-2+Number(f.clip_height)/2+Number(button.y); const bx=centerX-19.1*sx, by=centerY-19.4*sy, bw=39*sx, bh=37.5*sy; r=add('rect',{x:bx,y:by,width:bw,height:bh,class:'chrome',opacity:button.opacity,stroke:selected.kind=='chrome'&&selected.name=='drive_button'?'#fff':'#ffbd5a','stroke-width':selected.kind=='chrome'&&selected.name=='drive_button'?2:1}); bindDrag(r,'chrome','drive_button'); if(i==selectedCell){ add('rect',{x:bx,y:by,width:bw,height:bh,class:'readonly',stroke:'#62e6ff','stroke-width':2}); add('text',{x:bx,y:by-5,class:'label'}).textContent='active native Cursor preview'; } } const pf=data.fields.CurrentPath, pathButton=data.row_chrome.path_button, psx=Number(pf.width)/39*Number(pathButton.scale_x), psy=Number(pf.clip_height)/37.5*Number(pathButton.scale_y), pcx=Number(pf.x)-2+Number(pf.width)/2+Number(pathButton.x), pcy=Number(pf.y)-2+Number(pf.clip_height)/2+Number(pathButton.y), pbx=pcx-19.1*psx, pby=pcy-19.4*psy; r=add('rect',{x:pbx,y:pby,width:39*psx,height:37.5*psy,class:'chrome',opacity:pathButton.opacity,stroke:(selected.kind=='field'&&selected.name=='CurrentPath')||(selected.kind=='chrome'&&selected.name=='path_button')?'#fff':'#9bd0ff','stroke-width':(selected.kind=='field'&&selected.name=='CurrentPath')||(selected.kind=='chrome'&&selected.name=='path_button')?2:1}); bindDrag(r,'field','CurrentPath'); add('text',{x:pbx,y:pby-5,class:'label'}).textContent='clickable full-path control'; add('text',{x:-535,y:-24,class:'label'}).textContent='drive row: active Cursor uses the selected cell button bounds'; } }
function bindClipResize(el){ el.onpointerdown=(ev)=>{selected={kind:'clip',name:'EntryNameClip'};renderTree();renderEditor();drag={kind:'clip-resize',sx:ev.clientX,width:Number(data.fields.PlayerName.width)}; el.setPointerCapture(ev.pointerId); ev.stopPropagation();}; el.onpointermove=(ev)=>{if(!drag||drag.kind!='clip-resize')return; data.fields.PlayerName.width=Math.max(1,Math.round((drag.width+(ev.clientX-drag.sx))*10)/10);renderEditor();draw();}; el.onpointerup=()=>drag=null; }
function bindClipHeightResize(el){ el.onpointerdown=(ev)=>{selected={kind:'clip',name:'EntryNameClip'};renderTree();renderEditor();drag={kind:'clip-height-resize',sy:ev.clientY,height:Number(data.fields.PlayerName.clip_height)}; el.setPointerCapture(ev.pointerId); ev.stopPropagation();}; el.onpointermove=(ev)=>{if(!drag||drag.kind!='clip-height-resize')return; const f=data.fields.PlayerName; f.clip_height=Math.max(minClipHeightPx(f.font_height),Math.round((drag.height+(ev.clientY-drag.sy))*10)/10);renderEditor();draw();}; el.onpointerup=()=>drag=null; }
function bindPathEditorResize(el,axis){ el.onpointerdown=(ev)=>{selected={kind:'path_editor',name:'ActivePathEditor'};renderTree();renderEditor();const f=data.path_editor;drag={kind:'path-editor-'+axis,sx:ev.clientX,sy:ev.clientY,width:Number(f.width),height:Number(f.clip_height)};el.setPointerCapture(ev.pointerId);ev.stopPropagation();};el.onpointermove=(ev)=>{if(!drag||drag.kind!='path-editor-'+axis)return;const f=data.path_editor;if(axis=='width')f.width=Math.max(1,Math.round((drag.width+(ev.clientX-drag.sx))*10)/10);else f.clip_height=Math.max(minClipHeightPx(f.font_height),Math.round((drag.height+(ev.clientY-drag.sy))*10)/10);renderEditor();draw();};el.onpointerup=()=>drag=null; }
function bindDrag(el,kind,name){ el.onpointerdown=(ev)=>{selected={kind,name};renderTree();renderEditor();drag={kind,name,sx:ev.clientX,sy:ev.clientY,ox:getObj().x,oy:getObj().y}; el.setPointerCapture(ev.pointerId);}; el.onpointermove=(ev)=>{ if(!drag||drag.kind=='clip-resize') return; const o=getObj(); o.x=Math.round((drag.ox+(ev.clientX-drag.sx))*10)/10; o.y=Math.round((drag.oy+(ev.clientY-drag.sy))*10)/10; renderEditor(); draw();}; el.onpointerup=()=>{schedulePathEditorPositionSave();drag=null;}; }
function renderMode(){ return document.getElementById('renderMode').value; }
function sampleModeChanged(){ if(document.getElementById('mode').value=='drive_row'){ selected={kind:'path_editor',name:'ActivePathEditor'}; renderTree(); renderEditor(); } draw(); }
function renderModeChanged(){ save(); draw(); pollStatus(); }
function selectedNeedsHotGfx(){ return selected.kind!='chrome'||selected.name=='drive_button'||selected.name=='path_button'; }
async function postSchema(payload){ schemaWatchSuppressedUntil=Date.now()+1500; const r=await fetch('/schema',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(payload)}); const msg=await r.text(); document.getElementById('log').textContent=msg; if(!r.ok) throw new Error(msg); const meta=await schemaMeta(); lastSchemaMtimeNs=Number(meta.mtime_ns||0); return msg; }
async function save(){ try { const payload={data, render_mode:renderMode(), selected:payloadSelected(), text_probe:false}; await postSchema(payload); if(selectedNeedsHotGfx()){ await fetch('/rebuild',{method:'POST'}); poll(); } pollStatus(); } catch(e) { showError(e); } }
async function rebuild(){ try { const payload={data, render_mode:renderMode(), selected:payloadSelected(), text_probe:false}; await postSchema(payload); await fetch('/rebuild',{method:'POST'}); poll(); } catch(e) { showError(e); } }
async function probeText(){ try { const payload={data, render_mode:'live_runtime', selected:payloadSelected(), text_probe:true}; await postSchema(payload); pollStatus(); } catch(e) { showError(e); } }
async function poll(){ const s=await (await fetch('/rebuild')).json(); document.getElementById('log').textContent=(s.running?'RUNNING\n':'')+(s.output||''); if(s.running) setTimeout(poll,1000); }
async function pollStatus(){ runtimeStatus=await (await fetch('/status')).json(); const mode=renderMode(); const live=mode=='live_runtime'; const ack=runtimeStatus.ack_sequence||0; document.getElementById('runtimeStatus').textContent=JSON.stringify(runtimeStatus,null,2); const badge=document.getElementById('runtimeBadge'); if(!live){ badge.className='warn'; badge.textContent='offline approximate mode: not visually authoritative'; } else if(runtimeStatus.status=='live-runtime-command-deferred'){ badge.className='warn'; badge.textContent='edit HELD until you leave the profile view (writing to it live crashes the game) -- reopen Load Character to apply'; }
  else if(runtimeStatus.connected){ badge.className='ok'; badge.textContent='live runtime connected: ack '+ack+' status='+runtimeStatus.status; } else { badge.className='bad'; badge.textContent='live runtime requested but disconnected/no ack'; } setTimeout(pollStatus,1000); }
document.addEventListener('keydown',e=>{ if(e.target.tagName=='INPUT')return; const d=e.shiftKey?5:1; if(e.key=='ArrowLeft')nudge(-d,0); if(e.key=='ArrowRight')nudge(d,0); if(e.key=='ArrowUp')nudge(0,-d); if(e.key=='ArrowDown')nudge(0,d); });
Object.assign(window,{load,renderModeChanged,save,rebuild,draw,setVal,setText,setBool,setClipRight,setClipBottom,nudge});
document.getElementById('renderMode').addEventListener('change', renderModeChanged);
document.getElementById('saveBtn').addEventListener('click', save);
document.getElementById('rebuildBtn').addEventListener('click', rebuild);
document.getElementById('probeTextBtn').addEventListener('click', probeText);
document.getElementById('charStatsSafeBtn').addEventListener('click', charStatsSafeFit);
document.getElementById('reloadBtn').addEventListener('click', load);
document.getElementById('mode').addEventListener('change', sampleModeChanged);
document.getElementById('showClips').addEventListener('change', draw);
document.getElementById('clipPreview').addEventListener('change', draw);
window.addEventListener('error', e=>showError(e.error||e.message));
window.addEventListener('unhandledrejection', e=>showError(e.reason));
load().then(watchSchema).catch(showError); pollStatus().catch(showError);
</script>
</body>
"""


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def send_bytes(self, status, body: bytes, ctype: str):
        self.send_response(status)
        self.send_header("content-type", ctype)
        self.send_header("cache-control", "no-store")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = urlsplit(self.path).path
        if path == "/":
            paths = {"schema": str(SCHEMA), "rebuild": str(REBUILD), "repo": str(REPO), "control": str(CONTROL_FILE), "status": str(STATUS_FILE)}
            body = HTML.replace("%FIELDS%", json.dumps(FIELD_NAMES)).replace("%CHROME%", json.dumps(CHROME_NAMES)).replace("%PATHS%", json.dumps(paths))
            self.send_bytes(200, body.encode(), "text/html; charset=utf-8")
        elif path == "/favicon.ico":
            self.send_bytes(204, b"", "image/x-icon")
        elif path == "/schema":
            try:
                self.send_bytes(200, json.dumps(load_schema()).encode(), "application/json")
            except Exception as exc:
                self.send_bytes(400, str(exc).encode(), "text/plain; charset=utf-8")
        elif path == "/schema-meta":
            stat = SCHEMA.stat()
            self.send_bytes(200, json.dumps({"mtime_ns": stat.st_mtime_ns, "size": stat.st_size}).encode(), "application/json")
        elif path == "/rebuild":
            self.send_bytes(200, json.dumps(rebuild_state).encode(), "application/json")
        elif path == "/status":
            self.send_bytes(200, json.dumps(runtime_status(protocol_sequence)).encode(), "application/json")
        else:
            self.send_bytes(404, b"not found", "text/plain")

    def do_POST(self):
        path = urlsplit(self.path).path
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length)
        if path == "/schema":
            payload = json.loads(body.decode())
            if "data" in payload:
                data = payload["data"]
                render_mode = payload.get("render_mode", "offline_approximate")
                selected = payload.get("selected", {"kind": "field", "name": "PlayerName"})
                text_probe = bool(payload.get("text_probe", False))
            else:
                data = payload
                render_mode = "offline_approximate"
                selected = {"kind": "field", "name": "PlayerName"}
                text_probe = False
            errors = validate_data(data, selected)
            if errors:
                self.send_bytes(400, ("invalid 05_010 layout:\n" + "\n".join(f"- {e}" for e in errors) + "\n").encode(), "text/plain")
                return
            write_schema(data)
            sequence = write_control(data, render_mode, selected, text_probe)
            self.send_bytes(200, f"saved {SCHEMA}\nwrote {CONTROL_FILE}\nsequence={sequence}\n".encode(), "text/plain")
        elif self.path == "/rebuild":
            start_rebuild()
            self.send_bytes(202, b"rebuild started\n", "text/plain")
        else:
            self.send_bytes(404, b"not found", "text/plain")


def self_test():
    data = load_schema()
    missing = [name for name in FIELD_NAMES if name not in data["fields"]]
    if missing:
        raise SystemExit(f"missing fields: {missing}")
    for name in CHROME_NAMES:
        for key in ["x", "y", "scale_x", "scale_y", "opacity", "editable", "source"]:
            if key not in data["row_chrome"][name]:
                raise SystemExit(f"missing row_chrome.{name}.{key}")
    errors = validate_data(data)
    if errors:
        raise SystemExit("invalid editor schema:\n" + "\n".join(f"- {e}" for e in errors))
    if not REBUILD.exists():
        raise SystemExit(f"missing rebuild script {REBUILD}")
    for required_ui in (
        "drive cell width (all drives)",
        "drive cell height (all drives)",
        "button chrome height multiplier",
        "button chrome width (px)",
        "setButtonChromeWidthPx",
        "matchButtonChromeToRow",
        "setDriveCellValue",
        "PLAYER NAME ONLY — EntryNameClip",
        "ACTIVE DRIVE-PATH EDITOR — 02_990",
        "READ-ONLY DRIVE PATH LABEL — CurrentPath",
        "art opacity (0 removes outline)",
        "full-path button frame",
        "FULL-PATH SUB-ROW CONTROL",
        "syncCurrentPathChromeWidth",
        '<option value="live_runtime" selected>',
    ):
        if required_ui not in HTML:
            raise SystemExit(f"missing drive geometry UI: {required_ui}")
    clip_editor = HTML.split("if(selected.kind=='clip')", 1)[1].split(
        "else if(selected.kind=='field'||selected.kind=='path_editor')", 1
    )[0]
    for required_clip_handle in (
        "['x','y']",
        "setClipRight",
        "setClipBottom",
        "['width','clip_height','font_height']",
        "setText('align'",
        "padWidth",
        "fitWidthToSample",
        "padClipHeight",
        "fitClipHeightToFont",
        "sample_load_character",
        "sample_save_picker",
        "sample_drive_row",
    ):
        if required_clip_handle not in clip_editor:
            raise SystemExit(
                f"EntryNameClip is missing text-field editing handle: {required_clip_handle}"
            )
    if "name=='PlayerName'&&selected.kind=='clip'?'clip':'field'" not in HTML:
        raise SystemExit("EntryNameClip is missing its canvas move handle")
    for required_canvas_handle in ("bindClipResize", "bindClipHeightResize"):
        if required_canvas_handle not in HTML:
            raise SystemExit(f"EntryNameClip is missing canvas handle: {required_canvas_handle}")
    command_text = protocol_lines(data, 1, "offline_approximate", {"kind": "chrome", "name": "cursor"}, False)
    for required_protocol_key in (
        "path_editor.x",
        "path_editor.y",
        "path_editor.width",
        "path_editor.clip_height",
        "path_editor.font_height",
        "path_editor.align",
    ):
        if required_protocol_key not in command_text:
            raise SystemExit(f"active path editor is missing protocol key: {required_protocol_key}")
    for required_path_editor_ui in (
        "selected.kind=='field'||selected.kind=='path_editor'",
        "bindDrag(r,'path_editor','ActivePathEditor')",
        "bindPathEditorResize(widthHandle,'width')",
        "bindPathEditorResize(heightHandle,'height')",
        "schedulePathEditorPositionSave",
        "selected:payloadSelected()",
    ):
        if required_path_editor_ui not in HTML:
            raise SystemExit(f"active path editor is missing editing handle: {required_path_editor_ui}")
    if "if(selected.kind=='path_editor') return {kind:'field',name:'CurrentPath'}" in HTML:
        raise SystemExit("ActivePathEditor is still aliased to the read-only CurrentPath field")
    path_command = protocol_lines(
        data,
        2,
        "live_runtime",
        {"kind": "path_editor", "name": "ActivePathEditor"},
        False,
    )
    path_kind_line = next(
        (line for line in path_command.splitlines() if line.startswith("selected_kind =")),
        "",
    )
    if path_kind_line.split("=", 1)[-1].strip().strip('"') != "path_editor":
        raise SystemExit("ActivePathEditor does not preserve its own protocol selection kind")
    # EDITOR_DIR is created lazily by the server, so on a fresh checkout (or after a `target`
    # wipe) --self-test used to die with FileNotFoundError before testing anything. The
    # self-test must stand on its own: it is the gate that decides whether this tool is safe to
    # hand to a user.
    EDITOR_DIR.mkdir(parents=True, exist_ok=True)
    scratch = EDITOR_DIR / "control.self-test.txt"
    scratch.write_text(command_text)
    command = parse_protocol_file(scratch)
    try:
        scratch.unlink()
    except OSError:
        pass
    if command is None or int(command.get("sequence", 0)) != 1:
        raise SystemExit("protocol control file write/read failed")
    # Do not delete STATUS_FILE here: an editor self-test may run while Elden Ring is live, and
    # status.txt is the only honest runtime-ack surface the webview has.
    status = runtime_status(protocol_sequence)
    if "connected" not in status or "ack_sequence" not in status:
        raise SystemExit(f"runtime status parser returned malformed status: {status}")
    print(f"schema={SCHEMA}")
    print(f"control={CONTROL_FILE}")
    print(f"status={STATUS_FILE}")
    print(f"fields={len(data['fields'])} chrome={len(data['row_chrome'])}")
    print("self-test ok")


def main():
    global protocol_sequence
    parser = argparse.ArgumentParser(description="05_010_ProfileSelect local visual editor")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8776)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    # Sequence monotonicity survives editor restarts. Resetting to zero made a new valid control file
    # look older than the DLL's last ack, so the live bridge correctly ignored every post-restart edit.
    existing_control = parse_protocol_file(CONTROL_FILE) or {}
    existing_status = parse_protocol_file(STATUS_FILE) or {}
    for prior in (existing_control.get("sequence", 0), existing_status.get("ack_sequence", 0)):
        try:
            protocol_sequence = max(protocol_sequence, int(prior))
        except (TypeError, ValueError):
            pass
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    print("05_010 ProfileSelect visual editor")
    print("Approximate canvas only; in-game Scaleform runtime is final visual oracle.")
    print(f"url=http://{args.host}:{args.port}/")
    print(f"schema={SCHEMA}")
    print(f"rebuild={REBUILD}")
    server.serve_forever()


if __name__ == "__main__":
    main()
