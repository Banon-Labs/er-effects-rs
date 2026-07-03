#!/usr/bin/env python3
"""Tag-level unified diff of two uncompressed GFX movies.

Walks both tag streams recursively (descending into DefineSprite(39) bodies),
renders each tag as one line -- sprite path, tag name/code, short/long header
form, body length, body sha1 prefix -- and prints a unified diff. Identical
bodies at the same position hash equal, so the diff shows exactly which tags a
transform removed/changed/inserted and whether surviving tag BYTES are
verbatim-identical (same sha) or re-encoded (different sha at same position).

With --emit-rust, instead of a diff it emits a Rust `TagEdit` table (the
`crates/er-gfx` structured-edit format): for each container (root stream or a
DefineSprite body) the two tag sequences are aligned with SequenceMatcher and
every delete becomes `new_tag: None`, every replace pairs old/new full
serialized tag bytes (RecordHeader included). Inserts are rejected -- the edit
model is remove/replace only. This is the deterministic generator for
`crates/er-gfx/src/title_05_000_edits.rs` (er-effects-rs-h7x).

Usage:
  python3 scripts/gfx_tag_diff.py A.gfx B.gfx
  python3 scripts/gfx_tag_diff.py vanilla.gfx edited.gfx --emit-rust CONST_NAME
"""

import difflib
import hashlib
import sys


def parse_header(b: bytes):
    assert b[:3] == b"GFX", f"not an uncompressed GFX: magic={b[:4]!r}"
    nbits = b[8] >> 3
    rect_bytes = (5 + 4 * nbits + 7) // 8
    off = 8 + rect_bytes
    return off + 4  # skip frameRate u16 + frameCount u16


def walk_tags(b: bytes, off: int, path: list, out: list):
    while off < len(b):
        word = int.from_bytes(b[off : off + 2], "little")
        code = word >> 6
        short_len = word & 0x3F
        if short_len == 0x3F:
            body_len = int.from_bytes(b[off + 2 : off + 6], "little")
            hdr = 6
            long_form = True
        else:
            body_len = short_len
            hdr = 2
            long_form = False
        body = b[off + hdr : off + hdr + body_len]
        if code == 39:  # DefineSprite: recurse into the nested tag stream
            sprite_id = int.from_bytes(body[:2], "little")
            frames = int.from_bytes(body[2:4], "little")
            out.append((path, code, long_form, f"id={sprite_id} frames={frames}"))
            walk_tags(b[: off + hdr + body_len], off + hdr + 4, path + [f"sprite{sprite_id}"], out)
        else:
            h = hashlib.sha1(body).hexdigest()[:10]
            out.append((path, code, long_form, f"len={body_len} sha={h}"))
        off += hdr + body_len
        if code == 0:
            return


NAMES = {
    0: "End", 1: "ShowFrame", 2: "DefineShape", 9: "SetBgColor", 12: "DoAction",
    22: "DefineShape2", 26: "PlaceObject2", 28: "RemoveObject2", 32: "DefineShape3",
    36: "DefineBitsLossless2", 37: "DefineEditText", 39: "DefineSprite",
    43: "FrameLabel", 56: "ExportAssets", 69: "FileAttributes", 70: "PlaceObject3",
    71: "ImportAssets2", 74: "CSMTextSettings", 75: "DefineFont3", 76: "SymbolClass",
    77: "Metadata", 78: "DefineScalingGrid", 82: "DoABC", 83: "DefineShape4",
    86: "DefineSceneAndFrameLabelData", 88: "DefineFontName",
    1000: "ExporterInfo", 1001: "DefineExternalImage", 1002: "FontTextureInfo",
    1004: "DefineExternalImage2", 1005: "DefineSubImage",
}


def lines(fn: str):
    b = open(fn, "rb").read()
    out = []
    walk_tags(b, parse_header(b), [], out)
    res = []
    for path, code, lf, desc in out:
        p = "/".join(path) or "root"
        nm = NAMES.get(code, f"tag{code}")
        res.append(f"{p} {nm}({code}){'L' if lf else 's'} {desc}")
    return res


