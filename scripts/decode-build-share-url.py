#!/usr/bin/env python3
"""Decode an `er-build-planner` `?i=` share payload back into its JSON document.

The exporter (`crates/er-build-export`) writes `LZUTF8(base64(JSON))` and then base64url's
the result into the URL. This is the inverse, offline, so a link the DLL just emitted can be
inspected without a browser or the planner's own bundle -- which is what makes an exported
link usable as a read-back oracle for what the character actually holds.

Usage:
    python3 scripts/decode-build-share-url.py '<url-or-payload>'
    python3 scripts/decode-build-share-url.py < file-with-the-url

Prints the decoded document as indented JSON on stdout.
"""

import base64
import json
import sys

# LZ-UTF8 match headers; see crates/er-build-export/src/lzutf8.rs for the format prose.
SHORT_MATCH_HEADER = 0xC0
LONG_MATCH_HEADER = 0xE0
LENGTH_MASK = 0x1F


def base64url_decode(text: str) -> bytes:
    """Decode a base64url payload, tolerating the padding the URL drops."""
    text = text.replace("-", "+").replace("_", "/")
    return base64.b64decode(text + "=" * (-len(text) % 4))


def lzutf8_decompress(data: bytes) -> bytes:
    """Expand a single LZ-UTF8 block.

    A byte >= 0xC0 is a match header only when the next byte has its top bit clear; that is
    the same disambiguation rule the encoder relies on, and it is why the encoder only ever
    takes valid UTF-8 as input.
    """
    out = bytearray()
    index = 0
    end = len(data)
    while index < end:
        header = data[index]
        is_match = header >= SHORT_MATCH_HEADER and index + 1 < end and not data[index + 1] & 0x80
        if not is_match:
            out.append(header)
            index += 1
            continue
        length = header & LENGTH_MASK
        if header >= LONG_MATCH_HEADER:
            distance = (data[index + 1] << 8) | data[index + 2]
            index += 3
        else:
            distance = data[index + 1]
            index += 2
        start = len(out) - distance
        for step in range(length):
            out.append(out[start + step])
    return bytes(out)


def decode(payload: str) -> dict:
    """Turn a share URL (or a bare `?i=` payload) into the document it carries."""
    if "i=" in payload:
        payload = payload.split("i=", 1)[1]
    payload = payload.strip().strip("'\"")
    inner = lzutf8_decompress(base64url_decode(payload)).decode("utf-8")
    return json.loads(base64.b64decode(inner + "=" * (-len(inner) % 4)))


def main() -> int:
    payload = sys.argv[1] if len(sys.argv) > 1 else sys.stdin.read()
    print(json.dumps(decode(payload), indent=1, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
