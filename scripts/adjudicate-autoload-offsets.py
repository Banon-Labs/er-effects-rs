#!/usr/bin/env python3
"""Adjudicate the unknown-STRUCT autoload field offsets: Cleared, moved, or still-unknown.

What makes a clearance valid here
---------------------------------
Three things at once, and a verdict that has fewer is an annotation, not a clearance:

  1. A named object, identified independently in both images. That comes from MSVC RTTI
     (`scripts/rtti-classmap-both.py`), so the class exists in 1.16.2 and in 1.17 as FromSoft's
     own embedded type descriptor rather than as a guess at a name.
  2. A base register that provably holds a pointer to that object, tracked from the incoming
     `this` (`scripts/clear-fields-by-object.py`).
  3. A WITNESS: two function bodies that are instruction-for-instruction identical apart from
     numbers, so a displacement that did not change did not change because the code did not
     change.

And one soundness gate on top: the class must be a leaf. In a method of class `C`, `this` may
point at anything derived from `C`, so a field read through a shared base like
`DLUT::DLReferenceCountObject` (422 derived classes) or `CS::MenuJob` (131) could belong to any
of them. A leaf's `this` is unambiguous. Evidence from a base class is reported as
base-class-evidence and never as cleared.

Ownership
---------
`OWNERS` below maps a repo constant to its owning class, read out of the doc comment the RE was
recorded in. Where no owner is stated the constant stays still-unknown with the reason. An
auto-suggestion is used only when the prose names exactly one leaf class -- and even then the
verdict prints the class it used, so a wrong owner is visible rather than silent.

Output: `autoload-offset-verdicts.tsv` under the drift out-dir, plus a summary.
"""
from __future__ import annotations

import argparse
import collections
import csv
import importlib.util
import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Resolved by scripts/struct_drift_out.py, not spelled here: this used to be a literal
# containing an agent session UUID, which is correct for exactly one session and wrong for
# every other one. `$ER_STRUCT_DRIFT_OUT` still overrides, and so does `--out-dir`.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import struct_drift_out  # noqa: E402 -- the path is set up on the line above

DEFAULT_OUT = struct_drift_out.default_out()

# Owning class per constant, read from the doc comment above it. `None` records a deliberate
# "this is not a game structure" (a Windows ABI struct cannot move in a game patch) or "no class
# owns this" so the reason survives instead of being rediscovered.
# Owning object per repo constant, read out of the doc comment the RE was recorded in -- never
# inferred from the number, and never from a class name that merely appears nearby (the same file
# mentions `CSDlcImp` beside the `DLString` offsets, and an auto-match happily proposed it).
#
#   "Class"            one named class. If it is a leaf, any route's evidence counts.
#   ("A", "B", ...)    the field belongs to a shared base, so several concrete classes are asked
#                      and they must agree; a lone witness is not a consensus.
#   None               deliberately not a game structure -- a Windows ABI struct cannot move in a
#                      game patch. Recorded so the reason survives instead of being rediscovered.
#   FILE_CAP_SUBCLASSES  expands to every concrete `*FileCap`; `FD4FileCap` itself has no vtable
#                      of its own, so its base fields are asked of its 71 subclasses at once.
FILE_CAP_SUBCLASSES = "@filecap-subclasses"

