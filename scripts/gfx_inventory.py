#!/usr/bin/env python3
"""GFX (Scaleform, SWF-derived) tag-walker / feature inventory.

Walks uncompressed "GFX" version 0x0b files: verifies magic + version, reads
FileLength, skips the bit-packed movie RECT + frameRate + frameCount, then walks
the tag stream (u16 LE header: code = word>>6, len = word & 0x3f, long u32 length
when len == 0x3f), tallying per-code file_count and total_occurrences and checking
that each file ends cleanly (End tag, FileLength == filesize, no trailing garbage).
Pure python3 stdlib only.
"""
import sys, os, glob, json, struct

# ---- Tag-code -> name map -------------------------------------------------
TAG_NAMES = {
    0: "End", 1: "ShowFrame", 2: "DefineShape", 3: "FreeCharacter",
    4: "PlaceObject", 5: "RemoveObject", 6: "DefineBits", 7: "DefineButton",
    8: "JPEGTables", 9: "SetBackgroundColor", 10: "DefineFont", 11: "DefineText",
    12: "DoAction", 13: "DefineFontInfo", 14: "DefineSound", 15: "StartSound",
    17: "DefineButtonSound", 18: "SoundStreamHead", 19: "SoundStreamBlock",
    20: "DefineBitsLossless", 21: "DefineBitsJPEG2", 22: "DefineShape2",
    23: "DefineButtonCxform", 24: "Protect", 26: "PlaceObject2",
    28: "RemoveObject2", 32: "DefineShape3", 33: "DefineText2",
    34: "DefineButton2", 35: "DefineBitsJPEG3", 36: "DefineBitsLossless2",
    37: "DefineEditText", 39: "DefineSprite", 40: "NameCharacter",
    41: "ProductInfo", 43: "FrameLabel", 45: "SoundStreamHead2",
    46: "DefineMorphShape", 48: "DefineFont2", 56: "ExportAssets",
    57: "ImportAssets", 58: "EnableDebugger", 59: "DoInitAction",
    60: "DefineVideoStream", 61: "VideoFrame", 62: "DefineFontInfo2",
    64: "EnableDebugger2", 65: "ScriptLimits", 66: "SetTabIndex",
    69: "FileAttributes", 70: "PlaceObject3", 71: "ImportAssets2",
    73: "DefineFontAlignZones", 74: "CSMTextSettings", 75: "DefineFont3",
    76: "SymbolClass", 77: "Metadata", 78: "DefineScalingGrid",
    82: "DoABC", 83: "DefineShape4", 84: "DefineMorphShape2",
    86: "DefineSceneAndFrameLabelData", 87: "DefineBinaryData",
    88: "DefineFontName", 89: "StartSound2", 90: "DefineBitsJPEG4",
    91: "DefineFont4",
    # ---- Scaleform GFx extension tags (>= 1000) ----
    1000: "GFX_ExporterInfo", 1001: "GFX_DefineExternalImage",
    1002: "GFX_FontTextureInfo", 1003: "GFX_DefineExternalGradient",
    1004: "GFX_DefineSubImage", 1005: "GFX_Empty",
    1006: "GFX_DefineExternalSound", 1007: "GFX_DefineExternalStreamSound",
    1008: "GFX_DefineSubImageAlt", 1009: "GFX_DefineExternalImage2",
    1010: "GFX_FontTextureInfo2",
}

def tag_name(code):
    if code in TAG_NAMES:
        return TAG_NAMES[code]
    return "UNKNOWN-%d" % code


class BitReader:
    def __init__(self, data, pos=0):
        self.data = data
        self.bytepos = pos
        self.bitpos = 0
    def read_ub(self, n):
        v = 0
        for _ in range(n):
            byte = self.data[self.bytepos]
            bit = (byte >> (7 - self.bitpos)) & 1
            v = (v << 1) | bit
            self.bitpos += 1
            if self.bitpos == 8:
                self.bitpos = 0
                self.bytepos += 1
        return v
    def byte_align(self):
        if self.bitpos != 0:
            self.bitpos = 0
            self.bytepos += 1


