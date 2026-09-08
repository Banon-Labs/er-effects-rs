#!/usr/bin/env python3
"""Join chr FLVER bbox heights (scripts/chr-flver-bbox-census.py output) against
NpcParam.hitHeight (scripts/er-param-read.py output) and report the ratio
distribution. One-off measurement for the possession-camera framing question:
does hitHeight (physics capsule) track model bbox height?

    python3 scripts/er-param-read.py NpcParam --fields hitHeight,hitRadius --limit 8000 > /tmp/.../npcparam-raw.txt
    python3 scripts/chr-flver-bbox-census.py /tmp/.../chr-bbox.tsv
    python3 scripts/chr-hitheight-bbox-ratio.py /tmp/.../chr-bbox.tsv /tmp/.../npcparam-raw.txt
"""
import ast
import os
import statistics as st
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
NAMES_TBL = os.path.join(_HERE, "..", "crates", "er-npc-possess", "data", "chrnames.tbl")


def load_bbox(path):
    bbox = {}
    degenerate_ids = []
    lines = open(path, encoding="utf-8").read().splitlines()
    n_total = n_deg = 0
    for l in lines[1:]:
        if l.startswith("#") or not l.strip():
            continue
        parts = l.split("\t")
        chrid = parts[0]
        if parts[1] in ("NO_FLVER", "BAD_MAGIC"):
            continue
        height = float(parts[1])
        deg = int(parts[7])
        n_total += 1
        if deg:
            n_deg += 1
            degenerate_ids.append(chrid)
            continue
        bbox[chrid] = height
    return bbox, degenerate_ids, n_total, n_deg


def load_npcparam(path):
    rows = []
    for l in open(path, encoding="utf-8"):
        l = l.strip()
        if not l.startswith("{"):
            continue
        rows.append(ast.literal_eval(l))
    best = {}
    for r in rows:
        rid = r["id"]
        if rid <= 0:
            continue
        chrid_int = rid // 10000
        hh = r.get("hitHeight")
        if hh is None or hh <= 0:
            continue
        if chrid_int not in best or rid < best[chrid_int][0]:
            best[chrid_int] = (rid, hh)
    return best


def load_names(path):
    """chrnames.tbl keys are bare decimal chrids with no `c` prefix or zero-padding
    (e.g. `4600`, not `c4600`) -- normalize to `cNNNN` so callers can key by chrid string."""
    names = {}
    if not os.path.exists(path):
        return names
    for l in open(path, encoding="utf-8"):
        if l.startswith("#") or not l.strip() or l.strip() == "v1":
            continue
        parts = l.rstrip("\n").split("\t")
        if len(parts) >= 2 and parts[0].isdigit():
            names[f"c{int(parts[0]):04d}"] = parts[1]
    return names