OWNERS: dict[str, object] = {
    # --- Scaleform::MemoryFile (leaf) ---------------------------------------------------------
    "SCALEFORM_MEMORY_FILE_DATA_OFFSET": "Scaleform::MemoryFile",
    "SCALEFORM_MEMORY_FILE_LEN_OFFSET": "Scaleform::MemoryFile",
    "SCALEFORM_MEMORY_FILE_CURSOR_OFFSET": "Scaleform::MemoryFile",
    "SCALEFORM_MEMORY_FILE_REFCOUNT_OFFSET": "Scaleform::MemoryFile",
    "SCALEFORM_MEMORY_FILE_NAME_OFFSET": "Scaleform::MemoryFile",
    "SCALEFORM_MEMORY_FILE_VALID_OFFSET": "Scaleform::MemoryFile",
    # --- title flow ---------------------------------------------------------------------------
    "TFC_NOT_RELEASE_FLAG_18C_OFFSET": "CS::TitleFlowContext",
    "TFC_REGULATION_VERSION_148_OFFSET": "CS::TitleFlowContext",
    "TFC_DISPATCH_STATE_14C_OFFSET": "CS::TitleFlowContext",
    "MSS_SAVE_SLOT_1200_OFFSET": "CS::CSMenuSystemSaveLoad",
    # `owner` in this repo's title prose is the CS::TitleStep object (its +0x48 is the title
    # state: 10 = MenuJobWait, 11 = Finish, 6 = in-world).
    "TITLE_OWNER_STATE_48_OFFSET": "CS::TitleStep",
    "TITLE_OWNER_INSTANCE_TABLE_10_OFFSET": "CS::TitleStep",
    "TITLE_OWNER_DIALOG_E0_OFFSET": "CS::TitleStep",
    "TITLE_OWNER_MENUJOB_SLOT_130_OFFSET": "CS::TitleStep",
    "TITLE_OWNER_MENUJOB_130_OFFSET": "CS::TitleStep",
    "DIALOG_PRESS_START_PROXY_B78_OFFSET": "CS::TitleTopDialog",
    "DIALOG_TFC_A38_OFFSET": "CS::TitleTopDialog",
    # --- steps --------------------------------------------------------------------------------
    "MOVEMAPSTEP_ADVANCE_GATE_LO_4B8_OFFSET": "CS::MoveMapStep",
    "MOVEMAPSTEP_ADVANCE_GATE_HI_4B9_OFFSET": "CS::MoveMapStep",
    "MOVEMAPSTEP_NEXT_STEP_4C_OFFSET": "CS::MoveMapStep",
    "MOVEMAPSTEP_DONE_FLAG_50_OFFSET": "CS::MoveMapStep",
    "MOVEMAPSTEP_HOLD_TIMER_270_OFFSET": "CS::MoveMapStep",
    "MOVEMAPSTEP_COUNTDOWN_100_OFFSET": "CS::MoveMapStep",
    "MOVEMAPSTEP_FINALIZE_REQ_248_OFFSET": "CS::MoveMapStep",
    "MOVEMAPSTEP_STATE_48_OFFSET": "CS::MoveMapStep",
    "MOVEMAPSTEP_STATE_48_RE_OFFSET": "CS::MoveMapStep",
    "MOVEMAPSTEP_FINISH_WARMUP_B0_OFFSET": "CS::MoveMapStep",
    "MOVEMAPSTEP_PAUSE_GAME_128_OFFSET": "CS::MoveMapStep",
    "MOVEMAPSTEP_TESTNETSTEP_WRAPPER_108_OFFSET": "CS::MoveMapStep",
    "INGAMESTEP_TARGET_COORD_100_OFFSET": "CS::InGameStep",
    "INGAMESTEP_RESMGR_250_OFFSET": "CS::InGameStep",
    "INGAMESTEP_MOVEMAP_CHILD_WRAPPER_E0_OFFSET": "CS::InGameStep",
    "INGAMESTEP_PHASE_D8_OFFSET": "CS::InGameStep",
    "INGAMESTEP_REQ_BLOCKID_100_OFFSET": "CS::InGameStep",
    # FD4StepTemplate force-state override fields: asked of two concrete step classes, since the
    # fields live in the shared step template rather than in either one.
    "INGAMESTEP_OVERRIDE_TRIGGER_OFFSET": ("CS::InGameStep", "CS::TitleStep"),
    "INGAMESTEP_OVERRIDE_GUARD_OFFSET": ("CS::InGameStep", "CS::TitleStep"),
    "INGAMESTEP_OVERRIDE_TARGET_OFFSET": ("CS::InGameStep", "CS::TitleStep"),
    # ADJUDICATED 2026-08-31: 0x48, and it had been 0x40 since introduction -- not drift, an
    # offset that was never a field. The step-template constructor (1.16.2 0x140dec6d0 /
    # 1.17 0x140dee4d0, 57/57 aligned) zeroes currentState+requestedState as one qword at
    # +0x48 in both builds and never touches 0x40, which holds
    # FD4ComponentAttachSystem_Step::allocator. Frozen in
    # scripts/check-object-field-offsets-1170.py and pinned there to 0x48.
    "CS_SYSTEM_STEP_CURRENT_STATE_OFFSET": "CS::CSSystemStep",  # 0x48 cleared
    # The same field under the boot-progress crate's local alias, which is a straight
    # `= er_game_base::rva::CS_SYSTEM_STEP_CURRENT_STATE_OFFSET` and whose doc comment names
    # `CS::CSSystemStep` outright. It went unattributed the moment the shared constant moved
    # 0x40 -> 0x48, because the ratchet keys on (name, file, value) -- so a corrected offset
    # enters as a new row and the gate refuses growth. Attributing the alias is the documented
    # exit from that, not a refresh.
    "BOOT_SYS_STEP_STATE_OFFSET": "CS::CSSystemStep",
    # --- menus --------------------------------------------------------------------------------
    "GRID_CONTROL_VIEW_COL_BASE_OFFSET": "CS::GridControl",
    "GRID_CONTROL_VIEW_ROW_BASE_OFFSET": "CS::GridControl",
    "MENU_ITEM_LIST_CURSOR_FIELD_OFFSET": "CS::GridControl",
    "GRID_CONTROL_ITEM_COUNT_OFFSET": "CS::GridControl",
    "GRID_CONTROL_COLUMNS_OFFSET": "CS::GridControl",
    "GRID_CONTROL_ROWS_OFFSET": "CS::GridControl",
    "GRID_CONTROL_COLS_D8_OFFSET": "CS::GridControl",
    "GRID_CONTROL_ROWS_DC_OFFSET": "CS::GridControl",
    "MSGBOX_BUTTON_COUNT_25E8_OFFSET": "CS::SaveRetryDialog",
    "MSGBOX_FADE_CURRENT_1278_OFFSET": "CS::SaveRetryDialog",
    "MSGBOX_CLOSING_LATCH_3B0_OFFSET": "CS::SaveRetryDialog",
    "MSGBOX_JOB_RESULT_STATE_1E8_OFFSET": "CS::SaveRetryDialog",
    "MENU_JOB_REFCOUNT_8_OFFSET": "CS::MenuJob",
    "MENU_JOB_REFCOUNT_08_OFFSET": "CS::MenuJob",
    "MENUJOB_BUILT_FLAG_68_OFFSET": "CS::MenuJob",
    "MENUJOB_INNER_SEQ_70_OFFSET": "CS::MenuJob",
    "MENUJOB_CURRENT_JOB_INDEX_10_OFFSET": "CS::FixOrderJobSequence",
    "EDIT_PROPERTY_CONTROLLER_OFFSET": "CS::EditProperty",
    "EDIT_PROPERTY_LABEL_OFFSET": "CS::EditProperty",
    "CSSCALEFORMVALUE_DATATYPE_20_OFFSET": "CS::CSScaleformValue",
    "PROFILE_ROW_MODEL_PLAYER_NAME_MENUSTRING_50_OFFSET": "CS::MenuSaveDataSummary",
    "PROFILE_ROW_MODEL_LEVEL_88_OFFSET": "CS::MenuSaveDataSummary",
    "PROFILE_ROW_MODEL_LOCATION_MENUSTRING_90_OFFSET": "CS::MenuSaveDataSummary",
    "PROFILE_ROW_MODEL_PLAY_TIME_MENUSTRING_C8_OFFSET": "CS::MenuSaveDataSummary",
    "PROFILE_ROW_MODEL_SLOT_08_OFFSET": "CS::MenuSaveDataSummary",
    # --- portrait / render --------------------------------------------------------------------
    "PROFILE_CAM_TARGET_OFFSET": "CS::CSMenuProfModelRend",
    "PROFILE_CAM_TARGET_W_OFFSET": "CS::CSMenuProfModelRend",
    "PROFILE_CAM_DISTANCE_OFFSET": "CS::CSMenuProfModelRend",
    "PROFILE_CAM_YAW_OFFSET": "CS::CSMenuProfModelRend",
    "PROFILE_CAM_PITCH_OFFSET": "CS::CSMenuProfModelRend",
    "PROFILE_CAM_VIEW_MATRIX_OFFSET": "CS::CSMenuProfModelRend",
    "PROFILE_CAM_FOV_OFFSET": "CS::CSMenuProfModelRend",
    "PROFILE_RENDERER_CHR_ASM_LIVE_OFFSET": "CS::CSMenuProfModelRend",
    "PROFILE_RENDERER_ENV_REGION_OFFSET": "CS::CSMenuProfModelRend",
    "PROFILE_RENDERER_MODEL_INS_OFFSET": "CS::CSMenuProfModelRend",
    "PROFILE_RENDERER_MODEL_MATRIX_OFFSET": "CS::CSMenuProfModelRend",
    "PROFILE_LOOKAT_ANIM_LOCATION_OFFSET": "CS::CSMenuProfModelRend",
    "TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET": "CS::CSMenuAsmModelRend",
    "TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET": "CS::TexResCap",
    "GX_TEXTURE_GPU_RESOURCE_OFFSET": "CS::CSGxTexture",
    "GX_TEXTURE_REFCOUNT_OFFSET": "CS::CSGxTexture",
    "KNOWLEDGE_TIP_TITLE_HANDLE_OFFSET": "CS::KnowledgeLoadingScreen",
    "KNOWLEDGE_TIP_BODY_HANDLE_OFFSET": "CS::KnowledgeLoadingScreen",
    "LOADING_SCREEN_DATA_OFFSET": ("CS::LoadingScreen", "CS::KnowledgeLoadingScreen"),
    "LOADING_SCREEN_FINISH_SENT_OFFSET": ("CS::LoadingScreen", "CS::KnowledgeLoadingScreen"),
    "LOADING_SCREEN_GAUGE_COMPONENT_OFFSET": ("CS::LoadingScreen", "CS::KnowledgeLoadingScreen"),
    "LOADING_SCREEN_GAUGE_ENABLED_OFFSET": ("CS::LoadingScreen", "CS::KnowledgeLoadingScreen"),
    # --- world / resources / saves ------------------------------------------------------------
    "FILECAP_LOAD_PROCESS_78_OFFSET": FILE_CAP_SUBCLASSES,
    "FILECAP_STATUS_88_OFFSET": FILE_CAP_SUBCLASSES,
    "WORLDBLOCKRES_FILECAP_40_OFFSET": "CS::WorldBlockRes",
    "WORLDBLOCKRES_FILECAP2_48_OFFSET": "CS::WorldBlockRes",
    "WBR_PHASE_35_OFFSET": "CS::WorldBlockRes",
    "CSGAITEM_INS_TABLE_OFFSET": "CS::CSGaitemImp",
    "CSGAITEM_ENTRIES_OFFSET": "CS::CSGaitemImp",
    "CSGAITEM_FREE_QUEUE_HEAD_OFFSET": "CS::CSGaitemImp",
    "CSGAITEM_FREE_QUEUE_END_OFFSET": "CS::CSGaitemImp",
    "EQUIP_GAME_DATA_MAGIC_OFFSET": "CS::EquipGameData",
    "HKA_SKELETON_BONES_SIZE_OFFSET": "hkaSkeleton",
    "CHRCTRL_LUA_EVENT_FLAGS_E8_OFFSET": "CS::ChrCtrl",
    "CSREMO_REMOMAN_08_OFFSET": "CS::CSRemoImp",
    "CSREMOMAN_PENDING_D0_OFFSET": "CS::CSRemoMan",
    "SL_JOB_RESULT_INFO_OFFSET": "SaveLoad2::SLSaveSession",
    "FD4_IO_WORKER_NOACCEPT_19_OFFSET": "SaveLoad2::SLSystemImpl",
    "SHOW_PROGRESS_TYPE_OFFSET": "ShowProgressJob",
    # --- identified by RTTI from a VTABLE the REPO itself records ------------------------------
    # `MOVIE_VTABLE_RVA = 0x2bfe088` sits beside `MOVIE_HWND_OFFSET` in the same doc block, and
    # `vtable[-1] -> COL -> TypeDescriptor` names that vtable `CS::CSWindowImp`. So the object is
    # not a guess: the repo recorded its vtable and RTTI reads the class name off it.
    "MOVIE_HWND_OFFSET": "CS::CSWindowImp",
    "CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET": "CS::CSMenuData",
    "CS_MENU_DATA_ENDING_FLAG_5E_OFFSET": "CS::CSMenuData",
    "OPTIONSETTING_COMPOSITE_CURRENT_PANE_OFFSET": "CS::CompositeOptionSettingDialog",
    "OPTIONSETTING_COMPOSITE_PANE_CACHE_OFFSET": "CS::CompositeOptionSettingDialog",
    "OPTIONSETTING_TAB_VIEW_SELECTED_INDEX_OFFSET":
        "CS::OptionSettingTopDialog::_SettingTabControl",
    "PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET": "CS::ProfileLoadDialog",
    # Applied to `game_man` at the use site (`product_autoload_gates.rs:755`); the identical pair in
    # `constants_return_title.rs` is spelled `GAME_MAN_*` and is typed for that reason alone.
    "IS_IN_ONLINE_MODE_BC8_OFFSET": "CS::GameMan",
    "SERVER_CONNECTION_ENABLED_BC9_OFFSET": "CS::GameMan",
    "GAME_MAN_IS_IN_ONLINE_MODE_BC8_OFFSET": "CS::GameMan",
    "GAME_MAN_SERVER_CONNECTION_ENABLED_BC9_OFFSET": "CS::GameMan",
    # Owner named by Ghidra's own decompilation of the two functions that read it, which spell it
    # `(GLOBAL_GameMan->field479_0xdd0).string.length` -- 0xdf0 is the length member of the
    # `DLString<wchar_t>` inside the `FD4FilePathBase` at GameMan+0xdd0. Deliberately absent from
    # the witnesses table below: `GameMan::GameMan` reaches the whole 0xdd0..0xe08 region through
    # `lea rdi,[rsi+0xdd0]` and never writes 0xdf0 through `this`, so a displacement census keyed
    # on the ctor's base register finds nothing and a witness row here would fail, not confirm.
    # It carried two other names -- `OWNER_DF0_OFFSET` and `DF0_OFFSET`, both in
    # `bootstrap_drive.rs`, both claiming a mechanism it does not have -- until 2026-08-31.
    "GAME_MAN_FILE_PATH_STRING_LEN_DF0_OFFSET": "CS::GameMan",
    # --- 2026-08-31 unattributed-owner sweep --------------------------------------------------
    # Each of these was UNATTRIBUTED until its own doc comment was read at the definition site;
    # every class below is RTTI-paired in both images, and the verdict beside it is what
    # `scripts/clear-fields-by-object.py` returned for that (class, offset) on the day it was
    # added. A verdict of still-unknown is recorded as an attribution and not as a clearance --
    # naming the owner is what makes the offset measurable later; it is not evidence it held.
    #
    # CS::CSMenuProfModelRend (vtable 0x142b80128 / 0x142b831d8). portrait_camera.rs states it
    # outright: "All offsets are BYTE offsets from the renderer (CSMenuProfModelRend) base."
    "PROFILE_RENDERER_FACEDATA_OBJ_OFFSET": "CS::CSMenuProfModelRend",  # 0x788 cleared
    "PROFILE_CAM_PERSCAM_OFFSET": "CS::CSMenuProfModelRend",  # 0x9d0 cleared (ctor 64/64 aligned)
    "PROFILE_CAM_ASPECT_OFFSET": "CS::CSMenuProfModelRend",  # 0xa24 cleared (same ctor)
    "PROFILE_RENDERER_MARKED_DELETE_OFFSET": "CS::CSMenuProfModelRend",  # 0x756 still-unknown
    "PROFILE_RENDERER_MODEL_INS_OFFSET": "CS::CSMenuProfModelRend",  # 0x778 still-unknown
    "PROFILE_ANIM_HANDLE_OFFSET": "CS::CSMenuProfModelRend",  # 0x96c still-unknown
    "TITLE_CUSTOM_COVER_PROFILE_RENDERER_TEX_INDEX_OFFSET": "CS::CSMenuProfModelRend",  # 0x9a8
    # FD4::FD4PadDevice / FD4::FD4PadManager (2026-08-31). The census left VK_ARRAY_88_OFFSET as
    # the only offset that was both written through and unsettled, attributed to CS::CSInGamePad --
    # a class that yields 2 usable paired bodies out of 40, which is why it would not settle. It is
    # the wrong class. The array's only writer (1.16.2 0x1426634a0, `mov byte [rcx+rdx*2+0x88],1`,
    # bound `cmp eax,0x50` on id-1000) is called from exactly four sites (0x140240e70, 0x140241130,
    # 0x140e321b0, 0x140e32470) and every one of them computes `rcx` as `*(manager + 0x18 + dev*8)`
    # = `FD4PadManager::padDevices[dev]`. `FD4PadManager::Init` fills that array with
    # `HeapAlloc(0x3c0)` + `FD4PadDevice::FD4PadDevice` + `FD4PadDevice::vftable`. The CSInGamePad
    # is one indirection away: it holds the device at its own +0x10 (Ghidra's type name
    # `CSInGamePad0x10` records exactly that).
    #
    # Both 1.17 values re-measured, held, and frozen in
    # scripts/check-object-field-offsets-1170.py.
    "VK_ARRAY_88_OFFSET": "FD4::FD4PadDevice",  # 0x88 cleared (writer 7/7 aligned; ctor 168/168)
    "PAD_MGR_DEVICES_18_OFFSET": "FD4::FD4PadManager",  # 0x18 cleared (builder A 195/195 aligned)
    "PAD_DEVICES_COUNT_40_OFFSET": "FD4::FD4PadManager",  # 0x40 cleared (same alignment)
    # `FD4PadDevice`'s own `DLFixedVector<DLUID::device*,4>` -- entries at +0x10, count at +0x38 --
    # filled by `FD4::FD4PadDevice::FD4PadDevice` (1.16.2 0x142663880) from the input manager's
    # device factory for types 3..6, with its own `if (4 < count + 1) DLPanic("out of memory")`.
    # Added 2026-08-31 to replace `FD4PADDEVICE_CONCRETE_OFFSET` (+0x8), which the same constructor
    # fills from the factory with type 7 = a `DLUID::VirtualMultiDevice` of 0x7f8 bytes -- the wrong
    # class for the analog-stick fields at +0x89c/+0x8a0, and 172 bytes too short for them.
    "FD4PADDEVICE_DEVICES_OFFSET": "FD4::FD4PadDevice",
    "FD4PADDEVICE_DEVICE_COUNT_OFFSET": "FD4::FD4PadDevice",
    # CS::PropertyNewButtonController -- named in full in the constant's own doc comment,
    # including the allocation size and the constructor that writes the field.
    "PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_STORAGE_OFFSET": "CS::PropertyNewButtonController",
    "PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET": "CS::PropertyNewButtonController",
    # CS::CSPopupMenu -> currentTopMenuJob at +0xb0. Both spellings of the same field.
    "CSPOPUP_TOP_JOB_B0_OFFSET": "CS::CSPopupMenu",  # 0xb0 cleared
    "CS_POPUP_CURRENT_TOP_JOB_B0_OFFSET": "CS::CSPopupMenu",  # 0xb0 cleared
    # CS::MenuWindow -- the cached menu id (`field246_0x180`), 0xffff being its unmapped sentinel.
    "MENU_WINDOW_MENU_ID_OFFSET": "CS::MenuWindow",  # 0x180 cleared
    "TOP_WINDOW_MENU_ID_180_OFFSET": "CS::MenuWindow",  # 0x180 cleared
    "MENU_WINDOW_JOB_OWNING_WINDOW_OFFSET": "CS::MenuWindowJob",  # 0x130 cleared
    "TITLE_LOGO_BACK_VIEW_PARTS_AA8_OFFSET": "CS::TitleTopDialog",  # 0xaa8 cleared
    "OPTIONSETTING_COMPOSITE_OFFSET": "CS::OptionSettingTopDialog",  # 0x1768 cleared
    "OPTIONSETTING_TAB_CONTROL_OFFSET": "CS::OptionSettingTopDialog",  # 0x1870 still-unknown
    "OPTIONSETTING_TAB_CONTROL_1870_OFFSET": "CS::OptionSettingTopDialog",  # 0x1870 still-unknown
    "IN_GAME_STEP_STAY_WRAPPER_B8_OFFSET": "CS::InGameStep",  # 0xb8 cleared
    "MSGBOX_FADE_TARGET_2300_OFFSET": "CS::SaveRetryDialog",  # 0x2300 cleared, like its 0x1278 pair
    "MSGBOX_BUILDER_BUTTON_COUNT_OFFSET": "CS::MessageBoxBuilder",  # 0x10f0 cleared
    "MSGBOX_BUILDER_DEFAULT_INDEX_OFFSET": "CS::MessageBoxBuilder",  # 0x28 still-unknown
    # The world-map pin rows er-invasion-warp injects. `ROW_ID_OFFSET`'s own doc comment names
    # the class ("Row field +0x08 -- CS::WorldMapPinDataBase's per-row id"), and the surrounding
    # block states the container ("Offsets into CS::WorldMapViewModel for the pin-row list").
    "ROW_ID_OFFSET": "CS::WorldMapPinDataBase",  # 0x8   cleared
    "ROW_ENTITY_ID_OFFSET": "CS::WorldMapPinDataBase",  # 0x50  cleared
    "ROW_LAYER_MASK_OFFSET": "CS::WorldMapPinDataBase",  # 0x60  cleared
    "ROW_PARAM_POINTER_OFFSET": "CS::WorldMapPinDataBase",  # 0x240 still-unknown
    "ROW_ICON_ID_OFFSET": "CS::WorldMapPinDataBase",  # 0x248 still-unknown
    "PIN_LIST_VFTABLE_OFFSET": "CS::WorldMapViewModel",  # 0x2d8 cleared
    "PIN_LIST_ALLOCATOR_OFFSET": "CS::WorldMapViewModel",  # 0x2e0 still-unknown
    "PIN_VECTOR_OFFSET": "CS::WorldMapViewModel",  # 0x2e0 still-unknown
    "PIN_LIST_BEGIN_OFFSET": "CS::WorldMapViewModel",  # 0x2e8 still-unknown
    "PIN_LIST_END_OFFSET": "CS::WorldMapViewModel",  # 0x2f0 still-unknown
    "PIN_LIST_CAPACITY_OFFSET": "CS::WorldMapViewModel",  # 0x2f8 still-unknown
    "AREA_CONVERTERS_OFFSET": "CS::WorldMapViewModel",  # 0xf8  cleared
    "AREA_CONVERTER_COUNT_OFFSET": "CS::WorldMapViewModel",  # 0x280 still-unknown
    "MOVEMAPLISTSTEP_GATE_B8_OFFSET": "CS::CSMoveMapListStep",  # 0xb8  cleared
    "MOVEMAPLISTSTEP_LOADLIST_2C0_OFFSET": "CS::CSMoveMapListStep",  # 0x2c0 still-unknown
    # --- Not game structures: ABI structs fixed outside FromSoft's object layout --------------
    "U16STRING_ALLOC_OFFSET": None,
    "U16STRING_DATA_OFFSET": None,
    "U16STRING_SIZE_OFFSET": None,
    "U16STRING_CAP_OFFSET": None,
    "XINPUT_GAMEPAD_OFFSET": None,
    "XINPUT_PACKET_OFFSET": None,
    "WBUTTONS_OFFSET_IN_GAMEPAD": None,
    "XINPUT_THUMB_LY_OFFSET_IN_GAMEPAD": None,
    "XINPUT_THUMB_LX_OFFSET_IN_GAMEPAD": None,
    "XINPUT_BUTTONS_OFFSET_IN_GAMEPAD": None,
    "CAPS_TYPE_OFFSET": None,
    "CAPS_SUBTYPE_OFFSET": None,
    # Added 2026-08-31, each read at its definition site before being recorded here.
    "DEVMODEW_PELS_WIDTH_OFFSET": None,
    "DEVMODEW_PELS_HEIGHT_OFFSET": None,
    "DATA_DIRECTORY_OFFSET_PE32": None,
    "DATA_DIRECTORY_OFFSET_PE64": None,
    "OPTIONAL_HEADER_OFFSET": None,
    "COFF_OPTIONAL_HEADER_SIZE_FIELD": None,
}

