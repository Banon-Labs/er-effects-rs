#!/usr/bin/env python3
# check-no-magic-numbers: allow-file -- ER part filename families are data dictionary identifiers.
"""Stage all Mushroom Man model/face/default-slot aliases into a ModEngine2 mod.

The playable mushroom mesh is authored once into the FC_M_0000 high/low binders.
Elden Ring character presets request many FC_* body binders and many FG_A_*
face-part binders. Raw-copying the source binder under another filename is not
sufficient: the BND XML and inner FLVER/TPF/ANIBND paths must match the requested
part name. This helper builds properly renamed aliases, packs them with WitchyBND,
and copies the packed binders into the mod.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

PART_EXT = ".partsbnd.dcx"
SLOTS = ("hd", "bd", "am", "lg")
GENDERS = ("m", "f")
LODS = ("", "_l")
DEFAULT_SOURCE_FC_HIGH = Path(
    "target/mushroom-route-a-offline/blender-edit/live-blender-edit-fixed/fc_m_0000-blender-edit-parts"
)
DEFAULT_SOURCE_FC_LOW = Path(
    "target/mushroom-route-a-offline/blender-edit/live-blender-edit-fixed/fc_m_0000_l-blender-edit-parts"
)
SOURCE_FG = Path("target/mushroom-route-a-offline/prototype/fg_a_0000_m-mushroom-parts")


def existing_file(path: Path, label: str) -> Path:
    if not path.is_file():
        raise SystemExit(f"missing {label}: {path}")
    return path


def existing_dir(path: Path, label: str) -> Path:
    if not path.is_dir():
        raise SystemExit(f"missing {label}: {path}")
    return path


def find_dictionary(explicit: str | None, repo_root: Path) -> Path:
    candidates: list[Path] = []
    if explicit:
        candidates.append(Path(explicit))
    env_path = os.environ.get("ER_FILE_DICTIONARY_JSON")
    if env_path:
        candidates.append(Path(env_path))
    smithbox_dir = os.environ.get("SMITHBOX_BINARY_DIR")
    if smithbox_dir:
        candidates.append(
            Path(smithbox_dir)
            / "Assets"
            / "File Dictionaries"
            / "ER-File-Dictionary.json"
        )
    candidates.extend(
        [
            repo_root
            / ".deps"
            / "Smithbox"
            / "Assets"
            / "File Dictionaries"
            / "ER-File-Dictionary.json",
            repo_root
            / ".."
            / "Smithbox"
            / "Assets"
            / "File Dictionaries"
            / "ER-File-Dictionary.json",
            repo_root
            / ".."
            / "smithbox"
            / "Assets"
            / "File Dictionaries"
            / "ER-File-Dictionary.json",
            Path("/mnt/d/Smithbox/Assets/File Dictionaries/ER-File-Dictionary.json"),
        ]
    )
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise SystemExit(
        "could not find ER-File-Dictionary.json; pass --dictionary or set ER_FILE_DICTIONARY_JSON/SMITHBOX_BINARY_DIR"
    )


def find_witchy(explicit: str | None, repo_root: Path) -> Path:
    candidates: list[Path] = []
    if explicit:
        candidates.append(Path(explicit))
    env_path = os.environ.get("WITCHY_BND")
    if env_path:
        candidates.append(Path(env_path))
    candidates.extend(
        [
            repo_root / ".deps" / "WitchyBND" / "WitchyBND.exe",
            repo_root / ".." / "WitchyBND" / "WitchyBND.exe",
            Path("/mnt/d/Witchy BND/WitchyBND.exe"),
        ]
    )
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise SystemExit("could not find WitchyBND.exe; pass --witchy or set WITCHY_BND")


def dictionary_part_names(dictionary_path: Path) -> tuple[list[str], list[str]]:
    data = json.loads(dictionary_path.read_text(encoding="utf-8"))
    entries = data.get("Entries")
    if not isinstance(entries, list):
        raise SystemExit(f"unexpected dictionary shape in {dictionary_path}")
    fc_names: list[str] = []
    fg_names: list[str] = []
    for entry in entries:
        if not isinstance(entry, dict):
            continue
        path = str(entry.get("Path", "")).lower()
        filename = str(entry.get("Filename", "")).lower()
        if not path.startswith("/parts/") or not path.endswith(PART_EXT):
            continue
        full_name = Path(path).name
        if filename.startswith(("fc_m_", "fc_f_")):
            fc_names.append(full_name)
        elif filename.startswith("fg_a_"):
            fg_names.append(full_name)
    if not fc_names:
        raise SystemExit(f"dictionary has no FC body part entries: {dictionary_path}")
    if not fg_names:
        raise SystemExit(f"dictionary has no FG face part entries: {dictionary_path}")
    return sorted(set(fc_names)), sorted(set(fg_names))


def part_stem(name: str) -> str:
    if not name.endswith(PART_EXT):
        raise ValueError(f"not a partsbnd filename: {name}")
    return name[: -len(PART_EXT)]


def root_stem(upper_stem: str) -> str:
    for suffix in ("_L", "_M", "_F"):
        if upper_stem.endswith(suffix):
            return upper_stem[: -len(suffix)]
    return upper_stem


def copy_file(source: Path, target: Path) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    if source.resolve() == target.resolve():
        return
    shutil.copy2(source, target)


def replace_text(path: Path, replacements: list[tuple[str, str]]) -> None:
    text = path.read_text(encoding="utf-8-sig")
    for old, new in replacements:
        text = text.replace(old, new)
    path.write_text(text, encoding="utf-8")


def rename_if_exists(alias_dir: Path, old_name: str, new_name: str) -> None:
    old = alias_dir / old_name
    if old.exists() and old.name != new_name:
        new = alias_dir / new_name
        if new.exists():
            new.unlink()
        old.rename(new)


def run_witchy_pack(witchy: Path, alias_dir: Path, log_path: Path) -> None:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("w", encoding="utf-8", errors="replace") as log:
        proc = subprocess.run(
            [str(witchy), "-p", str(alias_dir)],
            stdout=log,
            stderr=subprocess.STDOUT,
            timeout=30,
        )
    if proc.returncode not in (0, 82):
        tail = ""
        if log_path.exists():
            tail = "\n".join(log_path.read_text(errors="replace").splitlines()[-80:])
        raise SystemExit(
            f"WitchyBND pack failed for {alias_dir} with exit {proc.returncode}\n{tail}"
        )


def build_fc_alias(
    witchy: Path,
    alias_root: Path,
    parts_dir: Path,
    name: str,
    source_high: Path,
    source_low: Path,
) -> None:
    stem = part_stem(name)
    target_upper = stem.upper()
    low = stem.endswith("_l")
    source_dir = existing_dir(
        source_low if low else source_high, "mushroom FC source dir"
    )
    source_upper = "FC_M_0000_L" if low else "FC_M_0000"
    source_root = "FC_M_0000"
    target_root = root_stem(target_upper)
    alias_dir = alias_root / f"{stem}-partsbnd-dcx"
    if alias_dir.exists():
        shutil.rmtree(alias_dir)
    shutil.copytree(source_dir, alias_dir)
    rename_if_exists(alias_dir, f"{source_upper}.flver", f"{target_upper}.flver")
    rename_if_exists(alias_dir, f"{source_upper}.tpf", f"{target_upper}.tpf")
    replace_text(
        alias_dir / "_witchy-bnd4.xml",
        [
            (
                f"<filename>fc_m_0000{'_l' if low else ''}.partsbnd.dcx</filename>",
                f"<filename>{name}</filename>",
            ),
            (source_upper, target_upper),
            (source_root, target_root),
        ],
    )
    run_witchy_pack(witchy, alias_dir, alias_root / f"{stem}-pack.log")
    copy_file(
        existing_file(alias_root / name, f"packed FC alias {name}"), parts_dir / name
    )


def build_fg_alias(witchy: Path, alias_root: Path, parts_dir: Path, name: str) -> None:
    stem = part_stem(name)
    target_upper = stem.upper()
    target_root = root_stem(target_upper)
    source_dir = existing_dir(SOURCE_FG, "hidden FG source dir")
    alias_dir = alias_root / f"{stem}-partsbnd-dcx"
    if alias_dir.exists():
        shutil.rmtree(alias_dir)
    shutil.copytree(source_dir, alias_dir)
    rename_if_exists(alias_dir, "FG_A_0000_M.flver", f"{target_upper}.flver")
    rename_if_exists(alias_dir, "FG_A_0000.anibnd", f"{target_root}.anibnd")
    replace_text(
        alias_dir / "_witchy-bnd4.xml",
        [
            (
                "<filename>fg_a_0000_m.partsbnd.dcx</filename>",
                f"<filename>{name}</filename>",
            ),
            ("FG_A_0000_M", target_upper),
            ("FG_A_0000", target_root),
        ],
    )
    run_witchy_pack(witchy, alias_dir, alias_root / f"{stem}-pack.log")
    copy_file(
        existing_file(alias_root / name, f"packed FG alias {name}"), parts_dir / name
    )


def stage_fc(
    witchy: Path,
    alias_root: Path,
    parts_dir: Path,
    names: list[str],
    source_high: Path,
    source_low: Path,
) -> int:
    for name in names:
        build_fc_alias(
            witchy, alias_root / "fc", parts_dir, name, source_high, source_low
        )
    return len(names)


def stage_fg(witchy: Path, alias_root: Path, parts_dir: Path, names: list[str]) -> int:
    compat = {"fg_a_0000_m.partsbnd.dcx", "fg_a_0000_f.partsbnd.dcx"}
    all_names = sorted(set(names) | compat)
    for name in all_names:
        build_fg_alias(witchy, alias_root / "fg", parts_dir, name)
    return len(all_names)


def stage_hidden_slots(parts_dir: Path, hidden_slot_dir: Path) -> int:
    count = 0
    for slot in SLOTS:
        for gender in GENDERS:
            for lod in LODS:
                name = f"{slot}_{gender}_0000{lod}.partsbnd.dcx"
                copy_file(
                    existing_file(
                        hidden_slot_dir / name, f"hidden naked-slot source {name}"
                    ),
                    parts_dir / name,
                )
                count += 1
    return count


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mod-dir", required=True, type=Path)
    parser.add_argument("--dictionary", type=Path)
    parser.add_argument("--witchy", type=Path)
    parser.add_argument(
        "--fc-source-high",
        type=Path,
        default=DEFAULT_SOURCE_FC_HIGH,
        help="source unpacked FC_M_0000 binder directory for high-detail mushroom aliases",
    )
    parser.add_argument(
        "--fc-source-low",
        type=Path,
        default=DEFAULT_SOURCE_FC_LOW,
        help="source unpacked FC_M_0000_L binder directory for low-detail mushroom aliases",
    )
    parser.add_argument(
        "--hidden-slot-dir",
        type=Path,
        default=Path("target/mushroom-route-a-offline/hidden-naked-slots"),
    )
    parser.add_argument(
        "--alias-root",
        type=Path,
        default=Path("target/mushroom-route-a-offline/generated-part-aliases"),
    )
    parser.add_argument(
        "--only",
        action="append",
        default=[],
        help="limit to a specific partsbnd filename; may be passed repeatedly",
    )
    parser.add_argument(
        "--stage-fg-aliases",
        action="store_true",
        help=(
            "opt in to generated FG_A aliases. Disabled by default because the "
            "profile-select renderer crashed on generated FG_A preset binders; "
            "facegen.fgbnd.dcx plus FG_A_0000_M/F compatibility aliases remain staged."
        ),
    )
    parser.add_argument("--summary", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo_root = Path.cwd()
    mod_dir = args.mod_dir
    parts_dir = mod_dir / "parts"
    if not parts_dir.is_dir():
        raise SystemExit(f"missing mod parts dir: {parts_dir}")
    dictionary = find_dictionary(
        str(args.dictionary) if args.dictionary else None, repo_root
    )
    witchy = find_witchy(str(args.witchy) if args.witchy else None, repo_root)
    fc_source_high = existing_dir(
        args.fc_source_high, "mushroom high-detail FC source dir"
    )
    fc_source_low = existing_dir(
        args.fc_source_low, "mushroom low-detail FC source dir"
    )
    fc_names, fg_names = dictionary_part_names(dictionary)
    if args.only:
        only = {name.lower() for name in args.only}
        fc_names = [name for name in fc_names if name.lower() in only]
        fg_names = [name for name in fg_names if name.lower() in only]
        extra = only - set(fc_names) - set(fg_names)
        if extra:
            print(
                f"warning: --only names not in dictionary and not staged: {sorted(extra)}",
                file=sys.stderr,
            )
    args.alias_root.mkdir(parents=True, exist_ok=True)
    fc_count = stage_fc(
        witchy, args.alias_root, parts_dir, fc_names, fc_source_high, fc_source_low
    )
    fg_inputs = fg_names if args.stage_fg_aliases else []
    fg_count = stage_fg(witchy, args.alias_root, parts_dir, fg_inputs)
    fg_mode = "dictionary" if args.stage_fg_aliases else "compatibility-only"
    hidden_slot_count = stage_hidden_slots(parts_dir, args.hidden_slot_dir)
    lines = [
        "Route A mushroom all-model-variant staging summary",
        f"mod_dir={mod_dir}",
        f"dictionary={dictionary}",
        f"witchy={witchy}",
        f"fc_variant_files={fc_count}",
        f"fg_hidden_face_files={fg_count}",
        f"fg_alias_mode={fg_mode}",
        f"hidden_naked_slot_files={hidden_slot_count}",
        f"fc_source_high={fc_source_high}",
        f"fc_source_low={fc_source_low}",
        "alias_mode=renamed-and-repacked",
    ]
    if args.summary:
        args.summary.parent.mkdir(parents=True, exist_ok=True)
        args.summary.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
