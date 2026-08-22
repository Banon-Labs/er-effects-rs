// === Stats-panel NATIVE TEXT (2026-07-04) =========================================================
// Render the character's attributes as text on the ProfileSelect rows using the GAME'S OWN font, each
// value in its own field. The 05_010 GFX edit (`er_gfx::title_05_010`) removes the 128px face box and
// adds a dedicated `ErStats` DefineEditText to the row template; the DLL hooks the native row-populate
// template `FUN_1408758d0` (the only writer of PlayerName/Level/Location/PlayTime) and, BEFORE calling
// the original (which destroys the row proxy at its end -- callee-dtor convention), resolves the
// row's `ErStats` child natively (assignComponentWithName) and pushes the attribute line through the
// native SetText wrapper -- the engine wraps the string in HTML and renders it through the field's own
// `MenuFont_01`. No font work, no new Scaleform surface, no substitution of native fields' text.
// (RE: bd profileselect-native-settext-RE-2026-07-04 + Ghidra dump FUN_1408758d0/FUN_14074c630;
// ARM/CONSUME SetText substitution was rejected because the FMG-static populate pass calls the SetText
// CORE directly, so no wrapper-level routing can target a field the row-populate never writes.)
/// `CS::CSScaleformValue::~CSScaleformValue` (dump 0x140d7f900) = deobf/live 0xd7f850
/// (`scripts/dump-deobf-shift.py` -> content-unique). fastcall(rcx=CSScaleformValue*). Releases the
/// GFx::Value handle a resolved child proxy holds; the stats push calls it exactly like the native
/// row-populate does after each field.
pub(crate) const CSSCALEFORMVALUE_DTOR_RVA: usize = 0xd7f850;
/// SceneObjProxy layout, corrected 2026-07-04 from the Ghidra dump structures + `ComponentProxy::
/// ComponentProxy` (dump 0x1407331b0) after the er-effects-rs-7e7 crash: +0x00 vfptr, +0x08/+0x10/
/// +0x18 an intrusive component-link node (ctor initializes all three to `self`; the resolve links
/// the live component object in), +0x20 context, +0x28 the EMBEDDED `CSScaleformValue` (0x38 bytes:
/// vfptr + GFx::Value). The old claim that the CSScaleformValue sits at +0x8 was WRONG -- +0x8 is
/// the component slot whose OBJECT POINTER the SetText wrapper reads (`rcx = *(proxy+0x8)`) and
/// virtual-calls (`vt+0x8` GetValue). Native row-populate `FUN_1408758d0` passes `&proxy->field_0x8`
/// to `FUN_14074a0f0` and destroys `&proxy.scaleformValue` (+0x28) afterward.
pub(crate) const SCENE_OBJ_PROXY_COMPONENT_SLOT_OFFSET: usize = 0x8;
/// Vtable slot the SetText wrapper dispatches on the linked component (`call *0x8(vt)` at
/// deobf 0x74a00f) -- the GetValue accessor returning the GFx::Value*. The 7e7 guard validates the
/// function pointer in this slot before allowing the wrapper's unvalidated dispatch to run.
pub(crate) const COMPONENT_GET_VALUE_VTABLE_SLOT_OFFSET: usize = 0x8;
/// Offset of the embedded `CSScaleformValue` inside a SceneObjProxy -- the correct
/// `CSSCALEFORMVALUE_DTOR_RVA` target after an `assignComponentWithName` resolve (the ctor
/// constructs it unconditionally on both the empty-name and named paths). Destroying +0x8 instead
/// (the pre-7e7 bug) stamps a vtable over the component-link node and "releases" whatever
/// `proxy+0x20` holds -- a UAF corrupter.
pub(crate) const SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET: usize = 0x28;
/// Stack size for an out SceneObjProxy passed to `assignComponentWithName`. The native row-populate
/// reserves 0x70 bytes; the binder fully constructs the out proxy without reading it, so a zeroed
/// buffer with headroom is safe.
pub(crate) const SCENE_OBJ_PROXY_STACK_BYTES: usize = 0x80;
/// GFx text object pointer inside the `GFx::Value` handle reached from a `CSScaleformValue` object
/// value. Native text helpers (`FUN_140d841b0`/`FUN_140d84220`/`FUN_140d84340`) use
/// `*(value.value + 0x88)` and require its virtual type tag to be 4 before touching the text doc.
pub(crate) const GFX_VALUE_TEXT_OBJECT_OFFSET: usize = 0x88;
/// Text object vtable slot returning its runtime type tag; 4 means TextField/text object for the
/// direct native text helpers above.
pub(crate) const GFX_TEXT_OBJECT_KIND_VTABLE_SLOT: usize = 0x290;
pub(crate) const GFX_TEXT_OBJECT_KIND_TEXT_FIELD: i32 = 4;
/// Text object slot holding the backing text document pointer (`text_object[0x1c]`).
pub(crate) const GFX_TEXT_OBJECT_TEXT_DOC_OFFSET: usize = 0xe0;
/// Backing text document bounds consumed by the native reflow routine. `+0xb0..+0xbc` are the source
/// x_min/y_min/x_max/y_max; `+0x80..+0x8c` are recomputed visible/layout bounds.
pub(crate) const GFX_TEXT_DOC_SOURCE_LEFT_OFFSET: usize = 0xb0;
pub(crate) const GFX_TEXT_DOC_SOURCE_RIGHT_OFFSET: usize = 0xb8;
pub(crate) const GFX_TEXT_DOC_LAYOUT_LEFT_OFFSET: usize = 0x80;
pub(crate) const GFX_TEXT_DOC_LAYOUT_RIGHT_OFFSET: usize = 0x88;
/// Native text document layout output from `GFx::TextField` reflow (`FUN_14114bf10`). These are not
/// SetText inputs: the reflow routine writes `content_width`/`content_height` after walking the live
/// text line/glyph records, and `layout_record_count` is the vector count it iterates for the rendered
/// text layout. This is the no-more-human-verdict oracle for `Madd` vs `Maddened Bean`: a shortened
/// renderer layout has the short content width/record count even if a SetText call accepted full text.
pub(crate) const GFX_TEXT_DOC_LAYOUT_RECORD_COUNT_OFFSET: usize = 0x58;
pub(crate) const GFX_TEXT_DOC_CONTENT_WIDTH_OFFSET: usize = 0xc0;
pub(crate) const GFX_TEXT_DOC_CONTENT_HEIGHT_OFFSET: usize = 0xc4;
/// `GFx::TextField`/text document reflow routine. It recomputes layout bounds from the source bounds,
/// refreshes wrapping/alignment, updates scroll range, and invalidates the backing render state.
pub(crate) const GFX_TEXT_DOC_REFLOW_RVA: usize = 0x114bf10;
/// `GFx::TextField::SetSelection(field, begin, end)` -- the caret/selection primitive, and the ONLY
/// thing that moves the caret in the 02_990 path editor. The native SoftwareKeyboard owns no caret at
/// all: `EnterName_` (0xe70c00) writes a prompt string, max length and flags, and the set-initial path
/// (0xe709f0 -> 0x142416ef0) is a pure `DLString` assign. The caret lives in Scaleform, and this is
/// the function ActionScript's `Selection.setSelection` (impl 0x140f47060) ends up calling once it has
/// resolved the focused character and checked [`GFX_TEXT_OBJECT_KIND_VTABLE_SLOT`] == 4.
///
/// It takes the same text object the native text helpers do (`*(value + 0x88)`), creates the field's
/// editor kit if absent, CLAMPS both indices to the current text length, then invalidates for redraw.
/// The clamp is why caret-to-end needs no string length: pass [`GFX_TEXT_FIELD_SELECTION_END`] for
/// both and the field itself resolves it to the end -- exactly what AS does when `setSelection` is
/// called without an end argument (it defaults end to `i64::MAX`).
///
/// Byte-verified against `eldenring-deobf.bin`: `48 89 5c 24 10 48 89 74 24 18 57 48 83 ec 20`
/// (`MOV [RSP+0x10],RBX; MOV [RSP+0x18],RSI; PUSH RDI; SUB RSP,0x20`), matching the 1.16.2 dump at
/// the same VA (shift 0). See bd `path-editor-caret-to-end-setselection-141198e50-2026-08-12`.
pub(crate) const GFX_TEXT_FIELD_SET_SELECTION_RVA: usize = 0x1198e50;
/// Selection index meaning "end of the text". The native setter clamps to the live text length, so
/// this is a request for the end rather than a guess at a position.
pub(crate) const GFX_TEXT_FIELD_SELECTION_END: i64 = i64::MAX;
/// Re-entrancy guard for the row-populate hook's `ErStats` push (its resolve re-enters the named-child
/// binder hook): skip the push block while set.
pub(crate) use er_telemetry::counters::PROFILE_STATS_PUSH_IN_PROGRESS;
/// Count of row populates observed (PlayerName trigger fired) while the stats panel is on (oracle).
pub(crate) use er_telemetry::counters::PROFILE_STATS_ROW_POPULATES;
/// Count of successful ErStats pushes (assign resolved an editable field and SetText accepted) (oracle).
pub(crate) use er_telemetry::counters::PROFILE_STATS_SETTEXT_SUBS;
/// Count of push attempts that failed (field missing -- e.g. GFX edit not served -- or SetText
/// rejected the value) (oracle).
pub(crate) use er_telemetry::counters::PROFILE_STATS_PUSH_FAILURES;
/// Count of native `PlayerName` SetText attempts made from the per-slot save-name cache.
pub(crate) use er_telemetry::counters::PROFILE_PLAYER_NAME_PUSH_ATTEMPTS;
/// Count of successful native `PlayerName` SetText pushes.
pub(crate) use er_telemetry::counters::PROFILE_PLAYER_NAME_SETTEXT_SUBS;
/// Count of rejected native `PlayerName` SetText pushes.
pub(crate) use er_telemetry::counters::PROFILE_PLAYER_NAME_PUSH_FAILURES;
/// Count of slot names decoded from the live `.sl2` cache.
pub(crate) use er_telemetry::counters::PROFILE_SLOT_NAMES_DECODED;
/// Count of pushes SKIPPED fail-closed because the resolved component at proxy+0x8 was not a live
/// image-vtabled object (er-effects-rs-7e7: a stale/garbage-vt component here crashed the native
/// SetText wrapper's unvalidated `call *0x8(vt)` dispatch). Distinct from PUSH_FAILURES so telemetry
/// separates "field missing / SetText rejected" from "component stale -- crash avoided" (oracle).
pub(crate) use er_telemetry::counters::PROFILE_STATS_PUSH_STALE_SKIPS;
/// Last stale component pointer observed by the fail-closed guard (diagnostic for 7e7 root-cause).
pub(crate) use er_telemetry::counters::PROFILE_STATS_PUSH_STALE_LAST_COMP;
/// Last stale component's vtable pointer observed by the fail-closed guard.
pub(crate) use er_telemetry::counters::PROFILE_STATS_PUSH_STALE_LAST_VT;
/// Row-populate template `FUN_1408758d0` (deobf/live 0x8757e0; dump 0x1408758d0 -> shift -0xf0,
/// content-unique). `longlong(rowModel* rcx, SceneObjProxy* rdx, undefined8 r8, undefined8 r9)`. The
/// only writer of the per-row PlayerName/Level/Location/PlayTime fields, invoked once per visible
/// ProfileSelect list row with a PER-SLOT row model. We hook its ENTRY so we can push the correct
/// slot's attributes BEFORE the original runs (the original destroys the row proxy's embedded
/// `CSScaleformValue` at its end, so a post-call resolve would operate on a released value).
pub(crate) const PROFILE_ROW_POPULATE_RVA: usize = 0x8757e0;
/// Row-model BUILDER `FUN_1408752c0(rowModel, int slot)` -- the only reader of a slot's
/// `ProfileSummary` record on the way to a row.
///
/// It resolves the record (`FUN_140261b80(summary, slot)`), pulls the level from `record[0x24]`, the
/// play time from `record[0x28]` and the **PlaceName id from `record[0x34]`**, then hands all three
/// to the field filler `FUN_1408759e0`, which turns the place id into the row model's `+0x90`
/// `Location` MenuString via `FUN_1407611c0` = `MsgRepository::GetAndFormat(id, "PlaceName", "PN")`.
///
/// That ordering is why the record must be lent its corrected place name HERE and not at
/// [`PROFILE_ROW_POPULATE_RVA`]: by the time the populate runs, `Location` is already a formatted
/// string and the record is no longer consulted. Patching there changed nothing on screen while the
/// telemetry happily reported a repair (observed 2026-08-07 -- seven rows still read the stale
/// "Midra's Manse").
///
/// Byte-verified against `eldenring-deobf.bin` before hooking: `40 55 56 57 48 81 ec 90 00 00 00`,
/// identical to the 1.16.2 dump's disassembly at the same VA (shift 0).
pub(crate) const PROFILE_ROW_MODEL_BUILD_RVA: usize = 0x8752c0;
/// Title/load-current-character row builder `FUN_140951220`: builds a transient current-player
/// `MenuSaveDataSummary` on its own STACK, then calls [`PROFILE_ROW_POPULATE_RVA`] to fill
/// PlayerName/Level/Location/PlayTime/Icon_0. Hooked after the original to push `ErCharStats` on
/// the same row proxy for the title Load Game view.
///
/// CORRECTED 2026-08-07 (decompile of `FUN_140951220`). This previously claimed the builder
/// "returns without going through the per-slot row-populate hook entry we own". It does NOT: the
/// body ends `pSVar3 = SceneObjProxy(&local_188, param_2); FUN_1408757e0(local_128, pSVar3);
/// FUN_140875030(local_128);` -- an ordinary call, so our entry hook on 0x8757e0 DOES see this row.
/// Believing the old comment cost a debugging session: a change was added to a second hook to work
/// around a bypass that never existed, while the real defect sat in the first hook's own guard.
///
/// What IS distinctive here: `FUN_1408753f0` builds that transient summary as
/// `FUN_1408759e0(summary, 0, &name, pgd->level)` -- SLOT INDEX 0 with the LIVE player's level. So a
/// row reaching the per-slot hook with `rowModel + 0x8 == 0` may be this current-player row rather
/// than save slot 0, and anything keyed on that slot index (a stats-cache lookup, a "is this the
/// picker?" test) will be wrong for it. Read per-row values off the row model instead.
pub(crate) const PROFILE_CURRENT_ROW_POPULATE_RVA: usize = 0x951220;
/// Row-model field holding the profile/save slot index (0-9). The native populate reads
/// `*(int*)(rowModel + 0x8) + 1` as the `Icon_0` face-sprite frame, i.e. the slot; we read the same
/// field to index the per-slot stats cache so each row shows ITS OWN character's attributes.
pub(crate) const PROFILE_ROW_MODEL_SLOT_08_OFFSET: usize = 0x8;
/// Offset of the row model's `PlayerName` `CS::MenuString` inside `CS::MenuSaveDataSummary`.
/// Static RE of 1.16.2 `FUN_1408757e0`: native row populate resolves `PlayerName`, reads raw
/// pointer at `rowModel + 0x50`, else falls back to the inline DLString buffer at `rowModel + 0x60`
/// when the DLString capacity at `rowModel + 0x78` is heap-backed. Stage this field before native
/// populate so the game's own writer, not a later out-of-band SetText, owns the rendered text.
pub(crate) const PROFILE_ROW_MODEL_PLAYER_NAME_MENUSTRING_50_OFFSET: usize = 0x50;

