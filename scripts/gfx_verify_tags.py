#!/usr/bin/env python3
"""Minimal GFX/SWF tag-stream parser for verifying the title cover GFX.

Checks magic 'GFX' + version 0x0b, reads FileLength, skips the bit-packed
RECT + frameRate(u16) + frameCount(u16), then walks the tag stream decoding
each tag header. (word>>6)=code, low6=len, len==0x3f -> following u32 long len.

Usage:
  python3 scripts/gfx_verify_tags.py emitted=/path/to/file.gfx original=/path/to/orig.gfx
"""
import sys, struct

TAG_NAMES = {
    0: "End",
    1: "ShowFrame",
    2: "DefineShape",
    9: "SetBackgroundColor",
    26: "PlaceObject2",
    34: "DefineButton2",
    39: "DefineSprite",
    69: "FileAttributes",
}

def gfx_tag_name(code):
    extra = {
        1000: "ExporterInfo",
        1001: "DefineExternalImage",
        1002: "DefineExternalGradient",
        1003: "DefineSubImage",
        1004: "DefineExternalSound",
        1005: "DefineExternalStreamSound",
        1007: "FontTextureInfo",
        1008: "DefineExternalImage2",
    }
    return extra.get(code) or TAG_NAMES.get(code) or f"Unknown_{code}"

class BitReader:
    def __init__(self, data, pos=0):
        self.data = data
        self.pos = pos
        self.bit = 0
    def read_ub(self, n):
        v = 0
        for _ in range(n):
            byte = self.data[self.pos]
            bitval = (byte >> (7 - self.bit)) & 1
            v = (v << 1) | bitval
            self.bit += 1
            if self.bit == 8:
                self.bit = 0
                self.pos += 1
        return v
    def align(self):
        if self.bit != 0:
            self.bit = 0
            self.pos += 1
        return self.pos

def parse_rect(data, pos):
    br = BitReader(data, pos)
    nbits = br.read_ub(5)
    xmin = br.read_ub(nbits)
    xmax = br.read_ub(nbits)
    ymin = br.read_ub(nbits)
    ymax = br.read_ub(nbits)
    end = br.align()
    return (nbits, xmin, xmax, ymin, ymax), end

def parse_define_shape(body):
    """DefineShape (tag 2): ShapeId u16, ShapeBounds RECT, then ShapeWithStyle:
    FillStyleCount u8 (0xff -> u16 extended), then FillStyles (solid = type 0x00 + RGB)."""
    out = {}
    p = 0
    out["shape_id"] = struct.unpack_from("<H", body, p)[0]; p += 2
    (rect, p) = parse_rect(body, p)
    out["bounds"] = rect
    fill_count = body[p]; p += 1
    if fill_count == 0xff:
        fill_count = struct.unpack_from("<H", body, p)[0]; p += 2
    out["fill_count"] = fill_count
    fills = []
    for _ in range(fill_count):
        fill_type = body[p]; p += 1
        if fill_type == 0x00:  # solid fill, RGB (no alpha in tag 2)
            r, g, b = body[p], body[p+1], body[p+2]; p += 3
            fills.append(("solid", (r, g, b)))
        else:
            fills.append(("type_0x%02x" % fill_type, None))
            break
    out["fills"] = fills
    return out

