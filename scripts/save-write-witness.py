#!/usr/bin/env python3
"""Read-only witness for "did ELDEN RING actually write a save file?".

This is the OFFLINE half of the save-disable proof. The in-process half (RAM
telemetry semaphores counting intercepted save requests) proves the game
*believed* it saved; this proves no bytes reached the real save data.

Take a snapshot before a run and another after, then diff them::

    python3 scripts/save-write-witness.py snapshot --out before.json
    # ... run the game, trigger saves ...
    python3 scripts/save-write-witness.py snapshot --out after.json
    python3 scripts/save-write-witness.py diff before.json after.json

``diff`` exits 0 when nothing changed (save suppression held) and 1 when any
save byte, size, or slot digest moved. mtime alone is reported but does NOT by
itself fail the diff: a rewrite of identical bytes still means the game reached
the disk, so content changes are the hard signal and an mtime-only change is
surfaced as a distinct, still-failing category rather than being conflated with
"the save contents changed".

Digests are taken per BND4 ``USER_DATA###`` entry as well as whole-file, so a
diff names the exact character slot that moved instead of only saying "the
container changed".

Every path is env-overridable and derived from the current user's ``$HOME``;
nothing here is specific to one machine or account:

    STEAM_COMPAT_DATA_PATH  Proton compatdata dir for app 1245620
    APPDATA_ER_ROOT         .../AppData/Roaming/EldenRing
    ER_SAVE_DIRS            explicit ':'-separated dirs, overrides discovery

This tool NEVER writes to, renames, or opens for writing anything under the
save root. It only reads.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]

# Save-data containers plus the settings/graphics files the game also persists.
# "Everywhere the game expects to save" includes the non-character writes, so a
# witness that only watched ER0000.sl2 would report a false clean.
SAVE_DATA_SUFFIXES = (".sl2", ".co2")
SETTINGS_FILE_NAMES = ("GraphicsConfig.xml",)
# Backups and temp/rename-swap artifacts. A suppressed save that still touches a
# .bak or a temp file has not actually been suppressed.
COMPANION_SUFFIXES = (".bak", ".tmp", ".temp")

STEAM_APP_ID = "1245620"


def default_appdata_root() -> Path:
    """Resolve .../AppData/Roaming/EldenRing for the current user."""
    explicit = os.environ.get("APPDATA_ER_ROOT")
    if explicit:
        return Path(explicit)
    compat = os.environ.get("STEAM_COMPAT_DATA_PATH")
    if not compat:
        home = Path(os.environ.get("HOME", "~")).expanduser()
        compat = str(home / ".local/share/Steam/steamapps/compatdata" / STEAM_APP_ID)
    return Path(compat) / "pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing"


def save_roots() -> list[Path]:
    explicit = os.environ.get("ER_SAVE_DIRS")
    if explicit:
        return [Path(p) for p in explicit.split(":") if p]
    return [default_appdata_root()]


def is_watched(path: Path) -> str | None:
    """Classify a file, or return None if it is not save-relevant."""
    name = path.name
    lowered = name.lower()
    for suffix in SAVE_DATA_SUFFIXES:
        if lowered.endswith(suffix):
            return "save-data"
        for companion in COMPANION_SUFFIXES:
            if lowered.endswith(suffix + companion):
                return "save-companion"
    if name in SETTINGS_FILE_NAMES:
        return "settings"
    return None


def load_bnd4_entries(data: bytes) -> list[dict[str, Any]]:
    """Reuse save-slot-oracle.py's BND4 walk rather than forking a second parser."""
    oracle = REPO_ROOT / "scripts" / "save-slot-oracle.py"
    spec = importlib.util.spec_from_file_location("er_save_slot_oracle", oracle)
    if spec is None or spec.loader is None:
        return []
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.bnd4_entries(data)


def digest_entries(data: bytes) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    try:
        entries = load_bnd4_entries(data)
    except Exception as exc:  # noqa: BLE001 - a witness must never crash the caller
        return [{"name": "<bnd4-parse-failed>", "error": str(exc)}]
    for entry in entries:
        start = int(entry.get("offset", 0))
        size = int(entry.get("size", 0))
        blob = data[start : start + size]
        out.append(
            {
                "index": entry.get("index"),
                "name": entry.get("name", ""),
                "size": size,
                "sha256": hashlib.sha256(blob).hexdigest(),
                "truncated": len(blob) != size,
            }
        )
    return out


def snapshot_file(path: Path) -> dict[str, Any]:
    stat = path.stat()
    record: dict[str, Any] = {
        "path": str(path),
        "kind": is_watched(path),
        "size": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
    }
    try:
        data = path.read_bytes()
    except OSError as exc:
        record["error"] = f"unreadable: {exc}"
        return record
    record["sha256"] = hashlib.sha256(data).hexdigest()
    if record["kind"] in ("save-data", "save-companion"):
        record["entries"] = digest_entries(data)
    return record


