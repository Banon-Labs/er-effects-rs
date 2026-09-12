//! Injected and suppressed input: what the harness stamped, and what the game read.
//!
//! The overlay hotkeys, the DirectInput keyboard and mouse `GetDeviceState` paths, the
//! user32 `GetKeyState` and `GetCursorPos` readers, and the pad gate that can shut on an
//! injected frame. They are split by read path on purpose: a key stamped into a buffer
//! the game never polls is indistinguishable from no input at all unless the stamps and
//! the reads are counted apart.
//!
//! Split out of `counters.rs` as a pure code move: nothing is renamed and no initial value
//! changes. Every name here is re-exported from `er_telemetry_core::counters` with a glob, so
//! each consumer still spells it `er_telemetry_core::counters::<name>`.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize};

pub static EFFECT_HOTKEY_PENDING_UP: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_PENDING_DOWN: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_PENDING_LEFT: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_PENDING_RIGHT: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_PENDING_TOGGLE: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_PENDING_OVERLAY_TOGGLE: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_HOOK_STARTED: AtomicBool = AtomicBool::new(false);
pub static EFFECT_HOTKEY_HOOK_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static EFFECT_SELECTOR_OVERLAY_VISIBLE_FOR_HOOK: AtomicBool = AtomicBool::new(false);
pub static EFFECT_SELECTOR_DINPUT_HOOK_INSTALL_ATTEMPTED: AtomicBool = AtomicBool::new(false);
pub static EFFECT_HOTKEY_HOOK_HITS: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_APPLIED_ACTIONS: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_INPUT_SUPPRESSED_KEYS: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_INPUT_SUPPRESSED_ARROW_KEYS: AtomicUsize = AtomicUsize::new(0);
pub static INJECTED_KEY: AtomicU8 = AtomicU8::new(0);

