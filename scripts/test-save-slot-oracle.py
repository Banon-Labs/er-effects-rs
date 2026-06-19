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
EXPECTED_FACE_DATA_MAGIC = "FACE"
EXPECTED_FACE_DATA_VERSION = 4
EXPECTED_FACE_DATA_BUFFER_SIZE = 288
EXPECTED_FACE_DATA_BUFFER_SHA256 = "376dd4ded50701a4f3cc96fb0f2b05b28dd20a7aa0759fc1350ab21be4b1921d"
EXPECTED_FACE_BODY_FIELDS = {
    "face_model": 0,
    "hair_model": 9,
    "eyebrow_model": 3,
    "beard_model": 1,
    "eye_patch_model": 0,
    "head_size": 128,
    "chest_size": 128,
    "abdomen_size": 128,
    "arms_size": 128,
    "legs_size": 128,
}


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
    assert fields["face_data_magic"] == EXPECTED_FACE_DATA_MAGIC
    assert fields["face_data_version"] == EXPECTED_FACE_DATA_VERSION
    assert fields["face_data_buffer_size"] == EXPECTED_FACE_DATA_BUFFER_SIZE
    assert fields["face_data_buffer_sha256"] == EXPECTED_FACE_DATA_BUFFER_SHA256
    for key, value in EXPECTED_FACE_BODY_FIELDS.items():
        assert fields["face_body_fields"][key] == value
    for key in ["saved_map_c30", "health", "stamina", "stats", "name", "gender", "face_data_buffer_sha256", "face_body_fields"]:
        assert key in decoded["decoded_field_sources"]

    live_face_offset = 0x18AFD
    live_player_offset = live_face_offset - oracle.LIVE_FACE_TO_PLAYER_GAME_DATA_DELTA
    synthetic_live_slot = bytearray(live_face_offset + oracle.FACE_DATA_BUFFER_SIZE)
    synthetic_live_slot[oracle.FIXTURE_SLOT_VERSION_OFFSET:oracle.FIXTURE_SLOT_VERSION_OFFSET + oracle.U32_SIZE] = slot_data[
        oracle.FIXTURE_SLOT_VERSION_OFFSET:oracle.FIXTURE_SLOT_VERSION_OFFSET + oracle.U32_SIZE
    ]
    synthetic_live_slot[oracle.FIXTURE_MAP_ID_OFFSET:oracle.FIXTURE_MAP_ID_OFFSET + oracle.U32_SIZE] = slot_data[
        oracle.FIXTURE_MAP_ID_OFFSET:oracle.FIXTURE_MAP_ID_OFFSET + oracle.U32_SIZE
    ]
    player_copy_size = 0x100
    synthetic_live_slot[live_player_offset:live_player_offset + player_copy_size] = slot_data[
        oracle.FIXTURE_PLAYER_GAME_DATA_OFFSET:oracle.FIXTURE_PLAYER_GAME_DATA_OFFSET + player_copy_size
    ]
    synthetic_live_slot[live_face_offset:live_face_offset + oracle.FACE_DATA_BUFFER_SIZE] = slot_data[
        oracle.FIXTURE_FACE_MAGIC_OFFSET:oracle.FIXTURE_FACE_MAGIC_OFFSET + oracle.FACE_DATA_BUFFER_SIZE
    ]
    live_decoded = oracle.decode_sl2_bt_fixture_fields(bytes(synthetic_live_slot))
    live_fields = live_decoded["decoded_fields"]
    assert live_decoded["layout"] == "sl2-bt-live-user-data"
    assert live_fields["saved_map_c30"] == EXPECTED_MAP_ID
    assert live_fields["health"] == EXPECTED_HEALTH
    assert live_fields["level"] == EXPECTED_LEVEL
    assert live_fields["face_data_buffer_sha256"] == EXPECTED_FACE_DATA_BUFFER_SHA256

    print("save-slot-oracle regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
