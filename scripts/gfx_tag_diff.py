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


def walk_full(b: bytes, off: int, sprite: int, out: list, sprite_full: dict):
    """Like walk_tags but collects (sprite_id_or_None, code, element). For a
    DefineSprite the element is the placeholder ("SPRITE", id) -- compared by
    id during alignment so nested-only changes stay nested -- and the sprite's
    full serialized tag bytes are recorded in sprite_full[id] for parent-level
    remove/replace edits."""
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
            sprite_full[sprite_id] = full
            out.append((sprite, code, ("SPRITE", sprite_id)))
            walk_full(b[: off + hdr + body_len], off + hdr + 4, sprite_id, out, sprite_full)
        else:
            out.append((sprite, code, full))
        off += hdr + body_len
        if code == 0:
            return


def containers(fn: str):
    """(raw_bytes, ordered dict container -> [(code, element)], sprite_full).
    Container is None for the root stream or the sprite id for a DefineSprite
    body; elements are full tag bytes, except DefineSprite placeholders (see
    walk_full)."""
    b = open(fn, "rb").read()
    flat = []
    sprite_full = {}
    walk_full(b, parse_header(b), None, flat, sprite_full)
    by = {}
    for sprite, code, elem in flat:
        by.setdefault(sprite, []).append((code, elem))
    return b, by, sprite_full


def rust_bytes(b: bytes) -> str:
    return "&[" + ", ".join(f"0x{x:02x}" for x in b) + "]"


def emit_rust(a_fn: str, b_fn: str, const_name: str):
    raw_a, ca, sf_a = containers(a_fn)
    raw_b, cb, sf_b = containers(b_fn)

    def elem_bytes(sf: dict, code: int, elem) -> bytes:
        # Resolve a DefineSprite placeholder to the sprite's full tag bytes so a
        # parent-level remove/replace of a whole sprite is expressible.
        if code == 39:
            return sf[elem[1]]
        return elem

    # Sprites removed or wholly replaced at the parent level: their nested
    # streams are covered by the parent edit's old_tag bytes, so they must be
    # skipped as containers (in A), and a replacement that introduces a sprite
    # would appear as a new container in B.
    # Each edit is (sprite, code, old_bytes, new_bytes_or_None, op) where op is
    # one of "Replace" / "Remove" / "InsertAfter".
    edits = []
    parent_edited_a, parent_edited_b = set(), set()

    def anchor_at(sa, sf, idx, sprite):
        """Bytes+code of the vanilla anchor tag at position `idx`; it must be
        UNIQUE in the container so `apply_edits` resolves it unambiguously."""
        code, elem = sa[idx]
        ab = elem_bytes(sf, code, elem)
        n = sum(1 for (c, e) in sa if elem_bytes(sf, c, e) == ab)
        assert n == 1, (
            f"insert anchor at container {sprite} idx {idx} is not unique ({n} matches)"
        )
        return code, ab

    for sprite in ca:
        if sprite is not None and sprite not in cb:
            continue  # covered by a parent-level edit collected below
        sa, sb = ca[sprite], cb.get(sprite, [])
        sm = difflib.SequenceMatcher(a=sa, b=sb, autojunk=False)
        for op, i1, i2, j1, j2 in sm.get_opcodes():
            if op == "equal":
                continue
            if op == "delete":
                for code, elem in sa[i1:i2]:
                    if code == 39:
                        parent_edited_a.add(elem[1])
                    edits.append((sprite, code, elem_bytes(sf_a, code, elem), None, "Remove"))
            elif op == "insert":
                # New tags with no vanilla counterpart: anchor each after the
                # preceding vanilla tag (i1-1) via InsertAfter. Insert-at-front
                # (i1==0) has no anchor tag and is unsupported.
                assert i1 > 0, f"insert at container {sprite} front has no anchor tag"
                acode, abytes = anchor_at(sa, sf_a, i1 - 1, sprite)
                for nc, ne in sb[j1:j2]:
                    if nc == 39:
                        parent_edited_b.add(ne[1])
                    edits.append(
                        (sprite, acode, abytes, elem_bytes(sf_b, nc, ne), "InsertAfter")
                    )
            elif op == "replace":
                olds, news = sa[i1:i2], sb[j1:j2]
                pair = min(len(olds), len(news))
                # Excess olds (a mixed replace+delete run): delete the tail.
                for code, elem in olds[pair:]:
                    if code == 39:
                        parent_edited_a.add(elem[1])
                    edits.append((sprite, code, elem_bytes(sf_a, code, elem), None, "Remove"))
                # Head-to-head replacements. TagEdit.code documents the TARGETED
                # (old) tag; the replacement may be a different tag kind (e.g. a
                # DefineSprite repurposed as a DefineEditText).
                for (oc, oe), (nc, ne) in zip(olds[:pair], news[:pair]):
                    if oc == 39:
                        parent_edited_a.add(oe[1])
                    if nc == 39:
                        parent_edited_b.add(ne[1])
                    edits.append(
                        (sprite, oc, elem_bytes(sf_a, oc, oe), elem_bytes(sf_b, nc, ne), "Replace")
                    )
                # Excess news (a mixed replace+insert run): insert after the last
                # paired vanilla anchor.
                if len(news) > len(olds):
                    assert olds, f"insert-only replace block in container {sprite} has no anchor"
                    acode, abytes = anchor_at(sa, sf_a, i1 + pair - 1, sprite)
                    for nc, ne in news[pair:]:
                        if nc == 39:
                            parent_edited_b.add(ne[1])
                        edits.append(
                            (sprite, acode, abytes, elem_bytes(sf_b, nc, ne), "InsertAfter")
                        )
            else:
                raise AssertionError(f"unsupported opcode {op!r} in container {sprite}")
    dangling_a = {s for s in ca if s is not None and s not in cb} - parent_edited_a
    assert not dangling_a, f"containers vanished without a parent-level edit: {dangling_a}"
    dangling_b = {s for s in cb if s is not None and s not in ca} - parent_edited_b
    assert not dangling_b, f"containers appeared without a parent-level edit: {dangling_b}"

    def hdr_line(name, raw):
        return (
            f"// {name}: len={len(raw)} sha256={hashlib.sha256(raw).hexdigest()}"
        )

    print("// GENERATED by scripts/gfx_tag_diff.py --emit-rust; do not hand-edit.")
    print(f"// python3 scripts/gfx_tag_diff.py {a_fn} {b_fn} --emit-rust {const_name}")
    print(hdr_line("A (vanilla)", raw_a))
    print(hdr_line("B (edited) ", raw_b))
    print(f"pub const {const_name}: &[TagEdit] = &[")
    for sprite, code, old, new, op in edits:
        sp = "None" if sprite is None else f"Some({sprite})"
        nt = "None" if new is None else f"Some({rust_bytes(new)})"
        print("    TagEdit {")
        print(f"        sprite_id: {sp},")
        print(f"        code: {code},")
        print(f"        old_tag: {rust_bytes(old)},")
        print(f"        new_tag: {nt},")
        print(f"        op: EditOp::{op},")
        print("    },")
    print("];")
    n_rm = sum(1 for e in edits if e[4] == "Remove")
    n_ins = sum(1 for e in edits if e[4] == "InsertAfter")
    n_rep = sum(1 for e in edits if e[4] == "Replace")
    print(
        f"// {len(edits)} edits: {n_rm} removals, {n_rep} replacements, {n_ins} insertions.",
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