def parse_gfx(data, label):
    print(f"===== {label} =====")
    print(f"total bytes: {len(data)}")
    issues = []
    if data[:3] != b"GFX":
        print(f"  MAGIC: {data[:3]!r} (NOT 'GFX')"); issues.append("bad magic")
    else:
        print(f"  MAGIC: 'GFX' OK")
    version = data[3]
    print(f"  version: 0x{version:02x} ({'OK' if version==0x0b else 'EXPECTED 0x0b'})")
    if version != 0x0b:
        issues.append("bad version")
    file_length = struct.unpack_from("<I", data, 4)[0]
    print(f"  FileLength field: {file_length}  (actual file: {len(data)}) "
          f"{'MATCH' if file_length==len(data) else 'MISMATCH'}")
    if file_length != len(data):
        issues.append("FileLength mismatch")
    (rect, pos) = parse_rect(data, 8)
    print(f"  movie RECT nbits={rect[0]} xmin={rect[1]} xmax={rect[2]} ymin={rect[3]} ymax={rect[4]}")
    frame_rate = struct.unpack_from("<H", data, pos)[0]; pos += 2
    frame_count = struct.unpack_from("<H", data, pos)[0]; pos += 2
    print(f"  frameRate(u16 raw)=0x{frame_rate:04x}, frameCount={frame_count}")
    print(f"  tag stream starts at offset {pos}")
    print("  --- tags ---")
    tags = []
    bg_rgb = None
    shape_solid_rgbs = []
    while pos < len(data):
        if pos + 2 > len(data):
            print(f"  TRUNCATED tag header at {pos}"); issues.append("truncated"); break
        tag_word = struct.unpack_from("<H", data, pos)[0]; pos += 2
        code = tag_word >> 6
        length = tag_word & 0x3f
        long_form = False
        if length == 0x3f:
            length = struct.unpack_from("<I", data, pos)[0]; pos += 4; long_form = True
        name = gfx_tag_name(code)
        body_start = pos
        body = data[body_start:body_start+length]
        extra = ""
        if code == 9 and length >= 3:
            r, g, b = body[0], body[1], body[2]
            bg_rgb = (r, g, b)
            extra = f"  RGB=({r:02x},{g:02x},{b:02x})"
        elif code == 2:
            try:
                ds = parse_define_shape(body)
                for (t, rgb) in ds["fills"]:
                    if t == "solid":
                        shape_solid_rgbs.append(rgb)
                fills_txt = ", ".join(
                    f"{t}:{('%02x%02x%02x' % rgb) if rgb else '?'}" for (t, rgb) in ds["fills"])
                extra = (f"  shapeId={ds['shape_id']} bounds={ds['bounds']} "
                         f"fillCount={ds['fill_count']} fills=[{fills_txt}]")
            except Exception as e:
                extra = f"  (shape parse error: {e})"
        elif code == 1000:
            printable = bytes(c for c in body if 32 <= c < 127)
            extra = f"  exporterStr~{printable!r}"
        hdr_off = body_start - (6 if long_form else 2)
        print(f"    @0x{hdr_off:04x} code={code:<4} len={length:<4} "
              f"{'(long)' if long_form else '     '} {name}{extra}")
        tags.append((code, name, length))
        pos = body_start + length
        if code == 0:
            break
    print(f"  ended at offset {pos} (file len {len(data)})")
    if tags and tags[-1][0] != 0:
        print("  WARNING: stream did not end with End(0) tag"); issues.append("no End tag")
    elif not tags:
        print("  WARNING: no tags decoded"); issues.append("no tags")
    if pos != len(data):
        print(f"  NOTE: trailing/garbage? pos={pos} != fileLen={len(data)}")
    print(f"  background RGB: {bg_rgb}")
    print(f"  shape solid fill RGBs: {shape_solid_rgbs}")
    print(f"  ISSUES: {issues if issues else 'none'}")
    print()
    return {
        "ok_magic": data[:3] == b"GFX",
        "ok_version": version == 0x0b,
        "file_length_match": file_length == len(data),
        "tags": tags,
        "issues": issues,
        "ends_with_end": bool(tags) and tags[-1][0] == 0,
        "bg_rgb": bg_rgb,
        "shape_solid_rgbs": shape_solid_rgbs,
    }

def main():
    results = {}
    for spec in sys.argv[1:]:
        label, path = spec.split("=", 1)
        data = open(path, "rb").read()
        results[label] = parse_gfx(data, f"{label} ({path})")
    if "emitted" in results and "original" in results:
        e_codes = [(c, n) for (c, n, l) in results["emitted"]["tags"]]
        o_codes = [(c, n) for (c, n, l) in results["original"]["tags"]]
        print("===== TAG-TREE COMPARISON =====")
        print(f"  emitted  tag codes: {[c for (c, n) in e_codes]}")
        print(f"  original tag codes: {[c for (c, n) in o_codes]}")
        print(f"  emitted  names: {[n for (c, n) in e_codes]}")
        print(f"  original names: {[n for (c, n) in o_codes]}")
        print(f"  SAME ORDERED TAG SET (code+name): {e_codes == o_codes}")

if __name__ == "__main__":
    main()
