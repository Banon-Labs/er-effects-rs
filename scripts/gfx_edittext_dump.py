#!/usr/bin/env python3
"""Dump every DefineEditText (SWF tag 37) in a GFX/SWF movie, plus the
PlaceObject2/3 instance names that reference each character id.

Answers the concrete questions a Scaleform text-truncation investigation asks:
per-field bounds (in twips and px), WordWrap/Multiline/HTML/AutoSize flags,
FontHeight, and MaxLength (which the runtime honours as a hard SetText clamp).

Usage: python3 scripts/gfx_edittext_dump.py <movie.gfx> [<movie2.gfx> ...]
"""
import struct
import sys

TWIPS = 20.0


class Bits:
    def __init__(self, data, pos=0):
        self.d = data
        self.p = pos
        self.bit = 0

    def align(self):
        if self.bit:
            self.bit = 0
            self.p += 1

    def ub(self, n):
        v = 0
        for _ in range(n):
            byte = self.d[self.p]
            v = (v << 1) | ((byte >> (7 - self.bit)) & 1)
            self.bit += 1
            if self.bit == 8:
                self.bit = 0
                self.p += 1
        return v

    def sb(self, n):
        v = self.ub(n)
        if n and (v >> (n - 1)) & 1:
            v -= 1 << n
        return v

    def rect(self):
        self.align()
        nb = self.ub(5)
        return (self.sb(nb), self.sb(nb), self.sb(nb), self.sb(nb), nb)

    def u8(self):
        self.align()
        v = self.d[self.p]
        self.p += 1
        return v

    def u16(self):
        self.align()
        v = struct.unpack_from("<H", self.d, self.p)[0]
        self.p += 2
        return v

    def s16(self):
        self.align()
        v = struct.unpack_from("<h", self.d, self.p)[0]
        self.p += 2
        return v

    def u32(self):
        self.align()
        v = struct.unpack_from("<I", self.d, self.p)[0]
        self.p += 4
        return v

    def cstr(self):
        self.align()
        e = self.d.index(b"\0", self.p)
        s = self.d[self.p:e]
        self.p = e + 1
        return s.decode("utf-8", "replace")


def tags(data, start, end):
    p = start
    while p + 2 <= end:
        (th,) = struct.unpack_from("<H", data, p)
        p += 2
        code = th >> 6
        ln = th & 0x3F
        if ln == 0x3F:
            (ln,) = struct.unpack_from("<I", data, p)
            p += 4
        body = data[p:p + ln]
        yield code, body, p
        p += ln
        if code == 0:
            return


def parse_edittext(body):
    b = Bits(body)
    cid = b.u16()
    x0, x1, y0, y1, nb = b.rect()
    f1 = b.u8()
    f2 = b.u8()
    flags = {
        "HasText": bool(f1 & 0x80),
        "WordWrap": bool(f1 & 0x40),
        "Multiline": bool(f1 & 0x20),
        "Password": bool(f1 & 0x10),
        "ReadOnly": bool(f1 & 0x08),
        "HasTextColor": bool(f1 & 0x04),
        "HasMaxLength": bool(f1 & 0x02),
        "HasFont": bool(f1 & 0x01),
        "HasFontClass": bool(f2 & 0x80),
        "AutoSize": bool(f2 & 0x40),
        "HasLayout": bool(f2 & 0x20),
        "NoSelect": bool(f2 & 0x10),
        "Border": bool(f2 & 0x08),
        "WasStatic": bool(f2 & 0x04),
        "HTML": bool(f2 & 0x02),
        "UseOutlines": bool(f2 & 0x01),
    }
    out = {
        "cid": cid,
        "bounds_twips": (x0, x1, y0, y1),
        "rect_nbits": nb,
        "width_twips": x1 - x0,
        "height_twips": y1 - y0,
        "width_px": (x1 - x0) / TWIPS,
        "height_px": (y1 - y0) / TWIPS,
        "flags": flags,
    }
    if flags["HasFont"]:
        out["FontID"] = b.u16()
    if flags["HasFontClass"]:
        out["FontClass"] = b.cstr()
    if flags["HasFont"]:
        out["FontHeight_twips"] = b.u16()
        out["FontHeight_px"] = out["FontHeight_twips"] / TWIPS
    if flags["HasTextColor"]:
        out["Color"] = tuple(b.u8() for _ in range(4))
    out["MaxLength"] = b.u16() if flags["HasMaxLength"] else None
    if flags["HasLayout"]:
        out["Align"] = b.u8()
        out["LeftMargin"] = b.u16()
        out["RightMargin"] = b.u16()
        out["Indent"] = b.u16()
        out["Leading"] = b.s16()
    try:
        out["VariableName"] = b.cstr()
        out["InitialText"] = b.cstr() if flags["HasText"] else None
    except Exception as exc:  # trailing Scaleform extension bytes
        out["tail_error"] = repr(exc)
    out["tail_bytes"] = len(body) - b.p
    return out