// === PER-SLOT INFO FIELDS ON A PROFILESELECT ROW =================================================
// WHAT PRODUCES "Level 0" AND "0:00:00" ON A ROW, and therefore why zeroing the staged record could
// never blank them (static RE of the 1.16.2 dump, which is the live image -- shift 0; row template
// = sprite 76 of `05_010_profileselect.gfx`, names read off the movie the DLL actually serves):
//   * `StaticText_110502` -- the "Level" CAPTION, an ENGINE-populated static field. The named-child
//     binder hands every child's name to the FMG static-text pass `FUN_14074c540`, which matches the
//     `StaticText_` prefix from the table at `PTR_s_StaticText__142a94aa0`, `atoi`s the trailing id
//     (110502) and calls the SetText CORE `FUN_140d842a0` DIRECTLY. The row-populate never writes
//     it, so it is written once per bind and survives row-clip reuse.
//   * `Level` -- the level VALUE. `FUN_140749ed0(&proxy->comp, *(u32*)(rowModel + 0x88))`, which
//     `DLTX::DLString::FormatW(L"%d")`s the int and SetTexts the digits. No int formats to nothing,
//     so a zeroed (or NULL, `FUN_140875790`) record still renders "0".
//   * `PlayTime` -- `FUN_14074a000(&proxy->comp, playtime)`, the string being the row model's own
//     `CS::MenuSaveDataSummary` playtime (+0xc8 literal, else the DLString at +0xd0), formatted from
//     seconds by `FUN_140875be0` inside the row-model builder `FUN_1408759e0`.
// LEVER, LEVEL FIELDS: neither can be emptied through the record, and a post-populate re-resolve is
// impossible (the named-child ctor 0x14074a7c0 resolves out of the PARENT proxy's embedded value at
// +0x28, which the populate destroys at its end). So a browse row HIDES them, per row, through the
// game's own visibility wrapper (`TITLE_PRESS_START_SET_VISIBLE_RVA`) -- content-free in both
// directions, which is what makes the restore as exact as the hide.
// LEVER, LOCATION/PLAYTIME: both strings come from the row model. A browse save-FILE row stages its
// timestamp into `Location` (top-right, same line as PlayerName) and hides the bottom `PlayTime`, so
// `ER0000.sl2` and `YYYY-MM-DD HH:MM` occupy one row instead of two vertical bands.
/// Offset of the row model's level word -- the exact `u32` the native `Level` field formats
/// (`FUN_140749ed0(&proxy->comp, *(u32*)(rowModel + 0x88))`, see the note above).
///
/// Reading the MERGED header's level from here rather than from a slot-indexed cache removes the
/// slot-mapping question entirely: whatever number the native field would have printed on THIS row
/// is the number the merged string prints, per-slot rows and the transient current-player row alike.
/// Static RE of `FUN_1408753f0` shows the current-player summary is built as
/// `FUN_1408759e0(summary, 0, &name, pgd->level)` -- slot 0 with the LIVE level -- so a cache lookup
/// keyed on that slot would describe save slot 0's character, not the loaded one.
pub(crate) const PROFILE_ROW_MODEL_LEVEL_88_OFFSET: usize = 0x88;
pub(crate) const PROFILE_ROW_LEVEL_CAPTION_FIELD_NAME: &str = "StaticText_110502\0";
pub(crate) const PROFILE_ROW_LEVEL_VALUE_FIELD_NAME: &str = "Level\0";
pub(crate) const PROFILE_ROW_LOCATION_FIELD_NAME: &str = "Location\0";
pub(crate) const PROFILE_ROW_PLAYTIME_FIELD_NAME: &str = "PlayTime\0";
/// Full-width row chrome. The drive row hides both because its independent button frames are the
/// only truthful click targets; every other row explicitly restores them.
pub(crate) const PROFILE_ROW_BACKING_FIELD_NAME: &str = "Backing\0";
/// The fields WE added to the row. They need the same per-row-kind visibility statement the native
/// ones get: the row clips are recycled across the character/browse/drive lists, so a field only one
/// kind mentions keeps that kind's text when another kind reuses the clip.
pub(crate) const PROFILE_ROW_ER_STATS_FIELD_NAME: &str = "ErStats\0";
pub(crate) const PROFILE_ROW_CHAR_STATS_FIELD_NAME: &str = "ErCharStats\0";
pub(crate) const PROFILE_ROW_DRIVE_CELL_FIELD_NAMES:
    [&str; er_gfx::title_05_010::DRIVE_CELL_CAPACITY] =
    er_gfx::title_05_010::DRIVE_CELL_FIELD_NAMES_NUL;
