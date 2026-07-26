#!/usr/bin/env python3
"""Regression test: product Wwise muting must be visible in telemetry."""

from __future__ import annotations

from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
HOOK = REPO_ROOT / "crates" / "er-effects-rs" / "src" / "experiments" / "startup_hooks" / "layout_global_hooks.rs"
CONSTANTS = REPO_ROOT / "crates" / "er-effects-rs" / "src" / "constants" / "stats_panel_text.rs"
TELEMETRY = REPO_ROOT / "crates" / "er-effects-rs" / "src" / "telemetry" / "save_policy_logs.rs"
BOOTSTRAP = REPO_ROOT / "crates" / "er-effects-rs" / "src" / "lib_parts" / "dll_entry_parts" / "bootstrap.rs"
WATCHER = REPO_ROOT / "scripts" / "er-readiness-watch.py"


def require_contains(path: Path, needle: str) -> None:
    text = path.read_text(encoding="utf-8", errors="replace")
    assert needle in text, f"{path.relative_to(REPO_ROOT)} missing {needle!r}"


def test_product_hook_mutes_and_counts_both_paths() -> None:
    text = HOOK.read_text(encoding="utf-8", errors="replace")
    require_contains(HOOK, "let muted = !in_world_seen || quickload_active || !player_present;")
    require_contains(HOOK, "let ret = if muted {\n        0")
    require_contains(HOOK, "SOUND_POST_EVENT_FORWARDED_HITS.fetch_add(1, Ordering::SeqCst);")
    require_contains(HOOK, "SOUND_POST_EVENT_MUTED_HITS.fetch_add(1, Ordering::SeqCst);")
    require_contains(HOOK, "SOUND_POST_EVENT_LAST_MUTED.store(usize::from(muted), Ordering::SeqCst);")
    assert "forwards every event unchanged" not in text


def test_product_telemetry_reports_muted_and_forwarded_state() -> None:
    require_contains(CONSTANTS, "SOUND_POST_EVENT_LAST_MUTED")
    telemetry = TELEMETRY.read_text(encoding="utf-8", errors="replace")
    for field in (
        "oracle_sound_post_event_hook_installed",
        "oracle_sound_post_event_hits",
        "oracle_sound_post_event_muted_hits",
        "oracle_sound_post_event_forwarded_hits",
        "oracle_sound_post_event_last_muted",
        "oracle_sound_post_event_last_muted_id",
        "oracle_sound_post_event_last_playing_id",
    ):
        assert field in telemetry, f"telemetry missing {field}"
    assert "product hook mutes" in telemetry
    assert "forwards every event unchanged" not in telemetry


def test_hook_is_installed_by_product_and_watcher_fails_only_on_forwarded_preworld_audio() -> None:
    require_contains(BOOTSTRAP, ".name(\"er-effects-sound-post-event\".to_owned())")
    require_contains(BOOTSTRAP, ".spawn(install_sound_post_event_observer_hook)")
    require_contains(WATCHER, "oracle_sound_post_event_forwarded_hits")
    require_contains(WATCHER, "pre-world event was forwarded to Wwise and could be heard")


def main() -> int:
    test_product_hook_mutes_and_counts_both_paths()
    test_product_telemetry_reports_muted_and_forwarded_state()
    test_hook_is_installed_by_product_and_watcher_fails_only_on_forwarded_preworld_audio()
    print("test-sound-post-event-telemetry passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
