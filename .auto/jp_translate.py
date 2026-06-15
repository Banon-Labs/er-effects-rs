#!/usr/bin/env python3
"""Decode + translate FromSoft (shift-JIS / UTF-16LE) debug strings to English.

The game's assert/debug strings are shift-JIS or UTF-16LE Japanese; recon_strings
dumps them as mojibake. This tool:
  1. Resolves a VA in eldenring.exe and decodes it with the correct codec.
  2. Translates the Japanese to English via a curated FD4/FromSoft engine-debug
     glossary (offline, deterministic). Terms outside the glossary are left
     marked [JP:...] so nothing is silently dropped.

Robust-for-our-domain by design: the debug/assert vocabulary is finite. For
arbitrary Japanese, plug an offline NMT (argostranslate) or an online API into
translate_jp() -- see NMT_BACKEND below.

Usage:
  python3 .auto/jp_translate.py va <hex_va> [hex_size]   # decode+translate a VA
  python3 .auto/jp_translate.py text "<japanese>"        # translate raw text
  python3 .auto/jp_translate.py crashlog <crash.log>     # annotate ASSERT lines
"""
from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import static_re_export as sre  # noqa: E402

EXE = sre.DEFAULT_EXE
IMAGE_BASE_DEFAULT = 0x140000000

# Curated FD4 / FromSoft engine-debug glossary (longest match first). Covers the
# assert/log vocabulary that actually appears in the crash logger output.
GLOSSARY: list[tuple[str, str]] = [
    ("未初期化のシングルトンにアクセスしました", "accessed an uninitialized singleton"),
    ("シングルトンにアクセスしました", "accessed the (null) singleton"),
    ("シングルトンが作成されていません", "singleton is not created"),
    ("シングルトンが既に作成されています", "singleton is already created"),
    ("未初期化", "uninitialized"),
    ("にアクセスしました", " was accessed"),
    ("作成されていません", "is not created"),
    ("初期化されていません", "is not initialized"),
    ("存在しません", "does not exist"),
    ("見つかりません", "not found"),
    ("読み込めません", "cannot load"),
    ("できません", "cannot"),
    ("しました", " done"),
    ("されました", " was done"),
    ("する前に", "before "),
    ("失敗", "failure"),
    ("成功", "success"),
    ("シングルトン", "singleton"),
    ("アクセス", "access"),
    ("インスタンス", "instance"),
    ("初期化", "initialization"),
    ("読み込み", "load"),
    ("読込", "load"),
    ("保存", "save"),
    ("セーブ", "save"),
    ("ロード", "load"),
    ("データ", "data"),
    ("ファイル", "file"),
    ("メモリ", "memory"),
    ("確保", "allocate"),
    ("解放", "free"),
    ("リスト", "list"),
    ("長すぎ", "too long"),
    ("範囲外", "out of range"),
    ("範囲", "range"),
    ("不正", "invalid"),
    ("無効", "invalid"),
    ("タスク", "task"),
    ("ステップ", "step"),
    ("状態", "state"),
    ("番号", "number"),
    ("取得", "get"),
    ("設定", "set"),
    ("作成", "create"),
    ("既に", "already"),
    ("登録", "register"),
    ("生成", "generate"),
    ("破棄", "destroy"),
    ("終了", "finish"),
    ("開始", "start"),
    ("待機", "wait"),
    ("処理", "process"),
    ("関数", "function"),
    ("引数", "argument"),
    ("型", "type"),
    ("値", "value"),
    ("数", "count"),
    ("が", " (subj) "),
    ("を", " (obj) "),
    ("に", " (to) "),
    ("は", " (topic) "),
    ("の", " of "),
    ("と", " and "),
    ("で", " by "),
    ("、", ", "),
    ("。", ". "),
    ("：", ": "),
    ("　", " "),
]

# Optional NMT backend for arbitrary Japanese (offline). None until installed.
NMT_BACKEND = None
try:  # pragma: no cover - optional dependency
    import argostranslate.translate as _argos  # type: ignore

    def NMT_BACKEND(text: str) -> str | None:  # type: ignore[misc]
        return _argos.translate(text, "ja", "en")
except Exception:
    NMT_BACKEND = None


def is_japanese(ch: str) -> bool:
    code = ord(ch)
    return (
        0x3040 <= code <= 0x30FF  # hiragana + katakana
        or 0x4E00 <= code <= 0x9FFF  # CJK ideographs
        or 0xFF00 <= code <= 0xFFEF  # fullwidth forms
        or code in (0x3001, 0x3002, 0x30FB)
    )


