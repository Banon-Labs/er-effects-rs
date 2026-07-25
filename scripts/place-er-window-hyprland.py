#!/usr/bin/env python3
"""Disabled stub for the old Elden Ring Hyprland window placer.

Runtime probes must observe Elden Ring's natural compositor geometry. They must not move, resize,
float, workspace-pin, monitor-pin, focus, or otherwise place the game window.
"""
from __future__ import annotations

import json


def main() -> int:
    print(
        json.dumps(
            {
                "event": "disabled",
                "reason": "Elden Ring window placement manipulation is disabled; observe natural compositor geometry instead",
            },
            sort_keys=True,
        ),
        flush=True,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
