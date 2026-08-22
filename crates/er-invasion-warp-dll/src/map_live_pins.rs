//! Writes to pin rows that ALREADY EXIST -- the live half of the world-map surface.
//!
//! Split from `map_hooks` because the two halves have opposite safety rules and conflating them is
//! what produced the 2026-08-05 crash. `map_hooks` builds the row list inside the
//! `WorldMapViewModel` constructor, the one moment the buffer may be relocated. This module never
//! appends, never reserves and never allocates: it writes fields in rows the constructor already
//! placed, so the buffer cannot move and no raw row pointer a fast-travel dialog holds can dangle.
//!
//! Two things are being solved here, and they share the same machinery:
//!
//! 1. RE-COLOURING. Marking a location has to change the map while the player is standing there,
//!    because the list is not rebuilt until the next world entry.
//! 2. TOPPING UP. A dungeon whose MSB became resident since the last world entry has points that
//!    have nowhere to go. The constructor reserves dormant rows for exactly this; a top-up claims
//!    one and retargets it.
//!
//! # The ownership rule, learned the expensive way
//!
//! A recorded row address is not permission to write to it. Freed MenuHeap pages stay MAPPED, so a
//! fault-tolerant read of a destroyed ViewModel's buffer succeeds and returns whatever now lives
//! there. A run that trusted a four-bit id-range check repainted 456 rows inside other objects and
//! crashed the game. Every write below is gated on, in order: the ViewModel read LIVE from the
//! engine's own `CSPopupMenu+0x250` slot (nulled by the same function that frees the object), that
//! being the same object our span was recorded against, the list still beginning where it did, and
//! the row pointing into the param slab this DLL leaked. The last is a full 64-bit match on an
//! address only we hand out.

use core::sync::atomic::{AtomicUsize, Ordering};

use super::map_hooks::{
    DORMANT_NEXT_SLOT, DORMANT_SPAN_BEGIN, DORMANT_SPAN_END, INJECTED_REGISTRY, LIVE_LIST_BEGIN,
    LIVE_RESTYLE_SIGNATURE, LIVE_SPAN_BEGIN, LIVE_SPAN_END, LIVE_VIEW_MODEL, MapCoordinates,
    PIN_ROW_STRIDE, PinListGeometry, RESTYLE_GENERATION, ROW_ENTITY_ID_OFFSET, ROW_ID_OFFSET,
    ROW_LAYER_MASK_OFFSET, ROW_PARAM_POINTER_OFFSET, authoritative_view_model, block_area,
    layer_bit_for_converter, map_dialog_is_attached, msb_block_targets, nearest_place_name_text_id,
    param_slab_bounds, project_to_map, read_pin_list, read_row_icon, record_place_name,
    record_top_up_target, row_is_verifiably_ours, stamped_row_id, write_row_icon,
};

