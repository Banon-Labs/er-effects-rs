// ============================================================================
//  ARMAMENT BADGE — LIVE TUNING
//  Edit these numbers and SAVE. badge-tune.py reloads on write; re-enter the
//  armament list (leave the menu and come back) so tiles re-measure.
// ============================================================================

const TUNING = {
  // ---- LIVE ----------------------------------------------------------------
  // Nudges the badge's on-screen position, in tile-local px.
  // +x = right, +y = down.  The badge's home is the ArtsIcon slot at (-32, +37).
  offsetX: 0.0,
  offsetY: 0.0,

  // ---- BAKE-ONLY (see note below) -----------------------------------------
  // Rendered badge size in px. 37 matches the vanilla infusion badge.
  // This one CANNOT be applied live -- changing the rect changes how much of
  // the texture atlas maps into the quad, which is what smeared every ash of
  // war into one badge. Set it here, then run:
  //     python3 scripts/frida/badge-tune.py --bake
  // which rewrites BADGE_RENDER_PX in crates/er-gfx/src/equip_02_011.rs,
  // rebuilds, and relaunches.
  renderPx: 37.0,
};

// Emitted so the host can read the values back out. Do not edit below.
module.exports = TUNING;
