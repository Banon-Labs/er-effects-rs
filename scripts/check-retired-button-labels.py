#!/usr/bin/env python3
"""Fail when prose names a Quit-tab button that is no longer on screen.

Why this is a gate and not a NOTE
---------------------------------
Both load rows on the Quit Game tab are ours -- vanilla ships only `Save Game` and
`Return to Desktop`. They were renamed on 2026-07-31 after a review found the original pair
indistinguishable, but the old words stayed in `AGENTS.md`, in the plan docs and in the
constant names. The cost is not cosmetic: on 2026-08-19 an agent read the goal statement in
`AGENTS.md` ("System->Quit->Load Profile"), told the user to click `Load Profile`, and the
user had to send a screenshot of a menu with no such button on it.

A note asking future readers to remember a rename is exactly the kind of advisory that gets
missed -- which is how it survived three weeks. So the rename is enforced here instead.

What it checks
--------------
1. The current labels are read out of the source that actually renders them, so if the rows
   are renamed again this gate breaks instead of silently guarding stale words.
2. A retired name may appear in prose only when it is marked as history -- next to its
   replacement, or behind an explicit "ex-"/"renamed"/"old name" marker. Bare use in the
   present tense fails.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
# The row text layer moved out of the product DLL into the quit-menu crate
# (`experiments/startup_hooks/quit_menu/system_quit_dialog_handlers.rs` ->
# `er_quit_menu_core::row_text`). This gate follows the bytes, because reading a file that no
# longer holds a label would make it pass on nothing.
LABEL_SOURCE = REPO_ROOT / "crates/er-quit-menu-core/src/row_text.rs"
PROSE = [REPO_ROOT / "AGENTS.md", *sorted((REPO_ROOT / "docs").rglob("*.md"))]

# Retired -> the row it became. The replacement is confirmed against the source below, so a
# further rename cannot leave this table quietly wrong.
RETIRED = {
    "Load Profile": "Load Character",
    "Load Save Profiles": "Load Character from File",
}
# Markers that make a retired name a historical reference rather than an instruction.
HISTORY = ("ex-", "renamed", "old name", "before 2026-07-31", "used to", "was called")


def current_labels() -> set[str]:
    text = LABEL_SOURCE.read_text(encoding="utf-8")
    return set(re.findall(r'system_quit_row_text\(b"([^"]+)"\)', text))


def main() -> int:
    labels = current_labels()
    if not labels:
        print(f"check-retired-button-labels: no row labels found in {LABEL_SOURCE}", file=sys.stderr)
        return 1

    problems: list[str] = []
    for retired, replacement in RETIRED.items():
        if replacement not in labels:
            problems.append(
                f"{LABEL_SOURCE.relative_to(REPO_ROOT)}: '{replacement}' is no longer a rendered "
                f"label (found {sorted(labels)}) -- the rows were renamed again; update RETIRED."
            )
        if retired in labels:
            problems.append(f"'{retired}' is rendered again; drop it from RETIRED.")

    for path in PROSE:
        if not path.is_file():
            continue
        for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            for retired, replacement in RETIRED.items():
                if retired not in line:
                    continue
                low = line.lower()
                if replacement.lower() in low or any(m in low for m in HISTORY):
                    continue
                problems.append(
                    f"{path.relative_to(REPO_ROOT)}:{lineno}: names the retired button "
                    f"'{retired}' with nothing marking it as history -- it is called "
                    f"'{replacement}' on screen. {line.strip()[:80]}"
                )

    if problems:
        print("retired Quit-tab button names used as if current:", file=sys.stderr)
        for p in problems:
            print(f"  {p}", file=sys.stderr)
        return 1
    print(f"[check-retired-button-labels] ok -- rendered rows {sorted(labels)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
