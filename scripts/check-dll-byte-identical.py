#!/usr/bin/env python3
"""Fail when two builds of the shipped cdylibs are not byte-identical.

Used by `.github/workflows/refactor-byte-identical.yml`: a PR that claims to be a
refactor / a move should not change what the game loads, so the DLLs built from
the merge-base and from the PR head are compared byte for byte.

Build noise is normalized first, and only build noise. Two clean builds of
*identical* source differ in three PE fields, none of which is derived from the
code:

    - the COFF header TimeDateStamp          (wall-clock link time)
    - IMAGE_DEBUG_DIRECTORY.TimeDateStamp    (same)
    - the RSDS GUID + Age in the CodeView    (LLD reseeds it per link)
      debug record

Measured on crates/er-crash-logging: clean-build A vs clean-build B, same
source, differed in 10 bytes, all inside those three fields
(scripts/probe-dll-build-determinism.sh reproduces it). Leaving them in would
make the gate red on an empty diff -- it would be measuring the linker's clock.
The OptionalHeader CheckSum is zeroed for the same reason: it is a function of
the bytes we just zeroed.

...AND A FOURTH, WHICH THAT MEASUREMENT WAS STRUCTURALLY UNABLE TO SEE. The probe
above builds the SAME COMMIT twice; this gate builds TWO DIFFERENT ONES. Between
those two, one more thing changes with no help from the code:
crates/er-game-base/build.rs bakes `git rev-parse --short=12 HEAD` into
`ER_BUILD_GIT`, which the build watermark draws on screen, so every DLL that links
er-game-base carries a 12-byte ASCII commit id in .rdata -- and that is EVERY
shipped DLL. It therefore differs on every PR that has any commit at all, which
made this gate unpassable by construction rather than by anything an author did.

Measured, PR #392 run 33794571494: all 26 shipped cdylibs changed, every one of
them by exactly 24 bytes in 2 runs of 12, every run in .rdata, and the report
printed the two strings for each -- 'a56af6a06c03' (the merge-base) against
'0be70e7616f6' (the head). The PR's only Rust changes were inside `#[cfg(test)]`
modules and a test-support crate, so the release cdylibs could not have differed
for any other reason. A gate that is red on a diff which cannot reach the
artifact is not measuring the artifact.

So the two build ids are masked, and only those two: each image has its own
recorded sha replaced by a fixed placeholder of the same length. They are passed
in explicitly (--base-build-sha / --head-build-sha) rather than pattern-matched,
because "any 12 hex bytes" would be an allowlist for real .rdata content, and
this is not one -- it is two exact strings the caller already knows. Omit them
and nothing is masked.

Nothing else is normalized. There is no allowlist, no tolerance, no per-file
exemption -- every remaining byte counts, which is the point of the gate.

    usage: check-dll-byte-identical.py BASE_DIR HEAD_DIR

Exit 0 when every shipped DLL matches, 1 otherwise (or when the two directories
do not contain the same DLL set -- a refactor that stops producing an artifact
is exactly the regression this is meant to catch).
"""

from __future__ import annotations

import argparse
import pathlib
import struct
import subprocess
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent

#: How much of the sha crates/er-game-base/build.rs bakes into `ER_BUILD_GIT`. It spells this as
#: `--short=12`; the two must agree or the mask silently matches nothing.
BUILD_SHA_LENGTH = 12
#: What a masked build id is replaced with. Same length, so no offset moves and the comparison
#: stays byte-for-byte; not hex, so a masked run is obvious in any dump of the normalized image.
BUILD_SHA_PLACEHOLDER = b"@" * BUILD_SHA_LENGTH

IMAGE_DIRECTORY_ENTRY_DEBUG = 6
IMAGE_DEBUG_TYPE_CODEVIEW = 2
IMAGE_DEBUG_TYPE_REPRO = 16
DEBUG_DIRECTORY_ENTRY_SIZE = 28
PE32PLUS_MAGIC = 0x20B
# Offsets within the optional header.
OPT_CHECKSUM = 0x40
OPT_DATA_DIRECTORIES_PE32PLUS = 0x70
OPT_DATA_DIRECTORIES_PE32 = 0x60