def main():
    bbox_tsv, npcparam_txt = sys.argv[1], sys.argv[2]
    bbox, degenerate_ids, n_total, n_deg = load_bbox(bbox_tsv)
    best = load_npcparam(npcparam_txt)
    names = load_names(NAMES_TBL)

    rows = []
    for chrid, height in bbox.items():
        chrid_int = int(chrid[1:])
        if chrid_int not in best:
            continue
        rowid, hit_height = best[chrid_int]
        rows.append({
            "chrid": chrid, "bboxHeight": height, "hitHeight": hit_height,
            "npcRowId": rowid, "ratio": height / hit_height,
            "name": names.get(chrid, "-"),
        })

    print(f"bbox entries (parsed flvers): {n_total}")
    print(f"degenerate bbox: {n_deg} -> {sorted(degenerate_ids)}")
    print(f"c0000 degenerate: {'c0000' in degenerate_ids}")
    print(f"chrids with usable bbox: {len(bbox)}")
    print(f"chrids with usable bbox AND matched NpcParam row: {len(rows)}")
    missing_npc = sorted(set(bbox) - {r['chrid'] for r in rows})
    print(f"usable-bbox chrids with NO NpcParam row: {len(missing_npc)} -> {missing_npc}")

    ratios = sorted(rows, key=lambda r: r["ratio"])
    vals = [r["ratio"] for r in ratios]
    n = len(vals)

    def pct(p):
        idx = min(n - 1, max(0, round(p * (n - 1))))
        return vals[idx]

    print()
    print(f"ratio (bboxHeight/hitHeight) over {n} creatures:")
    print(f"  min    = {vals[0]:.4f}")
    print(f"  p10    = {pct(0.10):.4f}")
    print(f"  median = {st.median(vals):.4f}")
    print(f"  p90    = {pct(0.90):.4f}")
    print(f"  max    = {vals[-1]:.4f}")
    print(f"  mean   = {st.mean(vals):.4f}  stdev = {st.pstdev(vals):.4f}")

    print()
    print("== 10 LARGEST ratio (model much taller than capsule) ==")
    for r in ratios[-10:][::-1]:
        print(f"  {r['chrid']}  ratio={r['ratio']:.3f}  bboxHeight={r['bboxHeight']:.3f}  hitHeight={r['hitHeight']:.3f}  name={r['name']}")

    print()
    print("== 10 SMALLEST ratio (model much shorter than capsule) ==")
    for r in ratios[:10]:
        print(f"  {r['chrid']}  ratio={r['ratio']:.3f}  bboxHeight={r['bboxHeight']:.3f}  hitHeight={r['hitHeight']:.3f}  name={r['name']}")

    # The question this was run to answer. A spread in the ratio only breaks the camera's
    # framing law if it trends with size -- a size-independent scatter cancels in a ratio law's
    # slope and leaves individual creatures off by a constant, which is what the per-creature
    # `[chr.cNNNN].camera_distance_scale` knob exists to correct. So report the ratio per size
    # bucket, and the rank correlation between the two, rather than only the pooled spread.
    print()
    print("== ratio by subject height: a size-DEPENDENT bias would slide the median ==")
    for lo, hi in ((0.0, 1.0), (1.0, 2.0), (2.0, 4.0), (4.0, 8.0), (8.0, 15.0), (15.0, 1.0e9)):
        inside = sorted(r["ratio"] for r in rows if lo <= r["hitHeight"] < hi)
        if not inside:
            continue
        edge = f"{hi:.1f}" if hi < 1.0e8 else "  inf"
        print(f"  {lo:5.1f}-{edge:>5} m  n={len(inside):3d}  median {st.median(inside):5.2f}  "
              f"p10 {inside[len(inside) // 10]:5.2f}  p90 {inside[len(inside) * 9 // 10]:5.2f}")

    def ranks(values):
        order = sorted(range(len(values)), key=lambda i: values[i])
        out = [0.0] * len(values)
        for position, index in enumerate(order):
            out[index] = float(position)
        return out

    rank_h = ranks([r["hitHeight"] for r in rows])
    rank_r = ranks([r["ratio"] for r in rows])
    mean_h, mean_r = st.mean(rank_h), st.mean(rank_r)
    cov = sum((a - mean_h) * (b - mean_r) for a, b in zip(rank_h, rank_r))
    spread = (sum((a - mean_h) ** 2 for a in rank_h) ** 0.5) * (
        sum((b - mean_r) ** 2 for b in rank_r) ** 0.5
    )
    print(f"  Spearman rank correlation, height vs ratio: {cov / spread:+.3f} "
          "(near zero == scatter, not a size trend)")

    print()
    for want in ("c0000", "c4600", "c4760"):
        r = next((x for x in rows if x["chrid"] == want), None)
        if r:
            print(f"named example {want} ({r['name']}): bboxHeight={r['bboxHeight']:.3f} hitHeight={r['hitHeight']:.3f} ratio={r['ratio']:.3f}")
        else:
            print(f"named example {want}: not in joined set (degenerate bbox or no matching NpcParam row)")


if __name__ == "__main__":
    main()