/// Native button-frame children paired one-to-one with `DriveCell_0..25`. Their visibility must
/// match the corresponding text field exactly: a blank cell must leave no empty frame, and a
/// recycled character row must inherit neither the label nor its button chrome.
pub(crate) const PROFILE_ROW_DRIVE_BUTTON_FIELD_NAMES:
    [&str; er_gfx::title_05_010::DRIVE_CELL_CAPACITY] =
    er_gfx::title_05_010::DRIVE_BUTTON_FIELD_NAMES_NUL;
pub(crate) const PROFILE_ROW_CURRENT_PATH_FIELD_NAME: &str =
    er_gfx::title_05_010::CURRENT_PATH_FIELD_NAME_NUL;
pub(crate) const PROFILE_ROW_CURRENT_PATH_BUTTON_NAME: &str =
    er_gfx::title_05_010::CURRENT_PATH_BUTTON_NAME_NUL;
/// Offset of the row model's `Location` `CS::MenuString` inside `CS::MenuSaveDataSummary`. Same
/// inline accessor as PlayTime: raw pointer first, else the inline DLString buffer.
pub(crate) const PROFILE_ROW_MODEL_LOCATION_MENUSTRING_90_OFFSET: usize = 0x90;
/// Offset of the row model's `PlayTime` `CS::MenuString` inside `CS::MenuSaveDataSummary`. The
/// struct is `{ wchar_t* rawString; DLString<wchar_t> dLString; }` (Ghidra `CS::MenuString`, 0x38
/// bytes) and every reader takes `rawString` when it is non-NULL, else the DLString's buffer -- the
/// row-populate `FUN_1408757e0` and the FMG static pass `FUN_14074c540` both spell that same
/// accessor inline. So pointing `rawString` at our own UTF-16 for the duration of ONE native
/// populate makes the game's own `FUN_14074a000` write our text, with no second SetText and nothing
/// left behind: `~MenuString` (0x1401bcc60) frees only the DLString, never `rawString`, and we
/// restore the displaced pointer as soon as the populate returns.
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const PROFILE_ROW_MODEL_PLAY_TIME_MENUSTRING_C8_OFFSET: usize = 0xc8;
/// Count of rows whose `PlayTime` was replaced with the file's last-saved timestamp (oracle).
pub(crate) use er_telemetry::counters::PROFILE_ROW_LAST_SAVED_ROWS;
/// Count of rows where the last-saved text could not be staged into the row model (the model field
/// was unreadable), so the row kept the native playtime string (oracle).
pub(crate) use er_telemetry::counters::PROFILE_ROW_LAST_SAVED_STAGE_FAILURES;
/// GFx value type (`CSScaleformValue+0x20 & 0x8f`) the visibility setter `FUN_140d844d0` requires:
/// it returns without doing anything unless the resolved value is a DISPLAY OBJECT (10). Recorded
/// per call as an oracle so "the fields are still visible in game" is diagnosable from telemetry
/// instead of guesswork -- a non-10 type means the hide silently no-ops.
pub(crate) const GFX_VALUE_TYPE_DISPLAY_OBJECT: usize = 10;
/// `GFx::Value::VT_Undefined` / `VT_Null` -- what a named-child resolve leaves in the out proxy when
/// the movie has NO child by that name.
///
/// This is the only observable difference between a hit and a miss, and missing it is what let one
/// movie's presentation be applied to another's. `SceneObjProxy::assignComponentWithName`
/// (`0x14074a2f0`) always returns a fully constructed out proxy: the named-child ctor
/// (`0x14074a7c0`) runs `ComponentProxy::ComponentProxy` -- which initialises the component slot at
/// +0x8 to the proxy ITSELF -- and then asks `FUN_140d7f9d0` for the member. On a miss the member
/// lookup writes an undefined `GFx::Value` and the self-link stands, so `*(out + 0x8)` is non-null
/// and its vtable IS in the game image. Any "did the field resolve?" test written on the component
/// pointer therefore answers YES for every name on every movie. Measured on the live game: 5
/// non-display resolves per populate on the System>Quit summary, exactly the five field names this
/// mod injects into `05_010_ProfileSelect` and that `02_040_OptionSetting` does not have.
pub(crate) const GFX_VALUE_TYPE_UNDEFINED: usize = 0;
pub(crate) const GFX_VALUE_TYPE_NULL: usize = 1;
/// True when a named-child resolve actually found a child (as opposed to leaving the out proxy's
/// value undefined).
pub(crate) fn gfx_value_type_is_resolved(datatype: usize) -> bool {
    datatype != GFX_VALUE_TYPE_UNDEFINED && datatype != GFX_VALUE_TYPE_NULL
}
/// Offset of the GFx value's type word inside a `CSScaleformValue` (the `lVar+0x20` every setter
/// guards on: `CSScaleformValue` = vfptr + `GFx::Value`, whose `dataType` lands at +0x20).
pub(crate) const CSSCALEFORMVALUE_DATATYPE_20_OFFSET: usize = 0x20;
/// MSVC's `_purecall` (`0x14251c480`) and the handler it jumps to, `purecall_crash_handler`
/// (`0x140c90080` = `xor eax,eax; mov dword [rax], 0xdead; ret`) -- a deliberate write to NULL.
///
/// THIS IS HOW A DESTROYED C++ OBJECT ANSWERS A VIRTUAL CALL. When an object is destructed its
/// vtable pointer is left describing an abstract base, whose slots are filled with `_purecall`; the
/// dispatch then "succeeds" and lands in the trap. Every liveness check in this module used to ask
/// only whether the resolved function pointer lies inside the game image -- and `_purecall` does,
/// so a destroyed component passed the check and the very next indirect call killed the process
/// (2026-08-07 17:03:32, `access-violation rva=0xc90082 access=1 fault_addr=0x0`, backtrace
/// `eldenring.exe+0x251c4b4` -> `+0x251c480` -> our DLL). Ruling these two targets out is what turns
/// "the object is gone" from a crash into an error string.
pub(crate) const PURECALL_RVA: usize = 0x251c480;
pub(crate) const PURECALL_CRASH_HANDLER_RVA: usize = 0xc90080;

