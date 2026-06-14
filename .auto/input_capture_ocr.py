#!/usr/bin/env python3
"""Fast capture + post-run OCR for input-reason evidence.

Runtime only captures frames on confirm_probe events. OCR/classification runs after
stop, so slow OCR cannot make the sidecar miss later pulses.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import select
import signal
import subprocess
import time
from pathlib import Path
from typing import Any

STOP = False


def handle_stop(_signum: int, _frame: Any) -> None:
    global STOP
    STOP = True


def normalize(text: str) -> str:
    return re.sub(r"[^a-z0-9]+", " ", text.lower()).strip()


def classify(texts: list[str]) -> list[str]:
    joined = normalize(" ".join(texts))
    compact = re.sub(r"\s+", "", joined)
    reasons: list[str] = []
    if ("press" in joined and "button" in joined) or re.search(
        r"pr[ef]ss[a-z0-9]{0,16}(?:b|l|i|t){1,4}(?:u|v|t|i|l|0|o){1,4}(?:t|i|l|0|o){1,4}(?:o|0){0,2}n?",
        compact,
    ):
        reasons.append("title_press_any_button")
    if "checking save" in joined or "checking savedata" in compact or "checking save data" in joined:
        reasons.append("checking_save_data")
    if "updating save" in joined or "updating savedata" in compact or "updating save data" in joined:
        reasons.append("updating_save_data")
    if (
        "seamless" in joined
        or "welcome" in joined
        or "launch with" in joined
        or "separate save" in joined
        or "matchmaking" in joined
        or "anticheat" in joined
        or "mod ok" in joined
        or "modok" in compact
    ):
        reasons.append("mod_or_welcome_dialog")
    if "continue" in joined:
        reasons.append("continue_prompt")
    return sorted(set(reasons))


def capture_image(args: argparse.Namespace, pulse: int) -> Path | None:
    image = args.artifact_dir / f"input-before-{pulse}.jpg"
    helper = Path(args.screenshot_helper)
    if not helper.exists() or not os.access(helper, os.X_OK):
        return None
    with (args.artifact_dir / f"input-before-{pulse}.capture.txt").open("w", encoding="utf-8") as log:
        subprocess.run(
            [
                str(helper),
                "--class",
                args.window_class,
                "--output",
                str(image),
                "--max-width",
                str(args.max_width),
                "--jpeg-quality",
                str(args.jpeg_quality),
            ],
            stdout=log,
            stderr=subprocess.STDOUT,
            check=False,
        )
    return image if image.exists() else None


def tesseract(path: Path, psm: str) -> str:
    proc = subprocess.run(
        ["tesseract", str(path), "stdout", "--psm", psm],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        check=False,
    )
    return proc.stdout.strip()


def ocr_image(args: argparse.Namespace, pulse: int, image: Path) -> list[dict[str, str]]:
    variants = [
        ("gray", ["-resize", "400%", "-colorspace", "Gray", "-contrast-stretch", "0x30%"]),
        ("invert", ["-resize", "400%", "-colorspace", "Gray", "-negate", "-contrast-stretch", "0x30%"]),
        ("threshold", ["-resize", "400%", "-colorspace", "Gray", "-threshold", "55%"]),
    ]
    crops = [
        ("full", None),
        ("center", "900x300+50+120"),
        ("bottom", "1000x260+0+302"),
        ("middlebottom", "900x220+50+260"),
        ("promptband", "900x120+50+330"),
    ]
    out: list[dict[str, str]] = []
    for crop_name, crop in crops:
        for variant_name, ops in variants:
            prepared = args.artifact_dir / f"input-before-{pulse}.{crop_name}.{variant_name}.png"
            cmd = ["magick", str(image)]
            if crop is not None:
                cmd += ["-crop", crop]
            cmd += ops + [str(prepared)]
            proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, check=False)
            if proc.returncode != 0 or not prepared.exists():
                continue
            for psm in ("6", "7", "11", "12", "13"):
                text = tesseract(prepared, psm)
                if text:
                    out.append({"crop": crop_name, "variant": variant_name, "psm": psm, "text": text})
    return out


def write_jsonl(path: Path, entry: dict[str, Any]) -> None:
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(entry, sort_keys=True) + "\n")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--log-path", type=Path, required=True)
    parser.add_argument("--stop-file", type=Path, required=True)
    parser.add_argument("--screenshot-helper", default="/home/banon/projects/scripts/hypr-window-screenshot.sh")
    parser.add_argument("--window-class", default="steam_app_1245620")
    parser.add_argument("--max-width", type=int, default=1280)
    parser.add_argument("--jpeg-quality", type=int, default=82)
    args = parser.parse_args()

    signal.signal(signal.SIGTERM, handle_stop)
    signal.signal(signal.SIGINT, handle_stop)
    args.artifact_dir.mkdir(parents=True, exist_ok=True)
    raw_path = args.artifact_dir / "input-capture-events.jsonl"
    evidence_path = args.artifact_dir / "input-reason-evidence.jsonl"
    summary_path = args.artifact_dir / "input-reason-summary.json"

    tail_err = (args.artifact_dir / "input-capture-tail.err").open("w", encoding="utf-8")
    tail = subprocess.Popen(
        ["tail", "-n", "0", "-F", str(args.log_path)],
        stdout=subprocess.PIPE,
        stderr=tail_err,
        text=True,
        start_new_session=True,
    )
    seen: set[int] = set()
    try:
        while not STOP and not args.stop_file.exists():
            if tail.stdout is None:
                break
            ready, _, _ = select.select([tail.stdout], [], [], 0.25)
            if not ready:
                if tail.poll() is not None:
                    break
                continue
            line = tail.stdout.readline()
            if not line:
                if tail.poll() is not None:
                    break
                continue
            match = re.search(r"confirm_probe phase=before_confirm pulse=(\d+)", line)
            if not match:
                continue
            pulse = int(match.group(1))
            if pulse in seen:
                continue
            seen.add(pulse)
            image = capture_image(args, pulse)
            write_jsonl(raw_path, {"ts": time.time(), "pulse": pulse, "image": str(image) if image else None, "source_line": line.strip()})
    finally:
        try:
            os.killpg(tail.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            tail.wait(timeout=2)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(tail.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        tail_err.close()

    entries: list[dict[str, Any]] = []
    if raw_path.exists():
        for raw in raw_path.read_text(encoding="utf-8", errors="replace").splitlines():
            try:
                event = json.loads(raw)
            except json.JSONDecodeError:
                continue
            pulse = int(event.get("pulse") or 0)
            image_text = event.get("image")
            image = Path(image_text) if image_text else None
            ocr = ocr_image(args, pulse, image) if image and image.exists() else []
            reasons = classify([item["text"] for item in ocr])
            entry = {**event, "ocr": ocr, "reasons": reasons}
            entries.append(entry)
            write_jsonl(evidence_path, entry)
    summary = {
        "entries": len(entries),
        "pulses_seen": sorted(entry.get("pulse") for entry in entries if isinstance(entry.get("pulse"), int)),
        "pulses_with_reasons": sorted(entry.get("pulse") for entry in entries if entry.get("reasons")),
        "reasons_by_pulse": {str(entry.get("pulse")): entry.get("reasons", []) for entry in entries},
    }
    summary_path.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