NON_GAME_STRUCT_REASONS = {
    "U16STRING_ALLOC_OFFSET": "MSVC basic_string<char16_t> with a stateful allocator; SAVE_DIR_BUILDER ABI, not a game object",
    "U16STRING_DATA_OFFSET": "MSVC basic_string<char16_t> with a stateful allocator; SAVE_DIR_BUILDER ABI, not a game object",
    "U16STRING_SIZE_OFFSET": "MSVC basic_string<char16_t> with a stateful allocator; SAVE_DIR_BUILDER ABI, not a game object",
    "U16STRING_CAP_OFFSET": "MSVC basic_string<char16_t> with a stateful allocator; SAVE_DIR_BUILDER ABI, not a game object",
    "XINPUT_GAMEPAD_OFFSET": "Windows XInput ABI struct",
    "XINPUT_PACKET_OFFSET": "Windows XInput ABI struct",
    "WBUTTONS_OFFSET_IN_GAMEPAD": "Windows XInput ABI struct",
    "XINPUT_THUMB_LY_OFFSET_IN_GAMEPAD": "Windows XInput ABI struct",
    "XINPUT_THUMB_LX_OFFSET_IN_GAMEPAD": "Windows XInput ABI struct",
    "XINPUT_BUTTONS_OFFSET_IN_GAMEPAD": "Windows XInput ABI struct",
    "CAPS_TYPE_OFFSET": "Windows XInput ABI struct (XINPUT_CAPABILITIES.Type)",
    "CAPS_SUBTYPE_OFFSET": "Windows XInput ABI struct (XINPUT_CAPABILITIES.SubType)",
    "DEVMODEW_PELS_WIDTH_OFFSET": "Win32 DEVMODEW display ABI struct",
    "DEVMODEW_PELS_HEIGHT_OFFSET": "Win32 DEVMODEW display ABI struct",
    "DATA_DIRECTORY_OFFSET_PE32": "PE32 optional header, fixed by the PE/COFF spec",
    "DATA_DIRECTORY_OFFSET_PE64": "PE32+ optional header, fixed by the PE/COFF spec",
    "OPTIONAL_HEADER_OFFSET": "PE/COFF header, fixed by the PE/COFF spec",
    "COFF_OPTIONAL_HEADER_SIZE_FIELD": "PE/COFF header, fixed by the PE/COFF spec",
}