/// Re-colour the pins that already exist, in place.
///
/// # Why this has to exist
///
/// Everything else in this module changes what the NEXT ViewModel is built with, and measurement
/// says there is no next one: closing and reopening the world map does not re-run the constructor
/// (`opens=2`, no `ctor #3`, live 2026-08-05), and neither does switching map layer -- the ctor is
/// reachable only from `STEP_MoveMap_Init`. So a mark made while playing could never reach the
/// screen, no matter how correct the param rows were, and they were correct: the log's tier tally
/// proved it.
///
/// # Why writing the icon alone was not enough
///
/// The icon field is not re-read on its own. `SetTo` (0x14087ae20) is what issues the clip's
/// `GotoAndStop(frame)`, and a clip slot is re-bound only when the draw's cached id differs from
/// `row+0x08`. Rewriting `row+0x248` therefore updated a field nothing would look at again. Each
/// repaint below bumps `row+0x08` to a fresh generation as well, which is the write that actually
/// makes the change appear.
///
/// Only rows this DLL injected are touched. Span membership alone is not authorisation to write --
/// a span outlives its ViewModel, and the block returns to the same heap at the same size class --
/// so each row must also still carry our own `+0x08` stamp.
///
/// Returns `(examined, rewritten)`.
///
/// # Safety
///
/// Game task thread. Every read is fault-closed; the writes go only to rows inside a span this
/// module appended and still owns.
#[cfg(windows)]
pub unsafe fn restyle_live_pins() -> (usize, usize) {
    let signature = crate::local_invasion_filter::pin_choice_signature();
    if LIVE_RESTYLE_SIGNATURE.load(Ordering::SeqCst) == signature {
        return (0, 0);
    }
    // Past this point the user's marks have CHANGED and the map is expected to change with them.
    // Every refusal below is therefore a visible failure -- the player pressed a key and nothing
    // happened -- so each one says why, deduped on the reason.
    macro_rules! decline {
        ($reason:expr) => {{
            let reason: &'static str = $reason;
            let key = reason.as_ptr() as usize;
            if LAST_RESTYLE_REFUSAL.swap(key, Ordering::SeqCst) != key {
                crate::standalone_log(format_args!(
                    "map-inject: restyle declined, the marks changed but the live map was not \
                     repainted -- {reason}"
                ));
            }
            return (0, 0);
        }};
    }
    // EXACTLY ONE VIEWMODEL, RE-VALIDATED BEFORE ANY WRITE.
    //
    // Three independent gates, because the previous single gate was not one: (1) the ViewModel must
    // be the one the last injection wrote into and its pin list must still be readable and
    // plausible; (2) that list must still START where it did when we recorded the span -- if it
    // moved, the buffer was reallocated and every recorded address is meaningless; (3) each row must
    // point into our own leaked param slab. Only then is a write authorised.
    // GATE 0, AND IT IS THE ONE THAT MATTERS: the ViewModel comes from the ENGINE'S slot, not from
    // ours. `CSPopupMenu+0x250` is nulled by the same function that frees the object, so a
    // destroyed ViewModel reads as `None` here. A remembered pointer cannot do that -- MenuHeap
    // hands the freed 0x450 block back at the same size class and the pages stay mapped, so every
    // read succeeds and returns another object's fields.
    let Some(view_model) = authoritative_view_model() else {
        decline!("CSPopupMenu+0x250 is null -- there is no world map to repaint right now")
    };
    // And it must be the SAME object our span was recorded against. If the engine has built a new
    // one since, the span describes a buffer that has been freed.
    if view_model != LIVE_VIEW_MODEL.load(Ordering::SeqCst) {
        decline!(
            "the live ViewModel is not the one the injection wrote into, so the recorded span \
             describes a freed buffer"
        )
    }
    let span_begin = LIVE_SPAN_BEGIN.load(Ordering::SeqCst);
    let span_end = LIVE_SPAN_END.load(Ordering::SeqCst);
    if span_begin == 0 || span_end <= span_begin {
        decline!("no injected span was recorded, so nothing of ours is on this map")
    }
    let Some(geometry) = (unsafe { read_pin_list(view_model) }) else {
        decline!("the pin row list is unreadable through the live ViewModel")
    };
    if !geometry.is_plausible() || geometry.begin != LIVE_LIST_BEGIN.load(Ordering::SeqCst) {
        // The list moved or stopped making sense. Our span addresses describe a buffer that no
        // longer exists; writing through them is what put 456 repaints into freed memory.
        decline!("the pin row list moved or stopped being plausible since the injection")
    }
    // And the span must still lie inside the list we just measured.
    if span_begin < geometry.begin || span_end > geometry.end {
        decline!("the injected span no longer lies inside the live pin row list")
    }
    let Some(slab) = param_slab_bounds() else {
        decline!("the synthetic param slab bounds are not published")
    };
    let registry_ptr = INJECTED_REGISTRY.load(Ordering::SeqCst);
    if registry_ptr == 0 {
        decline!("no injected row registry is published")
    }
    // SAFETY: leaked at injection time, never freed or mutated.
    let registry: &er_invasion_warp::map_surface::InvasionRowRegistry =
        unsafe { &*(registry_ptr as *const _) };
    let installed = crate::map_gfx::red_pin_frame_installed();
    let (mut examined, mut rewritten, mut foreign) = (0_usize, 0_usize, 0_usize);
    let mut rows: usize = (span_end - span_begin) / PIN_ROW_STRIDE;
    // One new generation for this whole pass, so every row it repaints gets an id the renderer has
    // not cached and is forced to re-bind.
    let generation = (RESTYLE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1) as u32;
    /// How many disagreeing rows to describe in the summary line. Enough to see a pattern, few
    /// enough that a pass over 500 rows still writes one line.
    const SAMPLE_LIMIT: usize = 4;
    let mut samples: Vec<(i32, Option<u32>, u16, Option<u32>)> = Vec::new();
    let mut param_disagreements = 0_usize;
    // BOTH SPANS. A pin the player can see is not necessarily in the INJECTED span: a legacy
    // dungeon harvested mid-session has its markers in CLAIMED DORMANT rows instead, put there by
    // `top_up_live_pins`. Walking only the injected span meant marking such a dungeon repainted
    // exactly one row -- its whole-dungeon marker, which the top-up had already HIDDEN -- and left
    // every marker actually on screen untouched. Measured 2026-08-05: `MARKED 0x1c000000` three
    // times, `1 of 467 repainted` every time, while nine visible Haligtree markers sat in dormant
    // rows and never changed. That is the reported "marking does nothing" in full.
    //
    // Only the CLAIMED prefix of the dormant span is walked. The rest are still blank rows with a
    // zero layer mask; they are not drawn and have no tier to show.
    let dormant_rows: Vec<usize> = er_invasion_warp::map_surface::claimed_dormant_span(
        DORMANT_SPAN_BEGIN.load(Ordering::SeqCst),
        DORMANT_NEXT_SLOT.load(Ordering::SeqCst),
        PIN_ROW_STRIDE,
        geometry.begin,
        geometry.end,
    )
    .map(|(begin, end)| (begin..end).step_by(PIN_ROW_STRIDE).collect())
    .unwrap_or_default();
    let claimed_dormant = dormant_rows.len();
    rows += claimed_dormant;
    for row in (span_begin..span_end)
        .step_by(PIN_ROW_STRIDE)
        .chain(dormant_rows)
    {
        // Ownership by POINTER, not by a four-bit id range. The row must point at a param row inside
        // the slab this module leaked, at an exact slab stride -- an address only we hand out.
        let Some(index) = row_is_verifiably_ours(row, slab) else {
            foreign += 1;
            continue;
        };
        examined += 1;
        let Some(entity_id) =
            (unsafe { er_game_base::mem::safe_read_i32(row + ROW_ENTITY_ID_OFFSET) })
        else {
            continue;
        };
        let block = registry
            .target_for_entity_id(entity_id)
            .copied()
            // A row a live top-up claimed carries an id past the registry's dense range. Resolving
            // it from the registry alone yields `None`, and the pin would then be painted as if it
            // belonged to no block -- so marking the place the player is standing in would leave
            // its own marker unchanged.
            .or_else(|| super::map_hooks::top_up_target_for_entity_id(entity_id))
            .map(|target| target.block.raw());
        let desired = er_invasion_warp::param_row::invasion_pin_icon_id_for(
            crate::local_invasion_filter::pin_appearance_for(block),
            installed,
            er_invasion_warp::warp::invasion_attempt_in_flight(),
        );
        let current = unsafe { read_row_icon(row) };
        if current == Some(u32::from(desired)) {
            continue;
        }
        // KEEP THE FIRST FEW DISAGREEMENTS. Counts alone cannot distinguish "these rows genuinely
        // changed tier" from "our write is not sticking": a live run showed the SAME 48 rows
        // rewritten on four consecutive passes (generations 62-65), which is only possible if the
        // value never reads back as what was written -- and the count-only log line could not say
        // which rows, what they resolved to, or what was actually in the field.
        if samples.len() < SAMPLE_LIMIT {
            samples.push((entity_id, block, desired, current));
        }
        // Read the icon the row's own BonfireWarpParam carries. If the engine repopulates the row
        // from its param, the param is the value that wins and a row-only write is cosmetic.
        let param_icon =
            unsafe { er_game_base::mem::safe_read_usize(row + ROW_PARAM_POINTER_OFFSET) }
                .filter(|param| *param != 0)
                .and_then(|param| unsafe {
                    er_game_base::mem::safe_read_u16(
                        param + er_invasion_warp::param_row::PARAM_ICON_ID_OFFSET,
                    )
                });
        if param_icon != Some(desired) {
            param_disagreements += 1;
        }
        // SAFETY: `row` is inside a span this module appended and still carries our stamp; both
        // fields are plain scalars the engine only reads.
        //
        // ALL FOUR descriptors, not just `+0x248`. The engine picks which one to draw from at read
        // time, out of state we do not control -- see `ROW_ICON_DESCRIPTOR_OFFSETS`.
        unsafe { write_row_icon(row, desired) };
        // AND BUMP THE RE-BIND TOKEN. Without this the write above is invisible: the draw re-reads
        // a row's icon only when re-binding its clip, and it re-binds only on an id it has not
        // cached. This is the difference between "the field says the new frame" and "the map shows
        // the new frame".
        unsafe { *((row + ROW_ID_OFFSET) as *mut i32) = stamped_row_id(generation, index) };
        rewritten += 1;
    }
    LIVE_RESTYLE_SIGNATURE.store(signature, Ordering::SeqCst);
    if rewritten > 0 {
        // A later refusal for the same reason is new information, not the continuation of a state
        // already reported.
        LAST_RESTYLE_REFUSAL.store(0, Ordering::SeqCst);
    }
    if rewritten > 0 || examined > 0 || foreign > 0 {
        crate::standalone_log(format_args!(
            "map-inject: restyled LIVE pins -- {rewritten} of {examined} repainted at generation \
             {generation}, {foreign} of {rows} span row(s) REFUSED because they do not point into \
             our param slab. foreign>0 means the span and the live list disagree and the refusal is \
             the only thing standing between this and a write into someone else's object. Each \
             repaint bumps the row's +0x08 re-bind token as well as its icon -- if the map STILL \
             looks unchanged with rewritten>0, the re-bind is not reaching the clip. \
             {param_disagreements} of the repainted rows' own BonfireWarpParam still holds a \
             different icon, which is EXPECTED: the live path writes the row's four icon \
             descriptors and never the param, because the param is only read when a row is built. \
             {claimed_dormant} claimed dormant row(s) were walked alongside the injected span -- a \
             dungeon harvested mid-session keeps its markers there, and skipping them is why \
             marking one used to change nothing visible. Disagreeing rows \
             (entity_id, block, wanted, found): {samples:x?}"
        ));
    }
    (examined, rewritten)
}

