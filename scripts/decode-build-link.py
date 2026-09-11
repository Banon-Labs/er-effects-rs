#!/usr/bin/env python3
"""Decode an er-build-planner ``?i=`` share link back into its build JSON.

This is the offline oracle for the System>Quit "Generate Build Link" row. The row writes the URL it
produced into ``er-build-import.log`` beside the game executable; this reads that URL back and
prints what the planner will actually see, so a runtime run is checked against the character it was
supposed to describe rather than against "a link appeared".

It is deliberately self-contained -- no node, no network, no dependency on the planner's bundle --
because the thing being verified is precisely whether our encoder produces something the planner's
decoder accepts, and an oracle that shared code with the encoder would agree with it by
construction.

Usage
-----
    scripts/decode-build-link.py <url>
    scripts/decode-build-link.py --log /path/to/er-build-import.log
    scripts/decode-build-link.py --selftest

``--summary`` prints the loadout instead of the raw JSON.
"""

from __future__ import annotations

import argparse
import base64
import json
import re
import sys
from pathlib import Path

# The planner's own legacy prefix markers, applied before the base64url swap. They are no-ops for
# payloads produced by the current format (which start "ZXlK"), and are reversed here anyway so an
# older link a player pastes in still decodes.
LEGACY_MARKERS = (("uwu", "eyI"), ("UWU", "eyJ"))

# Single-byte stand-ins the planner's decoder expands back into JSON punctuation. Also legacy --
# the current encoder never emits them -- but reversing them costs nothing and a link that used
# them would otherwise decode to invalid JSON.
TOKEN_SUBSTITUTIONS = (("\x01", "},{"), ("\x05", "[{"), ("\x06", "}]"))

# LZ-UTF8 match-header shapes. Length is the low five bits of the header in both forms.
MATCH_HEADER_MASK = 0b1110_0000
MATCH_HEADER_SHORT = 0b1100_0000  # 2-byte form: header + one distance byte
MATCH_HEADER_LONG = 0b1110_0000  # 3-byte form: header + two big-endian distance bytes
MATCH_LENGTH_MASK = 0b0001_1111
CONTINUATION_BIT = 0b1000_0000


class DecodeError(Exception):
    """The payload is not a well-formed share link."""


def lzutf8_decompress(data: bytes) -> str:
    """Expand an LZ-UTF8 stream.

    A byte >= 0xC0 is a match header only when the byte after it has its top bit clear; otherwise
    it is a literal UTF-8 lead byte. That one rule is the whole format, and getting it backwards
    silently produces different text rather than an error -- which is why it is spelled out here
    rather than folded into a lookup table.
    """
    out = bytearray()
    index = 0
    end = len(data)
    while index < end:
        byte = data[index]
        is_match = (
            byte >= MATCH_HEADER_SHORT
            and index + 1 < end
            and not data[index + 1] & CONTINUATION_BIT
        )
        if not is_match:
            out.append(byte)
            index += 1
            continue

        length = byte & MATCH_LENGTH_MASK
        shape = byte & MATCH_HEADER_MASK
        if shape == MATCH_HEADER_SHORT:
            distance = data[index + 1]
            index += 2
        elif shape == MATCH_HEADER_LONG:
            if index + 2 >= end:
                raise DecodeError("truncated 3-byte match header")
            distance = (data[index + 1] << 8) | data[index + 2]
            index += 3
        else:  # pragma: no cover - the two shapes above are exhaustive for byte >= 0xC0
            raise DecodeError(f"unrecognised match header 0x{byte:02x}")

        if distance == 0 or distance > len(out):
            raise DecodeError(f"match distance {distance} outside the {len(out)} bytes emitted")
        start = len(out) - distance
        # Copied one byte at a time on purpose: an LZ77 match may overlap its own output (that is
        # how a run is encoded), so a slice copy would be wrong for exactly the common case.
        for step in range(length):
            out.append(out[start + step])
    return out.decode("utf-8", errors="strict")