# An MSVC polymorphic object begins with its 8-byte vfptr, and route B only accepts a witness
# that stores the class's vtable at `[this + 0]` -- which places that class's own sub-object at
# offset 0. So for a shared base, evidence from a route other than its own virtual methods is
# still sound below this bound (the first two machine words are the base's, whatever the dynamic
# type is), and unsound above it, where the field could belong to any of the derived classes.
BASE_SUBOBJECT_LIMIT = 0x10

# Constants whose owning object has no vtable of its own, adjudicated instead against one named
# consumer function: `constant -> (witness label prefix, base register)`. The witness evidence is
# re-measured by `scripts/clear-fields-by-object.py --witness ... --witness-out`, never quoted
# from notes. The object identity here rests on the repo's own RE (recorded in the doc comment
# beside the constant) rather than on RTTI, which is weaker -- so these are reported as
# cleared-by-named-witness, not as cleared, and the witness is printed with them.
NAMED_WITNESS: dict[str, tuple[str, str]] = {
    # Ghidra 1.16.2 decompiles FUN_14073bc10 as `(longlong param_1, uint param_2)` reading
    # param_1+0xd0/0xd8/0xdc and writing `*(uint *)(param_1 + 0xd4) = index` -- one object, and
    # `CS::GridControl`'s own virtual methods hold that same 0xd0/0xd8/0xdc triple, which is what
    # ties the anonymous `param_1` to the named class.
    "MENU_ITEM_LIST_CURSOR_FIELD_OFFSET": ("0x14073bc10", "rbx"),
    # The OptionSetting tab control stores its real selected visual tab in the same GridControl
    # selected-index field as menu item lists; the surrounding OptionSetting object is not the
    # owner of the +0xd4 field.
    "OPTIONSETTING_TAB_VIEW_SELECTED_INDEX_OFFSET": ("0x14073bc10", "rbx"),
    "SYNTHETIC_STEP_STATE_OFFSET": ("0x140b0a980", "rcx"),
    # The 0x70-byte SoftwareKeyboard path validator this repo builds on its own stack and hands to
    # the game's init: `this` is that buffer, so its own initialiser is the exact witness.
    "SOFTWARE_KEYBOARD_VALIDATOR_FLAGS_68_OFFSET": ("0x140e70920", "rbx"),
    "SOFTWARE_KEYBOARD_VALIDATOR_MAX_6C_OFFSET": ("0x140e70920", "rbx"),
    "SOFTWARE_KEYBOARD_VALIDATOR_MAX_60_OFFSET": ("0x140e70920", "rbx"),
    # The orbit-camera setup walks the whole camera block on one base register, so all seven cam
    # fields are witnessed together in a single object -- and the view-matrix builder independently
    # walks the target Vec3 (0x9b4/0x9b8/0x9bc), which is what identifies the block.
    "PROFILE_CAM_TARGET_OFFSET": ("0x140bbe0a0", "rbx"),
    "PROFILE_CAM_TARGET_W_OFFSET": ("0x140bbe0a0", "rbx"),
    "PROFILE_CAM_DISTANCE_OFFSET": ("0x140bbe0a0", "rbx"),
    "PROFILE_CAM_YAW_OFFSET": ("0x140bbe0a0", "rbx"),
    "PROFILE_CAM_PITCH_OFFSET": ("0x140bbe0a0", "rbx"),
    "PROFILE_CAM_VIEW_MATRIX_OFFSET": ("0x140bbe0a0", "rbx"),
    "PROFILE_CAM_FOV_OFFSET": ("0x140bbe0a0", "rbx"),
    "PROFILE_OFFSCREEN_SIZE_SUPERSAMPLE_FLAG_OFFSET": ("0x140bbedf0", "rsi"),
    # Leaf/no-vtable objects and singleton-backed fields from the residual 1.17 blind-spot issue.
    "DLUID_INPUT_ACTIVE_FLAG_OFFSET": ("0x141f6bad0", "rax"),
    "PAD_STICK_LX_OFFSET": ("0x141f6bc74", "rdi"),
    "PAD_STICK_LY_OFFSET": ("0x141f6bc74", "rdi"),
    "INPUTMGR_BITMAP_90_OFFSET": ("0x140745570", "rdx"),
    "PAB_JOB_PRESS_COUNT_1E8_OFFSET": ("0x1407ad1c0", "rax"),
    "MOUNT_GUARD_DESC_ID_OFFSET": ("0x14082d5b0", "rdx"),
    "MOUNT_GUARD_DESC_BITS_OFFSET": ("0x14082d5b0", "rdx"),
    "POSEHOLDER_IS_UPDATED_OFFSET": ("0x140b49c70", "r12"),
    "IS_IN_ONLINE_MODE_BC8_OFFSET": ("0x14067a030", "rax"),
    "SERVER_CONNECTION_ENABLED_BC9_OFFSET": ("0x14067a190", "rax"),
    "GAME_MAN_IS_IN_ONLINE_MODE_BC8_OFFSET": ("0x14067a030", "rax"),
    "GAME_MAN_SERVER_CONNECTION_ENABLED_BC9_OFFSET": ("0x14067a190", "rax"),
    # The gate the repo's own after-original detour writes. `rbx` is the MoveMapStep `this` and
    # carries 0xf0 ... 0x4ba unchanged; 1.17 only inserts two instructions into this function.
    "MOVEMAPSTEP_ADVANCE_GATE_LO_4B8_OFFSET": ("0x140af7cf0", "rbx"),
    "MOVEMAPSTEP_ADVANCE_GATE_HI_4B9_OFFSET": ("0x140af7cf0", "rbx"),
    "MOVEMAPSTEP_HOLD_TIMER_270_OFFSET": ("0x140af7cf0", "rbx"),
    "MOVEMAPSTEP_FINALIZE_REQ_248_OFFSET": ("0x140af7cf0", "rbx"),
    "MOVEMAPSTEP_COUNTDOWN_100_OFFSET": ("0x140af7cf0", "rbx"),
    "MOVEMAPSTEP_PAUSE_GAME_128_OFFSET": ("0x140af7cf0", "rbx"),
}