/// Why the last restyle refused, deduped on the message address.
///
/// Separate from [`LAST_REFUSAL`] so a permanently-closed top-up gate cannot mask a restyle failure
/// or vice versa.
#[cfg(windows)]
static LAST_RESTYLE_REFUSAL: AtomicUsize = AtomicUsize::new(0);

/// Why the last top-up attempt did nothing.
///
/// THIS EXISTS BECAUSE THE FIRST VERSION HAD NONE. Every gate below was a bare `return 0`, so a
/// live run produced seven world entries, 512 dormant rows reserved on each, and not one line
/// saying why zero of them were ever claimed -- the same silent no-op this module's oracles were
/// written to eliminate. Deduped on the message's address so a gate that refuses every frame prints
/// once and a CHANGE of gate prints immediately.
#[cfg(windows)]
static LAST_REFUSAL: AtomicUsize = AtomicUsize::new(0);

/// Log a refusal reason once per distinct reason, and report "claimed nothing".
#[cfg(windows)]
fn refuse(reason: &'static str) -> usize {
    let key = reason.as_ptr() as usize;
    if LAST_REFUSAL.swap(key, Ordering::SeqCst) != key {
        crate::standalone_log(format_args!("map-inject: top-up declined -- {reason}"));
    }
    0
}

/// Clear the deduplication latch so the next refusal prints even if it repeats the last one.
///
/// Called when a top-up SUCCEEDS: the next time the same gate closes it is new information, not the
/// continuation of a state already reported.
#[cfg(windows)]
fn clear_refusal_latch() {
    LAST_REFUSAL.store(0, Ordering::SeqCst);
}