def take_snapshot() -> dict[str, Any]:
    files: list[dict[str, Any]] = []
    roots_seen: list[dict[str, Any]] = []
    for root in save_roots():
        exists = root.is_dir()
        roots_seen.append({"root": str(root), "exists": exists})
        if not exists:
            continue
        for path in sorted(root.rglob("*")):
            if not path.is_file() or is_watched(path) is None:
                continue
            files.append(snapshot_file(path))
    return {"schema": "er-save-write-witness/1", "roots": roots_seen, "files": files}


def index_by_path(snap: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {record["path"]: record for record in snap.get("files", [])}


def diff_entries(before: dict[str, Any], after: dict[str, Any]) -> list[str]:
    """Name the exact slots whose contents moved."""
    b = {e.get("name"): e for e in before.get("entries", []) or []}
    a = {e.get("name"): e for e in after.get("entries", []) or []}
    changes: list[str] = []
    for name in sorted(set(b) | set(a)):
        bd, ad = b.get(name), a.get(name)
        if bd is None:
            changes.append(f"slot {name}: added")
        elif ad is None:
            changes.append(f"slot {name}: removed")
        elif bd.get("sha256") != ad.get("sha256"):
            changes.append(
                f"slot {name}: contents changed"
                f" ({bd.get('size')}B -> {ad.get('size')}B)"
            )
    return changes


def diff_snapshots(before: dict[str, Any], after: dict[str, Any]) -> dict[str, Any]:
    """Diff two snapshots.

    Settings files are reported but never gate the verdict. GraphicsConfig.xml is
    graphics/display configuration, not character or profile data, and a save-disable
    feature is supposed to leave the game writing it exactly as vanilla does. Counting
    it as a violation made a clean run read as a failure.
    """
    b, a = index_by_path(before), index_by_path(after)
    content_changes: list[dict[str, Any]] = []
    mtime_only: list[dict[str, Any]] = []
    settings_changes: list[dict[str, Any]] = []

    for path in sorted(set(b) | set(a)):
        rb, ra = b.get(path), a.get(path)
        kind = (ra or rb or {}).get("kind")
        gates_verdict = kind in ("save-data", "save-companion")

        if rb is None:
            change = {"path": path, "reason": "file created", "slots": []}
        elif ra is None:
            change = {"path": path, "reason": "file deleted", "slots": []}
        elif rb.get("sha256") != ra.get("sha256") or rb.get("size") != ra.get("size"):
            change = {
                "path": path,
                "reason": f"contents changed ({rb.get('size')}B -> {ra.get('size')}B)",
                "slots": diff_entries(rb, ra),
            }
        elif rb.get("mtime_ns") != ra.get("mtime_ns"):
            change = {"path": path, "reason": "identical bytes rewritten (mtime moved)", "slots": []}
            if gates_verdict:
                mtime_only.append(change)
            else:
                settings_changes.append(change)
            continue
        else:
            continue

        (content_changes if gates_verdict else settings_changes).append(change)

    return {
        "content_changes": content_changes,
        "mtime_only_changes": mtime_only,
        "settings_changes": settings_changes,
        "clean": not content_changes and not mtime_only,
    }


def render_settings(result: dict[str, Any]) -> list[str]:
    """Settings churn is expected vanilla behaviour; show it, never fail on it."""
    if not result.get("settings_changes"):
        return []
    lines = ["", "Settings files changed (expected, not save data):"]
    lines += [f"  {c['path']}: {c['reason']}" for c in result["settings_changes"]]
    return lines


def render_diff(result: dict[str, Any]) -> str:
    if result["clean"]:
        return "\n".join(
            ["CLEAN: no save file was written (no content or mtime change)"]
            + render_settings(result)
        )
    lines: list[str] = []
    if result["content_changes"]:
        lines.append("SAVE WRITE DETECTED -- contents changed:")
        for change in result["content_changes"]:
            lines.append(f"  {change['path']}: {change['reason']}")
            lines.extend(f"    {slot}" for slot in change["slots"])
    if result["mtime_only_changes"]:
        lines.append("SAVE WRITE DETECTED -- file rewritten with identical bytes:")
        for change in result["mtime_only_changes"]:
            lines.append(f"  {change['path']}: {change['reason']}")
    return "\n".join(lines + render_settings(result))


def selftest() -> int:
    """Prove the witness detects each change class before anyone trusts a CLEAN."""
    bnd4 = bytearray(b"BND4" + b"\x00" * 0x3C)
    bnd4[0x0C:0x10] = (1).to_bytes(4, "little")
    name_offset = 0x60
    data_offset = 0x80
    payload = b"SLOTBYTES" + b"\x00" * 55
    entry = bytearray(0x20)
    entry[0x08:0x10] = len(payload).to_bytes(8, "little")
    entry[0x10:0x14] = data_offset.to_bytes(4, "little")
    entry[0x14:0x18] = name_offset.to_bytes(4, "little")
    blob = bytearray(bnd4 + entry)
    blob.extend(b"\x00" * (name_offset - len(blob)))
    blob.extend("USER_DATA000".encode("utf-16-le") + b"\x00\x00")
    blob.extend(b"\x00" * (data_offset - len(blob)))
    blob.extend(payload)

    failures: list[str] = []
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp) / "EldenRing" / "76561190000000000"
        root.mkdir(parents=True)
        save = root / "ER0000.sl2"
        save.write_bytes(bytes(blob))
        os.environ["ER_SAVE_DIRS"] = str(Path(tmp) / "EldenRing")

        before = take_snapshot()
        if len(before["files"]) != 1:
            failures.append(f"discovery: expected 1 watched file, got {len(before['files'])}")
        entries = before["files"][0].get("entries", []) if before["files"] else []
        if [e.get("name") for e in entries] != ["USER_DATA000"]:
            failures.append(f"bnd4: expected USER_DATA000, got {[e.get('name') for e in entries]}")

        # (1) unchanged must read CLEAN
        if not diff_snapshots(before, take_snapshot())["clean"]:
            failures.append("unchanged file reported as changed (false positive)")

        # (2) a slot content change must be caught and named
        mutated = bytearray(blob)
        mutated[data_offset] = ord("X")
        save.write_bytes(bytes(mutated))
        result = diff_snapshots(before, take_snapshot())
        if result["clean"]:
            failures.append("slot byte change NOT detected")
        elif not any("USER_DATA000" in slot for c in result["content_changes"] for slot in c["slots"]):
            failures.append(f"slot change not attributed to USER_DATA000: {result}")

        # (3) an identical-bytes rewrite must still be caught, as its own class
        save.write_bytes(bytes(blob))
        os.utime(save, ns=(before["files"][0]["mtime_ns"] + 10**9,) * 2)
        result = diff_snapshots(before, take_snapshot())
        if result["clean"]:
            failures.append("identical-bytes rewrite NOT detected")
        elif result["content_changes"]:
            failures.append("identical-bytes rewrite misclassified as a content change")

        # (4) deletion must be caught
        save.unlink()
        if diff_snapshots(before, take_snapshot())["clean"]:
            failures.append("file deletion NOT detected")

        # (5) a settings-only change must stay CLEAN. GraphicsConfig.xml is graphics
        # configuration the game is supposed to keep writing; failing a run over it
        # would mark a perfectly working save-disable as broken.
        save.write_bytes(bytes(blob))
        settings = root / "GraphicsConfig.xml"
        settings.write_text("<config/>", encoding="utf-8")
        settings_before = take_snapshot()
        settings.write_text("<config changed=\"1\"/>", encoding="utf-8")
        result = diff_snapshots(settings_before, take_snapshot())
        if not result["clean"]:
            failures.append(f"a settings-only change failed the verdict: {result}")
        if not result.get("settings_changes"):
            failures.append("the settings change was not reported at all")

    if failures:
        for failure in failures:
            print(f"SELFTEST FAIL: {failure}", file=sys.stderr)
        return 1
    print("SELFTEST PASS: detects slot edits, identical-byte rewrites, and deletions")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command")

    snap = sub.add_parser("snapshot", help="fingerprint every save file (read-only)")
    snap.add_argument("--out", type=Path, help="write JSON here instead of stdout")

    dif = sub.add_parser("diff", help="compare two snapshots; exit 1 if any save was written")
    dif.add_argument("before", type=Path)
    dif.add_argument("after", type=Path)
    dif.add_argument("--json", action="store_true", help="emit the diff as JSON")

    parser.add_argument("--selftest", action="store_true", help="verify the witness detects real changes")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    if args.command == "snapshot":
        snapshot = take_snapshot()
        text = json.dumps(snapshot, indent=2)
        if args.out:
            args.out.write_text(text, encoding="utf-8")
            watched = len(snapshot["files"])
            print(f"wrote {args.out} ({watched} watched file(s))")
        else:
            print(text)
        return 0

    if args.command == "diff":
        before = json.loads(args.before.read_text(encoding="utf-8"))
        after = json.loads(args.after.read_text(encoding="utf-8"))
        result = diff_snapshots(before, after)
        print(json.dumps(result, indent=2) if args.json else render_diff(result))
        return 0 if result["clean"] else 1

    parser.print_help()
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