def translate_jp(text: str) -> str:
    """Glossary translate, longest-match first; mark leftover JP runs."""
    out: list[str] = []
    i = 0
    pending_jp: list[str] = []

    def flush_pending() -> None:
        if pending_jp:
            run = "".join(pending_jp)
            if NMT_BACKEND is not None:
                try:
                    nmt = NMT_BACKEND(run)
                    if nmt:
                        out.append(nmt)
                        pending_jp.clear()
                        return
                except Exception:
                    pass
            out.append(f"[JP:{run}]")
            pending_jp.clear()

    while i < len(text):
        matched = False
        for src, dst in GLOSSARY:
            if text.startswith(src, i):
                flush_pending()
                out.append(dst)
                i += len(src)
                matched = True
                break
        if matched:
            continue
        ch = text[i]
        if is_japanese(ch):
            pending_jp.append(ch)
        else:
            flush_pending()
            out.append(ch)
        i += 1
    flush_pending()
    # Collapse the spacing the particle glossary introduces.
    return " ".join("".join(out).split())


def decode_va(va: int, size: int = 0x200) -> dict[str, str]:
    """Read a string at a VA and decode it as shift-JIS and UTF-16LE."""
    data = EXE.read_bytes()
    image_base, sections = sre.parse_pe(data)
    blob = sre.read_bytes(data, image_base, sections, va, size)
    nul = blob.find(b"\x00")
    sjis_bytes = blob[:nul] if nul >= 0 else blob
    wnul = blob.find(b"\x00\x00")
    # align wide terminator to an even offset
    if wnul >= 0 and wnul % 2 == 1:
        wnul += 1
    utf16_bytes = blob[:wnul] if wnul >= 0 else blob
    out = {}
    try:
        out["shift_jis"] = sjis_bytes.decode("shift_jis")
    except Exception as exc:
        out["shift_jis"] = f"<sjis decode error: {exc}>"
    try:
        out["utf16le"] = utf16_bytes.decode("utf-16le")
    except Exception as exc:
        out["utf16le"] = f"<utf16 decode error: {exc}>"
    return out


def best_decoding(va: int, size: int = 0x200) -> str:
    """Decode a string, auto-selecting the codec.

    ASCII stored as UTF-16LE has a zero high byte every other byte; a shift-JIS
    or ASCII string does not. Detect the wide pattern first, else treat it as
    shift-JIS (a superset of ASCII), so an ASCII name like "CSFeMan" is not
    misread as UTF-16 mojibake.
    """
    data = EXE.read_bytes()
    image_base, sections = sre.parse_pe(data)
    blob = sre.read_bytes(data, image_base, sections, va, size)
    if len(blob) >= 4 and blob[1] == 0 and blob[3] == 0:
        end = blob.find(b"\x00\x00")
        if end >= 0 and end % 2 == 1:
            end += 1
        wide = blob[:end] if end >= 0 else blob
        return wide.decode("utf-16le", errors="replace")
    end = blob.find(b"\x00")
    narrow = blob[:end] if end >= 0 else blob
    return narrow.decode("shift_jis", errors="replace")


def render_va(va: int, size: int = 0x200) -> str:
    text = best_decoding(va)
    english = translate_jp(text)
    return f"0x{va:x}: {text!r}  ->  EN: {english}"


def annotate_crash_log(path: Path) -> str:
    """Annotate crash.log ASSERT lines (a0_rva/a2_rva/a3) with decoded English."""
    lines_out: list[str] = []
    for line in path.read_text(errors="replace").splitlines():
        lines_out.append(line)
        if "ASSERT" not in line:
            continue
        for token in line.split():
            key, _, val = token.partition("=")
            if key in ("a0_rva", "a2_rva", "a3") and val.startswith("0x"):
                try:
                    rva = int(val, 16)
                except ValueError:
                    continue
                va = rva if rva >= IMAGE_BASE_DEFAULT else IMAGE_BASE_DEFAULT + rva
                try:
                    lines_out.append(f"    {key} -> {render_va(va)}")
                except Exception as exc:
                    lines_out.append(f"    {key}=0x{va:x}: <resolve error: {exc}>")
    return "\n".join(lines_out)


def main() -> None:
    if len(sys.argv) < 2:
        print(__doc__)
        return
    mode = sys.argv[1]
    if mode == "va":
        va = int(sys.argv[2], 16)
        size = int(sys.argv[3], 16) if len(sys.argv) > 3 else 0x200
        print(render_va(va, size))
    elif mode == "text":
        print(translate_jp(sys.argv[2]))
    elif mode == "crashlog":
        print(annotate_crash_log(Path(sys.argv[2])))
    else:
        print(f"unknown mode: {mode}")
        print(__doc__)


if __name__ == "__main__":
    main()
