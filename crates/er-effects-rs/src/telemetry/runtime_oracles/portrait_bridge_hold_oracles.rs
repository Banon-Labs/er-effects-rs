// Portrait SAME-IDENTITY BRIDGE HOLD oracles: what the make-before-break bridge kept across a
// character switch, and whether anything ever confirmed or refuted it.
//
// These live in their own include file rather than inline in `write_game_module_oracles.rs` for the
// reason `portrait_framing_oracles.rs` already documents: that file sits within ~20 lines of the
// 3200-line hard gate (`scripts/check-rust-file-sizes.py`) and has no room for a group this size.
//
// WHAT THE GROUP ANSWERS. The bridge exists so a System->Quit->Load switch does not flash an empty
// loading screen: when the incoming character looks like the one already published, the previous
// head keeps displaying until the new one is ready. The hold is decided at the switch rearm by
// comparing the incoming slot's ProfileSummary name hash against the published head's -- and BOTH
// of those are that same record read at two different times, so on a same-slot repeat load the
// comparison agrees with itself no matter how wrong the record is. Until 2026-08-22 nothing else
// weighed in, so a hold could never fail and no oracle could show one that should have.
//
// READING THE THREE TOGETHER:
//   holds > 0, revocations == 0, unproven == 0
//       the healthy shape. Every hold was superseded by that window publishing its own head.
//   revocations > 0
//       a held head was dropped because the record it was about to be rebuilt from disagreed with
//       the fingerprint taken from the picked save's own bytes. That is a defect signal about the
//       RECORD, not a safety check doing its job on a healthy run: an intact record cannot produce
//       one. Do NOT build a gate that requires this to be non-zero (bd k979) -- gate on `unproven`.
//   unproven > 0
//       a hold rode an entire loading window without publishing anything and without being
//       refuted. This is the check firing when it should NOT have to: the window displayed a head
//       that nothing in the run can attribute to the character that loaded. It is the counter that
//       names the 2026-08-22 defect (`PORTRAIT-LOADWIN VERDICT #3: cause=displayed-stale
//       displayed=65 publishes=0 cap_max_side=0`), and the one worth failing a run on.
//   holds == 0 everywhere
//       either no character switch happened, or every switch legitimately cleared. Check
//       `oracle_portrait_loadwin_total` before reading a zero here as a pass.

fn write_portrait_bridge_hold_oracles(body: &mut String) {
    push_json_usize(
        body,
        "oracle_portrait_bridge_same_identity_holds",
        er_telemetry::counters::PORTRAIT_BRIDGE_SAME_IDENTITY_HOLDS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_bridge_hold_revocations",
        er_telemetry::counters::PORTRAIT_BRIDGE_HOLD_REVOCATIONS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_bridge_hold_unproven",
        er_telemetry::counters::PORTRAIT_BRIDGE_HOLD_UNPROVEN.load(Ordering::SeqCst),
    );
    // The live latch, as slot+1 (0 = no hold outstanding). A snapshot taken mid-loading-screen with
    // this set is a window currently riding a held head; the same snapshot after the window closed
    // is one that never resolved it. Without it the two counters above can only be read as history.
    push_json_usize(
        body,
        "oracle_portrait_bridge_hold_provisional_slot",
        er_telemetry::counters::PORTRAIT_BRIDGE_HOLD_PROVISIONAL.load(Ordering::SeqCst),
    );
}
