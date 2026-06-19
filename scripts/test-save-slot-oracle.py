#!/usr/bin/env python3
"""Regression tests for scripts/save-slot-oracle.py's SL2.bt identity mapping."""
from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
ORACLE_PATH = REPO_ROOT / "scripts" / "save-slot-oracle.py"
FIXTURE_PATH = REPO_ROOT / "third_party" / "ER-Save-File-Readers" / "testdata" / "vagabond" / "save_slots" / "0.sl2"
EXPECTED_MAP_ID = 0x0A010000
EXPECTED_HEALTH = 522
EXPECTED_STAMINA = 97
EXPECTED_LEVEL = 9
EXPECTED_STATS = [15, 10, 11, 14, 13, 9, 9, 7]
EXPECTED_NAME = "1"
EXPECTED_GENDER = 1
EXPECTED_MAX_CRIMSON_FLASK_COUNT = 3
EXPECTED_MAX_CERULEAN_FLASK_COUNT = 1


def load_oracle():
    spec = importlib.util.spec_from_file_location("save_slot_oracle", ORACLE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {ORACLE_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def main() -> int:
    oracle = load_oracle()
    slot_data = FIXTURE_PATH.read_bytes()
    decoded = oracle.decode_sl2_bt_fixture_fields(slot_data)
    fields = decoded["decoded_fields"]

    assert decoded["layout"] == "sl2-bt-er-save-file-readers-fixture"
    assert fields["saved_map_c30"] == EXPECTED_MAP_ID
    assert fields["health"] == EXPECTED_HEALTH
    assert fields["stamina"] == EXPECTED_STAMINA
    assert fields["level"] == EXPECTED_LEVEL
    assert fields["stats"] == EXPECTED_STATS
    assert fields["name"] == EXPECTED_NAME
    assert fields["gender"] == EXPECTED_GENDER
    assert fields["max_crimson_flask_count"] == EXPECTED_MAX_CRIMSON_FLASK_COUNT
    assert fields["max_cerulean_flask_count"] == EXPECTED_MAX_CERULEAN_FLASK_COUNT
    for key in ["saved_map_c30", "health", "stamina", "stats", "name", "gender"]:
        assert key in decoded["decoded_field_sources"]

    print("save-slot-oracle regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