def decode_payload(payload: str) -> dict:
    """Turn a ``?i=`` payload into the build document."""
    text = payload.strip()
    for marker, original in LEGACY_MARKERS:
        if text.startswith(marker):
            text = original + text[len(marker) :]
            break
    text = text.replace("_", "/").replace("-", "+")
    try:
        compressed = base64.b64decode(text + "=" * (-len(text) % 4))
    except Exception as exc:  # noqa: BLE001 - any base64 failure is one condition to the caller
        raise DecodeError(f"payload is not base64: {exc}") from exc

    inner = lzutf8_decompress(compressed)
    inner = "".join(ch for ch in inner if ord(ch) > 0)
    for token, original in TOKEN_SUBSTITUTIONS:
        inner = inner.replace(token, original)
    inner = inner.strip()

    try:
        character_json = base64.b64decode(inner + "=" * (-len(inner) % 4)).decode("latin-1")
    except Exception as exc:  # noqa: BLE001
        raise DecodeError(f"inner layer is not base64: {exc}") from exc
    try:
        return json.loads(character_json)
    except json.JSONDecodeError as exc:
        raise DecodeError(f"inner layer is not JSON: {exc}") from exc


def payload_of(url: str) -> str:
    """Pull the ``?i=`` payload out of a URL, or accept a bare payload."""
    if "?i=" in url:
        return url.split("?i=", 1)[1].split("&", 1)[0].strip()
    if "?" in url or "://" in url:
        raise DecodeError("that URL carries no ?i= payload")
    return url.strip()


def urls_in_log(path: Path) -> list[str]:
    """Every URL the exporter wrote into a log, oldest first."""
    pattern = re.compile(r"\[build-export\] URL (\S+)")
    return pattern.findall(path.read_text(encoding="utf-8", errors="replace"))


def summarise(doc: dict) -> str:
    """The loadout, as a human checks it against the character they were just playing."""
    lines = []
    stats = doc.get("stats", {})
    lines.append(f"name          {doc.get('name', '')!r}")
    lines.append(f"class         {doc.get('characterClass')}")
    lines.append(
        "level         RL{}  vig {} mnd {} end {} str {} dex {} int {} fth {} arc {}".format(
            stats.get("rl"),
            stats.get("vig"),
            stats.get("mnd"),
            # The planner's "vit" key is Endurance. Labelled correctly here so a reader checking
            # this against the game's own Status screen is not quietly misled.
            stats.get("vit"),
            stats.get("str"),
            stats.get("dex"),
            stats.get("int"),
            stats.get("fth"),
            stats.get("arc"),
        )
    )
    lines.append(f"weaponUpgrade +{doc.get('weaponUpgrade')}   two-handing {doc.get('is2h')}")
    flasks = doc.get("items", {}).get("flasks", {})
    lines.append(
        f"flasks        {flasks.get('crimson')} crimson / {flasks.get('cerulean')} cerulean"
    )

    def slots(container, *path):
        node = container
        for key in path:
            node = (node or {}).get(key, {})
        return (node or {}).get("slots", []) or []

    for label, entries in (
        ("armaments", slots(doc, "inventory")),
        ("talismans", slots(doc, "talismans")),
        ("spells", slots(doc, "spells")),
    ):
        lines.append(f"{label} ({len(entries)}):")
        for entry in entries:
            extra = []
            if entry.get("infusion"):
                extra.append(entry["infusion"])
            if entry.get("weaponArt"):
                extra.append(f"art={entry['weaponArt']}")
            if entry.get("upgrade") is not None:
                extra.append(f"+{entry['upgrade']}")
            if entry.get("equipIndex") is not None:
                extra.append(f"slot={entry['equipIndex']}")
            suffix = f"  [{', '.join(extra)}]" if extra else ""
            lines.append(f"    {entry.get('name')}{suffix}")
    protectors = doc.get("protectors", {}) or {}
    worn = [
        f"{part}={(protectors.get(part) or {}).get('slots', [{}])[0].get('name')}"
        for part in ("head", "body", "arms", "legs")
        if (protectors.get(part) or {}).get("slots")
    ]
    lines.append(f"armour        {', '.join(worn) if worn else '(none)'}")
    tears = [t for t in (doc.get("items", {}).get("crystalTears") or []) if t]
    lines.append(f"physick       {', '.join(tears) if tears else '(empty)'}")
    lines.append(f"great rune    {doc.get('greatRune') or '(none)'}")
    # The appearance, under the planner's own key. This used to be an invented `faceData` holding
    # the game's whole 288-byte buffer as hex, which nothing on the site read; it is now
    # `sliders`, the shape its Cosmetics tab renders. A handful of named values is printed rather
    # than all two hundred, with the count so a truncated set is still visible.
    cosmetics = doc.get("sliders") or {}
    sliders = cosmetics.get("sliders") or {}
    if sliders:
        named = ", ".join(
            f"{key}={sliders[key]}"
            for key in ("age", "boneStructure", "musculature", "hairModelId", "eyeModelId")
            if key in sliders
        )
        body = cosmetics.get("bodyType", "?")
        lines.append(f"appearance    body {body}, {len(sliders)} sliders")
        if named:
            lines.append(f"    {named}")
    else:
        lines.append("appearance    (none)")
    return "\n".join(lines)