/// Make points harvested SINCE the last world entry visible, by retargeting dormant rows in place.
///
/// This is the answer to "I walked into Leyndell, opened the map, and my markers were not there".
/// The row list is populated only in the `WorldMapViewModel` constructor, which runs on a world
/// entry and, worse, runs during the loading screen BEFORE the destination's `MsbResCap`s exist --
/// so the injection that accompanies arriving somewhere is built from a catalog that does not yet
/// contain the place being arrived at. The points land in the catalog a second later and then have
/// nowhere to go until the next transition.
///
/// Nothing here appends. The rows were appended by the constructor; this writes fields in rows that
/// already exist:
///   `+0x10` position   -- read per frame through vtable slot `+0x20` (`FUN_14087c210` is literally
///                          `return this+0x10`), so a live write is seen.
///   `+0x50` entity id  -- what the confirm interceptor maps back to a warp target.
///   `+0x60` layer mask -- 0 while dormant; the converter's bit once claimed. This is the field
///                          `UpdateVisible` gates the draw flag on.
///   `+0x248` icon      -- the tier frame.
///   `+0x08` id         -- bumped last, to force the clip re-bind that makes the rest visible.
/// The param row's label is written too: a row whose eight label text ids are all negative is not
/// drawn at all, so an unnamed claim would be an invisible pin.
///
/// # Safety
/// Game task thread. Refuses unless the engine's own slots say it is safe.
#[cfg(windows)]
pub unsafe fn top_up_live_pins() -> usize {
    use er_invasion_warp::param_row::{
        PARAM_CATEGORY_BITS_OFFSET, PARAM_ENTITY_ID_OFFSET, PARAM_ICON_ID_OFFSET,
        PARAM_LABEL_TEXT_ID_BASE, SYNTHETIC_PARAM_ROW_LEN,
    };
    let Ok(base) = er_game_base::mem::game_module_base() else {
        return refuse("the game module base is unreadable");
    };
    // GATE 1: the ViewModel from the ENGINE's slot, and it must be the one our span describes.
    let Some(view_model) = authoritative_view_model() else {
        // `+0x250` is written only from `MoveMapStep`'s constructor (`FUN_1407ed840`, store at
        // 0x1407ed8d9) and nulled only from `~MoveMapStep` (`FUN_1407ed790`, null-store at
        // 0x1407ed821), so this null also covers the loading screen of an in-session area change.
        // Refusing through that window is correct; the gate reopens by itself once the constructor
        // hook re-stamps LIVE_VIEW_MODEL on the other side.
        return refuse(
            "CSPopupMenu+0x250 is null -- no WorldMapViewModel exists right now (no world loaded, \
             or an area change is mid-flight)",
        );
    };
    if view_model != LIVE_VIEW_MODEL.load(Ordering::SeqCst) {
        return refuse(
            "the live ViewModel is not the one the last injection wrote into, so the recorded \
             dormant span describes a buffer that has been freed",
        );
    }
    // GATE 2: no map dialog attached. A dialog holds RAW row pointers in
    // `CS::WorldMapWarpData+0x08` and its clip pool caches the list base; retargeting underneath it
    // would change what a bound clip points at mid-frame. Waiting costs nothing -- the player is
    // not looking at the map.
    if map_dialog_is_attached(view_model) {
        return refuse(
            "a world-map dialog is attached (ViewModel+0x08 is non-null); it holds raw row pointers, \
             so retargeting waits until the map is closed",
        );
    }
    let span_begin = DORMANT_SPAN_BEGIN.load(Ordering::SeqCst);
    let span_end = DORMANT_SPAN_END.load(Ordering::SeqCst);
    if span_begin == 0 || span_end <= span_begin {
        return refuse("the last injection recorded no dormant span to claim from");
    }
    // GATE 3: the list must still begin where it did. If it moved, every recorded address is a
    // description of a buffer that has been freed.
    let Some(geometry) = (unsafe { read_pin_list(view_model) }) else {
        return refuse("the pin row list is unreadable through the live ViewModel");
    };
    if !geometry.is_plausible() || geometry.begin != LIVE_LIST_BEGIN.load(Ordering::SeqCst) {
        return refuse(
            "the pin row list moved or stopped being plausible since the injection -- the buffer was \
             reallocated and every recorded address is meaningless",
        );
    }
    if span_begin < geometry.begin || span_end > geometry.end {
        return refuse("the dormant span no longer lies inside the live pin row list");
    }
    let registry_ptr = INJECTED_REGISTRY.load(Ordering::SeqCst);
    if registry_ptr == 0 {
        return refuse("no injected row registry is published, so nothing has been injected yet");
    }
    // SAFETY: leaked at injection time, never freed.
    let registry: &er_invasion_warp::map_surface::InvasionRowRegistry =
        unsafe { &*(registry_ptr as *const _) };
    let Some(slab) = param_slab_bounds() else {
        return refuse("the synthetic param slab bounds are not published");
    };

    // IDENTITY IS (BLOCK, POINT), NOT BLOCK -- and getting that wrong is what made this function a
    // permanent no-op.
    //
    // Keying on block alone reads "this block already has a pin, so there is nothing to add". But
    // the pin a not-yet-entered legacy dungeon has is the WHOLE-DUNGEON marker: one row at the
    // dungeon's centre, placed precisely because its interior was unknown. The moment its MSB
    // becomes resident, its real points arrive -- and they are all in that same block, so a
    // block-keyed filter discarded every one of them. Measured live 2026-08-05: the Haligtree's 34
    // points merged to 9 markers, the harvest reported them for ~35,900 consecutive frames, and the
    // top-up refused all of them on every frame because block 0x1c000000 already carried its
    // whole-dungeon marker. That is exactly the reported gap -- "I see the legacy locations only
    // after I warp" -- because the next constructor DOES supersede the marker properly.
    //
    // Superseding it live is the whole job: claim a row per real point, then hide the marker they
    // replace.
    let mut injected_points: std::collections::BTreeSet<(u32, u32)> = registry
        .targets()
        .iter()
        .map(|t| (t.block.raw(), t.point_index))
        .collect();
    // A claim leaves no trace in the leaked registry, so without folding the claim table in here
    // every point stays "fresh" forever and a dormant row is burned for it on every frame.
    injected_points.extend(super::map_hooks::top_up_claimed_points());
    // Likewise for points this ViewModel cannot place: re-deriving the same refusal every frame
    // costs a full harvest read and a projection per point and changes nothing.
    injected_points.extend(super::map_hooks::top_up_refused_points());
    let fresh =
        er_invasion_warp::map_surface::points_not_yet_shown(&msb_block_targets(), &injected_points);
    if fresh.is_empty() {
        return refuse(
            "nothing new to show: every point the MSB harvest knows about already has an injected \
             pin. This is the ordinary steady state, not a fault.",
        );
    }
    // The whole-dungeon marker each fresh block is about to outgrow, by registry index. Its row is
    // hidden once its replacements are on the map, so the dungeon does not carry both a centre blob
    // and its real points until the next world entry.
    let superseded: std::collections::BTreeMap<u32, usize> = registry
        .targets()
        .iter()
        .enumerate()
        .filter(|(_, t)| t.is_provisional())
        .map(|(index, t)| (t.block.raw(), index))
        .collect();

    let installed = crate::map_gfx::red_pin_frame_installed();
    let mut claimed = 0_usize;
    // Per-target refusals. A fresh block whose points all fall out here looks identical to "the
    // gate never opened" unless the counts are reported, which is the mistake this whole pass is
    // undoing.
    let (mut unprojectable, mut unlayered, mut unnamed) = (0_usize, 0_usize, 0_usize);
    let candidates = fresh.len();
    // Points, not places. A legacy dungeon's whole per-point set lives in ONE block, so 48 markers
    // can be a single markable location -- and marking it flips all 48 rows at once. Reading a
    // per-point count as a per-place count is what made a correct restyle look like a stuck write.
    let candidate_blocks = fresh
        .iter()
        .map(|t| t.block.raw())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let mut replaced_blocks: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    let slots = (span_end - span_begin) / PIN_ROW_STRIDE;
    for target in fresh {
        let slot = DORMANT_NEXT_SLOT.load(Ordering::SeqCst);
        if slot >= slots {
            crate::standalone_log(format_args!(
                "map-inject: top-up ran out of dormant rows ({slots} reserved, all claimed). The \
                 remaining points appear at the next world entry, which rebuilds the list properly. \
                 Raise DORMANT_ROW_COUNT if this is common."
            ));
            break;
        }
        let row = span_begin + slot * PIN_ROW_STRIDE;
        // Ownership: the row must still point into our slab. This is the 64-bit test, not the
        // four-bit id range that was not one.
        let Some(param_index) = row_is_verifiably_ours(row, slab) else {
            crate::standalone_log(format_args!(
                "map-inject: dormant row {slot} does not point into our param slab; refusing the \
                 whole top-up rather than writing into something we do not own"
            ));
            break;
        };
        // Project through the ViewModel's own converters, exactly as the constructor does.
        let Some((coords, converter_index, _)) =
            (unsafe { project_to_map(base, view_model, target.block.raw(), target.position) })
        else {
            unprojectable += 1;
            super::map_hooks::record_top_up_refusal(target.block.raw(), target.point_index);
            continue;
        };
        let block_area_byte = block_area(target.block.raw());
        let Some(layer_bits) = layer_bit_for_converter(base, converter_index, block_area_byte)
        else {
            unlayered += 1;
            super::map_hooks::record_top_up_refusal(target.block.raw(), target.point_index);
            continue;
        };
        let place_name_text_id = unsafe {
            nearest_place_name_text_id(
                geometry.begin,
                existing_row_count(&geometry),
                block_area_byte,
                coords,
            )
        };
        if place_name_text_id <= 0 {
            // An unnamed row is an UNDRAWN row. Claiming a slot for one would burn headroom and
            // show nothing.
            unnamed += 1;
            super::map_hooks::record_top_up_refusal(target.block.raw(), target.point_index);
            continue;
        }
        let entity_id = er_invasion_warp::map_surface::INVASION_ENTITY_ID_BASE
            + (registry.len() + claimed) as i32;
        let appearance = crate::local_invasion_filter::pin_appearance_for(Some(target.block.raw()));
        let icon = er_invasion_warp::param_row::invasion_pin_icon_id_for(
            appearance,
            installed,
            er_invasion_warp::warp::invasion_attempt_in_flight(),
        );

        // SAFETY: the slab is ours, leaked, never freed, and sized to include this index.
        let param = slab.0 + param_index * SYNTHETIC_PARAM_ROW_LEN;
        unsafe {
            *((param + PARAM_ENTITY_ID_OFFSET) as *mut i32) = entity_id;
            *((param + PARAM_ICON_ID_OFFSET) as *mut u16) = icon;
            *((param + PARAM_CATEGORY_BITS_OFFSET) as *mut u8) = layer_bits;
            *((param + PARAM_LABEL_TEXT_ID_BASE) as *mut i32) = place_name_text_id;
        }
        // SAFETY: `row` is a row this module appended and still owns, verified above. Every field
        // is a plain scalar the engine only reads.
        unsafe {
            *((row + 0x10) as *mut MapCoordinates) = coords;
            *((row + ROW_ENTITY_ID_OFFSET) as *mut i32) = entity_id;
            *((row + ROW_LAYER_MASK_OFFSET) as *mut u32) = u32::from(layer_bits);
            write_row_icon(row, icon);
        }
        record_place_name(target.block.raw(), place_name_text_id);
        // The registry is a dense index and this id is past its end, so the confirm hook cannot
        // resolve it from there. Still recorded even though no confirm can warp any more: an id the
        // hook cannot resolve takes its UNRESOLVED path, which means "our registry and our rows
        // disagree" and is a bug to chase. Registering it here keeps that signal honest, so the
        // only thing selecting a top-up pin reports is the policy refusal every other pin gives.
        record_top_up_target(entity_id, target);
        // LAST: the re-bind token. Everything above is invisible until the draw re-binds this row's
        // clip, and it re-binds only on an id it has not cached.
        let generation = (RESTYLE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1) as u32;
        unsafe { *((row + ROW_ID_OFFSET) as *mut i32) = stamped_row_id(generation, param_index) };
        DORMANT_NEXT_SLOT.store(slot + 1, Ordering::SeqCst);
        replaced_blocks.insert(target.block.raw());
        claimed += 1;
    }
    // Hide what the new rows replaced. Only for blocks that actually got a replacement: a block
    // whose points were all refused above must KEEP its whole-dungeon marker, or the refusal would
    // remove the only way to reach that dungeon from the map.
    let mut hidden = 0_usize;
    for block in &replaced_blocks {
        let Some(index) = superseded.get(block).copied() else {
            continue;
        };
        if unsafe { hide_injected_row_for_param_index(geometry.begin, index, slab) } {
            hidden += 1;
        }
    }
    if claimed > 0 {
        clear_refusal_latch();
        crate::standalone_log(format_args!(
            "map-inject: TOP-UP claimed {claimed} of {candidates} fresh POINT(s) into dormant rows \
             -- these markers are now visible WITHOUT a fast travel, which is the gap the map had. \
             Nothing was appended and nothing moved; only fields in rows the constructor already \
             built were written; they sit in {candidate_blocks} distinct block(s), which is how \
             many separately markable places they represent. {hidden} whole-dungeon marker(s) \
             hidden because their real points \
             now stand in for them. Skipped: {unprojectable} unprojectable, {unlayered} with no map \
             layer, {unnamed} with no place name."
        ));
    } else {
        // Every gate passed and there WERE fresh blocks, yet nothing was claimed. Without this the
        // outcome is indistinguishable from the gates never opening.
        refuse(if unnamed == candidates {
            "every fresh block projected onto the map but none is near a named place, and a row              with no label text id is never drawn -- a claimed slot would be an invisible pin"
        } else if unprojectable == candidates {
            "every fresh block failed to project through the ViewModel's own converters"
        } else if unlayered == candidates {
            "every fresh block projected but no map layer bit could be resolved for its converter"
        } else {
            "fresh blocks existed and all gates passed, but each candidate was individually              refused (mixed projection / layer / place-name failures)"
        });
    }
    claimed
}

