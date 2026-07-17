#!/usr/bin/env python3
"""Offline helper for the DS1/DSR mushroom -> Elden Ring Route A prototype.

This script never launches either game. It only discovers local files, extracts
archive contents with fstools_cli when archives are present, and unpacks BND/DCX
files with WitchyBND for static inspection.
"""
from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import xml.etree.ElementTree as ET
from collections.abc import Iterable
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

DSR_TARGETS = [
    Path("chr/c2280.chrbnd.dcx"),
    Path("chr/c2280.anibnd.dcx"),
    Path("chr/c2270.chrbnd.dcx"),
    Path("chr/c2270.anibnd.dcx"),
]
ER_PLAYER_FILTER = "chr/c0000"
ER_DONOR_FILTER = "parts/bd_m_1010"
DEFAULT_ER_GAME = Path("/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game")
DEFAULT_DSR_CANDIDATES = [
    Path("/mnt/c/SteamLibrary/steamapps/common/DARK SOULS REMASTERED"),
    Path("/mnt/d/Steam/steamapps/common/DARK SOULS REMASTERED"),
    Path("/mnt/d/steam/steamapps/common/DARK SOULS REMASTERED"),
    Path("/mnt/c/SteamLibrary/steamapps/common/Dark Souls Prepare to Die Edition"),
]
DEFAULT_FSTOOLS = Path("/home/choza/projects/fstools-rs/target/debug/fstools_cli")
DEFAULT_WITCHY = Path("/mnt/d/Witchy BND/WitchyBND.exe")


@dataclass
class PathStatus:
    path: str
    exists: bool
    kind: str | None = None
    size: int | None = None


@dataclass
class CommandResult:
    command: list[str] | str
    returncode: int
    stdout: str
    stderr: str
    accepted: bool


def path_status(path: Path) -> PathStatus:
    if not path.exists():
        return PathStatus(str(path), False)
    if path.is_dir():
        return PathStatus(str(path), True, "dir")
    return PathStatus(str(path), True, "file", path.stat().st_size)


def run_command(
    command: list[str],
    *,
    cwd: Path | None = None,
    timeout: int = 30,
    accept: Iterable[int] = (0,),
) -> CommandResult:
    proc = subprocess.run(
        command,
        cwd=str(cwd) if cwd else None,
        text=True,
        capture_output=True,
        timeout=timeout,
        check=False,
    )
    accepted = proc.returncode in set(accept)
    return CommandResult(command, proc.returncode, proc.stdout, proc.stderr, accepted)


def wsl_to_windows(path: Path) -> str:
    result = run_command(["wslpath", "-w", str(path)], timeout=10)
    if not result.accepted:
        raise RuntimeError(f"wslpath failed for {path}: {result.stderr or result.stdout}")
    return result.stdout.strip()


def find_dsr_game(explicit: Path | None) -> Path | None:
    candidates = [explicit] if explicit else DEFAULT_DSR_CANDIDATES
    for candidate in candidates:
        if candidate and candidate.exists():
            return candidate
    return None


def has_dsr_archives(game_dir: Path) -> bool:
    archive_sets = [
        [game_dir / f"dvdbnd{i}.bhd5" for i in range(4)] + [game_dir / f"dvdbnd{i}.bdt" for i in range(4)],
        [game_dir / f"dvdbnd{i}.bhd" for i in range(4)] + [game_dir / f"dvdbnd{i}.bdt" for i in range(4)],
        [game_dir / "dvdbnd.bhd", game_dir / "dvdbnd.bdt"],
    ]
    return any(all(path.exists() for path in archive_set) for archive_set in archive_sets)


def copy_loose_dsr_targets(game_dir: Path, output_dir: Path) -> list[PathStatus]:
    copied: list[PathStatus] = []
    source_dir = output_dir / "dsr-loose-mushroom"
    source_dir.mkdir(parents=True, exist_ok=True)
    for relative in DSR_TARGETS:
        source = game_dir / relative
        dest = source_dir / relative.name
        if source.exists():
            shutil.copy2(source, dest)
            copied.append(path_status(dest))
    return copied