/// True when an about-to-be-called vtable slot is the pure-virtual trap, i.e. the object behind it
/// has been destructed. Check this before EVERY indirect call through a resolved component.
pub(crate) fn dispatch_target_is_purecall(target: usize, base: usize) -> bool {
    target == base + PURECALL_RVA || target == base + PURECALL_CRASH_HANDLER_RVA
}
/// Count of rows the hide was DRIVEN on because the row has no character -- i.e. the native setter
/// was called for at least one of the three fields; pair it with `_NON_DISPLAY` below to know the
/// call took effect (oracle). Also the latch that arms the symmetric re-show: until it is non-zero
/// nothing was ever driven, so the vanilla path stays completely untouched.
pub(crate) use er_telemetry::counters::PROFILE_ROW_SLOT_INFO_HIDDEN_ROWS;
/// Count of rows the re-show was driven on (oracle). Row clips are reused across rows and across
/// windows, so every row that KEEPS its slot info re-asserts visibility.
pub(crate) use er_telemetry::counters::PROFILE_ROW_SLOT_INFO_SHOWN_ROWS;
/// Count of per-field visibility calls skipped fail-closed: the child did not resolve, or the out
/// proxy did not carry the game's own `CS::SceneObjProxy` vtable (oracle).
pub(crate) use er_telemetry::counters::PROFILE_ROW_SLOT_INFO_VIS_SKIPS;
/// Count of per-field visibility calls whose resolved value was NOT a display object, i.e. calls the
/// native setter ignored (oracle). Non-zero means this lever does not reach that field.
pub(crate) use er_telemetry::counters::PROFILE_ROW_SLOT_INFO_NON_DISPLAY;
/// Last GFx value type observed by the visibility path (diagnostic companion to the counter above).
pub(crate) use er_telemetry::counters::PROFILE_ROW_SLOT_INFO_LAST_DATATYPE;
/// Count of summary populates handed back to the game untouched because the row is not ours (oracle).
pub(crate) use er_telemetry::counters::PROFILE_FOREIGN_SUMMARY_ROWS;
/// Count of summary populates on our own edited ProfileSelect row template (oracle).
pub(crate) use er_telemetry::counters::PROFILE_OWN_SUMMARY_ROWS;
/// Count of text pushes refused because the movie has no child by that name (oracle).
pub(crate) use er_telemetry::counters::PROFILE_STATS_PUSH_MISSING_FIELD;
pub(crate) static PROFILE_ROW_POPULATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// Trampoline for [`PROFILE_ROW_MODEL_BUILD_RVA`], and its one-shot install latch.
pub(crate) static PROFILE_ROW_MODEL_BUILD_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static PROFILE_ROW_MODEL_BUILD_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Rows whose record was lent a corrected `PlaceName` id by the builder hook (oracle).
pub(crate) static PROFILE_ROW_PLACE_NAME_REPAIRS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_CURRENT_ROW_POPULATE_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::PROFILE_ROW_POPULATE_INSTALLED;
/// `PlayerGameData::GetName`-style native getter used by System/Quit and the current ProfileSelect
/// summary builder. Static RE of 1.16.2 `FUN_14025f8e0`: it reads the word-checked string at
/// `PlayerGameData + 0x8e8`, which can hold the shortened display name (`Madd`) even while the raw PGD
/// name at `+0x9c` is the full save name (`Maddened Bean`). The product UI wants the save name, so the
/// hook below overrides only the main loaded player's getter result with the raw PGD name.
pub(crate) const PLAYER_GAME_DATA_NAME_GETTER_RVA: usize = 0x25f8e0;
pub(crate) static PLAYER_GAME_DATA_NAME_GETTER_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static PLAYER_GAME_DATA_NAME_GETTER_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PLAYER_GAME_DATA_NAME_GETTER_OVERRIDE_LOGGED: AtomicUsize = AtomicUsize::new(0);
/// Per-slot stats cache state (oracle): 0 = not attempted, 1 = loaded (`.sl2` read + parsed), 2 =
/// load failed (save unreadable/too small) -- the hook then falls back to the loaded character.
pub(crate) use er_telemetry::counters::PROFILE_SLOT_STATS_CACHE_STATE;
/// Count of per-slot cache drops on a save swap (oracle).
pub(crate) use er_telemetry::counters::PROFILE_SLOT_CACHE_INVALIDATIONS;
/// Count of per-slot cache refills from picker-held bytes (oracle).
pub(crate) use er_telemetry::counters::PROFILE_SLOT_CACHE_PREVIEW_RELOADS;
/// Count of save slots that decoded to a real character in the per-slot stats cache (oracle).
pub(crate) use er_telemetry::counters::PROFILE_SLOT_STATS_DECODED;
pub(crate) use er_telemetry::counters::TITLE_PRESS_START_BIND_HITS;
pub(crate) static TITLE_PRESS_START_BIND_LAST_PARENT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_PRESS_START_BIND_LAST_OUT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_PRESS_START_BIND_LAST_NAME: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_PRESS_START_BIND_LAST_CONTEXT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::TITLE_PRESS_START_BIND_HIDE_CALLS;
pub(crate) use er_telemetry::counters::TITLE_PRESS_START_GFX_HIDE_CALLS;
pub(crate) use er_title_flow::TITLE_LOGO_BACK_VIEW_PARTS_AA8_OFFSET;
pub(crate) use er_title_flow::TITLE_LOGO_BACK_VIEW_PARTS_NAME;
pub(crate) use er_title_flow::TITLE_LOGO_RESOURCE_NAME;
/// `TitleBackViewParts` embeds its `SceneObjProxy` at `this+0x70`; the GFx/ScaleformValue handle
/// used by the native label/frame helpers is the proxy field at `this+0x88` (`SceneObjProxy+0x18`).
pub(crate) const TITLE_LOGO_GFX_VALUE_88_OFFSET: usize = 0x88;
/// Current-frame reader for the resolved Scaleform value (`FUN_140d82620`, dump 0x140d82620 ->
/// live/deobf 0x140d82570). Static FFDec XML for `05_001_title_logo.gfx` shows root depth 3 is the
/// visible logo surface and maps frames to alpha: FadeIn 2..60, TextFadeIn 60, TextFadeOut 93,
/// Title_TopMenu 112, FadeOut 113..133.
pub(crate) const TITLE_LOGO_GFX_CURRENT_FRAME_RVA: usize = 0xd82570;
pub(crate) const TITLE_LOGO_GFX_UNKNOWN_FRAME: i32 = -1;
pub(crate) const TITLE_LOGO_GFX_UNKNOWN_ALPHA: i32 = -1;
pub(crate) const TITLE_LOGO_GFX_FULL_ALPHA: i32 = 256;
pub(crate) const TITLE_LOGO_GFX_ROOT_DEPTH: usize = 3;
pub(crate) const TITLE_LOGO_GFX_ROOT_SPRITE_CHAR: usize = 7;
pub(crate) const TITLE_LOGO_GFX_MAIN_ASSET_CHAR: usize = 4;
pub(crate) const TITLE_LOGO_GFX_MAIN_ASSET_NAME: &str = "MENU_Title_EldenRing_01";
/// Stronger native hide lever than FadeIn/FadeOut: `CS::TitleBackViewParts::SetVisible` (dump
/// 0x1409a6410 -> deobf/live 0x1409a62c0, content verified as `add rcx,0x70; jmp 0x140733340`)
/// calls the generic `SceneObjProxy` visible setter on the embedded proxy at `this+0x70`.
/// `TitleTopDialog` itself calls this with `1` in the start-login path (dump 0x1409b3050), so using
/// it with `0` is a native visibility semantic, not a timeline FadeIn/FadeOut guess.
pub(crate) const TITLE_LOGO_BACK_VIEW_PARTS_CTOR_RVA: usize = 0x9a6180;
pub(crate) use er_title_flow::TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA;
/// TitleTopDialog start-login/native accept path (dump 0x1409b3050 -> deobf/live 0x1409b2f00).
/// It calls `TitleBackViewParts::SetVisible(1)` on dialog+0xaa8 before continuing through native
/// login/save-load setup, so detouring it and hiding the logo after the original is the earliest
/// proven TitleTopDialog-owned logo visibility point on the zero-input Continue path.
pub(crate) const TITLE_TOP_START_LOGIN_RVA: usize = 0x9b2f00;
pub(crate) const TITLE_TOP_START_LOGIN_HIDE_NOT_INSTALLED: usize = 0;
pub(crate) const TITLE_TOP_START_LOGIN_HIDE_INSTALLED_YES: usize = 1;
pub(crate) static TITLE_TOP_START_LOGIN_HIDE_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static TITLE_TOP_START_LOGIN_HIDE_INSTALLED: AtomicUsize =
    AtomicUsize::new(TITLE_TOP_START_LOGIN_HIDE_NOT_INSTALLED);