#[cfg(not(windows))]
pub unsafe fn top_up_live_pins() -> usize {
    0
}

/// Stop drawing the injected row that carries `param_index`, and report whether one was found.
///
/// Used to retire a whole-dungeon marker once the dungeon's real points are on the map. The layer
/// mask is the field `WorldMapPinData::UpdateVisible` (0x14087afa0) ANDs into the draw flag, so
/// clearing it is how a row stops being drawn without being removed -- and removal is exactly what
/// is forbidden here, because shrinking the list would move every row after it.
///
/// The row is found by scanning OUR span for the row that points at that param index, rather than
/// by assuming row order matches registry order. It does not: the injection drops targets no
/// converter placed, so row `i` and registry entry `i` diverge after the first drop.
///
/// # Safety
/// Game task thread; the span is the one this module appended and every row is re-verified as ours
/// before it is written.
#[cfg(windows)]
unsafe fn hide_injected_row_for_param_index(
    list_begin: usize,
    param_index: usize,
    slab: (usize, usize),
) -> bool {
    let span_begin = LIVE_SPAN_BEGIN.load(Ordering::SeqCst);
    let span_end = LIVE_SPAN_END.load(Ordering::SeqCst);
    if span_begin == 0 || span_end <= span_begin || span_begin < list_begin {
        return false;
    }
    for row in (span_begin..span_end).step_by(PIN_ROW_STRIDE) {
        if row_is_verifiably_ours(row, slab) != Some(param_index) {
            continue;
        }
        // SAFETY: the row carries our own param pointer at the exact slab stride, so it is one this
        // module appended. Both fields are plain scalars the engine only reads.
        unsafe { *((row + ROW_LAYER_MASK_OFFSET) as *mut u32) = 0 };
        let generation = (RESTYLE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1) as u32;
        unsafe { *((row + ROW_ID_OFFSET) as *mut i32) = stamped_row_id(generation, param_index) };
        return true;
    }
    false
}

/// Rows the shipped list held before our span began.
fn existing_row_count(geometry: &PinListGeometry) -> usize {
    let span_begin = LIVE_SPAN_BEGIN.load(Ordering::SeqCst);
    if span_begin <= geometry.begin {
        return 0;
    }
    (span_begin - geometry.begin) / PIN_ROW_STRIDE
}
