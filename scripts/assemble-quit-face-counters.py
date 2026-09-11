#!/usr/bin/env python3
"""One-shot helper: write `counters/quit_face_portrait.rs` from an extracted counter block.

Kept because the same move will be wanted again. `crates/er-telemetry-core/src/counters.rs` was
already 3311 lines when `scripts/check-rust-file-sizes.py` fails above 3200, so a new counter family
belongs in a submodule beside it rather than appended to it -- and the mechanical part of that move
(prepend a module doc and the two atomic imports, write the file) is what this does.

Usage: `python3 scripts/assemble-quit-face-counters.py <block.txt> <out.rs>`
"""

import sys

HEADER = '''//! The System>Quit panel's own character portrait, which has a different producer from the ten
//! profile targets the `BUILD_URL_PORTRAIT` family measures.
//!
//! Its own file rather than more lines of `counters.rs`, which is already past the hard size limit
//! `scripts/check-rust-file-sizes.py` enforces. The parent re-exports everything here, so a
//! consumer still spells these `er_telemetry_core::counters::BUILD_URL_QUIT_FACE_CALLS` and
//! friends.
//!
//! What each field means, and how the two surfaces differ, is in
//! `er_profile_summary_core::quit_panel_portrait`.

use std::sync::atomic::{AtomicU64, AtomicUsize};

'''


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    block = open(sys.argv[1], encoding="utf-8").read().rstrip()
    with open(sys.argv[2], "w", encoding="utf-8") as handle:
        handle.write(HEADER + block + "\n")
    print(f"wrote {sys.argv[2]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