pub(crate) static TITLE_LOGO_SET_VISIBLE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::TITLE_LOGO_SET_VISIBLE_INSTALLED;
pub(crate) static TITLE_LOGO_CTOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::TITLE_LOGO_CTOR_INSTALLED;
pub(crate) use er_telemetry::counters::TITLE_LOGO_GFX_HIDE_CALLS;
pub(crate) use er_title_flow::TITLE_LOGO_GFX_HIDE_LAST_DIALOG;
pub(crate) use er_title_flow::TITLE_LOGO_GFX_HIDE_LAST_LOGO;
pub(crate) use er_title_flow::TITLE_LOGO_GFX_HIDE_LAST_CALLER_PHASE;
pub(crate) static TITLE_LOGO_GFX_HIDE_LAST_REQUESTED_VISIBLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Passive observer for `CSScaleformSystem::AcquireMenuResource` (`dump 0x140d786e0 ->
/// deobf/live 0x140d78630`). Signature from Ghidra dump:
/// `resource = f(CSScaleformSystem* this, CSScaleformLoadInfo* loadParams, u8 flags)`, where
/// `loadParams+0x8` is the UTF-16 resource filename/key. This is the epilogue-neutral seam for
/// replacing the already-scheduled `TitleBackViewParts` / `05_001_Title_Logo` resource; keep the
/// first hook observe-only until the native request/return is proven in telemetry.
pub(crate) const TITLE_MENU_RESOURCE_ACQUIRE_RVA: usize = 0xd78630;
pub(crate) static TITLE_MENU_RESOURCE_ACQUIRE_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::TITLE_MENU_RESOURCE_ACQUIRE_INSTALLED;
pub(crate) use er_telemetry::counters::TITLE_MENU_RESOURCE_ACQUIRE_HITS;
pub(crate) use er_telemetry::counters::TITLE_MENU_RESOURCE_ACQUIRE_LOGO_HITS;
pub(crate) static TITLE_MENU_RESOURCE_ACQUIRE_LAST_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_MENU_RESOURCE_ACQUIRE_LAST_LOAD_PARAMS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_MENU_RESOURCE_ACQUIRE_LAST_FILENAME_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::TITLE_MENU_RESOURCE_ACQUIRE_LAST_PARAM3;
pub(crate) static TITLE_MENU_RESOURCE_ACQUIRE_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_MENU_RESOURCE_ACQUIRE_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Scaleform LoaderImpl file-open wrapper (`dump 0x1411ceda0 -> deobf/live 0x1411ced80`).
/// Signature: `file = f(loader_impl, char* url, flags)`, calls FileOpener vtable +0x18. Observe-only
/// until we know the exact returned file object's vtable/buffer contract.
pub(crate) const TITLE_SCALEFORM_FILE_OPEN_RVA: usize =
    er_game_base::rva::TITLE_SCALEFORM_FILE_OPEN_RVA;
