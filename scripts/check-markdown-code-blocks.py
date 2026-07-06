#!/usr/bin/env python3
"""Validate markdown fenced code blocks with explicit test directives.

A non-text fenced block must be preceded by one of:

  <!-- md-test: bash-n -->              # bash/sh/shell block parses with bash -n
  <!-- md-test: bash-run -->            # run shell block with safe test stubs in PATH
  <!-- md-test: parse-toml -->          # TOML block parses with tomllib
  <!-- md-test: run <command> -->       # run an explicit validation command
  <!-- md-test: skip <reason> -->       # unsafe/illustrative; reason required

This is intentionally strict for README/product docs: code snippets should never
look executable while being neither tested nor explicitly exempted.
"""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib

TEXT_LANGS = {"", "text", "plain", "plaintext"}
SHELL_LANGS = {"bash", "sh", "shell", "console"}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="+", type=Path)
    return parser.parse_args()


def directive_before(lines: list[str], fence_idx: int) -> tuple[str, str] | None:
    idx = fence_idx - 1
    while idx >= 0 and not lines[idx].strip():
        idx -= 1
    if idx < 0:
        return None
    line = lines[idx].strip()
    prefix = "<!-- md-test:"
    suffix = "-->"
    if not (line.startswith(prefix) and line.endswith(suffix)):
        return None
    body = line[len(prefix) : -len(suffix)].strip()
    if not body:
        return None
    parts = body.split(maxsplit=1)
    action = parts[0]
    arg = parts[1].strip() if len(parts) > 1 else ""
    return action, arg


def check_bash_n(path: Path, line_no: int, code: str) -> list[str]:
    with tempfile.NamedTemporaryFile("w", suffix=".sh", delete=False) as f:
        f.write(code)
        tmp = Path(f.name)
    try:
        proc = subprocess.run(["bash", "-n", str(tmp)], text=True, capture_output=True)
    finally:
        tmp.unlink(missing_ok=True)
    if proc.returncode != 0:
        return [f"{path}:{line_no}: bash -n failed: {proc.stderr.strip()}"]
    return []


def check_bash_run(path: Path, line_no: int, code: str) -> list[str]:
    with tempfile.TemporaryDirectory(prefix="md-code-stubs-") as tmpdir:
        stub_dir = Path(tmpdir)
        me3 = stub_dir / "me3"
        me3.write_text(
            "#!/usr/bin/env bash\n"
            "printf 'stub me3:'\n"
            "printf ' %q' \"$@\"\n"
            "printf '\\n'\n",
            encoding="utf-8",
        )
        me3.chmod(0o755)
        script = stub_dir / "snippet.sh"
        script.write_text("set -euo pipefail\n" + code, encoding="utf-8")
        env = os.environ.copy()
        env["PATH"] = f"{stub_dir}{os.pathsep}" + env.get("PATH", "")
        env.setdefault("REGULATION_BIN", str(stub_dir / "regulation.bin"))
        proc = subprocess.run(["bash", str(script)], text=True, capture_output=True, env=env)
    if proc.returncode != 0:
        return [
            f"{path}:{line_no}: bash-run failed ({proc.returncode})\n"
            f"stdout:\n{proc.stdout[-2000:]}\nstderr:\n{proc.stderr[-2000:]}"
        ]
    return []


def check_toml(path: Path, line_no: int, code: str) -> list[str]:
    try:
        tomllib.loads(code)
    except tomllib.TOMLDecodeError as err:
        return [f"{path}:{line_no}: TOML parse failed: {err}"]
    return []


def check_run(path: Path, line_no: int, command: str) -> list[str]:
    if not command:
        return [f"{path}:{line_no}: md-test run requires a command"]
    proc = subprocess.run(command, shell=True, text=True, capture_output=True)
    if proc.returncode != 0:
        return [
            f"{path}:{line_no}: md-test run failed ({proc.returncode}): {command}\n"
            f"stdout:\n{proc.stdout[-2000:]}\nstderr:\n{proc.stderr[-2000:]}"
        ]
    return []


def check_file(path: Path) -> list[str]:
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    errors: list[str] = []
    in_fence = False
    fence_lang = ""
    fence_start = 0
    fence_lines: list[str] = []
    fence_directive: tuple[str, str] | None = None

    for idx, line in enumerate(lines, start=1):
        if not line.startswith("```"):
            if in_fence:
                fence_lines.append(line)
            continue
        if not in_fence:
            in_fence = True
            fence_start = idx
            fence_lang = line[3:].strip().split(maxsplit=1)[0].lower()
            fence_lines = []
            fence_directive = directive_before(lines, idx - 1)
            continue

        # Closing fence.
        code = "\n".join(fence_lines) + "\n"
        in_fence = False
        if fence_lang in TEXT_LANGS:
            continue
        if fence_directive is None:
            errors.append(f"{path}:{fence_start}: missing md-test directive before ```{fence_lang}")
            continue
        action, arg = fence_directive
        if action == "skip":
            if not arg:
                errors.append(f"{path}:{fence_start}: md-test skip requires a reason")
            continue
        if action == "bash-n":
            if fence_lang not in SHELL_LANGS:
                errors.append(f"{path}:{fence_start}: bash-n used for non-shell fence `{fence_lang}`")
            else:
                errors.extend(check_bash_n(path, fence_start, code))
            continue
        if action == "bash-run":
            if fence_lang not in SHELL_LANGS:
                errors.append(f"{path}:{fence_start}: bash-run used for non-shell fence `{fence_lang}`")
            else:
                errors.extend(check_bash_run(path, fence_start, code))
            continue
        if action == "parse-toml":
            if fence_lang != "toml":
                errors.append(f"{path}:{fence_start}: parse-toml used for non-toml fence `{fence_lang}`")
            else:
                errors.extend(check_toml(path, fence_start, code))
            continue
        if action == "run":
            errors.extend(check_run(path, fence_start, arg))
            continue
        errors.append(f"{path}:{fence_start}: unknown md-test action `{action}`")

    if in_fence:
        errors.append(f"{path}:{fence_start}: unclosed fenced code block")
    return errors


def main() -> int:
    errors: list[str] = []
    for path in parse_args().paths:
        errors.extend(check_file(path))
    if errors:
        print("markdown code-block validation failed:", file=sys.stderr)
        for err in errors:
            print(err, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
