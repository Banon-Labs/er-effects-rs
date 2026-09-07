#!/usr/bin/env python3
"""Predict where a possessed creature lands on screen, over the real height distribution.

`er-npc-possess`'s camera layer turns one creature height into a `LockCamParam` row. This
script is the offline oracle for that arithmetic: it reads every possessable creature's
`NpcParam.hitHeight` out of the installed regulation, runs a candidate size law over all of
them, and reports where each one's head would sit on screen and how much headroom it gets --
so a law can be judged against 400 real creatures instead of two hand-picked ones.

Everything it needs from the game is measured, not assumed:

  * `LockCamParam` row 0 -- camDistTarget 3.8, chrOrgOffset_Y 1.45, camFovY 48.0, rotRangeMinX
    -40. Read with `python3 scripts/er-param-read.py LockCamParam --row 0`.
  * `camFovY` is the vertical field of view in degrees. `CS::ChrExFollowCam::ApplyZoomLerp`
    (1.16.2 0x1403b7560) multiplies it by `GLOBAL_DegreeToRadian` into `CSCam.fov`, and
    `CS::CSPersCam::ToPerspective` (0x1403e9ac0) builds the projection as
    `m11 = cot(fov/2)`, `m00 = cot(fov/2) / aspectRatio` -- the extra `/aspect` on X is what
    makes the angle vertical.
  * `NpcParam` row 0 (the player) hitHeight 1.5, hitRadius 0.4.

Usage:

    timeout 28 python3 scripts/er-param-read.py NpcParam --fields hitHeight,hitRadius \
        --limit 100000 > /tmp/npc-heights.txt
    python3 scripts/er-possess-camera-framing.py --heights /tmp/npc-heights.txt

`--law similarity` (default) is the law the crate ships; `--law chest` reproduces the older
chest-pivot law for comparison.
"""

from __future__ import annotations

import argparse
import ast
import math
import os
import statistics

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# LockCamParam row 0 / NpcParam row 0 -- the one vanilla data point the whole law is anchored on.
PLAYER_HIT_HEIGHT = 1.5
PLAYER_HIT_RADIUS = 0.4
PLAYER_CAM_DIST = 3.8
PLAYER_PIVOT = 1.45
PLAYER_FOV_Y_DEG = 48.0

RADIUS_CLEARANCE = 1.25
MIN_DISTANCE = 0.5

# tan of the half vertical FOV: one half-screen-height, per metre of depth.
HALF_FRAME = math.tan(math.radians(PLAYER_FOV_Y_DEG / 2.0))
# How far above the aim point the player's head sits, per metre of camera distance.
HEAD_ABOVE_AIM = (PLAYER_HIT_HEIGHT - PLAYER_PIVOT) / PLAYER_CAM_DIST


def similarity(height: float, radius: float, exponent: float, ceiling: float, per_chr: float):
    """The shipped law: distance scales with height, pivot solved to hold the head's place."""
    scale = height / PLAYER_HIT_HEIGHT
    distance = min(max(PLAYER_CAM_DIST * scale**exponent * per_chr, MIN_DISTANCE), ceiling)
    distance = max(distance, radius * RADIUS_CLEARANCE)
    pivot = max(height - distance * HEAD_ABOVE_AIM, 0.0)
    return distance, pivot


def chest(height: float, radius: float, exponent: float, ceiling: float, per_chr: float):
    """The older law, kept so the difference can be measured rather than argued."""
    scale = height / PLAYER_HIT_HEIGHT
    ramp = min(max((scale - 1.0) / 5.0, 0.0), 1.0)
    distance = min(max(PLAYER_CAM_DIST * scale**exponent * per_chr, MIN_DISTANCE), ceiling)
    distance = max(distance, radius * RADIUS_CLEARANCE)
    pivot = PLAYER_PIVOT * scale + (0.65 * height - PLAYER_PIVOT * scale) * ramp
    return distance, pivot


LAWS = {"similarity": similarity, "chest": chest}


def framing(distance: float, pivot: float, height: float, pitch_deg: float = 0.0):
    """Where the subject's head sits, and how much sky is above it.

    `pitch_deg` is `ChrExFollowCam.anglesEuler.x` in degrees, positive meaning the camera is
    above the subject looking down (proven: `angleFromXZPlane` at 0x1403b0b70 returns the
    NEGATED elevation of the camera->target vector, and `applyControlMovement` clamps that
    angle between `+0x258` = rotRangeMinX and `+0x25c` = +70 deg).

    Returns `(head_screen_y, headroom_heights)`: the head's height above screen centre in
    half-screen-heights (1.0 == the top edge), and the gap from the head to the top edge
    measured in subject heights.
    """
    above_aim = height - pivot
    sin_p, cos_p = math.sin(math.radians(pitch_deg)), math.cos(math.radians(pitch_deg))
    depth = distance - above_aim * sin_p
    head_screen_y = (above_aim * cos_p / depth) / HALF_FRAME
    headroom = (1.0 - head_screen_y) * depth * HALF_FRAME / height
    return head_screen_y, headroom