def place_names(body, code):
    """Return (name, character_id) for PlaceObject2 (26) / PlaceObject3 (70)."""
    b = Bits(body)
    f = b.u8()
    f2 = b.u8() if code == 70 else 0
    b.u16()  # depth
    if code == 70 and (f2 & 0x10):
        b.cstr()  # class name
    cid = b.u16() if (f & 0x02) else None
    if f & 0x04:  # matrix
        b.align()
        has = b.ub(1)
        if has:
            n = b.ub(5)
            b.sb(n)
            b.sb(n)
        has = b.ub(1)
        if has:
            n = b.ub(5)
            b.sb(n)
            b.sb(n)
        n = b.ub(5)
        b.sb(n)
        b.sb(n)
        b.align()
    if f & 0x08:  # CXFORMWITHALPHA
        # HasAddTerms UB[1], HasMultTerms UB[1], Nbits UB[4], then FOUR mult terms
        # (RGBA) if HasMultTerms and FOUR add terms if HasAddTerms -- the layout the
        # crate's own codec reads (crates/er-gfx/src/codec/shape/primitives.rs,
        # CxformWithAlpha::read, round-trip tested against the corpus).
        #
        # This used to consume ONE flag bit and pick 3-vs-4 terms from it, so every tag
        # carrying a color transform desynced the reader by a few bits and the instance
        # NAME that follows was read mid-string: the alpha-zeroed PlayTime placement in
        # the edited 05_010 movie dumped as 'ime'. A name that silently loses its first
        # characters reads as a renamed field, which is a different bug entirely.
        b.align()
        has_add = b.ub(1)
        has_mult = b.ub(1)
        n = b.ub(4)
        if has_mult:
            for _ in range(4):
                b.sb(n)
        if has_add:
            for _ in range(4):
                b.sb(n)
        b.align()
    if f & 0x10:
        b.u16()  # ratio
    name = b.cstr() if (f & 0x20) else None
    return name, cid


def walk(data, start, end, sprite, out_fields, out_places):
    for code, body, off in tags(data, start, end):
        if code == 37:
            try:
                rec = parse_edittext(body)
            except Exception as exc:
                rec = {"parse_error": repr(exc)}
            rec["sprite"] = sprite
            rec["file_off"] = off
            out_fields.append(rec)
        elif code in (26, 70):
            try:
                name, cid = place_names(body, code)
            except Exception:
                name, cid = None, None
            if name is not None and cid is not None:
                out_places.setdefault(cid, set()).add((sprite, name))
        elif code == 39:  # DefineSprite
            sid = struct.unpack_from("<H", body, 0)[0]
            walk(data, off + 4, off + len(body), sid, out_fields, out_places)


def main():
    for path in sys.argv[1:]:
        data = open(path, "rb").read()
        sig = data[:3]
        ver = data[3]
        (flen,) = struct.unpack_from("<I", data, 4)
        print(f"== {path}  sig={sig!r} ver={ver} declared_len={flen} actual={len(data)}")
        if sig in (b"CFX", b"CWS"):
            print("   COMPRESSED movie -- decompress first; not handled here")
            continue
        b = Bits(data, 8)
        b.rect()
        b.u16()  # frame rate (8.8)
        b.u16()  # frame count
        fields, places = [], {}
        walk(data, b.p, len(data), None, fields, places)
        for r in fields:
            names = sorted(places.get(r.get("cid"), []))
            print(f"  DefineEditText cid={r.get('cid')} sprite={r.get('sprite')} off=0x{r.get('file_off'):x}")
            print(f"    instances: {names}")
            print(f"    bounds_twips={r.get('bounds_twips')} nbits={r.get('rect_nbits')} "
                  f"w={r.get('width_twips')}tw ({r.get('width_px')}px) "
                  f"h={r.get('height_twips')}tw ({r.get('height_px')}px)")
            fl = r.get("flags", {})
            print("    flags: " + " ".join(k for k, v in fl.items() if v))
            print(f"    FontID={r.get('FontID')} FontHeight={r.get('FontHeight_twips')}tw "
                  f"({r.get('FontHeight_px')}px) MaxLength={r.get('MaxLength')} "
                  f"Align={r.get('Align')} LeftMargin={r.get('LeftMargin')} "
                  f"RightMargin={r.get('RightMargin')} Indent={r.get('Indent')} "
                  f"Leading={r.get('Leading')}")
            print(f"    VariableName={r.get('VariableName')!r} InitialText={r.get('InitialText')!r} "
                  f"tail_bytes={r.get('tail_bytes')} {r.get('tail_error','')}")


main()