/// The cursor position the game is told it has, packed `(x << 32) | y`, or `u64::MAX` for "no
/// injection". One atomic rather than two so the pair cannot be read half-updated.
pub static INJECTED_CURSOR_POS: AtomicU64 = AtomicU64::new(u64::MAX);
/// How many `GetCursorPos` calls were answered with the injected position.
pub static USER32_INJECTED_CURSOR_STAMPS: AtomicUsize = AtomicUsize::new(0);
pub static SUPPRESS_ARROW_KEYS: AtomicBool = AtomicBool::new(false);
pub static DINPUT_SUPPRESSED_ARROW_KEYS: AtomicUsize = AtomicUsize::new(0);
pub static DINPUT_KB_HOOK_FIRES: AtomicUsize = AtomicUsize::new(0);
/// How many times the DInput keyboard `GetDeviceState` detour stamped the harness's injected DIK
/// into the buffer the game is about to read. This is the focus-independent injection stage: the
/// stamp happens after DInput has filled (or zeroed) the buffer, so it lands whether or not ER owns
/// the keyboard focus. Non-zero here with `DINPUT_KB_HOOK_FIRES` non-zero means the game read a
/// buffer we authored; a zero here while the harness is injecting means the stamp never ran.
pub static DINPUT_INJECTED_KEY_STAMPS: AtomicUsize = AtomicUsize::new(0);
/// Win32 virtual-key code the harness is holding down at the USER32 layer (0 = nothing held). ER
/// 1.17 imports `GetKeyState`/`GetKeyboardState`/`ToAscii` from USER32 and no RawInput API at all,
/// so this is a keyboard stage the game genuinely reads. Stamped into the results of the two USER32
/// getters below, which makes it focus-INDEPENDENT: those getters return the calling thread's key
/// table, which Windows only populates for the focused thread -- we author the answer afterwards.
pub static INJECTED_VK: AtomicU8 = AtomicU8::new(0);
/// How many times the game called USER32 `GetKeyboardState` through our detour. Zero means the game
/// does not read the keyboard that way and the stamp below is meaningless.
pub static USER32_GET_KEYBOARD_STATE_FIRES: AtomicUsize = AtomicUsize::new(0);
/// How many times the game called USER32 `GetKeyState` through our detour.
pub static USER32_GET_KEY_STATE_FIRES: AtomicUsize = AtomicUsize::new(0);
/// How many times a USER32 detour actually reported `INJECTED_VK` as held to the game.
pub static USER32_INJECTED_VK_STAMPS: AtomicUsize = AtomicUsize::new(0);
/// How many times the game called USER32 `GetCursorPos` through our detour. The OptionSetting
/// tab-switch (Game / Quit Game) has no keyboard bind -- it is mouse-only -- so driving a real menu
/// path to the cloned load rows needs a focus-independent mouse stage the same way movement needed a
/// keyboard one. `eldenring.exe` 1.17 imports `GetCursorPos`, `SetCursorPos`, `ClientToScreen`,
/// `ScreenToClient` and `ClipCursor` from USER32; this counter says whether the pointer position the
/// menu uses comes through that import (and is therefore stampable) or from DirectInput's mouse
/// device instead. Measurement only -- nothing is injected on the mouse path yet.
pub static USER32_GET_CURSOR_POS_FIRES: AtomicUsize = AtomicUsize::new(0);
/// The PAD gate, sampled on the can-move probe's own inject-on frames. These decide whether any
/// injected input is read, and they are the difference between "the key never arrived" and "the key
/// arrived at a device the game had already decided to skip". `FD4PadManager+0x2f8` is the
/// inactive-window request `CS::CSPadStep::STEP_Update` raises on an unfocused frame;
/// `FD4PadManager::Update` latches it forward into `+0x2f9`; and every `CSInGamePad` query
/// (`FUN_142664380`/`142664280`/`1426640f0`, all from `PollInput@0x142665060`) opens with
/// `if (field625_0x2f9 == false)`. `GAME_DEBUG_BYTE` is the `.data` byte
/// `Game.Debug.IsEnableControlOnDisactiveWindow` reads (1.16.2 0x144588af1), which `er-focus-input`
/// holds at 1 -- sampling it says whether that hold is actually in effect at the instant we inject,
/// rather than assuming it from the DLL being loaded. 0xff = could not read.
pub static PAD_GATE_MGR_2F8: AtomicUsize = AtomicUsize::new(0xff);
pub static PAD_GATE_MGR_2F9: AtomicUsize = AtomicUsize::new(0xff);
pub static PAD_GATE_DEBUG_BYTE: AtomicUsize = AtomicUsize::new(0xff);
/// Inject-on frames on which `FD4PadManager+0x2f9` was LATCHED shut -- i.e. frames where the game
/// short-circuited every pad read no matter what we had stamped into the device.
pub static PAD_GATE_SHUT_ON_INJECT_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Total horizontal displacement, in THOUSANDTHS of a world unit, accumulated across the can-move
/// probe's inject-on frames, and across its inject-off tail. These exist because the frame-count
/// verdict cannot tell "the key never reached the game" from "the key reached the game and the
/// character is standing against a wall": both report a low moved-frame ratio. Run
/// br-20260905-033648-7f4c is the case in point -- 30 of 30 inject-on frames stamped DIK_W into the
/// buffer the game read, and only 5 of 29 frames cleared the per-frame threshold.
pub static ON_DISP_MILLI: AtomicUsize = AtomicUsize::new(0);
pub static OFF_TAIL_DISP_MILLI: AtomicUsize = AtomicUsize::new(0);
pub static DINPUT_MOUSE_HOOK_FIRES: AtomicUsize = AtomicUsize::new(0);
pub static DINPUT_KB_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static DINPUT_MOUSE_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static DINPUT_KB_ALSO_MOUSE: AtomicBool = AtomicBool::new(false);
pub static SIMULATED_INPUT_PRESSES_TOTAL: AtomicUsize = AtomicUsize::new(0);
pub static AUTOLOAD_HANDOFF_PARENT_STATE_FIX_COUNT: AtomicUsize = AtomicUsize::new(0);