def extract_with_fstools(fstools: Path, game_dir: Path, output_dir: Path, filters: Iterable[str]) -> list[CommandResult]:
    results: list[CommandResult] = []
    if fstools.parent.name in {"debug", "release"} and fstools.parent.parent.name == "target":
        fstools_cwd = fstools.parent.parent.parent
    else:
        fstools_cwd = fstools.parent
    for filter_text in filters:
        output_dir.mkdir(parents=True, exist_ok=True)
        results.append(
            run_command(
                [str(fstools), "--game-path", str(game_dir), "extract", "-o", str(output_dir), filter_text],
                cwd=fstools_cwd,
                timeout=30,
            )
        )
    return results


def expected_witchy_dir(input_path: Path) -> Path:
    # Witchy usually maps file.ext.dcx -> file-ext-dcx, with special ANIBND suffixes possible.
    stem = input_path.name.replace(".", "-")
    parent = input_path.parent
    candidates = [parent / stem, parent / f"{stem}-wanibnd", parent / f"{stem}-bnd4", parent / f"{stem}-bnd3"]
    for candidate in candidates:
        if candidate.exists():
            return candidate
    return parent / stem


def run_witchy(witchy: Path, input_path: Path, timeout: int = 45) -> CommandResult:
    if not witchy.exists():
        return CommandResult(str(witchy), 127, "", "WitchyBND.exe not found", False)
    cmd_dir = input_path.parent / ".witchy-cmd"
    cmd_dir.mkdir(parents=True, exist_ok=True)
    script = cmd_dir / f"unpack-{input_path.name}.cmd"
    witchy_dir = wsl_to_windows(witchy.parent)
    target = wsl_to_windows(input_path)
    script.write_text(f'@echo off\r\ncd /d "{witchy_dir}"\r\nWitchyBND.exe "{target}"\r\n', encoding="utf-8")
    script_win = wsl_to_windows(script)
    result = run_command(["cmd.exe", "/c", script_win], timeout=timeout, accept=(0, 82))
    out_dir = expected_witchy_dir(input_path)
    result.accepted = result.accepted and out_dir.exists()
    return result


def parse_witchy_manifest(directory: Path) -> dict[str, object]:
    manifests = list(directory.glob("_witchy-*.xml"))
    files = [path for path in directory.rglob("*") if path.is_file()]
    extension_counts: dict[str, int] = {}
    for path in files:
        if path.name.startswith("_witchy-"):
            continue
        ext = path.suffix.lower() or "<none>"
        extension_counts[ext] = extension_counts.get(ext, 0) + 1
    manifest_entries: list[dict[str, str]] = []
    for manifest in manifests:
        try:
            root = ET.fromstring(manifest.read_text(encoding="utf-8-sig"))
        except Exception as exc:  # noqa: BLE001 - inventory should not hard-fail on malformed metadata.
            manifest_entries.append({"manifest": str(manifest), "error": repr(exc)})
            continue
        for file_node in root.findall(".//file")[:200]:
            entry = {child.tag: (child.text or "") for child in list(file_node)}
            entry["manifest"] = str(manifest)
            manifest_entries.append(entry)
    return {
        "directory": str(directory),
        "exists": directory.exists(),
        "file_count": len(files),
        "extension_counts": extension_counts,
        "manifests": [str(path) for path in manifests],
        "manifest_entries_sample": manifest_entries[:80],
    }


def discover_outputs(output_dir: Path) -> list[dict[str, object]]:
    summaries: list[dict[str, object]] = []
    for directory in sorted(output_dir.rglob("*")):
        if directory.is_dir() and any(
            child.name.startswith("_witchy-") and child.suffix == ".xml" for child in directory.iterdir() if child.is_file()
        ):
            summaries.append(parse_witchy_manifest(directory))
    return summaries


def resolve_output_dir(workspace: Path, requested_output: Path) -> Path:
    candidate = requested_output if requested_output.is_absolute() else workspace / requested_output
    resolved = candidate.resolve()
    try:
        resolved.relative_to(workspace)
    except ValueError as exc:
        raise ValueError(f"output path must stay under workspace {workspace}: {resolved}") from exc
    return resolved


def emit_json(payload: dict[str, Any]) -> None:
    sys.stdout.write(json.dumps(payload, indent=2))
    sys.stdout.write("\n")


