#!/usr/bin/env python3
"""Reconstruct sprite hierarchy + root display list from gfx_display_list.py JSON."""
import json, sys

d = json.load(open(sys.argv[1]))
tags = d["tags"]

# Reconstruct ownership: a DefineSprite is followed by its children at level+1
# (walk() recursed immediately). Track a stack of open sprites by level.
sprite_children = {}   # sprite_id -> list of tag dicts
root_tags = []
stack = []  # (level, sprite_id)

for t in tags:
    lv = t["level"]
    while stack and lv <= stack[-1][0]:
        stack.pop()
    if stack:
        sprite_children[stack[-1][1]].append(t)
    else:
        root_tags.append(t)
    if t["tag"] == "DefineSprite":
        stack.append((lv, t["sprite_id"]))
        sprite_children[t["sprite_id"]] = []

names = {}   # char id -> best-known name
for t in tags:
    if t["tag"] == "GFX_DefineExternalImage2":
        names[t["img_char"]] = "IMG:" + t["export_name"]
    elif t["tag"] == "DefineEditText":
        names[t["id"]] = "EDITTEXT"
    elif t["tag"].startswith("DefineShape"):
        names[t["id"]] = "SHAPE" + str(t["bounds_twips"])
    elif t["tag"] == "DefineSprite":
        names[t["sprite_id"]] = "SPRITE"
    elif t["tag"] == "SymbolClass":
        for cid, nm in t["symbols"]:
            if cid:
                names[cid] = names.get(cid, "") + " <sym:" + nm + ">"
    elif t["tag"] == "ExportAssets":
        for cid, nm in t.get("symbols", []):
            names[cid] = names.get(cid, "") + " <exp:" + nm + ">"

def fmt_place(t):
    parts = [f"depth={t['depth']}"]
    if "char" in t:
        parts.append(f"char={t['char']} ({names.get(t['char'],'?')})")
    if t.get("move"):
        parts.append("MOVE")
    if "name" in t:
        parts.append(f"name='{t['name']}'")
    if "clipdepth" in t:
        parts.append(f"clipdepth={t['clipdepth']}")
    m = t.get("matrix")
    if m:
        s = f"tx={m['tx']/20.0:g}px ty={m['ty']/20.0:g}px"
        if m.get("scale"):
            s += f" scale=({m['scale'][0]:g},{m['scale'][1]:g})"
        if m.get("rotate"):
            s += f" rot=({m['rotate'][0]:g},{m['rotate'][1]:g})"
        parts.append(s)
    if t.get("has_image"):
        parts.append("HAS_IMAGE")
    if "cxform" in t and "char" not in t:
        parts.append("cxform-only")
    return " ".join(parts)

def dump_sprite(sid, indent=0, seen=None):
    seen = seen or set()
    if sid in seen:
        print("  " * indent + f"(recursion {sid})")
        return
    seen = seen | {sid}
    kids = sprite_children.get(sid, [])
    # summarize: first-frame placements + frame count of cxform anims
    place_by_depth = {}
    n_cx = 0
    for t in kids:
        if t["tag"] in ("PlaceObject2", "PlaceObject3"):
            if "char" in t:
                key = (t["depth"], t.get("frame", 0))
                print("  " * indent + f"f{t['frame']} {t['tag']} {fmt_place(t)}")
                if t["char"] in sprite_children:
                    dump_sprite(t["char"], indent + 1, seen)
            else:
                n_cx += 1
        elif t["tag"] == "RemoveObject2":
            print("  " * indent + f"f{t['frame']} Remove depth={t['remove_depth']}")
        elif t["tag"] == "FrameLabel":
            print("  " * indent + f"f{t['frame']} LABEL '{t['label']}'")
    if n_cx:
        print("  " * indent + f"({n_cx} cxform-only keyframes)")

print("=== ROOT TIMELINE (frame 0 placements and labels) ===")
for t in root_tags:
    if t["tag"] in ("PlaceObject2", "PlaceObject3") and "char" in t:
        print(f"f{t['frame']} {t['tag']} {fmt_place(t)}")
    elif t["tag"] == "RemoveObject2":
        print(f"f{t['frame']} Remove depth={t['remove_depth']}")
    elif t["tag"] == "FrameLabel":
        print(f"f{t['frame']} LABEL '{t['label']}'")
    elif t["tag"] in ("SymbolClass", "ExportAssets", "ImportAssets2"):
        print(t["tag"], json.dumps(t.get("symbols", t)))

print()
print("=== SPRITE TREES (root-placed) ===")
placed_root = [t["char"] for t in root_tags
               if t["tag"] in ("PlaceObject2", "PlaceObject3") and "char" in t]
for sid in placed_root:
    if sid in sprite_children:
        print(f"--- sprite {sid} {names.get(sid,'')} ---")
        dump_sprite(sid, 1)

print()
print("=== EDITTEXT geometry ===")
for t in tags:
    if t["tag"] == "DefineEditText":
        b = t["bounds_twips"]
        print(f"id={t['id']} bounds_twips={b} bounds_px=({b[0]/20:g},{b[2]/20:g})..({b[1]/20:g},{b[3]/20:g}) "
              f"font={t.get('font_class')} h={t.get('font_height_twips',0)/20:g}px var='{t.get('var','')}' "
              f"align={t.get('layout',{}).get('align')} text[:20]='{t.get('text','')[:20]}'")