def walk_file(path):
    """Return dict: ok, error, header info, list of (code,len,bodystart) tags."""
    with open(path, "rb") as fh:
        data = fh.read()
    fsize = len(data)
    res = {
        "path": path, "name": os.path.basename(path), "filesize": fsize,
        "ok": True, "errors": [], "tags": [],
    }
    if fsize < 8:
        res["ok"] = False; res["errors"].append("too small"); return res
    sig = data[0:3]; ver = data[3]
    res["sig"] = sig.decode("latin1"); res["version"] = ver
    if sig != b"GFX":
        res["ok"] = False; res["errors"].append("bad magic %r" % sig)
    if ver != 0x0b:
        res["ok"] = False; res["errors"].append("bad version 0x%02x" % ver)
    filelen = struct.unpack_from("<I", data, 4)[0]
    res["filelength_field"] = filelen
    if filelen != fsize:
        res["ok"] = False
        res["errors"].append("FileLength %d != filesize %d" % (filelen, fsize))

    # Body begins at byte 8: bit-packed RECT, then frameRate u16, frameCount u16.
    br = BitReader(data, 8)
    nbits = br.read_ub(5)
    res["rect_nbits"] = nbits
    for _ in range(4):
        br.read_ub(nbits)  # xmin,xmax,ymin,ymax
    br.byte_align()
    pos = br.bytepos
    if pos + 4 > fsize:
        res["ok"] = False; res["errors"].append("truncated header"); return res
    framerate = struct.unpack_from("<H", data, pos)[0]; pos += 2
    framecount = struct.unpack_from("<H", data, pos)[0]; pos += 2
    res["framerate"] = framerate
    res["framecount"] = framecount

    # Walk tag stream.
    saw_end = False
    while pos + 2 <= fsize:
        word = struct.unpack_from("<H", data, pos)[0]; pos += 2
        code = word >> 6
        length = word & 0x3f
        is_long = (length == 0x3f)
        if is_long:
            if pos + 4 > fsize:
                res["ok"] = False; res["errors"].append("truncated long-len header"); break
            length = struct.unpack_from("<I", data, pos)[0]; pos += 4
        body_start = pos
        if code == 0:  # End tag
            res["tags"].append((code, length, body_start, is_long))
            saw_end = True
            # Anything after End (besides nothing) is trailing garbage.
            trailing = fsize - pos
            if trailing != 0:
                res["ok"] = False
                res["errors"].append("%d trailing bytes after End" % trailing)
            break
        if body_start + length > fsize:
            res["ok"] = False
            res["errors"].append(
                "tag code=%d len=%d overruns file at off=%d" % (code, length, body_start))
            break
        res["tags"].append((code, length, body_start, is_long))
        pos += length
    if not saw_end:
        res["ok"] = False; res["errors"].append("no End tag")
    return res


def main():
    pat = sys.argv[1] if len(sys.argv) > 1 else \
        "/home/banon/er-extract/nuxe-menu-20260619-170932/menu/*.gfx"
    out_json = sys.argv[2] if len(sys.argv) > 2 else None
    files = sorted(glob.glob(pat))
    code_filecount = {}     # code -> set of files
    code_total = {}         # code -> total occurrences
    per_file = []
    not_ok = []
    for f in files:
        r = walk_file(f)
        per_file.append(r)
        if not r["ok"]:
            not_ok.append((r["name"], r["errors"]))
        seen = set()
        for (code, length, bs, il) in r["tags"]:
            code_total[code] = code_total.get(code, 0) + 1
            seen.add(code)
        for code in seen:
            code_filecount.setdefault(code, set()).add(f)

    print("=== FILES: %d ===" % len(files))
    print("all_ok:", len(not_ok) == 0)
    if not_ok:
        print("NOT OK FILES:")
        for nm, errs in not_ok:
            print("  ", nm, errs)

    print("\n=== TAG INVENTORY (code: name  file_count  total_occurrences) ===")
    rows = []
    for code in sorted(code_total):
        rows.append((code, tag_name(code), len(code_filecount[code]), code_total[code]))
        print("  %5d  %-32s files=%-4d total=%d" % (
            code, tag_name(code), len(code_filecount[code]), code_total[code]))

    out = {
        "num_files": len(files),
        "all_ok": len(not_ok) == 0,
        "not_ok": [{"name": nm, "errors": e} for nm, e in not_ok],
        "tags": [
            {"code": c, "name": n, "file_count": fc, "total_occurrences": tot}
            for (c, n, fc, tot) in rows
        ],
        "files": [
            {"name": r["name"], "filesize": r["filesize"],
             "filelength_field": r.get("filelength_field"),
             "version": r.get("version"), "ok": r["ok"],
             "framecount": r.get("framecount"),
             "ntags": len(r["tags"]),
             "tag_codes": sorted(set(c for (c, l, b, il) in r["tags"]))}
            for r in per_file
        ],
    }
    if out_json:
        with open(out_json, "w") as fh:
            json.dump(out, fh, indent=1)
        print("\nJSON ->", out_json)


if __name__ == "__main__":
    main()