def main() -> int:
    parser = argparse.ArgumentParser(description="Offline DS1 mushroom -> ER Route A asset helper")
    parser.add_argument("--workspace", type=Path, default=Path.cwd(), help="Repo/worktree root for relative output paths")
    parser.add_argument("--output", type=Path, default=Path("target/mushroom-route-a-offline"), help="Output directory")
    parser.add_argument("--dsr-game", type=Path, help="Dark Souls/DSR game directory")
    parser.add_argument("--er-game", type=Path, default=DEFAULT_ER_GAME, help="Elden Ring Game directory")
    parser.add_argument("--fstools", type=Path, default=DEFAULT_FSTOOLS, help="fstools_cli path")
    parser.add_argument("--witchy", type=Path, default=DEFAULT_WITCHY, help="WitchyBND.exe path")
    parser.add_argument("--status", action="store_true", help="Only report availability/status")
    parser.add_argument("--prepare-er", action="store_true", help="Extract and unpack ER c0000 + bd_m_1010 donor references")
    parser.add_argument("--extract-dsr", action="store_true", help="Extract/unpack c2270/c2280 from loose files or DSR archives if present")
    args = parser.parse_args()

    workspace = args.workspace.resolve()
    output = resolve_output_dir(workspace, args.output)
    output.mkdir(parents=True, exist_ok=True)
    dsr_game = find_dsr_game(args.dsr_game)

    summary: dict[str, Any] = {
        "workspace": str(workspace),
        "output": str(output),
        "tools": {
            "fstools": asdict(path_status(args.fstools)),
            "witchy": asdict(path_status(args.witchy)),
        },
        "er_game": asdict(path_status(args.er_game)),
        "dsr_game": asdict(path_status(dsr_game)) if dsr_game else None,
        "dsr_targets": [asdict(path_status(dsr_game / rel)) for rel in DSR_TARGETS] if dsr_game else [],
        "dsr_archives_present": has_dsr_archives(dsr_game) if dsr_game else False,
        "commands": [],
        "copied_dsr_targets": [],
        "witchy_outputs": [],
    }

    if args.status:
        (output / "inventory-status.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
        emit_json(summary)
        return 0

    if args.prepare_er:
        er_out = output / "er"
        if args.fstools.exists() and args.er_game.exists():
            results = extract_with_fstools(args.fstools, args.er_game, er_out, [ER_PLAYER_FILTER, ER_DONOR_FILTER])
            summary["commands"].extend(asdict(result) for result in results)
            er_unpack_names = {"c0000.chrbnd.dcx", "c0000.anibnd.dcx", "bd_m_1010.partsbnd.dcx"}
            for input_path in sorted(er_out.glob("*.dcx")):
                if input_path.name not in er_unpack_names:
                    continue
                result = run_witchy(args.witchy, input_path)
                summary["commands"].append(asdict(result))
                out_dir = expected_witchy_dir(input_path)
                if out_dir.exists():
                    summary["witchy_outputs"].append(parse_witchy_manifest(out_dir))
                    for nested_tpf in sorted(out_dir.glob("*.tpf")):
                        tpf_result = run_witchy(args.witchy, nested_tpf)
                        summary["commands"].append(asdict(tpf_result))
                        tpf_out_dir = expected_witchy_dir(nested_tpf)
                        if tpf_out_dir.exists():
                            summary["witchy_outputs"].append(parse_witchy_manifest(tpf_out_dir))

    if args.extract_dsr and dsr_game:
        dsr_out = output / "dsr"
        copied = copy_loose_dsr_targets(dsr_game, dsr_out)
        summary["copied_dsr_targets"] = [asdict(item) for item in copied]
        if args.fstools.exists() and has_dsr_archives(dsr_game):
            results = extract_with_fstools(args.fstools, dsr_game, dsr_out, ["chr/c2280", "chr/c2270"])
            summary["commands"].extend(asdict(result) for result in results)
        for input_path in sorted(dsr_out.rglob("c22*.dcx")):
            result = run_witchy(args.witchy, input_path)
            summary["commands"].append(asdict(result))
            out_dir = expected_witchy_dir(input_path)
            if out_dir.exists():
                summary["witchy_outputs"].append(parse_witchy_manifest(out_dir))

    summary["witchy_outputs"].extend(discover_outputs(output))
    status_path = output / "inventory-status.json"
    status_path.write_text(json.dumps(summary, indent=2), encoding="utf-8")
    emit_json(summary)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except subprocess.TimeoutExpired as exc:
        sys.stderr.write(f"command timed out: {exc}\n")
        raise SystemExit(124) from exc