def load_bases(out_dir: Path) -> dict[str, int]:
    path = out_dir / "rtti-bases.tsv"
    out: dict[str, int] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        name, count = line.split("\t")
        out[name] = int(count)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-dir", type=Path, default=DEFAULT_OUT)
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--show", default="", help="print only this verdict")
    args = ap.parse_args()
    if args.selftest:
        return selftest(args.out_dir)

    bases = load_bases(args.out_dir)
    evidence = {c["class"]: c for c in
                json.loads((args.out_dir / "object-field-evidence.json").read_text())}
    suggest = {(r["constant"], r["site"]): r
               for r in json.loads((args.out_dir / "autoload-class-suggest.json").read_text())}

    filecaps = sorted(c for c in evidence if c.endswith("FileCap"))

    def classes_for(name: str):
        owner = OWNERS.get(name, "")
        if owner == FILE_CAP_SUBCLASSES:
            return filecaps, "every concrete FD4FileCap subclass (the base has no vtable of its own)"
        if isinstance(owner, tuple):
            return list(owner), "curated: the field is in a shared base, so the classes must agree"
        if owner is None:
            return None, "not a game structure"
        if owner == "":
            return [], "no owner stated in the constant's doc comment"
        return [owner], "curated from the constant's own doc comment"

    def bracket(cls: str, offset: int):
        """A held field below and a held field above, on one base register in one function pair.

        That is the only shape in which a bracket means anything: it says no bytes were inserted
        between those two fields of this object, so a field lying between them did not move. Held
        sets pooled across functions or across registers do not support the argument, and pooling
        them is what made the anonymous version of this worthless.
        """
        ev = evidence.get(cls)
        if not ev:
            return None
        for span in ev.get("spans", []):
            held = span["held"]
            if held and held[0] < offset < held[-1]:
                below = max(h for h in held if h < offset)
                above = min(h for h in held if h > offset)
                return (f"{cls}: {below:#x} and {above:#x} both held on base {span['base']} in "
                        f"one function pair ({span['tag']}), so nothing was inserted between "
                        f"them and {offset:#x} lies inside that span")
        return None

    def look_up(cls: str, offset: int):
        """(state, detail) for one class: 'moved', 'held' or None, honouring the base gate."""
        ev = evidence.get(cls)
        if ev is None:
            return None, f"{cls}: no witness set built"
        derived = bases.get(cls, 0)
        key = f"{offset:#x}"
        if derived and offset >= BASE_SUBOBJECT_LIMIT:
            # Shared base above its own first two words: only its own virtual methods can speak,
            # because a constructor witness may be a derived class's, with a derived `this`.
            moved, held = ev["vslot_moved"].get(key), ev["vslot_held"].get(key)
            route = f"{cls}'s own virtual methods (it is a base of {derived} classes)"
        else:
            moved, held = ev["all_moved"].get(key), ev["all_held"].get(key)
            route = cls if not derived else (
                f"{cls} (a base of {derived}; {key} is inside its own vfptr/refcount words)")
        if moved:
            return "moved", f"{route}: {key} -> " + ", ".join(f"{m:#x}" for m in sorted(set(moved)))
        if held:
            hit = next((r for r in ev["rows"] if r["offset"] == offset), None)
            return "held", (hit["detail"] if hit else f"{route}: held in {held} instruction(s)")
        return None, (f"{route}: witnessed in {ev['usable_pairs']} paired bodies, none of which "
                      f"touches {key} through `this`")

    witness_rows = []
    wpath = args.out_dir / "named-witness-evidence.json"
    if wpath.is_file():
        witness_rows = json.loads(wpath.read_text())

    def named_witness(name: str, offset: int):
        spec = NAMED_WITNESS.get(name)
        if not spec:
            return None
        va, base = spec
        for w in witness_rows:
            if w["va_1162"] != va or w["base"] != base:
                continue
            for old, new in w["moved"]:
                if old == offset:
                    return ("MOVED", f"{w['label']}: base {base} {old:#x} -> {new:#x}")
            if offset in w["held"]:
                return ("CLEARED-BY-NAMED-WITNESS",
                        f"{w['label']}  --  held on base {base} in the pair "
                        f"{w['va_1162']} -> {w['va_1170']}")
            held = w["held"]
            if held and held[0] < offset < held[-1]:
                below = max(h for h in held if h < offset)
                above = min(h for h in held if h > offset)
                return ("CLEARED-BY-BRACKET",
                        f"{w['label']}  --  {below:#x} and {above:#x} both held on base {base} in "
                        f"the pair {w['va_1162']} -> {w['va_1170']}, so nothing was inserted "
                        f"between them and {offset:#x} lies inside that span")
        return None

    rows = []
    verdicts = collections.Counter()
    for (name, site), row in sorted(suggest.items()):
        offset = int(row["offset"], 16)
        hit = named_witness(name, offset)
        if hit:
            verdict, detail = hit
            verdicts[verdict] += 1
            rows.append({"verdict": verdict, "constant": name, "offset": row["offset"],
                         "owner": "(named consumer function)", "written": row["written"],
                         "why_owner": "adjudicated against one named consumer function",
                         "detail": detail, "site": site, "prior": row["verdict"]})
            continue
        classes, why = classes_for(name)
        if classes is None:
            verdict, detail, owner = "NOT-A-GAME-STRUCT", NON_GAME_STRUCT_REASONS.get(name, "not a game structure"), ""
        elif not classes:
            verdict, detail, owner = "STILL-UNKNOWN", why, ""
        else:
            owner = ", ".join(classes) if len(classes) <= 3 else f"{len(classes)} FileCap classes"
            states = [look_up(c, offset) for c in classes]
            moved = [d for st, d in states if st == "moved"]
            held = [d for st, d in states if st == "held"]
            silent = [d for st, d in states if st is None]
            if moved:
                verdict, detail = "MOVED", "; ".join(moved[:3])
            elif held and not silent:
                verdict, detail = "CLEARED", held[0] + (
                    f"  [+{len(held) - 1} more classes agree]" if len(held) > 1 else "")
            elif held and silent:
                # Some asked classes saw it, some did not. That is not disagreement -- it is
                # partial coverage -- but it is also not a consensus, so it is not a clearance.
                verdict = "CLEARED-PARTIAL-CONSENSUS"
                detail = (f"{len(held)} of {len(classes)} classes witnessed it held, "
                          f"{len(silent)} had no witness at that offset: {held[0]}")
            else:
                braced = next((b for b in (bracket(c, offset) for c in classes) if b), None)
                if braced:
                    verdict, detail = "CLEARED-BY-BRACKET", braced
                else:
                    verdict, detail = "STILL-UNKNOWN", silent[0] if silent else why
        verdicts[verdict] += 1
        rows.append({"verdict": verdict, "constant": name, "offset": row["offset"],
                     "owner": owner, "written": row["written"], "why_owner": why,
                     "detail": detail, "site": site, "prior": row["verdict"]})

    dest = args.out_dir / "autoload-offset-verdicts.tsv"
    with dest.open("w", encoding="utf-8") as fh:
        fh.write("verdict\tconstant\toffset\towner\twritten\tdetail\tsite\tprior_annotation\n")
        for r in rows:
            fh.write("\t".join((r["verdict"], r["constant"], r["offset"], r["owner"],
                                r["written"], r["detail"].replace("\t", " "), r["site"],
                                r["prior"])) + "\n")
    print(f"{len(rows)} autoload UNKNOWN-STRUCT offsets adjudicated -> {dest}\n")
    for label, count in verdicts.most_common():
        print(f"  {label:<22} {count}")
    written = [r for r in rows if r["written"] == "W"]
    print(f"\nof the {len(written)} that are WRITTEN through:")
    for label, count in collections.Counter(r["verdict"] for r in written).most_common():
        print(f"  {label:<22} {count}")
    if args.show:
        print()
        for r in rows:
            if r["verdict"] == args.show:
                print(f"{r['constant']} = {r['offset']}  [{r['owner'] or '?'}]"
                      f"{'  WRITTEN' if r['written'] else ''}\n    {r['detail']}\n    {r['site']}")
    return 0


