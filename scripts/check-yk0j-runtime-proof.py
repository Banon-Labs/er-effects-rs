#!/usr/bin/env python3
"""Validate the structured YK0J fail-closed runtime proof without image evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import tempfile
from pathlib import Path


def fnv1a64(data: bytes) -> int:
    value = 0xCBF29CE484222325
    for byte in data:
        value ^= byte
        value = (value * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return max(value, 1)


def file_sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate(artifact_dir: Path) -> list[str]:
    failures: list[str] = []
    contract_path = artifact_dir / "yk0j-probe-contract.json"
    telemetry_path = artifact_dir / "er-effects-telemetry.json"
    result_path = artifact_dir / "runtime-probe.out"
    debug_path = artifact_dir / "er-effects-autoload-debug.log"
    for path in (contract_path, telemetry_path, result_path, debug_path):
        if not path.is_file():
            failures.append(f"missing proof artifact: {path}")
    if failures:
        return failures

    contract = json.loads(contract_path.read_text(encoding="utf-8"))
    telemetry = json.loads(telemetry_path.read_text(encoding="utf-8"))
    result = json.loads(result_path.read_text(encoding="utf-8"))
    debug = debug_path.read_text(encoding="utf-8", errors="replace")
    source = Path(contract["source_path"])
    expected_fingerprint = int(contract["expected_fingerprint"])

    checks = {
        "probe contract is armed": contract.get("probe") == "yk0j-unresolvable-save",
        "source exists after run": source.is_file(),
        "source remained byte-identical": source.is_file()
        and file_sha256(source) == contract.get("source_sha256"),
        "runtime rejection state is terminal": telemetry.get("oracle_own_load_save_rejection_state") == 1,
        "runtime resolution attempted exactly once": telemetry.get("oracle_own_load_save_rejection_attempts") == 1,
        "runtime guard was checked": int(telemetry.get("oracle_own_load_save_rejection_guard_checks") or 0) > 0,
        "runtime proof seam armed": telemetry.get("oracle_own_load_save_rejection_probe_armed") == 1,
        "runtime proof seam fired": telemetry.get("oracle_own_load_save_rejection_probe_fired") == 1,
        "runtime fingerprint matches armed path": telemetry.get("oracle_own_load_save_rejection_fingerprint")
        == expected_fingerprint,
        "runtime expected fingerprint matches armed path": telemetry.get(
            "oracle_own_load_save_rejection_probe_expected_fingerprint"
        )
        == expected_fingerprint,
        "identical rejection never repeated": telemetry.get(
            "oracle_own_load_save_repeated_identical_rejections"
        )
        == 0,
        "different rejection never repeated": telemetry.get(
            "oracle_own_load_save_repeated_different_rejections"
        )
        == 0,
        "loading progress escaped the preserved 51.5% stall": int(
            telemetry.get("oracle_boot_view_last_permille") or 0
        )
        > 515,
        "CreateFileW did not reproduce the 120959-call churn": int(
            telemetry.get("oracle_save_redirect_createfilew_calls") or 0
        )
        < 120_959,
        "player exists": telemetry.get("oracle_player_present") is True,
        "character identity is real": isinstance(telemetry.get("oracle_char_name"), str)
        and len(telemetry["oracle_char_name"].strip()) >= 2
        and int(telemetry.get("oracle_char_level") or 0) > 0,
        "watcher accepted a live player": result.get("ready") is True
        and result.get("reason") == "player_available",
        "probe fired once in debug evidence": debug.count("own-load: YK0J PROBE armed") == 1,
        "terminal resolver was never re-entered": "TERMINAL save rejection re-entered" not in debug,
    }
    failures.extend(label for label, passed in checks.items() if not passed)
    return failures


def selftest() -> int:
    with tempfile.TemporaryDirectory(prefix="yk0j-proof-selftest-") as tmp:
        root = Path(tmp)
        source = root / "source.sl2"
        source.write_bytes(b"read-only-source")
        source.chmod(0o444)
        fingerprint = fnv1a64(f"{source}|ER0000.sl2".encode())
        (root / "yk0j-probe-contract.json").write_text(
            json.dumps(
                {
                    "probe": "yk0j-unresolvable-save",
                    "source_path": str(source),
                    "source_sha256": file_sha256(source),
                    "expected_fingerprint": fingerprint,
                }
            )
        )
        telemetry = {
            "oracle_own_load_save_rejection_state": 1,
            "oracle_own_load_save_rejection_attempts": 1,
            "oracle_own_load_save_rejection_guard_checks": 2,
            "oracle_own_load_save_rejection_probe_armed": 1,
            "oracle_own_load_save_rejection_probe_fired": 1,
            "oracle_own_load_save_rejection_fingerprint": fingerprint,
            "oracle_own_load_save_rejection_probe_expected_fingerprint": fingerprint,
            "oracle_own_load_save_repeated_identical_rejections": 0,
            "oracle_own_load_save_repeated_different_rejections": 0,
            "oracle_boot_view_last_permille": 900,
            "oracle_save_redirect_createfilew_calls": 1000,
            "oracle_player_present": True,
            "oracle_char_name": "Test",
            "oracle_char_level": 1,
        }
        (root / "er-effects-telemetry.json").write_text(json.dumps(telemetry))
        (root / "runtime-probe.out").write_text(
            json.dumps({"ready": True, "reason": "player_available"})
        )
        (root / "er-effects-autoload-debug.log").write_text("own-load: YK0J PROBE armed\n")
        if validate(root):
            print("selftest valid fixture failed")
            return 1
        telemetry["oracle_own_load_save_repeated_identical_rejections"] = 1
        (root / "er-effects-telemetry.json").write_text(json.dumps(telemetry))
        if not validate(root):
            print("selftest failed to reject repeated identical rejection")
            return 1
    print("[check-yk0j-runtime-proof] selftest ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifact-dir", type=Path)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if args.artifact_dir is None:
        parser.error("--artifact-dir is required")
    failures = validate(args.artifact_dir)
    for failure in failures:
        print(f"YK0J runtime proof FAILED: {failure}")
    if failures:
        return 1
    print(f"[check-yk0j-runtime-proof] PASS artifact_dir={args.artifact_dir.resolve()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