pub(crate) static TITLE_SCALEFORM_FILE_OPEN_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::TITLE_SCALEFORM_FILE_OPEN_INSTALLED;
pub(crate) use er_telemetry::counters::TITLE_SCALEFORM_FILE_OPEN_HITS;
pub(crate) use er_telemetry::counters::TITLE_SCALEFORM_FILE_OPEN_LOGO_HITS;
pub(crate) static TITLE_SCALEFORM_FILE_OPEN_LAST_LOADER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_SCALEFORM_FILE_OPEN_LAST_URL_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::TITLE_SCALEFORM_FILE_OPEN_LAST_FLAGS;
pub(crate) static TITLE_SCALEFORM_FILE_OPEN_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_SCALEFORM_FILE_OPEN_LAST_RET_VTABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_SCALEFORM_FILE_OPEN_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// MemoryFile-backed replacement state for `ER_EFFECTS_TITLE_RESOURCE_MEMORY_GFX`. The replacement
/// is deliberately opt-in: default file-open observer mode still calls the original loader.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const SCALEFORM_MEMORY_GLOBAL_RVA: usize = 0x4593250;
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const SCALEFORM_DLSTRING_CHAR_COPY_RVA: usize = 0x1140ec0;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SCALEFORM_MEMORY_FILE_SIZE: usize = 0x30;
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const SCALEFORM_MEMORY_FILE_REFCOUNT_OFFSET: usize = 0x8;
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const SCALEFORM_MEMORY_FILE_NAME_OFFSET: usize = 0x10;
pub(crate) const SCALEFORM_MEMORY_FILE_CURSOR_OFFSET: usize = 0x24;
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const SCALEFORM_MEMORY_FILE_VALID_OFFSET: usize = 0x28;
pub(crate) use er_telemetry::counters::TITLE_SCALEFORM_MEMORY_GFX_REPLACEMENTS;
/// 05_000_title-only slice of `TITLE_SCALEFORM_MEMORY_GFX_REPLACEMENTS` (which aggregates both the
/// 05_001 logo and 05_000 title slots): the product-strip proof oracle must show the STRIPPED TITLE
/// movie was served on every title visit (cold boot + each System-Quit reload), unambiguously.
pub(crate) use er_telemetry::counters::TITLE_SCALEFORM_05_000_MEMORY_GFX_REPLACEMENTS;
pub(crate) static TITLE_SCALEFORM_MEMORY_GFX_LAST_FILE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

/// Product-default 05_000_title strip is armed for RUNTIME derivation (er-effects-rs-h7x): no
/// embedded stripped movie; the Scaleform file-open hook reads the game's own vanilla bytes out of
/// the native MemoryFile the FileOpener returns (payload owned by GLOBAL_GfxRepository) and applies
/// `er_gfx::title_05_000::strip` -- 18 content-addressed tag edits, all-or-nothing, byte-identical
/// to the previously-embedded `TITLE_05_000_TEXT_SUPPRESSED_GFX` for the known vanilla input
/// (fixture-gated proof in `crates/er-gfx/tests/title_strip.rs`). 0 = disarmed, 1 = armed.
pub(crate) use er_telemetry::counters::TITLE_05_000_RUNTIME_STRIP_ARMED;

/// Wwise post-event core (`AK::SoundEngine::PostEvent` shared numeric-id backend):
/// Ghidra dump `FUN_14223a120`, deobf/live `0x14223a130` (`dump-deobf-shift.py`, content-unique).
/// This is an audio-side semaphore generator: it records actual sound events the engine submits,
/// including the audible startup/title-logo music that has no visible-only oracle.
pub(crate) const SOUND_POST_EVENT_CORE_RVA: usize = 0x223a130;
pub(crate) static SOUND_POST_EVENT_CORE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::SOUND_POST_EVENT_CORE_INSTALLED;
pub(crate) use er_telemetry::counters::SOUND_POST_EVENT_HITS;
pub(crate) use er_telemetry::counters::SOUND_POST_EVENT_MUTED_HITS;
pub(crate) use er_telemetry::counters::SOUND_POST_EVENT_FORWARDED_HITS;
pub(crate) use er_telemetry::counters::SOUND_POST_EVENT_FIRST_ID;
pub(crate) use er_telemetry::counters::SOUND_POST_EVENT_LAST_ID;
pub(crate) use er_telemetry::counters::SOUND_POST_EVENT_FIRST_MUTED_ID;
pub(crate) use er_telemetry::counters::SOUND_POST_EVENT_LAST_MUTED_ID;
pub(crate) use er_telemetry::counters::SOUND_POST_EVENT_LAST_PLAYING_ID;
pub(crate) static SOUND_POST_EVENT_LAST_GAME_OBJECT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::SOUND_POST_EVENT_LAST_FLAGS;
pub(crate) static SOUND_POST_EVENT_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

/// Successful runtime-strip serves (native MemoryFile data/len swapped to the derived movie).
pub(crate) use er_telemetry::counters::TITLE_05_000_RUNTIME_STRIP_SERVES;
/// Runtime-strip failures (unexpected file vtable, unreadable payload, parse/edit/write error).
/// Every failure falls closed to the untouched native file (vanilla title UI).
pub(crate) use er_telemetry::counters::TITLE_05_000_RUNTIME_STRIP_FAILURES;
/// Observed native 05_000 payload length at first successful read (0 until then).
pub(crate) use er_telemetry::counters::TITLE_05_000_RUNTIME_STRIP_INPUT_LEN;
/// Derived stripped movie length (0 until derived).
pub(crate) use er_telemetry::counters::TITLE_05_000_RUNTIME_STRIP_OUTPUT_LEN;
/// Input provenance: 0 = not yet classified, 1 = known vanilla (output proven byte-identical to
/// the validated asset), 2 = unknown input (game update / other mod; edits applied all-or-nothing).
/// NOTE (run 20260702-203258): the LIVE repository payload is 12,176 bytes vs the 12,174-byte
/// extracted corpus file -- 2 trailing bytes after the root End tag -- so class 2 is the expected
/// steady state on the current game build; the output still derived byte-identical.
pub(crate) use er_telemetry::counters::TITLE_05_000_RUNTIME_STRIP_INPUT_CLASS;
/// Whether the derived output matches the validated-asset fingerprint (len 11707 +
/// FNV 0x17906a0e91ce5374): 0 = not derived yet, 1 = byte-identical to the validated v2 asset,
/// 2 = clean all-or-nothing edit result with a DIFFERENT fingerprint (expected only after a game
/// update changes untouched tags; not an error, but the proof of visual equivalence is then open).
pub(crate) use er_telemetry::counters::TITLE_05_000_RUNTIME_STRIP_OUTPUT_VALIDATED;

/// Stats-panel 05_010_ProfileSelect runtime edit armed (mirrors the 05_000 runtime strip): the
/// Scaleform file-open hook reads the game's own vanilla `05_010_profileselect.gfx` payload out of
/// the native MemoryFile and applies `er_gfx::title_05_010::stats_panel` -- 6 content-addressed tag
/// edits (face box removed, `ErStats` field added, left column reflowed), all-or-nothing, output
/// fingerprint-verified for the known vanilla input (`crates/er-gfx/tests/profile_stats.rs`).
/// 0 = disarmed, 1 = armed. Armed with the stats panel itself (product lever, no new env gates).
pub(crate) use er_telemetry::counters::PROFILE_05_010_RUNTIME_EDIT_ARMED;
/// Successful 05_010 runtime-edit serves (native MemoryFile data/len swapped to the derived movie).
pub(crate) use er_telemetry::counters::PROFILE_05_010_RUNTIME_EDIT_SERVES;
/// 05_010 runtime-edit failures (unexpected vtable, unreadable payload, parse/edit/write error).
/// Every failure falls closed to the untouched native file (vanilla ProfileSelect rows).
pub(crate) use er_telemetry::counters::PROFILE_05_010_RUNTIME_EDIT_FAILURES;
/// Observed native 05_010 payload length at first successful read (0 until then).
pub(crate) use er_telemetry::counters::PROFILE_05_010_RUNTIME_EDIT_INPUT_LEN;
/// Derived edited movie length (0 until derived).
pub(crate) use er_telemetry::counters::PROFILE_05_010_RUNTIME_EDIT_OUTPUT_LEN;
/// Input provenance: 0 = unclassified, 1 = known vanilla, 2 = unknown input (the live repository
/// payload may carry trailing bytes after the root End tag, as observed for 05_000).
pub(crate) use er_telemetry::counters::PROFILE_05_010_RUNTIME_EDIT_INPUT_CLASS;
/// Whether the derived output matches the generated-asset fingerprint
/// (`er_gfx::title_05_010::EDITED_LEN` + `EDITED_FNV1A64`, the single source of truth): 0 = not derived
/// yet, 1 = byte-identical, 2 = clean all-or-nothing edit with a different fingerprint (expected after a
/// game update changes untouched tags).
pub(crate) use er_telemetry::counters::PROFILE_05_010_RUNTIME_EDIT_OUTPUT_VALIDATED;

