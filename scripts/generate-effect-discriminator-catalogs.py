#!/usr/bin/env python3
"""Generate selector catalog JSONC files from an er-net-effects master catalog.

The DLL reads every ``*.jsonc`` file in ``er-net-effect-catalogs`` as a JSONC
array of SpEffect IDs. This helper keeps the richer discriminator logic in
source control instead of hand-editing runtime catalog files.
"""

from __future__ import annotations

import argparse
import json
import re
from collections.abc import Callable
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_GAME_DIR = Path("/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game")
DEFAULT_MASTER = DEFAULT_GAME_DIR / "er-net-effect-master-catalog.json"
DEFAULT_CATALOG_DIR = DEFAULT_GAME_DIR / "er-net-effect-catalogs"
DEFAULT_EFFECTS = REPO_ROOT / "data" / "effects.json"

LIFETIME_TAG = "lifetime"
VFX_TAG = "presentation.vfx"
AUDIO_TAG = "presentation.audio"
STAT_TAGS = {"stat.hp", "stat.fp", "stat.stamina"}
COMBAT_DAMAGE_TAG = "combat.damage"
COMBAT_DEFENSE_TAG = "combat.defense"
MOVEMENT_TAG = "movement_or_timing"
AI_TAG_PREFIX = "ai."

STAT_FIELD_PATTERN = re.compile(
    r"(?:"
    r"changeHp|maxHp|hpRecover|isHpBurn|destinedDeathHp|conditionHp|"
    r"changeMp|maxMp|magicConsumption|miracleConsumption|artsConsumption|goodsConsumption|shamanConsumption|"
    r"changeStamina|maxStamina|staminaRecover|consumeStamina|guardStamina|"
    r"addStrength|changeStrength|bAdjustStrength|"
    r"addDexterity|dexterityCancel|"
    r"addMagic|changeMagic|bAdjustMagic|"
    r"addFaith|bAdjustFaith|addLuck|"
    r"equipWeight|allItemWeight"
    r")",
    re.IGNORECASE,
)
RECOVERY_FIELD_PATTERN = re.compile(
    r"(?:hpRecoverRate|staminaRecoverChangeSpeed|changeMp(?:Point|Rate)|maxMpRate|recoverArtsPoint_)",
    re.IGNORECASE,
)
WEAPON_FIELD_PATTERN = re.compile(r"(?:^wepParamChange$|weapon)", re.IGNORECASE)


Effect = dict[str, Any]
Predicate = Callable[[Effect], bool]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--master", type=Path, default=DEFAULT_MASTER)
    parser.add_argument("--catalog-dir", type=Path, default=DEFAULT_CATALOG_DIR)
    parser.add_argument("--effects", type=Path, default=DEFAULT_EFFECTS)
    parser.add_argument(
        "--clean",
        action="store_true",
        help="Delete existing catalog JSON/JSONC files first.",
    )
    return parser.parse_args()


def load_effects(path: Path) -> list[Effect]:
    data = json.loads(path.read_text(encoding="utf-8"))
    effects = data.get("effects")
    if not isinstance(effects, list):
        raise SystemExit(f"master catalog has no effects array: {path}")
    return effects


def load_bundled_ids(path: Path) -> list[int]:
    if not path.is_file():
        return []
    data = json.loads(path.read_text(encoding="utf-8"))
    calls = data.get("calls", [])
    return [int(call["id"]) for call in calls]


def tags(effect: Effect) -> set[str]:
    return {str(tag) for tag in effect.get("tags", [])}


def fields(effect: Effect) -> dict[str, Any]:
    raw = effect.get("fields", {})
    return raw if isinstance(raw, dict) else {}


def field_names(effect: Effect) -> set[str]:
    return set(fields(effect))


def substantive_tags(effect: Effect) -> set[str]:
    return tags(effect) - {LIFETIME_TAG}


def has_any_tag(effect: Effect, wanted: set[str]) -> bool:
    return bool(tags(effect) & wanted)


def has_ai_tag(effect: Effect) -> bool:
    return any(tag.startswith(AI_TAG_PREFIX) for tag in tags(effect))


def has_stat_field(effect: Effect) -> bool:
    return any(STAT_FIELD_PATTERN.search(name) for name in field_names(effect))


def has_recovery_field(effect: Effect) -> bool:
    return any(RECOVERY_FIELD_PATTERN.search(name) for name in field_names(effect))


def has_weapon_field(effect: Effect) -> bool:
    return any(WEAPON_FIELD_PATTERN.search(name) for name in field_names(effect))


def has_appear_ai_sound(effect: Effect) -> bool:
    return "AppearAiSoundId" in field_names(effect)


def has_vfx(effect: Effect) -> bool:
    return VFX_TAG in tags(effect) or any(
        name.startswith("vfxId") for name in field_names(effect)
    )


def has_audio(effect: Effect) -> bool:
    return AUDIO_TAG in tags(effect) or has_appear_ai_sound(effect)


def has_stat(effect: Effect) -> bool:
    return has_any_tag(effect, STAT_TAGS) or has_stat_field(effect)