def load_heights(path: str, moveset: str, names: str):
    rows = []
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            if line.startswith("{"):
                rows.append(ast.literal_eval(line))
    possessable = set()
    with open(moveset, encoding="utf-8") as fh:
        for line in fh:
            head = line.split()
            if head and head[0].isdigit():
                possessable.add(int(head[0]))
    base = {}
    for row in rows:
        chr_id = row["id"] // 10000
        if chr_id in possessable and (chr_id not in base or row["id"] < base[chr_id]["id"]):
            base[chr_id] = row
    label = {}
    with open(names, encoding="utf-8") as fh:
        for line in fh:
            parts = line.rstrip("\n").split("\t")
            if len(parts) == 2 and parts[0].isdigit():
                label[int(parts[0])] = parts[1]
    return base, label


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--heights", required=True, help="output of er-param-read.py NpcParam")
    ap.add_argument("--moveset", default=os.path.join(REPO, "crates/er-npc-possess/data/moveset.tbl"))
    ap.add_argument("--names", default=os.path.join(REPO, "crates/er-npc-possess/data/chrnames.tbl"))
    ap.add_argument("--law", default="similarity", choices=sorted(LAWS))
    ap.add_argument("--exponent", type=float, default=1.0)
    # `camera::geometry::MAX_FRAMING_DISTANCE`, the shipped `[camera].distance_max`.
    ap.add_argument("--ceiling", type=float, default=512.0)
    args = ap.parse_args()

    law = LAWS[args.law]
    base, label = load_heights(args.heights, args.moveset, args.names)

    ref = law(PLAYER_HIT_HEIGHT, PLAYER_HIT_RADIUS, args.exponent, args.ceiling, 1.0)
    ref_y, ref_headroom = framing(*ref, PLAYER_HIT_HEIGHT)
    print(f"law={args.law} exponent={args.exponent} ceiling={args.ceiling}")
    print(f"vertical FOV {PLAYER_FOV_Y_DEG} deg -> half-frame tangent {HALF_FRAME:.6f}")
    print(f"PLAYER: dist {ref[0]:.3f} pivot {ref[1]:.3f} -> head_screen_y {ref_y:+.5f}, "
          f"headroom {ref_headroom:.4f} subject-heights")

    usable = {c: r for c, r in base.items() if r["hitHeight"] > 0}
    print(f"{len(base)} possessable creatures, {len(usable)} with a usable hitHeight")
    heights = sorted(r["hitHeight"] for r in usable.values())
    print(f"height range {heights[0]:.2f} .. {heights[-1]:.2f} m, median {statistics.median(heights):.2f}")

    for pitch in (-40.0, -20.0, 0.0, 20.0, 45.0, 70.0):
        pref_y, pref_hr = framing(*ref, PLAYER_HIT_HEIGHT, pitch)
        ys, hrs, worst = [], [], (0.0, None)
        for chr_id, row in usable.items():
            shot = law(row["hitHeight"], row["hitRadius"], args.exponent, args.ceiling, 1.0)
            y, hr = framing(*shot, row["hitHeight"], pitch)
            ys.append(y)
            hrs.append(hr)
            drift = abs(hr - pref_hr) / pref_hr
            if drift > worst[0]:
                worst = (drift, chr_id)
        tag = f"c{worst[1]} {label.get(worst[1], '?')}" if worst[1] else "-"
        print(f"  pitch {pitch:+6.1f}: player y={pref_y:+.5f} headroom={pref_hr:.4f}H | "
              f"y[{min(ys):+.5f},{max(ys):+.5f}] headroom[{min(hrs):.4f},{max(hrs):.4f}] | "
              f"worst headroom drift {worst[0] * 100:.2f}% ({tag})")

    print("\nthe framing the biggest subjects get (pitch 0):")
    for chr_id, row in sorted(usable.items(), key=lambda kv: -kv[1]["hitHeight"])[:8]:
        shot = law(row["hitHeight"], row["hitRadius"], args.exponent, args.ceiling, 1.0)
        y, hr = framing(*shot, row["hitHeight"])
        print(f"  c{chr_id} {label.get(chr_id, '?'):32.32s} h={row['hitHeight']:5.1f} "
              f"dist={shot[0]:7.1f} pivot={shot[1]:6.1f} y={y:+.4f} headroom={hr:.4f}H")

    floor_wins = [
        (c, r["hitHeight"], r["hitRadius"])
        for c, r in usable.items()
        if r["hitRadius"] * RADIUS_CLEARANCE > PLAYER_CAM_DIST * (r["hitHeight"] / PLAYER_HIT_HEIGHT) ** args.exponent
    ]
    print(f"\nclearance floor beats the law for {len(floor_wins)} creatures: "
          + ", ".join(f"c{c}({h:.1f}m/{r:.1f}m)" for c, h, r in sorted(floor_wins)[:10]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