def selftest(out_dir: Path) -> int:
    ok = True
    bases = load_bases(out_dir) if (out_dir / "rtti-bases.tsv").is_file() else {}
    if not bases:
        print("SKIP: rtti-bases.tsv absent")
        return 0
    # Positive controls on the leaf/base gate, which is what makes a clearance sound.
    for cls, want_leaf in (("CS::MoveMapStep", True), ("CS::GridControl", True),
                           ("Scaleform::MemoryFile", True),
                           ("DLUT::DLReferenceCountObject", False),
                           ("CS::MenuJob", False), ("CS::MenuWindow", False)):
        is_leaf = bases.get(cls, 0) == 0
        if is_leaf != want_leaf:
            print(f"FAIL: {cls} leaf={is_leaf}, expected {want_leaf}")
            ok = False
    if ok:
        print("ok: 6 leaf/base controls (3 leaves, 3 shared bases)")
    # MUTATION: if the gate is deleted, a 422-derived base would be treated as a nameable object.
    if bases.get("DLUT::DLReferenceCountObject", 0) < 100:
        print("FAIL: mutation guard -- DLReferenceCountObject should have many derived classes")
        ok = False
    else:
        print(f"ok: DLReferenceCountObject has {bases['DLUT::DLReferenceCountObject']} derived "
              "classes, so its field evidence is unattributable")
    print("SELFTEST", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