/// Scaleform GFx resource/movie constructor (`dump 0x14116a930 -> deobf/live 0x14116a910`).
/// Signature from dump: `resource = f(out_resource, loader_data, file_type, char* url, file_obj,
/// external_flag, heap_arg)`. After return, `resource+0x40` holds the movie-data pointer.
pub(crate) const TITLE_SCALEFORM_RESOURCE_CTOR_RVA: usize = 0x116a910;
pub(crate) static TITLE_SCALEFORM_RESOURCE_CTOR_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::TITLE_SCALEFORM_RESOURCE_CTOR_INSTALLED;
pub(crate) use er_telemetry::counters::TITLE_SCALEFORM_RESOURCE_CTOR_HITS;
pub(crate) use er_telemetry::counters::TITLE_SCALEFORM_RESOURCE_CTOR_LOGO_HITS;
pub(crate) static TITLE_SCALEFORM_RESOURCE_CTOR_LAST_OUT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_SCALEFORM_RESOURCE_CTOR_LAST_URL_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_SCALEFORM_RESOURCE_CTOR_LAST_FILE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_SCALEFORM_RESOURCE_CTOR_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_SCALEFORM_RESOURCE_CTOR_LAST_MOVIE_DATA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_SCALEFORM_RESOURCE_CTOR_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Dead-end note: `CS::TitleBackViewParts::FadeIn` (dump 0x1409a63f0 -> live 0x1409a62a0)
/// only calls the state transition helper on `this+0x88` with `"FadeIn"`; runtime/user evidence
/// proved suppressing it does NOT hide the visible logo. Keep as RE context, not a product hook.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const TITLE_LOGO_BACK_VIEW_PARTS_FADEIN_RVA: usize = 0x9a62a0;
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const TITLE_PRESS_START_NAME_RVA: usize = 0x2b26500;
/// Diagnostic span: if *(proxy) is NOT SceneObjProxy, scan [proxy .. proxy+0x40] stride 8 logging
/// each qword + its [0] vtable so the next probe reveals the real layout. Also bounds the fallback.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SCENE_PROXY_DIAG_SCAN_SPAN: usize = 0x40;
pub(crate) use er_title_flow::OWN_STEPPER_FALSE;
pub(crate) use er_title_flow::OWN_STEPPER_MENU_OPENED_NO;
/// STAGE1d probes the dialog's FD4 state immediately and opens only on the semantic Loop+latch
/// predicate; it does not use a fixed pre-probe settle delay.
/// Interval (frames) for logging the state probe (FadeIn/Loop/TextFadeOut + latch), so the log
/// shows the dialog progressing without spamming every frame.
pub(crate) const STAGE1D_RETRY_INTERVAL: u64 = 30;
/// Set by cap_sequence_iter_hook when the Sequence iterator first walks a MenuWindowJob child
/// (vt 0x142aa97e8) -- i.e. the main menu actually opened and its entries are registered. The
/// retry loop stops once this is set (the title views tick via a different pump, so this fires
/// ONLY on the real main-menu entries).
pub(crate) const MENU_ENTRIES_SEEN_NO: usize = false as usize;
pub(crate) const MENU_ENTRIES_SEEN_YES: usize = true as usize;
pub(crate) static MENU_ENTRIES_SEEN: AtomicUsize = AtomicUsize::new(MENU_ENTRIES_SEEN_NO);
pub(crate) use er_title_flow::OWN_STEPPER_MENU_OPENED;
/// Count of TitleTopDialog entry-vector dumps emitted (the Continue/Load-Game rows live there,
/// not in the FD4 tree). Capped so the diagnostic samples the entries as they realize after
/// menu-open without spamming the log every frame.
pub(crate) static OWN_STEPPER_TITLETOP_DUMPS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// Max TitleTopDialog entry dumps + the per-dump frame interval.
pub(crate) const OWN_STEPPER_TITLETOP_DUMP_CAP: usize = TraceSampleLimit::Value8 as usize;
/// Recon-only Load-Game fingerprint scan (`scan_dialog_for_loadgame`) counter + cap. Runs in the
/// post-open SAFE-DEFAULT park, independent of the pre-open `OWN_STEPPER_TITLETOP_DUMPS` (which the
/// d180-locate exhausts before menu-open). 2026-06-18 reconciliation: the title rows are
/// TitleTopDialog registry entries, not FD4 jobs.
pub(crate) static OWN_STEPPER_LOADGAME_SCANS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const OWN_STEPPER_LOADGAME_SCAN_CAP: usize = TraceSampleLimit::Value12 as usize;
pub(crate) use er_title_flow::TITLE_STATE_OWNER_GONE;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const FORCE_PLAY_GAME_STATE_UNOBSERVED: i32 = -999;
/// One-shot "PlayGame requested" flag on the TitleStep owner. STEP_PlayGame only
/// runs its real load-trigger (`consume_owner300` 0x140ca89e0 on owner+0x300,
/// gated at 0x140b0d70c) when this byte is nonzero, then clears it. The menu
/// "Continue" selection normally sets it; we set it so the forced PlayGame step
/// actually starts the load instead of resetting via GameStepWait.
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const TITLE_OWNER_PLAY_GAME_REQUEST_FLAG_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, play_game_request_flag);
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const TITLE_OWNER_PLAY_GAME_REQUEST_FLAG_SET: u8 = true as u8;
/// The save slot STEP_PlayGame actually loads. Its handler (0x140b0d5b0) reads
/// `mov eax,[owner+0xbc]` and feeds it through submit -> validate -> pair, which
/// writes the value to GameMan+0x14 (the load value). The +0xac0 save slot only
/// feeds global+0x1200, not the load pair — so this is the field to select.
pub(crate) const TITLE_OWNER_PLAY_GAME_SLOT_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, play_game_slot);
pub(crate) use er_title_flow::TITLE_STEP_GAME_STEP_WAIT;
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const TITLE_OWNER_JOB_PENDING_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLoadJobLayout, pending);
pub(crate) use er_title_flow::FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA;
pub(crate) const TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, new_game_flag);
/// Packed map id for m60_42_34_00 (the new-game default; resolver 0x14071fd60 packs
/// mAA_BB_CC_DD decimal -> byte3=AA..byte0=DD). A valid map to pass the PlayGame
/// map-area gate (area byte 0x32..0x58) while we prove the SetState(5) path builds
/// CSFeMan; the real slot map comes from GameMan+0xc30 once peeked.
pub(crate) const DEFAULT_PLAY_GAME_MAP: i32 = 0x3c2a2200;
/// Full sync slot deserialize 0x14067b290(ecx=slot) -- CSFeMan-LESS (verified): reads
/// the save, writes the real saved map to GameMan+0xc30, applies the character. The
/// cycle-breaker for slot loading (slot9-load-phase-machine-b80-csfeman-less-2026).
pub(crate) const DESERIALIZE_SLOT_RVA: usize = 0x67b290;
/// `GameMan::WriteSaveToSlot(slot, flushToDisk, blankSlot) -> char`. **This WRITES a save;
/// it does not read one.** Corrected 2026-08-01 against the 1.16.2 Ghidra dump after this
/// address was found declared under two contradictory names across three crates (it was
/// `CONTINUE_LOAD_RVA` here and in er-save-loader, `SAVE_DISPATCH_CHAR_RVA` in
/// er-save-suppress -- the latter was right).
///
/// Decompile evidence at 0x14067b750: early-bails on `CanShowSaveMenu()` and clears
/// `GameMan->saveRequested`; requires `GameMan->saveState == 0`; resolves `slot == -1` to
/// `GameMan->saveSlot`; allocates 0x280000 from the main heap; calls
/// `CS::ProfileSummary::MarkProfileIndexAsUsed(summary, slot)`; SERIALIZES game state into
/// that buffer via 0x14067dc00 (which builds a `DLMemoryOutputStream` and calls `Write` /
/// `Seek` / `GetGameDataVersion`); stamps `DLDateTime` into GameMan+0xb88; fetches the IO
/// device via [`IODEV_GETTER_RVA`] (0xe6e060) and writes the buffer out through 0x140e6ec70;
/// then sets `saveState = 1` and clears `saveRequested`.
///
/// `blankSlot == 1` takes the `memset(buf, 0, 0x280000)` branch, i.e. it writes an ERASED
/// slot -- destructive. The prior doc claimed this "submits the 0x280000 save read"; the
/// 0x280000 buffer is the save WRITE buffer.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const SAVE_WRITE_TO_SLOT_RVA: usize = 0x67b750;
pub(crate) use er_title_flow::GAME_MAN_SAVED_MAP_C30_OFFSET;
pub(crate) use er_title_flow::GAME_MAN_RETURN_TITLE_JOB_PREDICATE_BC4_OFFSET;
pub(crate) use er_title_flow::GAME_MAN_RETURN_TITLE_JOB_PREDICATE_READY;
/// submit_play_game 3-phase states: build CSFeMan -> deserialize slot -> re-submit
/// the real map. Driven one step per game-task tick.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SUBMIT_PHASE_INIT: i32 = 0;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SUBMIT_PHASE_BUILT: i32 = 1;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SUBMIT_PHASE_DESER: i32 = 2;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SUBMIT_PHASE_DONE: i32 = 3;
/// Phase C world-priming (ingameinit-decoded-world-built-by-movemapstep-msbload-2026):
/// the world singletons are built by the MoveMapStep's OWN step machine (STEP_MsbLoad),
/// driven by per-frame update 0x140aff640(rcx=MoveMapStep, rdx=FD4TaskData). The
/// MoveMapStep is held at InGameStep(owner+0x2e8)+0xe8. Pump it until child+0xd8 drains.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const MOVEMAPSTEP_UPDATE_RVA: usize = 0xaff640;
pub(crate) use er_title_flow::INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const INGAMESTEP_PENDING_D8_PENDING: i32 = 1;
/// play_game_submit-handoff discriminators on the InGameStep object (own-load-worldreswait-is-block-
/// registration-not-coord-2026-06-22). play_game_submit 0x140aebdc0 sets InGameStep+0xd8=1 and
/// InGameStep+0x100=requested BlockId. So reading these PURELY (no call) tells us whether the native
/// request handoff ran: +0x100 == the saved BlockId (e.g. 0x1c000000) means play_game_submit primed
/// the m28 request; +0x100 == 0/unset means it did NOT. +0xd8 is the matching pending phase byte.
pub(crate) const INGAMESTEP_PHASE_D8_OFFSET: usize = 0xd8;
pub(crate) const INGAMESTEP_REQ_BLOCKID_100_OFFSET: usize = 0x100;
/// Native b80 load INITIATOR 0x14067b4e0(ecx=slot): begins the async slot-IO read
/// and sets GameMan+0xb80=1. The scheduler ticks CSTaskGroup 20 (MoveMapStep) every
/// frame (fd4-scheduler-no-group-active-gate-runs-all-2026), so once initiated the
/// b80 machine (dispatcher-1 deserialize + dispatcher-2 apply) + MsbLoad PRIME the
/// world-stream natively -> resident -> child+0xd8 drains. This is the stream-priming
/// step the direct 0x14067b290 deserialize skipped.
/// **BLANKS THE ENTIRE SAVE CONTAINER.** Not a load initiator, despite the old name.
/// 0x67b4e0 stages one memset-to-zero 0x280000 buffer into EVERY container entry via
/// 0x140e6ec80 (whose callees are `memset` + `SLSaveContent` + `AllocateAligned`), submits on
/// the write lane, and sets `GameMan->saveState = 1`. It is NOT among the code xrefs of the
/// READ builder 0x140e6eb80. Corrected 2026-08-01; nothing in this repo calls it (the two
/// remaining references sit in `let _ = (...)` warning-suppression tuples, and one trace hook
/// observes it), so this is a badly-named loaded gun rather than a live hazard.
pub(crate) const BLANK_SAVE_CONTAINER_REQUEST_RVA: usize = 0x67b4e0;
/// FULL-LOAD (deserialize-arm) initiator 0x14067b1a0(ecx=slot): begins the slot read and
/// sets GameMan+0xb80=2 (the b80==2 deserialize arm), NOT b80=1 (the preview lane that
/// 0x14067b4e0 uses and that resets to 0 without deserializing). Runtime-proven the
/// preview lane never reaches b80==3; the b80=2 arm is the one the poll 0x140679180
/// advances 2->3 (resident) so the full deserialize 0x14067b290 can run.
pub(crate) const B80_FULL_LOAD_INITIATOR_RVA: usize = 0x67b1a0;
/// The MENU's STEP_LoadSaveData initiator 0x14067b200(ecx=slot): sets GameMan+0xb80=2
/// (the deserialize arm) the way the real Load-Game list does. Distinct from the
/// preview 0x67b4e0 (b80=1) and the 0x67b1a0 variant. Hooked for the b80-mount capture
/// to pin which initiator the real .co2 load fires and in what order.
pub(crate) const B80_LOAD_SAVE_DATA_INITIATOR_RVA: usize = 0x67b200;
/// The save-header parser 0x14067bd70(rcx=GameMan, rdx=buf, r8d=size) -- the SOLE
/// writer of GameMan+0xc30 (3 callers). Hooked with a caller stack so a real load
/// reveals WHICH deserializer set the saved map (vanilla 0x67b290/0x67e150 chain or,
/// if it never fires under Seamless, ERSC writing c30 from its own module).
pub(crate) const C30_WRITER_RVA: usize = 0x67bd70;
/// GameMan save-load entry that reaches PlayerGameData::Deserialize -> CSGaitemImp::Deserialize.
/// Guarded during System quickload return-title transition so ProfileSelect OK cannot deserialize
/// into the live in-world player state before native title ownership is rebuilt.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_RVA: u32 = 0x67bd70;
/// CSGaitemImp::Deserialize leaf that crashes during in-world ProfileSelect OK handoff.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const SYSTEM_QUIT_GAITEM_DESERIALIZE_RVA: u32 = 0x671130;
/// CSGaitemImp indexed-handle lookup used by ChrAsm::Deserialize after equipment stream reads.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const SYSTEM_QUIT_GAITEM_LOOKUP_RVA: u32 = 0x671810;
/// CSGaitemImp post-deserialize finalize/reindex path that panics while singleton state is reset.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const SYSTEM_QUIT_GAITEM_FINALIZE_RVA: u32 = 0x671670;
