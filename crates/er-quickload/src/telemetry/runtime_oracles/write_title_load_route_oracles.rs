// Title-time load-route oracles: did a save get deserialized at the boot title, and why.
//
// Split into its own include file because `write_game_module_oracles.rs` is already at the repo's
// hard Rust file-size limit -- adding to it there would fail `scripts/check-rust-file-sizes.py`.

/// Emit the title-time deserialize + picked-summary oracle fields.
///
/// `oracle_title_time_deser_calls` must be 0 on a correct run. Non-zero means a save was
/// deserialized at the boot title through `0x14067b290` instead of in-world from
/// `CS::MoveMapStep::DoSaveStuff`, its only caller in the whole image -- the picked-save crash
/// (`gaitemInsTable[-1]` AV at `0x67141a`), which before this counter existed left no trace but a
/// debug log that stopped mid-line with no shutdown sequence.
///
/// The picked-summary fields alongside it say why that route was taken:
///   * `oracle_picked_summary_state` 1 = the game's own boot save-data read had already populated
///     `CS::ProfileSummary`; 2 = this DLL re-read the staged container at the title; 0 = neither,
///     which is the only case that legitimately falls back to the title-time full read.
///   * `oracle_picked_summary_attempts` counts real re-read attempts (a file read + record
///     rewrite), so a run that never got a summary pointer is distinguishable from one that never
///     tried.
///   * `oracle_picked_summary_slot_mask` is which slots the re-read rewrote (bit N = slot N).
///
/// The drift pair is the loading-screen identity oracle (bd er-effects-rs-ccud):
///   * `oracle_picked_summary_record_drifts` must be 0 for a run whose records were never
///     overwritten after we wrote them. Non-zero means something -- the game's own boot
///     `CS::ProfileSummary::Deserialize` is the known one -- replaced the body-derived records with
///     the container's stale `USER_DATA010` table, which is what puts a different character's face
///     and stats on the loading screen.
///   * `oracle_picked_summary_reasserts` counts the corrections made in response, capped at
///     `REASSERT_MAX_REWRITES`. `reasserts == drifts` means every drift was corrected; a value
///     above the cap means the correction lost and the screen is showing record identity.
///
/// `oracle_title_time_deser_last_slot` is -1 when no title-time deserialize ever ran.
fn write_title_load_route_oracles(body: &mut String) {
    use er_telemetry_core::counters as ttctr;
    use std::sync::atomic::Ordering as TtOrd;
    let last_slot = ttctr::TITLE_TIME_DESER_LAST_SLOT.load(TtOrd::SeqCst);
    let last_slot_i: i64 = last_slot as i64 - 1;
    body.push_str(&format!(
        "  \"oracle_title_time_deser_calls\": {},\n  \"oracle_title_time_deser_last_slot\": {last_slot_i},\n  \"oracle_picked_summary_state\": {},\n  \"oracle_picked_summary_attempts\": {},\n  \"oracle_picked_summary_slot_mask\": {},\n  \"oracle_picked_summary_record_drifts\": {},\n  \"oracle_picked_summary_reasserts\": {},\n  \"oracle_picked_summary_body_level\": {},\n",
        ttctr::TITLE_TIME_DESER_CALLS.load(TtOrd::SeqCst),
        ttctr::PICKED_SUMMARY_REFRESH_STATE.load(TtOrd::SeqCst),
        ttctr::PICKED_SUMMARY_REFRESH_ATTEMPTS.load(TtOrd::SeqCst),
        ttctr::PICKED_SUMMARY_REFRESH_SLOT_MASK.load(TtOrd::SeqCst),
        ttctr::PICKED_SUMMARY_RECORD_DRIFTS.load(TtOrd::SeqCst),
        ttctr::PICKED_SUMMARY_REASSERTS.load(TtOrd::SeqCst),
        ttctr::PICKED_SUMMARY_BODY_LEVEL.load(TtOrd::SeqCst),
    ));
}