def walk_full(b: bytes, off: int, sprite: int, out: list):
    """Like walk_tags but collects (sprite_id_or_None, code, full_tag_bytes)."""
    while off < len(b):
        word = int.from_bytes(b[off : off + 2], "little")
        code = word >> 6
        short_len = word & 0x3F
        if short_len == 0x3F:
            body_len = int.from_bytes(b[off + 2 : off + 6], "little")
            hdr = 6
        else:
            body_len = short_len
            hdr = 2
        full = b[off : off + hdr + body_len]
        if code == 39:
            sprite_id = int.from_bytes(b[off + hdr : off + hdr + 2], "little")
            walk_full(b[: off + hdr + body_len], off + hdr + 4, sprite_id, out)
        else:
            out.append((sprite, code, full))
        off += hdr + body_len
        if code == 0:
            return


def containers(fn: str):
    """Ordered dict container -> [(code, full_tag_bytes)]. Container is None for
    the root stream or the sprite id for a DefineSprite body."""
    b = open(fn, "rb").read()
    flat = []
    walk_full(b, parse_header(b), None, flat)
    by = {}
    for sprite, code, full in flat:
        by.setdefault(sprite, []).append((code, full))
    return b, by


def rust_bytes(b: bytes) -> str:
    return "&[" + ", ".join(f"0x{x:02x}" for x in b) + "]"


def emit_rust(a_fn: str, b_fn: str, const_name: str):
    raw_a, ca = containers(a_fn)
    raw_b, cb = containers(b_fn)
    assert set(ca) == set(cb), f"container sets differ: {set(ca) ^ set(cb)}"

    edits = []
    for sprite in ca:
        sa, sb = ca[sprite], cb[sprite]
        sm = difflib.SequenceMatcher(a=sa, b=sb, autojunk=False)
        for op, i1, i2, j1, j2 in sm.get_opcodes():
            if op == "equal":
                continue
            if op == "delete":
                for code, full in sa[i1:i2]:
                    edits.append((sprite, code, full, None))
            elif op == "replace":
                olds, news = sa[i1:i2], sb[j1:j2]
                assert len(olds) == len(news), (
                    f"unpaired replace in container {sprite}: {len(olds)} old vs {len(news)} new"
                )
                for (oc, of), (nc, nf) in zip(olds, news):
                    assert oc == nc, f"replace changes tag code {oc}->{nc} in container {sprite}"
                    edits.append((sprite, oc, of, nf))
            else:
                raise AssertionError(f"unsupported opcode {op!r} in container {sprite} (insert?)")

    def hdr_line(name, raw):
        return (
            f"// {name}: len={len(raw)} sha256={hashlib.sha256(raw).hexdigest()}"
        )

    print("// GENERATED by scripts/gfx_tag_diff.py --emit-rust; do not hand-edit.")
    print(f"// python3 scripts/gfx_tag_diff.py {a_fn} {b_fn} --emit-rust {const_name}")
    print(hdr_line("A (vanilla)", raw_a))
    print(hdr_line("B (edited) ", raw_b))
    print(f"pub const {const_name}: &[TagEdit] = &[")
    for sprite, code, old, new in edits:
        sp = "None" if sprite is None else f"Some({sprite})"
        nt = "None" if new is None else f"Some({rust_bytes(new)})"
        print("    TagEdit {")
        print(f"        sprite_id: {sp},")
        print(f"        code: {code},")
        print(f"        old_tag: {rust_bytes(old)},")
        print(f"        new_tag: {nt},")
        print("    },")
    print("];")
    n_rm = sum(1 for e in edits if e[3] is None)
    print(
        f"// {len(edits)} edits: {n_rm} removals, {len(edits) - n_rm} replacements.",
    )


def main():
    a_fn, b_fn = sys.argv[1], sys.argv[2]
    if len(sys.argv) > 3 and sys.argv[3] == "--emit-rust":
        emit_rust(a_fn, b_fn, sys.argv[4] if len(sys.argv) > 4 else "TITLE_05_000_STRIP_EDITS")
        return
    a, b = lines(a_fn), lines(b_fn)
    print(f"# A={a_fn} tags={len(a)}")
    print(f"# B={b_fn} tags={len(b)}")
    n_diff = 0
    for line in difflib.unified_diff(a, b, "A", "B", lineterm="", n=1):
        print(line)
        if line.startswith(("+", "-")) and not line.startswith(("+++", "---")):
            n_diff += 1
    print(f"# diff lines: {n_diff}")


if __name__ == "__main__":
    main()
