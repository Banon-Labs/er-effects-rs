#!/usr/bin/env python3
"""Tests for scripts/check-dll-byte-identical.py and scripts/me3-dll-list.py.

The comparator is only useful if it draws the line in exactly the right place:
blind to link-time noise, and unable to miss a single changed code byte. Both
halves are asserted here against a REAL cdylib from `target/`, because a
synthetic PE would not exercise the debug-directory/CodeView walk that the
normalizer depends on.

Skips (exit 0) when no built DLL is available -- same convention as the
corpus-dependent tests in crates/er-gfx.
"""

from __future__ import annotations

import importlib.util
import pathlib
import struct
import subprocess
import sys
import tempfile

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
RELEASE_DIR = REPO_ROOT / "target" / "x86_64-pc-windows-msvc" / "release"


def load_checker():
    path = REPO_ROOT / "scripts" / "check-dll-byte-identical.py"
    spec = importlib.util.spec_from_file_location("check_dll_byte_identical", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def find_sample_dll() -> pathlib.Path | None:
    """Smallest SHIPPED cdylib, so the test exercises a PE the gate actually compares."""
    if not RELEASE_DIR.is_dir():
        return None
    shipped = set(
        subprocess.run(
            [sys.executable, str(REPO_ROOT / "scripts" / "me3-dll-list.py"), "--artifacts"],
            check=True,
            capture_output=True,
            text=True,
            timeout=30,
        ).stdout.split()
    )
    candidates = sorted(
        (path for path in RELEASE_DIR.glob("*.dll") if path.name in shipped),
        key=lambda path: path.stat().st_size,
    )
    return candidates[0] if candidates else None


def write(tmp: pathlib.Path, name: str, data: bytes) -> pathlib.Path:
    path = tmp / name
    path.write_bytes(data)
    return path


def bump_link_noise(checker, data: bytes) -> bytes:
    """Rewrite exactly the fields a second link would change, and nothing else."""
    pe = checker.Pe(data)
    struct.pack_into("<I", pe.data, pe.coff + 4, 0x11111111)  # COFF TimeDateStamp
    struct.pack_into("<I", pe.data, pe.opt + checker.OPT_CHECKSUM, 0x22222222)
    debug_rva, debug_size = struct.unpack_from(
        "<II", pe.data, pe.data_dirs + checker.IMAGE_DIRECTORY_ENTRY_DEBUG * 8
    )
    touched_codeview = False
    if debug_rva and debug_size:
        debug_offset = pe.rva_to_offset(debug_rva)
        for index in range(debug_size // checker.DEBUG_DIRECTORY_ENTRY_SIZE):
            entry = debug_offset + index * checker.DEBUG_DIRECTORY_ENTRY_SIZE
            struct.pack_into("<I", pe.data, entry + 4, 0x33333333)
            entry_type, raw_size, _, raw_pointer = struct.unpack_from("<IIII", pe.data, entry + 12)
            if entry_type == checker.IMAGE_DEBUG_TYPE_CODEVIEW and raw_size >= 24:
                if bytes(pe.data[raw_pointer : raw_pointer + 4]) == b"RSDS":
                    pe.data[raw_pointer + 4 : raw_pointer + 24] = bytes(range(20))
                    touched_codeview = True
    assert touched_codeview, "sample DLL has no RSDS record; normalizer path untested"
    return bytes(pe.data)


def flip_text_byte(checker, data: bytes) -> bytes:
    """Change one byte of executable code -- the thing the gate must never miss."""
    pe = checker.Pe(data)
    for name, _, _, raw_pointer, raw_size in pe.sections:
        if name == ".text" and raw_size:
            target = raw_pointer + raw_size // 2
            pe.data[target] ^= 0xFF
            return bytes(pe.data)
    raise AssertionError("sample DLL has no .text section")


def main() -> int:
    checker = load_checker()
    failures: list[str] = []

    # --- me3-dll-list.py stays in sync with the link gate -------------------
    pairs = subprocess.run(
        [sys.executable, str(REPO_ROOT / "scripts" / "me3-dll-list.py"), "--pairs"],
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    ).stdout.split()
    if len(pairs) < 10:
        failures.append(f"me3-dll-list parsed only {len(pairs)} cdylibs; expected the full shipped set")
    if not any(pair.startswith("er-effects-rs:") for pair in pairs):
        failures.append("me3-dll-list omitted the product crate er-effects-rs")
    # The four crates that override [lib] name must keep both halves distinct.
    overrides = {"er-better-refills-dll": "er_better_refills", "mushroom-man-runtime": "mushroom_man"}
    for package, artifact in overrides.items():
        if f"{package}:{artifact}" not in pairs:
            failures.append(f"me3-dll-list lost the [lib] name override {package} -> {artifact}")

    sample = find_sample_dll()
    if sample is None:
        print("[test-dll-byte-identical] no built DLL under target/; skipping comparator tests")
        if failures:
            for line in failures:
                print(f"  FAIL: {line}")
            return 1
        print("[test-dll-byte-identical] ok (list tests only)")
        return 0

    original = sample.read_bytes()
    with tempfile.TemporaryDirectory() as raw_tmp:
        tmp = pathlib.Path(raw_tmp)
        base = write(tmp, "base.dll", original)

        # --- link noise must NOT fail the gate ----------------------------
        noisy = write(tmp, "noisy.dll", bump_link_noise(checker, original))
        identical, lines = checker.compare(base, noisy)
        if not identical:
            failures.append(
                "link-time noise (timestamps + RSDS GUID) was reported as a real change: "
                + "; ".join(lines[:2])
            )

        # --- one changed code byte MUST fail the gate ---------------------
        patched = write(tmp, "patched.dll", flip_text_byte(checker, original))
        identical, lines = checker.compare(base, patched)
        if identical:
            failures.append("a flipped .text byte was normalized away -- the gate is blind to code changes")
        elif not any(".text" in line for line in lines):
            failures.append(f"changed .text byte was not attributed to .text: {lines}")

        # --- a truncated image must fail, not crash ------------------------
        truncated = write(tmp, "truncated.dll", original[: len(original) - 4096])
        identical, lines = checker.compare(base, truncated)
        if identical:
            failures.append("a truncated image compared equal")
        elif not any("size:" in line for line in lines):
            failures.append(f"size mismatch not reported: {lines}")

    if failures:
        for line in failures:
            print(f"  FAIL: {line}")
        return 1
    print(f"[test-dll-byte-identical] ok (comparator exercised against {sample.name})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