def has_combat(effect: Effect) -> bool:
    return COMBAT_DAMAGE_TAG in tags(effect) or COMBAT_DEFENSE_TAG in tags(effect)


def has_movement(effect: Effect) -> bool:
    return MOVEMENT_TAG in tags(effect)


def is_visuals_only(effect: Effect) -> bool:
    return (
        has_vfx(effect)
        and not has_audio(effect)
        and not has_stat(effect)
        and not has_combat(effect)
        and not has_movement(effect)
        and not has_weapon_field(effect)
        and not has_ai_tag(effect)
    )


def is_sounds_only(effect: Effect) -> bool:
    return (
        has_appear_ai_sound(effect)
        and not has_vfx(effect)
        and not has_stat(effect)
        and not has_combat(effect)
        and not has_movement(effect)
        and not has_weapon_field(effect)
    )


def is_hp_fp_stats_only(effect: Effect) -> bool:
    return (
        has_stat(effect)
        and not has_vfx(effect)
        and not has_audio(effect)
        and not has_combat(effect)
        and not has_movement(effect)
        and not has_weapon_field(effect)
        and not has_ai_tag(effect)
    )


def sorted_ids(effects: list[Effect], predicate: Predicate) -> list[int]:
    return sorted({int(effect["id"]) for effect in effects if predicate(effect)})


def comment_for_effect(effect_id: int, by_id: dict[int, Effect]) -> str:
    effect = by_id.get(effect_id)
    if effect is None:
        return ""
    label = str(
        effect.get("name") or effect.get("curated_name") or effect.get("row_name") or ""
    )
    label = " ".join(label.split())
    return label.replace("*/", "* /")


def jsonc_catalog_text(ids: list[int], by_id: dict[int, Effect]) -> str:
    if not ids:
        return "[]\n"
    lines = ["["]
    last_index = len(ids) - 1
    for index, effect_id in enumerate(ids):
        comma = "," if index != last_index else ""
        comment = comment_for_effect(effect_id, by_id)
        suffix = f" // {comment}" if comment else ""
        lines.append(f"  {effect_id}{comma}{suffix}")
    lines.append("]")
    return "\n".join(lines) + "\n"


def write_catalog(
    catalog_dir: Path, name: str, ids: list[int], by_id: dict[int, Effect]
) -> None:
    (catalog_dir / name).write_text(jsonc_catalog_text(ids, by_id), encoding="utf-8")
    print(f"catalog_file={catalog_dir / name} count={len(ids)}")


def main() -> int:
    args = parse_args()
    if not args.master.is_file():
        raise SystemExit(f"missing master catalog: {args.master}")
    effects = load_effects(args.master)
    bundled_ids = load_bundled_ids(args.effects)
    by_id = {int(effect["id"]): effect for effect in effects}

    args.catalog_dir.mkdir(parents=True, exist_ok=True)
    if args.clean:
        for pattern in ("*.json", "*.jsonc"):
            for existing in args.catalog_dir.glob(pattern):
                existing.unlink()

    named_ids = sorted_ids(
        effects,
        lambda effect: (
            bool(str(effect.get("name", "")).strip())
            and not str(effect.get("name", "")).startswith(f"SpEffect {effect['id']} (")
        ),
    )
    if bundled_ids:
        bundled_set = set(bundled_ids)
        named_bundled_ids = [
            effect_id for effect_id in named_ids if effect_id in bundled_set
        ]
    else:
        named_bundled_ids = named_ids

    catalogs: dict[str, list[int]] = {
        "all-sp-effects.jsonc": sorted(by_id),
        "network-test.jsonc": [8355],
        "visual-effects.jsonc": sorted_ids(effects, has_vfx),
        "visuals-only.jsonc": sorted_ids(effects, is_visuals_only),
        "sound-effects.jsonc": sorted_ids(effects, has_audio),
        "sounds-only.jsonc": sorted_ids(effects, is_sounds_only),
        "hp-fp-stats.jsonc": sorted_ids(effects, has_stat),
        "hp-fp-stats-only.jsonc": sorted_ids(effects, is_hp_fp_stats_only),
        "fp-recovery.jsonc": sorted_ids(effects, has_recovery_field),
        "regen-and-recovery.jsonc": sorted_ids(effects, has_recovery_field),
        "weapon-buffs.jsonc": sorted_ids(effects, has_weapon_field),
        "damage-buffs.jsonc": sorted_ids(
            effects, lambda effect: COMBAT_DAMAGE_TAG in tags(effect)
        ),
        "defense-buffs.jsonc": sorted_ids(
            effects, lambda effect: COMBAT_DEFENSE_TAG in tags(effect)
        ),
        "movement-and-timing.jsonc": sorted_ids(effects, has_movement),
        "ai-perception-targeting.jsonc": sorted_ids(effects, has_ai_tag),
        "named-effects.jsonc": named_bundled_ids,
    }
    if bundled_ids:
        catalogs["all-bundled-effects.jsonc"] = sorted(bundled_ids)

    for name in sorted(catalogs):
        write_catalog(args.catalog_dir, name, catalogs[name], by_id)
    print(f"master={args.master}")
    print(f"catalog_dir={args.catalog_dir}")
    print(f"master_effect_count={len(effects)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
