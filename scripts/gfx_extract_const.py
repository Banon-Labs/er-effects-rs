#!/usr/bin/env python3
"""Extract a `const NAME: &[u8] = &[ ... ];` byte array from a Rust source file
into a raw binary file. Reads the REAL file bytes (no rtk redaction)."""
import sys, re

def extract(src_path, const_name):
    text = open(src_path, encoding="utf-8", errors="replace").read()
    m = re.search(r"const\s+" + re.escape(const_name) + r"\s*:\s*&\[u8\]\s*=\s*&\[", text)
    if not m:
        raise SystemExit(f"const {const_name} not found")
    start = m.end()
    depth = 1
    i = start
    while i < len(text) and depth > 0:
        if text[i] == "[":
            depth += 1
        elif text[i] == "]":
            depth -= 1
            if depth == 0:
                break
        i += 1
    body = text[start:i]
    bytes_found = [int(h, 16) for h in re.findall(r"0x([0-9a-fA-F]{1,2})", body)]
    return bytes(bytes_found)

if __name__ == "__main__":
    src, name, out = sys.argv[1], sys.argv[2], sys.argv[3]
    data = extract(src, name)
    open(out, "wb").write(data)
    print(f"wrote {len(data)} bytes from {name} -> {out}")
