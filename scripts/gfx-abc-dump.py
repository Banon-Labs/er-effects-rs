#!/usr/bin/env python3
"""Extract and summarize Scaleform GFX/SWF ActionScript/timeline evidence.

This is an agent-facing static RE helper for Elden Ring menu .gfx files. It uses
FFDec when available to export XML/AS/p-code, then derives a compact JSON/text
summary of external images, frame labels, timeline placements, alpha transforms,
and whether named image resources are actually placed on a timeline.
"""
from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Any


def run(cmd: list[str], log_path: Path) -> int:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    proc = subprocess.run(cmd, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    log_path.write_text(proc.stdout, encoding="utf-8", errors="replace")
    return proc.returncode


def export_with_ffdec(gfx: Path, out_dir: Path, ffdec: str) -> Path:
    stem = gfx.name
    xml_path = out_dir / "xml" / f"{stem}.xml"
    xml_path.parent.mkdir(parents=True, exist_ok=True)
    rc = run([ffdec, "-swf2xml", str(gfx), str(xml_path)], out_dir / "logs" / f"{stem}.swf2xml.log")
    if rc != 0 or not xml_path.exists() or xml_path.stat().st_size == 0:
        raise RuntimeError(f"ffdec -swf2xml failed for {gfx}; see {out_dir / 'logs' / (stem + '.swf2xml.log')}")

    for kind, fmt in [("as", None), ("pcode", "script:pcode"), ("text", None), ("image", None)]:
        target = out_dir / "ffdec-export" / kind / stem
        target.mkdir(parents=True, exist_ok=True)
        cmd = [ffdec, "-onerror", "ignore"]
        if fmt:
            cmd += ["-format", fmt]
        item_type = "script" if kind in {"as", "pcode"} else kind
        cmd += ["-export", item_type, str(target), str(gfx)]
        run(cmd, out_dir / "logs" / f"{stem}.{kind}.export.log")
    return xml_path


def attrs(e: ET.Element, keys: list[str]) -> dict[str, str | None]:
    return {k: e.attrib.get(k) for k in keys}


def parse_gfx_xml(xml_path: Path) -> dict[str, Any]:
    root = ET.parse(xml_path).getroot()
    images: dict[str, dict[str, str | None]] = {}
    for e in root.iter("item"):
        if e.attrib.get("type") == "DefineExternalImage2":
            cid = e.attrib.get("characterID")
            if cid:
                images[cid] = attrs(e, ["exportName", "fileName", "targetWidth", "targetHeight", "imageID"])

    placement_counts: dict[str, int] = {}
    timeline_summaries: list[dict[str, Any]] = []
    root_placed_character_ids: list[str] = []
    sprite_children: dict[str, list[str]] = {}

    def iter_rows(container: ET.Element) -> tuple[int, list[dict[str, Any]], list[str]]:
        frame = 1
        rows: list[dict[str, Any]] = []
        placed_character_ids: list[str] = []
        for it in list(container):
            typ = it.attrib.get("type")
            if typ == "FrameLabelTag":
                rows.append({"frame": frame, "kind": "label", "name": it.attrib.get("name")})
            elif typ in {"PlaceObject2Tag", "PlaceObject3Tag"}:
                char_id = it.attrib.get("characterId")
                if char_id:
                    placement_counts[char_id] = placement_counts.get(char_id, 0) + 1
                    placed_character_ids.append(char_id)
                matrix = it.find("matrix")
                color = it.find("colorTransform")
                alpha = None
                if color is not None:
                    alpha = color.attrib.get("alphaMultTerm")
                    if alpha is None and color.attrib.get("hasMultTerms") == "false":
                        alpha = "identity"
                rows.append(
                    {
                        "frame": frame,
                        "kind": "place",
                        "tag": typ,
                        "depth": it.attrib.get("depth"),
                        "character_id": char_id,
                        "asset": (images.get(char_id or "") or {}).get("exportName"),
                        "move": it.attrib.get("placeFlagMove"),
                        "alpha_mult_term": alpha,
                        "x": matrix.attrib.get("translateX") if matrix is not None else None,
                        "y": matrix.attrib.get("translateY") if matrix is not None else None,
                        "scale_x": matrix.attrib.get("scaleX") if matrix is not None else None,
                        "scale_y": matrix.attrib.get("scaleY") if matrix is not None else None,
                    }
                )
            elif typ == "RemoveTag":
                rows.append({"frame": frame, "kind": "remove", "depth": it.attrib.get("depth")})
            elif typ == "ShowFrameTag":
                frame += 1
        return frame, rows, placed_character_ids

    tags = root.find("tags")
    if tags is not None:
        frame_end, rows, root_placed_character_ids = iter_rows(tags)
        timeline_summaries.append({"name": "root/tags", "frame_end": frame_end, "rows": rows})

    for e in root.iter("item"):
        if e.attrib.get("type") != "DefineSpriteTag":
            continue
        sub = e.find("subTags")
        if sub is None:
            continue
        frame_end, rows, placed_character_ids = iter_rows(sub)
        sprite_id = e.attrib.get("spriteId")
        if sprite_id:
            sprite_children[sprite_id] = placed_character_ids
        if rows:
            timeline_summaries.append(
                {
                    "name": f"sprite {sprite_id}",
                    "sprite_id": sprite_id,
                    "frame_count": e.attrib.get("frameCount"),
                    "frame_end": frame_end,
                    "rows": rows,
                }
            )

    for cid in images:
        placement_counts.setdefault(cid, 0)

    reachable_external_images: dict[str, dict[str, str | None]] = {}
    seen: set[str] = set()

    def visit_character(cid: str) -> None:
        if cid in seen:
            return
        seen.add(cid)
        if cid in images:
            reachable_external_images[cid] = images[cid]
        for child in sprite_children.get(cid, []):
            visit_character(child)

    for cid in root_placed_character_ids:
        visit_character(cid)

    return {
        "xml": str(xml_path),
        "external_images": images,
        "placement_counts": placement_counts,
        "root_placed_character_ids": root_placed_character_ids,
        "sprite_children": sprite_children,
        "root_reachable_external_images": reachable_external_images,
        "timelines": timeline_summaries,
    }


def interesting_rows(summary: dict[str, Any]) -> list[str]:
    lines: list[str] = []
    images = summary["external_images"]
    counts = summary["placement_counts"]
    lines.append(f"# {Path(summary['xml']).name}")
    lines.append("external_images:")
    for cid in sorted(images, key=lambda s: int(s) if s.isdigit() else 999999):
        image = images[cid]
        lines.append(
            f"  char {cid}: {image.get('exportName')} {image.get('targetWidth')}x{image.get('targetHeight')} placements={counts.get(cid, 0)}"
        )
    lines.append("root_placed_character_ids: " + repr(summary.get("root_placed_character_ids", [])))
    reachable = summary.get("root_reachable_external_images", {})
    lines.append("root_reachable_external_images:")
    for cid in sorted(reachable, key=lambda s: int(s) if s.isdigit() else 999999):
        image = reachable[cid]
        lines.append(f"  char {cid}: {image.get('exportName')}")
    lines.append("timelines:")
    for tl in summary["timelines"]:
        rows = tl["rows"]
        labels = [r for r in rows if r["kind"] == "label"]
        placed_assets = [r for r in rows if r["kind"] == "place" and r.get("asset")]
        alpha_rows = [r for r in rows if r["kind"] == "place" and r.get("alpha_mult_term") is not None]
        if not labels and not placed_assets and not alpha_rows:
            continue
        lines.append(f"  {tl['name']} frame_end={tl.get('frame_end')} labels={[ (r['frame'], r['name']) for r in labels ]}")
        for r in placed_assets[:20]:
            lines.append(
                "    place "
                f"f={r['frame']} depth={r['depth']} char={r['character_id']} asset={r['asset']} "
                f"alpha={r['alpha_mult_term']} x={r['x']} y={r['y']} sx={r['scale_x']} sy={r['scale_y']}"
            )
        if alpha_rows:
            first = alpha_rows[:5]
            last = alpha_rows[-5:] if len(alpha_rows) > 5 else []
            lines.append("    alpha_sample_first=" + repr([(r["frame"], r["depth"], r["alpha_mult_term"]) for r in first]))
            if last:
                lines.append("    alpha_sample_last=" + repr([(r["frame"], r["depth"], r["alpha_mult_term"]) for r in last]))
    return lines


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("gfx", nargs="+", type=Path, help="GFX/SWF files to export and summarize")
    ap.add_argument("--out-dir", type=Path, default=Path("target/autoresearch/gfx-analysis"))
    ap.add_argument("--ffdec", default=shutil.which("ffdec") or "ffdec")
    args = ap.parse_args()

    summaries = []
    for gfx in args.gfx:
        gfx = gfx.resolve()
        xml_path = export_with_ffdec(gfx, args.out_dir, args.ffdec)
        summaries.append(parse_gfx_xml(xml_path))

    args.out_dir.mkdir(parents=True, exist_ok=True)
    summary_json = args.out_dir / "summary.json"
    summary_txt = args.out_dir / "summary.txt"
    summary_json.write_text(json.dumps(summaries, indent=2, sort_keys=True), encoding="utf-8")
    summary_txt.write_text("\n\n".join("\n".join(interesting_rows(s)) for s in summaries) + "\n", encoding="utf-8")
    print(summary_txt)
    return 0


if __name__ == "__main__":
    sys.exit(main())