def lzutf8_compress_reference(text: str) -> bytes:
    """A minimal LZ-UTF8 encoder, used only by ``--selftest``.

    Deliberately not the one the DLL ships: the point of the self-test is to prove the DECODER in
    this file expands a stream built independently of it, including overlapping matches, which is
    the case a slice-copy implementation gets wrong.
    """
    data = text.encode("utf-8")
    out = bytearray()
    index = 0
    while index < len(data):
        best_length = 0
        best_distance = 0
        window_start = max(0, index - 32767)
        for candidate in range(window_start, index):
            length = 0
            while (
                length < 31
                and index + length < len(data)
                and data[candidate + length] == data[index + length]
            ):
                length += 1
            if length > best_length:
                best_length, best_distance = length, index - candidate
        if best_length >= 4:
            if best_distance < 128:
                out.append(MATCH_HEADER_SHORT | best_length)
                out.append(best_distance)
            else:
                out.append(MATCH_HEADER_LONG | best_length)
                out.append((best_distance >> 8) & 0x7F)
                out.append(best_distance & 0xFF)
            index += best_length
        else:
            out.append(data[index])
            index += 1
    return bytes(out)


def selftest() -> int:
    """Prove the decoder before anyone is asked to trust its output."""
    cases = {
        "empty": "",
        "ascii": "the quick brown fox jumps over the lazy dog",
        "repetitive": "abcabcabc" * 40,
        "overlapping run": "x" * 200,
        "json-like": json.dumps({"slots": [{"name": "Misericorde", "order": i} for i in range(30)]}),
        "multibyte": "Dongerino éテスト " * 20,
    }
    failures = 0
    for label, text in cases.items():
        try:
            got = lzutf8_decompress(lzutf8_compress_reference(text))
        except DecodeError as exc:
            print(f"  FAIL {label}: {exc}")
            failures += 1
            continue
        if got != text:
            print(f"  FAIL {label}: {len(got)} chars back, expected {len(text)}")
            failures += 1
        else:
            print(f"  ok   {label} ({len(text)} chars)")

    # And the full pipeline, built the way the planner builds it.
    doc = {"name": "selftest", "stats": {"rl": 150, "vit": 40}, "spells": {"slots": []}}
    inner = base64.b64encode(json.dumps(doc).encode("latin-1")).decode("ascii")
    payload = (
        base64.b64encode(lzutf8_compress_reference(inner))
        .decode("ascii")
        .replace("+", "-")
        .replace("/", "_")
    )
    try:
        round_tripped = decode_payload(payload)
    except DecodeError as exc:
        print(f"  FAIL full pipeline: {exc}")
        failures += 1
    else:
        if round_tripped == doc:
            print("  ok   full pipeline (JSON -> base64 -> lzutf8 -> base64url -> back)")
        else:
            print("  FAIL full pipeline: document changed")
            failures += 1

    print("SELFTEST PASSED" if not failures else f"SELFTEST FAILED ({failures})")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("url", nargs="?", help="a ?i= share link, or a bare payload")
    parser.add_argument("--log", type=Path, help="read the newest URL out of er-build-import.log")
    parser.add_argument("--all", action="store_true", help="with --log, decode every URL found")
    parser.add_argument("--summary", action="store_true", help="print the loadout, not the JSON")
    parser.add_argument("--selftest", action="store_true", help="verify the decoder itself")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    urls: list[str] = []
    if args.log:
        if not args.log.is_file():
            print(f"no such log: {args.log}", file=sys.stderr)
            return 2
        found = urls_in_log(args.log)
        if not found:
            print(f"no '[build-export] URL ...' line in {args.log}", file=sys.stderr)
            return 2
        urls = found if args.all else found[-1:]
    elif args.url:
        urls = [args.url]
    else:
        parser.error("give a URL, --log, or --selftest")

    for position, url in enumerate(urls):
        try:
            payload = payload_of(url)
            doc = decode_payload(payload)
        except DecodeError as exc:
            print(f"DECODE FAILED: {exc}", file=sys.stderr)
            return 1
        if len(urls) > 1:
            print(f"===== link {position + 1} of {len(urls)} ({len(url)} chars) =====")
        print(summarise(doc) if args.summary else json.dumps(doc, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