class Pe:
    """Just enough PE parsing to locate the volatile fields and name sections."""

    def __init__(self, data: bytes) -> None:
        self.data = bytearray(data)
        self.pe_offset = struct.unpack_from("<I", self.data, 0x3C)[0]
        if bytes(self.data[self.pe_offset : self.pe_offset + 4]) != b"PE\0\0":
            raise ValueError("not a PE image (missing PE\\0\\0 signature)")
        self.coff = self.pe_offset + 4
        self.opt = self.pe_offset + 24
        magic = struct.unpack_from("<H", self.data, self.opt)[0]
        self.data_dirs = self.opt + (
            OPT_DATA_DIRECTORIES_PE32PLUS
            if magic == PE32PLUS_MAGIC
            else OPT_DATA_DIRECTORIES_PE32
        )
        section_count = struct.unpack_from("<H", self.data, self.coff + 2)[0]
        opt_size = struct.unpack_from("<H", self.data, self.coff + 16)[0]
        table = self.opt + opt_size
        self.sections = []
        for index in range(section_count):
            entry = table + index * 40
            name = bytes(self.data[entry : entry + 8]).rstrip(b"\0").decode("ascii", "replace")
            virtual_size, virtual_address, raw_size, raw_pointer = struct.unpack_from(
                "<IIII", self.data, entry + 8
            )
            self.sections.append((name, virtual_address, virtual_size, raw_pointer, raw_size))

    def rva_to_offset(self, rva: int) -> int | None:
        for _, virtual_address, virtual_size, raw_pointer, raw_size in self.sections:
            span = max(virtual_size, raw_size)
            if virtual_address <= rva < virtual_address + span:
                return raw_pointer + (rva - virtual_address)
        return None

    def section_of_offset(self, offset: int) -> str:
        for name, virtual_address, _, raw_pointer, raw_size in self.sections:
            if raw_pointer <= offset < raw_pointer + raw_size:
                return name
        return "<header>" if offset < self.sections[0][3] else "<unmapped>"

    def zero(self, offset: int, length: int) -> None:
        self.data[offset : offset + length] = b"\0" * length

    def normalize(self) -> bytes:
        """Zero the fields that differ between two links of identical source."""
        self.zero(self.coff + 4, 4)  # COFF TimeDateStamp
        self.zero(self.opt + OPT_CHECKSUM, 4)  # OptionalHeader CheckSum

        debug_rva, debug_size = struct.unpack_from(
            "<II", self.data, self.data_dirs + IMAGE_DIRECTORY_ENTRY_DEBUG * 8
        )
        if debug_rva and debug_size:
            debug_offset = self.rva_to_offset(debug_rva)
            if debug_offset is not None:
                for index in range(debug_size // DEBUG_DIRECTORY_ENTRY_SIZE):
                    entry = debug_offset + index * DEBUG_DIRECTORY_ENTRY_SIZE
                    self.zero(entry + 4, 4)  # per-entry TimeDateStamp
                    entry_type, raw_size, _, raw_pointer = struct.unpack_from(
                        "<IIII", self.data, entry + 12
                    )
                    if entry_type == IMAGE_DEBUG_TYPE_CODEVIEW and raw_size >= 24:
                        if bytes(self.data[raw_pointer : raw_pointer + 4]) == b"RSDS":
                            # 16-byte GUID + 4-byte Age; LLD reseeds both per link.
                            self.zero(raw_pointer + 4, 20)
                    elif entry_type == IMAGE_DEBUG_TYPE_REPRO:
                        self.zero(raw_pointer, raw_size)
        return bytes(self.data)


def build_sha_bytes(sha: str | None) -> bytes | None:
    """The exact ASCII run er-game-base's build.rs writes, or None when not supplied.

    Refuses anything that is not a hex sha long enough to truncate: a caller that passes a branch
    name or a short sha would otherwise mask a shorter, commoner string, and a mask that hits the
    wrong bytes is the one failure mode this whole file exists to avoid.
    """
    if sha is None:
        return None
    cleaned = sha.strip().lower()
    if len(cleaned) < BUILD_SHA_LENGTH or any(c not in "0123456789abcdef" for c in cleaned):
        raise ValueError(
            f"--*-build-sha must be a hex commit id of at least {BUILD_SHA_LENGTH} "
            f"characters; got {sha!r}"
        )
    return cleaned[:BUILD_SHA_LENGTH].encode("ascii")


def mask_build_sha(data: bytes, sha: bytes | None) -> tuple[bytes, int]:
    """Replace this image's own recorded build id, reporting how many times it was found.

    The count is returned rather than swallowed so a mask that matched nothing can be said out
    loud. Zero is not automatically wrong -- a DLL that does not link er-game-base carries no
    build id -- but a run where every DLL reports zero means the sha being masked is not the sha
    that was built, and the gate would then be passing for the wrong reason.
    """
    if sha is None:
        return data, 0
    return data.replace(sha, BUILD_SHA_PLACEHOLDER), data.count(sha)


def printable_context(data: bytes, offset: int, radius: int = 60) -> str:
    """Best-effort: the printable run containing `offset`, for diagnosing .rdata hits."""
    start = offset
    while start > 0 and offset - start < radius and 0x20 <= data[start - 1] < 0x7F:
        start -= 1
    end = offset
    while end < len(data) - 1 and end - offset < radius and 0x20 <= data[end] < 0x7F:
        end += 1
    text = data[start:end].decode("ascii", "replace")
    return text if len(text) >= 4 else ""


def differing_runs(left: bytes, right: bytes) -> list[tuple[int, int]]:
    limit = min(len(left), len(right))
    offsets = [index for index in range(limit) if left[index] != right[index]]
    runs: list[tuple[int, int]] = []
    if not offsets:
        return runs
    start = offsets[0]
    for current, following in zip(offsets, offsets[1:] + [None]):
        if following is None or following != current + 1:
            runs.append((start, current))
            start = following
    return runs


def compare(
    base_path: pathlib.Path,
    head_path: pathlib.Path,
    base_sha: bytes | None = None,
    head_sha: bytes | None = None,
) -> tuple[bool, list[str], int]:
    """Return (identical, report_lines, build_ids_masked) for one DLL pair."""
    base_pe = Pe(base_path.read_bytes())
    head_pe = Pe(head_path.read_bytes())
    base, base_hits = mask_build_sha(base_pe.normalize(), base_sha)
    head, head_hits = mask_build_sha(head_pe.normalize(), head_sha)
    masked = base_hits + head_hits

    if base == head:
        return True, [], masked

    lines: list[str] = []
    if len(base) != len(head):
        lines.append(f"    size: {len(base)} (base) vs {len(head)} (head)")
    runs = differing_runs(base, head)
    total = sum(end - start + 1 for start, end in runs)
    lines.append(
        f"    {total} differing bytes in {len(runs)} run(s)"
        f" ({total * 100.0 / max(len(base), 1):.1f}% of the image)"
    )
    by_section: dict[str, int] = {}
    for start, end in runs:
        by_section[head_pe.section_of_offset(start)] = by_section.get(
            head_pe.section_of_offset(start), 0
        ) + (end - start + 1)
    lines.append(
        "    by section: "
        + ", ".join(f"{name} {count}" for name, count in sorted(by_section.items()))
    )
    for start, end in runs[:3]:
        section = head_pe.section_of_offset(start)
        lines.append(f"    first diffs: 0x{start:x}-0x{end:x} ({end - start + 1} bytes) in {section}")
        for label, blob in (("base", base), ("head", head)):
            context = printable_context(blob, start)
            if context:
                lines.append(f"        {label}: {context!r}")
    return False, lines, masked


def shipped_artifacts(artifacts_file: pathlib.Path | None) -> list[str]:
    """The DLL set to compare.

    CI passes `--artifacts-file`: the list is frozen once, from the PR head, and
    reused for both builds. Recomputing it here would let a PR that edits the
    cdylib list change the comparison set out from under itself.
    """
    if artifacts_file is not None:
        return artifacts_file.read_text(encoding="utf-8").split()
    output = subprocess.run(
        [sys.executable, str(REPO_ROOT / "scripts" / "me3-dll-list.py"), "--artifacts"],
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    ).stdout
    return output.split()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base_dir", type=pathlib.Path, help="DLLs built from the merge-base")
    parser.add_argument("head_dir", type=pathlib.Path, help="DLLs built from the PR head")
    parser.add_argument(
        "--artifacts-file",
        type=pathlib.Path,
        default=None,
        help="file of DLL filenames to compare (default: derive from scripts/me3-dll-list.py)",
    )
    parser.add_argument(
        "--base-build-sha",
        default=None,
        help="commit the BASE DLLs were built from; its first "
        f"{BUILD_SHA_LENGTH} hex characters are the ER_BUILD_GIT string masked out of them",
    )
    parser.add_argument(
        "--head-build-sha",
        default=None,
        help="the same for the HEAD DLLs",
    )
    args = parser.parse_args()

    base_sha = build_sha_bytes(args.base_build_sha)
    head_sha = build_sha_bytes(args.head_build_sha)

    artifacts = shipped_artifacts(args.artifacts_file)
    if not artifacts:
        print("[dll-byte-identical] FAIL: empty artifact list -- nothing would be compared")
        return 1
    print(f"[dll-byte-identical] comparing {len(artifacts)} shipped cdylibs")
    if base_sha and head_sha:
        print(
            f"[dll-byte-identical] masking the build watermark: base "
            f"{base_sha.decode()} / head {head_sha.decode()} -> {BUILD_SHA_PLACEHOLDER.decode()}"
        )
    else:
        print(
            "[dll-byte-identical] NOT masking the build watermark (no --base-build-sha/"
            "--head-build-sha); every DLL linking er-game-base will differ by its 12-byte "
            "ER_BUILD_GIT string alone"
        )

    missing: list[str] = []
    changed: list[str] = []
    masked_total = 0
    for artifact in artifacts:
        base_path = args.base_dir / artifact
        head_path = args.head_dir / artifact
        if not base_path.is_file() or not head_path.is_file():
            where = "base" if not base_path.is_file() else "head"
            print(f"  MISSING  {artifact}  (absent from the {where} build)")
            missing.append(artifact)
            continue
        identical, lines, masked = compare(base_path, head_path, base_sha, head_sha)
        masked_total += masked
        if identical:
            print(f"  ok       {artifact}")
        else:
            print(f"  CHANGED  {artifact}")
            for line in lines:
                print(line)
            changed.append(artifact)

    print()
    # Said out loud, because a mask that matched nothing is indistinguishable from a mask that
    # was not needed -- and the difference decides whether a green run means anything.
    if (base_sha or head_sha) and not masked_total:
        print(
            "[dll-byte-identical] WARNING: the build watermark was never found. Either these "
            "DLLs do not link er-game-base, or the shas passed in are not the ones that were "
            "built -- in which case nothing was masked and a PASS here proves less than it looks."
        )
    elif masked_total:
        print(f"[dll-byte-identical] build watermark masked in {masked_total} place(s)")
    if not missing and not changed:
        print(f"[dll-byte-identical] PASS: all {len(artifacts)} DLLs byte-identical")
        return 0

    if missing:
        print(f"[dll-byte-identical] {len(missing)} DLL(s) not produced by both builds: "
              + ", ".join(missing))
    if changed:
        print(f"[dll-byte-identical] {len(changed)} DLL(s) changed: " + ", ".join(changed))
    print()
    print(
        "This PR is scoped as a refactor/move, so the shipped DLLs were expected to be\n"
        "unchanged. Note that a pure code move DOES change the bytes: rustc embeds the\n"
        "crate-relative source path and line number of every panic site (core::panic::\n"
        "Location) and derives symbol hashes from module paths, so relocating a function\n"
        "rewrites .rdata even when behaviour is identical -- measured at 17% of the image\n"
        "for a single moved fn (scripts/probe-dll-build-determinism.sh). If that is what\n"
        "happened here, this gate cannot be satisfied by any change to the code; drop the\n"
        "refactor/move wording from the PR title and branch name, or prove equivalence\n"
        "with a runtime smoke instead."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
