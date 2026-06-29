#!/usr/bin/env python3
"""Deep feature probe for GFX corpus: external-image (tag 1009) string refs +
shape fill-style classification (solid / gradient / bitmap) by actually parsing
FILLSTYLEARRAY bit/byte structures. Pure stdlib.
"""
import sys, os, glob, struct, json, re

def rects_nbits_skip(data, pos):
    """Skip one bit-packed RECT starting at byte pos (bit 0). Return new byte pos
    (byte-aligned)."""
    br = Bits(data, pos)
    nb = br.ub(5)
    for _ in range(4):
        br.ub(nb)
    br.align()
    return br.pos

class Bits:
    def __init__(self, data, pos):
        self.data = data; self.pos = pos; self.bit = 0
    def ub(self, n):
        v = 0
        for _ in range(n):
            b = (self.data[self.pos] >> (7 - self.bit)) & 1
            v = (v << 1) | b
            self.bit += 1
            if self.bit == 8:
                self.bit = 0; self.pos += 1
        return v
    def align(self):
        if self.bit:
            self.bit = 0; self.pos += 1

def skip_matrix(data, pos):
    br = Bits(data, pos)
    if br.ub(1):                 # HasScale
        n = br.ub(5); br.ub(n); br.ub(n)
    if br.ub(1):                 # HasRotate
        n = br.ub(5); br.ub(n); br.ub(n)
    n = br.ub(5); br.ub(n); br.ub(n)   # Translate
    br.align()
    return br.pos

def parse_fillstyles(data, pos, alpha, shape4):
    """Parse FILLSTYLEARRAY at byte pos. alpha=True -> RGBA colors.
    Returns (newpos, types_list) or (None, []) on parse failure."""
    types = []
    try:
        count = data[pos]; pos += 1
        if count == 0xFF:
            count = struct.unpack_from("<H", data, pos)[0]; pos += 2
        for _ in range(count):
            t = data[pos]; pos += 1
            types.append(t)
            if t == 0x00:        # solid
                pos += 4 if alpha else 3
            elif t in (0x10, 0x12, 0x13):   # gradient (linear/radial/focal)
                pos = skip_matrix(data, pos)
                info = data[pos]; pos += 1
                numg = info & 0x0F
                for _ in range(numg):
                    pos += 1                 # ratio
                    pos += 4 if alpha else 3 # color
                if t == 0x13:
                    pos += 2                 # focal point FIXED8
            elif t in (0x40, 0x41, 0x42, 0x43):  # bitmap fill
                pos += 2                     # bitmap id
                pos = skip_matrix(data, pos)
            else:
                return None, types           # unknown fill type -> bail
    except (IndexError, struct.error):
        return None, types
    return pos, types

def shape_fill_types(data, bodystart, length, code):
    """Return list of fill-style type bytes found in a DefineShape* tag body."""
    pos = bodystart
    pos += 2                                  # ShapeId u16
    pos = rects_nbits_skip(data, pos)         # ShapeBounds RECT
    shape4 = (code == 83)
    if shape4:
        pos = rects_nbits_skip(data, pos)     # EdgeBounds RECT
        pos += 1                              # flags byte (UsesFillWindingRule etc.)
    alpha = code in (32, 83)                  # Shape3/Shape4 use RGBA
    newpos, types = parse_fillstyles(data, pos, alpha, shape4)
    return types

def ascii_strings(b, minlen=4):
    return re.findall(rb"[\x20-\x7e]{%d,}" % minlen, b)

def main():
    pat = "/home/banon/er-extract/nuxe-menu-20260619-170932/menu/*.gfx"
    files = sorted(glob.glob(pat))
    # tag header walk (reuse simple logic)
    grad_files = set(); bitmapfill_files = set()
    grad_type_counts = {}
    img_refs = {}          # filename ext -> count
    sample_img_strings = []
    title_logo_strings = None
    for f in files:
        with open(f, "rb") as fh:
            data = fh.read()
        fsize = len(data)
        # skip header
        br = Bits(data, 8); nb = br.ub(5)
        for _ in range(4): br.ub(nb)
        br.align(); pos = br.pos + 4   # +frameRate u16 +frameCount u16
        while pos + 2 <= fsize:
            word = struct.unpack_from("<H", data, pos)[0]; pos += 2
            code = word >> 6; length = word & 0x3f
            if length == 0x3f:
                length = struct.unpack_from("<I", data, pos)[0]; pos += 4
            bs = pos
            if code == 0:
                break
            if bs + length > fsize:
                break
            if code in (2, 22, 32, 83):
                types = shape_fill_types(data, bs, length, code)
                for t in types:
                    if t in (0x10, 0x12, 0x13):
                        grad_files.add(os.path.basename(f))
                        grad_type_counts[t] = grad_type_counts.get(t, 0) + 1
                    elif t in (0x40, 0x41, 0x42, 0x43):
                        bitmapfill_files.add(os.path.basename(f))
            if code == 1009:
                body = data[bs:bs+length]
                for s in ascii_strings(body):
                    sd = s.decode("latin1")
                    m = re.search(r"\.([A-Za-z0-9]{2,4})$", sd)
                    if m:
                        img_refs[m.group(1).lower()] = img_refs.get(m.group(1).lower(), 0) + 1
                    if len(sample_img_strings) < 12:
                        sample_img_strings.append((os.path.basename(f), sd))
            pos = bs + length
        if os.path.basename(f) == "05_001_title_logo.gfx":
            # collect all strings of 1009 tags already captured in sample
            pass

    print("=== GRADIENT FILL STYLES IN SHAPES ===")
    print("files with gradient fills:", len(grad_files))
    print("  ", sorted(grad_files)[:30])
    names = {0x10: "linear", 0x12: "radial", 0x13: "focal-radial"}
    for t, c in sorted(grad_type_counts.items()):
        print("  type 0x%02x %-12s count=%d" % (t, names.get(t, "?"), c))
    print("\n=== BITMAP FILL STYLES (0x40-0x43) IN SHAPES ===")
    print("files with bitmap fills:", len(bitmapfill_files))
    print("  ", sorted(bitmapfill_files)[:40])
    print("\n=== EXTERNAL IMAGE (tag 1009) REFERENCED EXTENSIONS ===")
    print("  ", img_refs)
    print("\n=== SAMPLE 1009 STRINGS ===")
    for nm, s in sample_img_strings:
        print("  %-28s %r" % (nm, s))

if __name__ == "__main__":
    main()
