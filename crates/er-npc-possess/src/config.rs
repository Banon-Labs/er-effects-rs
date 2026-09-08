//! `er-npc-possess.toml`, written on first run and re-read while the game runs.
//!
//! Same shape as `er-enemynpc-effects` and `er-refill-all`, for the same reasons: the file lives
//! in the game's own directory (the process CWD), the DLL writes a fully commented default the
//! first time it cannot find one, and `er_hotkey_config::HotFile` notices edits by comparing the
//! file's text rather than its mtime -- mtime has one-second resolution on the filesystems a Wine
//! prefix sits on, so two saves inside a second are invisible to it, which reads as "changing the
//! key did nothing".
//!
//! # The one thing this file does that the other two do not
//!
//! **`[target]` is not live.** Everything else here takes effect on the next reload, about a
//! second after the file is saved. `[target]` decides who you are about to become, and moving that
//! while a possession is in flight would leave the mapping, the camera and the moveset describing
//! a different character than the body on screen. So an edit to it is STAGED: the value on disk is
//! remembered, the value in force does not move, and [`adopt_staged_target`] promotes one to the
//! other at the next possession. The log says `[target] staged` rather than `[target] now`, so
//! nobody is left believing an edit took that has not.
//!
//! # The second file
//!
//! A later layer writes `er-npc-possess.derived.toml` -- one line per animation, auto-classified,
//! for the player to correct. It is not implemented here, and nothing below assumes there is only
//! one file: [`PossessConfig::apply`] takes text rather than a path, the watcher is a field rather
//! than a global, and [`DERIVED_CONFIG_FILE_NAME`] is already spelled out. Adding it is a second
//! `HotFile` in [`ConfigState`] and a second `apply`, not a rewrite.

// Windows-only crate in practice; this module is pure text handling and stays ungated so its
// tests run on the host.
#![cfg_attr(not(windows), allow(dead_code))]

use std::{
    fs,
    path::PathBuf,
    sync::{
        Mutex, MutexGuard, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use er_hotkey_config::{
    Binding, BindingUpdate, FileChange, HotFile, chord_name,
    keys::{Chord, KeyParseError, parse_chord},
    pad::{PadChord, pad_chord_name, parse_pad_chord},
};

use crate::{
    engine::PossessionRequest,
    log::possess_log,
    settings::{
        CameraSettings, ChrOverride, HudSettings, MovementSettings, PickerSettings, Rejections,
        Tables, TargetMode, TargetSettings,
    },
    toml::Document,
};

const CONFIG_FILE_NAME: &str = "er-npc-possess.toml";

/// The same name, for log lines that have to tell the player which of the two files they should
/// be typing in. Separate so the private constant stays private to the path logic.
pub(crate) const CONFIG_FILE_NAME_FOR_LOG: &str = CONFIG_FILE_NAME;

/// The auto-classified moveset table a later layer writes and the player then edits. Named here
/// so the second file has a spelling before it has an implementation; nothing reads it yet.
pub(crate) const DERIVED_CONFIG_FILE_NAME: &str = "er-npc-possess.derived.toml";

const DEFAULT_HOTKEY: &str = "F9";
/// Both sticks clicked. Not a vanilla pair -- L3 and R3 each do something on their own, and this
/// DLL does not suppress either, so the individual actions still happen.
const DEFAULT_GAMEPAD_HOTKEY: &str = "ls+rs";
const DEFAULT_RADIAL: &str = "DPadDown";

const DEFAULT_CONFIG_TOML: &str = r##"# er-npc-possess.toml -- the standalone "become an NPC" DLL.
#
# EDITS TAKE EFFECT WHILE THE GAME RUNS. This file is re-read about once a second; there is no
# need to restart, and the log names the old value and the new one each time one moves. The one
# exception is the whole [target] table, which is explained at that table.
#
# WHAT WORKS TODAY. Press the hotkey and you BECOME the character [target] selects: the camera and
# lock-on move to it, your own body goes invisible, silent, invincible and unable to attack, and it
# is carried along with the creature every frame so other players see you standing where it stands
# and releasing drops you exactly there. The left stick walks the creature. Press again -- or let
# it die -- to get out.
#
# ATTACKING. Four buttons, the same on every creature: R1 light, R2 heavy, L1 ranged, L2 movement
# (on mouse and keyboard, the game's own default: click, shift+click, right click, shift+right
# click). Which attack you get depends on how far away the nearest enemy is and how many times you
# have pressed the button in a row -- see [mapping]. Every creature's attacks were classified
# offline; er-npc-possess.derived.toml is written each time you possess something and lists what
# that creature can do and, for anything withheld, exactly why.
#
# WHAT IS NOT WIRED YET, so you can tell a missing feature from a broken one:
#   * The `radial` binding is parsed and reported and there is no wheel to open.
#   * Range is measured to the nearest enemy, NOT to what you are locked on to.
#   * A GRAB SWINGS BUT DOES NOT GRAB. The grab itself fires -- see allow_grabs -- but ThrowParam
#     picks the victim by exact chr id, and that is the player for 189 of the game's 190 creature
#     throw rows. Your own body is the only thing the creature could grab, and possession makes it
#     invincible, which is the same flag the game checks before it will start a throw. So the
#     swing plays and nothing is caught. Dropping the invincibility is not the fix: it would let
#     the creature you are wearing throw and kill you. er-npc-possess.derived.toml marks each
#     affected move REFUSED so you can tell this apart from a move that is simply missing.
#   * Your own body stays LOCK-ON-ABLE while you are away from it. It cannot be hurt and cannot be
#     seen, so this is an oddity rather than a hazard -- and it is also what YOUR OWN lock-on
#     finds, because the creature has become the one doing the locking rather than a target. The
#     reticle therefore lands on your body, so possession scales that (invisible) body to the
#     creature's size and the marker sits on the creature instead of at its feet. The log line
#     starting "lock-on anchor:" says what was done or why it was not.
#   * A creature whose own NpcParam says it does not move -- a turret, something rooted in place --
#     still will not, however hard you push. That gate is the game's, not the mod's.
#
# A value this file does not understand is REPORTED AND IGNORED, and the last value that worked
# stays in force. A typo never leaves you with no hotkey and never silently resets a setting.

# LIVE. Master switch. false leaves the DLL loaded and inert: neither hotkey fires.
enabled = true

# LIVE. Keyboard binding. Press to possess what you are targeting, press again to release.
# Modifiers ctrl/alt/shift plus one key, e.g. "ctrl+shift+p". Empty means no keyboard binding.
# The same key names as every other er-* DLL in this profile:
#
#   A..Z, 0..9, F1..F15
#   Insert Delete Home End PageUp PageDown Backspace Tab Enter Escape Space
#   Left Up Right Down PrintScreen ScrollLock NumLock Pause CapsLock
#   punctuation by symbol or name: - = [ ] \ ; ' , . / `  (Minus, Equals, LeftBracket,
#     RightBracket, Backslash, Semicolon, Quote, Comma, Period, Slash, Grave)
#   keypad: KP_0..KP_9, KP_Plus, KP_Minus, KP_Multiply, KP_Divide, KP_Period, KP_Enter
hotkey = "F9"

# LIVE. Controller binding for the same thing. Every named button must be held together; order
# does not matter, and holding other buttons as well is fine. Empty disables it.
#
#   select (back, share, view)   start (options, menu)
#   a/cross  b/circle  x/square  y/triangle
#   lb/l1  rb/r1  ls/l3  rs/r3
#   dpad_up  dpad_down  dpad_left  dpad_right
#
# These buttons are NOT suppressed, so whatever they do in vanilla still happens as well.
gamepad_hotkey = "ls+rs"

# LIVE BINDING, RESERVED BEHAVIOUR. Hold for the possessed character's attack wheel. The binding
# is read and re-bound live today and the log names it; there is no wheel to open until the
# mapping layer lands.
radial = "DPadDown"

# THE ONLY TABLE HERE THAT IS NOT LIVE.
#
# It decides who you are about to become. Changing it mid-possession would leave the mapping, the
# camera and the moveset belonging to a different character than the body on screen, so an edit
# is STAGED and takes effect at the NEXT possession -- release and press again. The log says
# "[target] staged" when it reads one, and "[target] adopted" when it takes.
[target]
# lock_on   -- whatever you are locked on to
# nearest   -- the closest character that passes the selection filter
# crosshair -- whatever the middle of the screen is pointed at. NOT IMPLEMENTED YET: it currently
#              behaves exactly like "nearest". The camera's forward vector is no longer the
#              blocker -- it is ChrExFollowCam+0xe0 minus +0x110 and the possessed creature is
#              already aimed along it every frame -- nothing has wired it into target SELECTION.
#              The spelling is kept so this file does not have to change when it lands.
# chr_id    -- the literal chr_id below, ignoring where you are looking
# spawn     -- CREATE the creature [spawn] names, in front of you, and become that. The only mode
#              that puts something new in the world rather than borrowing something already there.
mode = "lock_on"
# Used only when mode = "chr_id". This is an NpcParam row, e.g. 45000000 for a Flying Dragon.
chr_id = 0
# Hand the body back when it dies, instead of staying inside a corpse.
release_on_death = true

# USED ONLY WHEN mode = "spawn", above. Staged with [target] and adopted at the next possession,
# for the same reason: it decides who you become.
#
# ANY four-digit creature works. Assets load on demand -- the character's own ChrRes step machine
# goes and fetches its chrbnd, anibnd, behbnd and texbnd -- so you are NOT limited to creatures
# that happen to be loaded near you, and there is no residency check to fail.
#
# WHAT DOES FAIL is an id with no chrbnd on disk at all, and it fails by WAITING: there is no error
# state anywhere in the game's asset step machines, so such a character sits half-built forever
# rather than reporting anything. readiness_ms below is the deadline that turns that into a message;
# when it expires the creature is removed and er-npc-possess.derived.toml says which stage it died
# at and what that means.
[spawn]
# The MODEL number. 4500 becomes c4500. 0..9999.
chr_id = 4500
# The NpcParam row, which drives stats, behaviour and which resources get loaded. 0 derives
# chr_id * 10000 -- 45000000 for c4500 -- which is the row the shipped moveset table is keyed by,
# so leaving this at 0 is the answer that keeps the attacks matching the body.
npc_param_id = 0
# The NpcThinkParam row. Rarely worth setting: an invalid one is not a failure, because the lookup
# pre-initialises its result and the loader treats the resulting empty AI script as satisfied.
npc_think_id = 0
# The MINIMUM distance in front of you it appears, in metres. 1 to 50.
#
# THIS IS A FLOOR, NOT THE ANSWER. The creature is placed at whichever is larger: this number, or
# far enough out that its own physics capsule clears yours (its hitRadius, plus yours, plus a metre
# of gap). A Ballista lands exactly here; a Fallingstar Beast has a 4.00 m radius and is pushed out
# to 5.45 m whatever this says, because 3.0 m put it around you rather than in front of you -- and
# possession then keeps your body at that creature's root for as long as you wear it.
#
# The log line for each spawn says which of the two won and what the creature's radius was.
distance_m = 3.0
# How long to wait for it to become drivable before giving up and removing it. 1000 to 60000 ms.
readiness_ms = 5000
# Take the creature away again when the possession ends. Leaving it is not free -- it is a live NPC
# nothing else will ever remove, and it holds one of the fourteen roster slots the game's own
# dynamic spawner shares -- but it is a real choice and the log says which one was made.
#
# ONE CASE IGNORES THIS, and it is not a bug: if the game is closing (the DLL is being unloaded),
# the creature is left standing. Removing it is a call into the game and that path does not run on
# the game's thread. It gets its own AI back either way and the next map load clears it.
despawn_on_release = true

# LIVE. How one press turns into one of the possessed character's attacks.
[mapping]
# context -- the range band picks which attacks are eligible, then repeated presses walk them in
#            order. The default, and the only model that still means something on a character with
#            thirty attacks and four buttons.
# layered -- range picks a THIRD of the attack list rather than filtering by reach. More
#            predictable, less situationally right.
# slots   -- range is ignored entirely; every press walks the whole bucket. Use this on a creature
#            whose attacks mostly came back with an unknown reach.
model = "context"
# Neutral time after which the attack rank falls back to 0. Standing still resets it too, and
# that is the one that usually fires first.
#
# This is about WHICH attack a press gives you, not about when a press happens -- see
# input_buffer_ms below, which is the one that reads almost the same and does something else.
combo_window_ms = 1200
# How long a press made mid-swing waits before it is given up on.
#
# A press while the possessed creature is still committed to an attack NEVER cancels that attack.
# It is held, and fired the moment the creature will accept it: either when the attack's damage
# window has closed -- which is a chain, and the creature's own TimeAct data says when that is,
# per animation -- or when the animation ends, whichever comes first. Only one press is held, so
# mashing gives you one follow-up and not six; the last press wins.
#
# Set it to 0 and a press made mid-swing is simply lost, which is the honest way to turn the
# buffer off. There is no setting that brings the old cancel-on-press behaviour back: cancelling
# an attack you have already committed to is the thing this exists to stop.
input_buffer_ms = 1000
# close < 4m < mid < 12m < far. Two numbers, both required, near first. Measured to the nearest
# enemy. While you are moving, the band is shifted one step CLOSER, on the reasoning that an
# attack you start mid-run lands after the run has closed some of the gap.
bands_m = [4.0, 12.0]
# Offer the creature's grabs. ON as a policy -- a grab is a real attack and withholding it by
# default would quietly remove the signature move of most bosses. It gates 153 attacks across 78
# creatures; turn it off and those buttons give you the creature's other attacks instead.
#
# A grab is NOT the 4000-band animation you may have gone looking for. Malenia's a4100 and a4101
# are what the game plays AFTER a grab has been accepted, and nothing can fire them -- the throw
# system reaches them by name and picks which one from a ThrowParam row. The grab you fire is an
# ordinary attack (hers is a3022); when it lands, the game hands the hit to the throw system.
# er-npc-possess.derived.toml marks each one GRAB and says who it can be used on -- almost always
# "the player", because ThrowParam.DefChrId is matched exactly and is 0 for 189 of the game's 190
# creature throw rows. A grab whose only victim is the player is marked REFUSED there as well: it
# swings, and the throw cannot be accepted while your body is invincible. Turning this off does
# not fix that; it just gives you the creature's other attacks on those buttons.
allow_grabs = true
# promote -- an input whose bucket is empty borrows from another one rather than doing nothing.
#            It drops the RANGE requirement first and only then changes bucket, so the button
#            keeps meaning what it says for as long as possible.
# deny    -- it stays dead
unbound_inputs = "promote"
# Softlock guard. If the creature is animating, going nowhere, and you have not asked for
# anything for this long, the animation is forced back to idle and never offered again this
# session -- and it is written into er-npc-possess.derived.toml so you can see which one it was.
# Some animations only exit on a condition the AI would have met, and the AI is what possession
# switches off.
watchdog_seconds = 4.0

# LIVE. Which bucket each input draws from. Identical on every character, which is the point:
# the same button means the same KIND of thing whoever you are wearing.
# Buckets: light, heavy, ranged, movement.
#
# l2 is the creature's own dodges and steps, animation 6000-6023. The graph never names those
# W_Event -- only W_Step -- so they are fired by name rather than by the field write this mod
# prefers. That is the one place the mod calls a game function. A creature with no fireable step
# gets nothing in this bucket and l2 promotes to an attack instead.
[buttons]
r1 = "light"
r2 = "heavy"
l1 = "ranged"
l2 = "movement"
# LIVE from the next possession. THE ATTACK-SET PAGE KEYS.
#
# A creature has up to sixty attacks and four buttons. Repeated presses already walk a bucket, but
# that is a COMBO -- it falls back to the first move the moment you stop. These two keys are the
# other thing: a standing choice of WHICH attack each button leads with. The right arrow pages
# r1/r2 and the left arrow pages l1/l2, which is the hand each key already swaps an armament for in
# vanilla -- and a possessed creature has no armaments, so neither key does anything else while you
# are wearing one.
#
# er-npc-possess.derived.toml has a [pages] block saying which set you are on and what each button
# leads with, and the log says so every time you turn a page. A creature with only one set on a
# hand ignores that key rather than cycling one page. Leave a value empty to unbind it.
page_right = "Right"
page_left = "Left"
# The controller spelling of the same two. The d-pad, for the same reason.
pad_page_right = "DPad_Right"
pad_page_left = "DPad_Left"

# How the possessed body moves. LIVE -- an edit applies within a second, mid-possession.
[movement]
# The keyboard standing in for the left stick, because a keyboard has no stick and a creature is
# driven by one. WASD, the same keys the game binds movement to. Leave a value empty to unbind it.
# A controller's left stick still wins whenever it is deflected, so holding both does not fight.
forward = "W"
back = "S"
left = "A"
right = "D"

# LIVE. A push closer to straight ahead than this counts as "no turn asked for" and is treated as
# exactly straight ahead. 0..180 degrees. The body is steered by being told WHERE to walk, so
# without a floor a stick two degrees off centre is a standing request to turn and the creature
# weaves down a corridor you meant it to walk straight along. 0 turns the floor off.
turn_deadzone_deg = 20.0
# LIVE. How far ahead of itself the possessed character is told to walk, as a multiplier. Higher
# is a longer stride between re-aims and a body that runs on further after you let go; lower is
# twitchier and stops sooner.
#
# It does NOT set the gait. Past half deflection the creature runs and below it walks, because the
# engine has exactly those two speeds for a character following an order and picks between them
# itself. WASD is full deflection, so the keyboard always runs.
speed_scale = 1.0

# How the camera frames the creature you are wearing. LIVE -- save the file and the framing moves
# about a second later, mid-possession, without letting go.
#
# Vanilla's follow camera is tuned for a 1.5 m capsule, so wearing an Ancient Dragon puts the
# camera inside the model. The mod reads the creature's OWN physics capsule height and writes two
# numbers -- how far back the camera sits and how high it aims -- into a LockCamParam row nothing
# else in the game references, for the length of the possession. Every byte goes back on release.
#
# The two numbers are solved, not tuned: they are the ones that put the top of the creature in the
# same place on your screen, with the same gap above it in proportion to its own size, that your
# own character gets. At player size they ARE the vanilla numbers, so possessing something
# human-sized looks like nothing happened.
#
# er-npc-possess.derived.toml prints where the creature actually landed (head_screen_y and
# headroom_heights) next to your own character's +0.0296 and 1.09, so you can see rather than
# guess whether the framing came out right.
#
# er-npc-possess.derived.toml records what it read, what it wrote, and -- if it did not -- why.
[camera]
# The off switch. false frames every creature the way your own body would be framed.
enabled = true
# Which LockCamParam row is patched in memory. 1000 is one of 73 rows the shipped regulation never
# references. If you run a regulation mod that DOES use it, the mod refuses rather than stealing it
# and says so in the log; pick another free row here. It must already exist as a row -- an id the
# table does not contain makes the engine's camera update do nothing at all.
param_row = 1000
# How hard the camera backs off as the creature gets bigger: distance = 3.8 * (height / 1.5) ^ this.
# 1 is the default and is not a taste setting: with a fixed field of view, holding a framing means
# distance scales STRAIGHT with height, so 1 gives a Fire Giant the same headroom above its head
# that your own character gets. Below 1 the shot gets TIGHTER the bigger the creature, which is
# what used to clip them off the top of the screen; 0 pins the camera at your own distance whatever
# you are wearing. Above 1 backs off faster than size, for a wider establishing shot. 0..2.
distance_exponent = 1.0
# Metres. A ceiling on the formula above, for anyone who wants their camera closer than the framing
# asks -- but NOT on the clearance the camera keeps over the creature's own collision radius,
# because a camera far away is a bad shot and a camera inside the model is no shot at all.
#
# The default sits above the distance the formula asks for at the largest height the mod will
# accept, so it cannot crop anything real. Every previous value here did: 40 cropped everything
# above 3.8 m tall, and even 120 cropped the Walking Mausoleum (59 m, which wants 149.5 m). Lower it
# and the shot gets tighter on the big ones -- their heads stay in frame, because the aim point
# follows whatever distance comes out, but the room above them shrinks.
distance_max = 512.0

# What possession puts on screen. LIVE -- an edit applies within a second, and turning either of
# these off mid-possession takes effect on the next frame.
[hud]
# While something is possessed, the three bars read THAT CREATURE instead of your own character.
#
# Runes, equipment, the great rune and the memorised spells are NOT retargeted and keep showing
# you. That is deliberate rather than unfinished: a creature has no PlayerGameData and no armament
# slots, so pointing those at it would not show its equipment, it would empty the panel.
#
# Almost no creature has FP -- NpcParam.mp is 0 on 6,989 of the 7,045 shipped rows and on every
# boss -- so the FP bar is normally drawn EMPTY while you are wearing something. That is the
# param value, not a failure to read it; er-npc-possess.derived.toml prints the number. Stamina
# is real on all but two rows, and it drains and recovers the way the creature's own does.
#
# false installs nothing at all: no detour is written into the game image.
enabled = true
# The ATTACK-SET PANEL, in the top-right corner while you are wearing something. It says which
# set each hand is on and out of how many -- `R 2/17`, `L 1/2` -- and what r1/r2/l1/l2 fire on
# that set, so pressing the page keys is something you can WATCH rather than something you have
# to remember.
#
# It exists because the page is a MODE and had no indicator: the keys worked from the day they
# landed, wrote a line to er-npc-possess.log and a [pages] block to er-npc-possess.derived.toml,
# and neither of those is visible from inside a full-screen game. The hand you just paged is
# highlighted for a couple of seconds so the change itself is visible, not only its result.
#
# It appears when a possession starts and disappears when you release. The panel is drawn on the
# same overlay the creature picker uses, so this costs no second hook; set it to false and no
# panel is drawn, but the [pages] block in er-npc-possess.derived.toml still says the same thing.
pages = true

# Per-character overrides, one table per chr id, added as you need them. The DLL reads whatever ids
# this file happens to name -- there is no fixed list -- so a chr nobody has written down yet works
# the moment you add its table. camera_distance_scale, pin, unusable and usable are LIVE;
# turn_rate_deg_per_sec and speed_scale are RESERVED.
# THE IN-GAME CREATURE PICKER. LIVE -- an edit applies within a second.
#
# Press the hotkey below and a list of every creature this mod knows appears on screen, by name,
# with what each one can do. Move the cursor, then press YOUR POSSESS HOTKEY (the `hotkey` /
# `gamepad_hotkey` at the top of this file) to choose. That press does not possess anything: it
# writes the highlighted creature into `[target]` above exactly as if you had typed
# `mode = "chr_id"` and its id yourself, and the log prints the two lines so you can paste them
# in here to keep it. Press the picker hotkey again to close without choosing.
#
# THE LIST DOES NOT OPEN WHILE YOU ARE WEARING SOMETHING, and it closes itself if a possession
# starts while it is up. That is on purpose: the possess hotkey means CHOOSE while the list is
# open and RELEASE while you are possessing, so those two states are never both true. Pick who
# to become while you are yourself; to switch, release first and then pick.
#
# The pick is STAGED, like every other `[target]` edit: it takes effect at the next possession,
# not the current one. And the file wins -- the next time you save this file, `[target]` above is
# read again and the pick is replaced. That is why the log prints the lines to paste.
#
# WHAT PICKING DOES NOT DO, TODAY: it does not summon anything. `mode = "chr_id"` looks through
# the characters ALREADY LOADED in the map for one with that id, exactly as it does when you type
# the id in by hand. Pick Malenia in Limgrave and the next press reports `no loaded enemy matches
# chr_id` in the log, because there is not one. So the list is every creature the mod KNOWS,
# which is a larger set than the creatures you can become where you happen to be standing. The
# list says what each one can do, not whether it is nearby.
#
# THE PICKER DOES NOT TAKE THESE KEYS AWAY FROM THE GAME. This DLL does not hook the game's own
# keyboard reader -- three other mods in this profile already share that one -- so it can SEE a
# keypress but cannot stop Elden Ring from also seeing it. Whether the defaults below collide with
# anything on your setup is a guess, not a reading of the game's binding table; rebind them if
# they do, and the log will tell you if a navigation key never fires.
#
# EVERY PAD BINDING IS EMPTY ON PURPOSE: a controller has few buttons and the game uses them, so a
# shipped default for "cursor down" would very likely swap your spell every time you pressed it.
# Fill them in if you want pad navigation and can live with that; the spellings are the same as
# `gamepad_hotkey`.
[picker]
enabled = true
# Rows on screen at once, 5..40. The list is always all 408 creatures -- this is the window.
visible_rows = 15
# Open and close the list.
hotkey = "F10"
gamepad_hotkey = ""
# One row at a time. Held keys repeat.
#
# These are the ARROWS rather than the numeric keypad, and the reason is worth knowing before you
# rebind: "KP_8" means the virtual key VK_NUMPAD8, and with NUMLOCK OFF the numpad's 8 key does
# not send that -- it sends the same code as the up arrow. So a keypad binding is SILENTLY DEAD
# unless NumLock is on. "Up"/"Down"/"Left"/"Right" are sent by the arrow cluster and by the numpad
# with NumLock off, so both work. Bind KP_* if you prefer, and turn NumLock on.
up = "Up"
down = "Down"
# The other axis, and the reason 408 rows are navigable with four keys: these jump to the
# previous and next INITIAL of the name. The list is sorted by name, so `prev_group` from the
# middle of a long run of "G" names goes to the first of them and again goes to the "F"s.
prev_group = "Left"
next_group = "Right"
# Pad equivalents. Empty = unbound; see the note above before filling these in.
pad_up = ""
pad_down = ""
pad_prev_group = ""
pad_next_group = ""

# RESERVED. Per-character overrides, one table per chr id, added as you need them. The DLL reads
# whatever ids this file happens to name -- there is no fixed list -- so a chr nobody has written
# down yet works the moment you add its table.
#
# Left commented out on purpose: the ids below are an EXAMPLE of the shape, not a tuned profile,
# and a shipped default that pins real animation ids for a character nobody has measured would be
# a lie the first player to read it would take at face value. Uncomment and edit.
#
# [chr.c4500]
# turn_rate_deg_per_sec = 45.0
# # LIVE. Multiplies [camera].distance_exponent's answer for this character only. Above 1 pulls
# # the camera back -- quadrupeds and anything wider than it is tall usually want it -- below 1
# # brings it in. It cannot push the camera inside the creature's own collision radius.
# camera_distance_scale = 2.4
# speed_scale = 1.15
# # force one input onto one animation id
# pin = { r2 = 3046 }
# # animation ids this character must never be given
# unusable = [3300, 3301]
# # ...and ids to force back in, overriding the auto-classifier
# usable = [3120]
"##;

/// The settings in force, and nothing that needs Windows -- so the whole reload decision is
/// `cargo test`-able on the host.
#[derive(Clone, Debug)]
pub(crate) struct PossessConfig {
    pub(crate) config_path: PathBuf,
    pub(crate) enabled: bool,
    keyboard: Binding<Option<Chord>>,
    pub(crate) keyboard_text: String,
    /// Not an `er_hotkey_config::Binding`: that type's parser must fail with `KeyParseError`,
    /// whose `Unknown` message tells the reader to pick a key -- and listing keyboard names at
    /// somebody who mistyped a pad button is a worse answer than none. The keep-the-last-working
    /// -value rule it exists to enforce is reimplemented in [`Self::apply_pad`], which is the
    /// half that matters. Same call `er-refill-all` makes.
    gamepad: PadChord,
    pub(crate) gamepad_text: String,
    radial: PadChord,
    pub(crate) radial_text: String,
    /// `[target]` as the possession engine would see it right now.
    target_in_force: TargetSettings,
    /// `[target]` as the file says. Differs from the above between an edit and the next
    /// possession; see the module docs.
    target_on_disk: TargetSettings,
    pub(crate) tables: Tables,
}

/// The per-frame slice of the config: four Copy values, so the input path does not clone the
/// per-character override tables sixty times a second.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LiveBindings {
    pub(crate) enabled: bool,
    pub(crate) keyboard: Option<Chord>,
    pub(crate) gamepad: PadChord,
    pub(crate) radial: PadChord,
}

/// What re-reading the file did, in the terms the log line needs. `Default` is "nothing moved",
/// which is what almost every poll produces.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ConfigUpdate {
    pub(crate) enabled_moved: Option<(bool, bool)>,
    /// `(old name, new name)`. This is the edge-reset signal for the keyboard.
    pub(crate) keyboard_moved: Option<(String, String)>,
    pub(crate) keyboard_rejected: Option<(String, String)>,
    pub(crate) gamepad_moved: Option<(String, String)>,
    pub(crate) gamepad_rejected: Option<(String, String)>,
    pub(crate) radial_moved: Option<(String, String)>,
    pub(crate) radial_rejected: Option<(String, String)>,
    /// `[target]` changed on disk and is waiting for the next possession. Not in force.
    pub(crate) target_staged: Option<(String, String)>,
    /// One of the live tables moved; carries the whole new summary rather than a per-field diff,
    /// because these are reserved settings and the useful log line is "here is what is in force
    /// now", not "combo_window_ms went from 1200 to 800".
    pub(crate) tables_moved: Option<String>,
    pub(crate) rejections: Rejections,
}

impl ConfigUpdate {
    /// Did anything move? A poll that changed nothing must produce no log line at all.
    pub(crate) fn is_quiet(&self) -> bool {
        self.enabled_moved.is_none()
            && self.keyboard_moved.is_none()
            && self.keyboard_rejected.is_none()
            && self.gamepad_moved.is_none()
            && self.gamepad_rejected.is_none()
            && self.radial_moved.is_none()
            && self.radial_rejected.is_none()
            && self.target_staged.is_none()
            && self.tables_moved.is_none()
            && self.rejections.is_empty()
    }

    /// Did a binding the input path edge-detects on move?
    ///
    /// Reported for the log, not as a control signal: `crate::input::Edges::rebind` compares each
    /// of the three bindings against its own latch every tick, which is what stops an edit to the
    /// pad chord from clearing the keyboard latch and manufacturing a press out of a held key.
    /// Routing the reset through one crate-wide boolean is exactly the bug that would reintroduce.
    pub(crate) fn bindings_moved(&self) -> bool {
        self.keyboard_moved.is_some() || self.gamepad_moved.is_some() || self.radial_moved.is_some()
    }
}

/// An empty value is a real setting -- "no keyboard binding" -- not a parse failure.
fn parse_optional_chord(raw: &str) -> Result<Option<Chord>, KeyParseError> {
    if raw.trim().is_empty() {
        return Ok(None);
    }
    parse_chord(raw).map(Some)
}

fn keyboard_name(chord: Option<Chord>) -> String {
    chord.map_or_else(|| "(none)".to_owned(), chord_name)
}

fn default_pad(spelling: &str) -> PadChord {
    parse_pad_chord(spelling).expect("a built-in default pad chord parses")
}

impl PossessConfig {
    fn new(path: PathBuf) -> Self {
        Self {
            config_path: path,
            enabled: true,
            keyboard: Binding::new(parse_optional_chord(DEFAULT_HOTKEY).ok().flatten()),
            keyboard_text: DEFAULT_HOTKEY.to_owned(),
            gamepad: default_pad(DEFAULT_GAMEPAD_HOTKEY),
            gamepad_text: DEFAULT_GAMEPAD_HOTKEY.to_owned(),
            radial: default_pad(DEFAULT_RADIAL),
            radial_text: DEFAULT_RADIAL.to_owned(),
            target_in_force: TargetSettings::default(),
            target_on_disk: TargetSettings::default(),
            tables: Tables::default(),
        }
    }

    pub(crate) fn bindings(&self) -> LiveBindings {
        LiveBindings {
            enabled: self.enabled,
            keyboard: self.keyboard.code(),
            gamepad: self.gamepad,
            radial: self.radial,
        }
    }

    /// The `[target]` table a possession starting now would use.
    pub(crate) const fn target(&self) -> TargetSettings {
        self.target_in_force
    }

    /// The `[target]` edit waiting for the next possession, if there is one.
    pub(crate) fn staged_target(&self) -> Option<TargetSettings> {
        (self.target_on_disk != self.target_in_force).then_some(self.target_on_disk)
    }

    /// Stage a creature the in-game picker chose, exactly as if the file had named it.
    ///
    /// # Why this writes `target_on_disk` and adds no field of its own
    ///
    /// `target_on_disk` is not "what the file says" -- it is "what is staged for the next
    /// possession", and the file is merely its usual author. Writing there means the pick goes
    /// through the same staging, the same `[target] staged` log line and the same
    /// [`Self::adopt_staged_target`] promotion as a hand edit, with no second code path that
    /// could stage differently. A `target_picked` field beside it would have been exactly that
    /// second path.
    ///
    /// The consequence is worth stating because it is a rule rather than an accident: the next
    /// edit to the config file re-stages from the file and the pick is gone. [`Self::apply`]
    /// rebuilds `target_on_disk` from the file on every reload, and the shipped file always names
    /// `mode` and `chr_id`. Editing the file wins; that is the same precedence the rest of this
    /// config has, and it means a player who wants their pick to stick copies the two lines the
    /// log prints into their own `[target]`.
    ///
    /// Returns the `(from, to)` summaries for the log.
    pub(crate) fn pick_target(&mut self, chr_id: u32) -> (String, String) {
        let before = self.target_on_disk.summary();
        // Spawn mode is preserved, and this is not a nicety. `mode = "chr_id"` searches the
        // characters already loaded in the map, so choosing from a 408-entry catalogue and being
        // switched into it means the next press is almost always refused with "no loaded enemy
        // matches chr_id" -- measured live 2026-09-02: eight consecutive refusals against 428
        // loaded characters, because the picker had silently overwritten the mode that worked.
        // In spawn mode the pick names `[spawn].chr_id` instead, which is the field that decides
        // which creature gets created, and the mode the player chose is left alone.
        if self.target_on_disk.mode == TargetMode::Spawn {
            self.target_on_disk.spawn.chr_id = chr_id;
            return (before, self.target_on_disk.summary());
        }
        self.target_on_disk = TargetSettings {
            mode: TargetMode::ChrId,
            // `[target] chr_id` is an `i32` because the file can name a negative and the parser
            // has to be able to reject it out loud. The catalogue's ids are `u32` and run
            // 100..9001, so this conversion cannot fail in practice; a value that did not fit
            // would not be a chr id at all, and `0` is what `mode = "chr_id"` already means by
            // "no creature named".
            chr_id: i32::try_from(chr_id).unwrap_or(0),
            // Not reset: `release_on_death` is a preference about how a possession ends, and the
            // picker is a statement about who to possess. Carrying it forward is what stops the
            // picker from silently undoing a setting it has no opinion about.
            release_on_death: self.target_on_disk.release_on_death,
            // Carried forward for the same reason: `[spawn]` is how a creature is brought into the
            // world, not which creature, so picking one must not silently reset it.
            spawn: self.target_on_disk.spawn,
        };
        (before, self.target_on_disk.summary())
    }

    /// The chr id a possession starting now would use, or `None` when `[target]` is not in
    /// `chr_id` mode. Read by the picker so re-opening lands on what is already chosen.
    ///
    /// The staged value rather than the one in force, because that is what the picker last wrote
    /// and what the next possession will use.
    pub(crate) fn staged_chr_id(&self) -> Option<u32> {
        let target = self.staged_target().unwrap_or(self.target_in_force);
        if target.mode != TargetMode::ChrId {
            return None;
        }
        // A negative in the file is a typo, not a creature. It is already reported as a rejection
        // by the parser; here it simply names nobody, and the picker opens at the top instead.
        u32::try_from(target.chr_id).ok().filter(|id| *id != 0)
    }

    /// Promote a staged `[target]` into force. Called at the start of a possession, never during
    /// one. Returns the `(from, to)` summaries when something actually moved.
    pub(crate) fn adopt_staged_target(&mut self) -> Option<(String, String)> {
        let staged = self.staged_target()?;
        let before = self.target_in_force.summary();
        self.target_in_force = staged;
        Some((before, self.target_in_force.summary()))
    }

    /// Everything an engine needs to start a possession.
    pub(crate) fn request(&self) -> PossessionRequest {
        PossessionRequest {
            target: self.target_in_force,
            mapping: self.tables.mapping,
            buttons: self.tables.buttons,
        }
    }

    /// One pad binding, with the keep-the-last-working-value rule spelled out.
    fn apply_pad(
        current: &mut PadChord,
        text: &mut String,
        raw: &str,
        moved: &mut Option<(String, String)>,
        rejected: &mut Option<(String, String)>,
    ) {
        // An empty value is a real setting -- "no controller binding" -- not a typo.
        let parsed = if raw.trim().is_empty() {
            Ok(PadChord::default())
        } else {
            parse_pad_chord(raw)
        };
        match parsed {
            Ok(chord) if chord == *current => {
                // Record the spelling they used, so the status line echoes their file rather than
                // the last spelling of the same chord. Not a change.
                *text = raw.to_owned();
            }
            Ok(chord) => {
                let before = pad_chord_name(*current);
                *current = chord;
                *text = raw.to_owned();
                *moved = Some((before, pad_chord_name(chord)));
            }
            // A rejection is not a change and the last working chord stays. Not the shipped
            // default -- that would drag somebody back onto a collision they had just escaped.
            Err(error) => {
                *rejected = Some((format!("{raw:?}: {error}"), pad_chord_name(*current)));
            }
        }
    }

    /// Apply one file's text to the settings in force.
    ///
    /// An absent key keeps what is already in force: the built-in default on the first load, the
    /// last good value on a reload. That is what makes deleting a line the same as never having
    /// written it.
    pub(crate) fn apply(&mut self, text: &str) -> ConfigUpdate {
        let doc = Document::parse(text);
        let mut update = ConfigUpdate::default();
        let mut rejections = Rejections::default();

        if let Some(raw) = doc.scalar("", "enabled") {
            match crate::settings::parse_bool(raw) {
                Some(enabled) if enabled != self.enabled => {
                    update.enabled_moved = Some((self.enabled, enabled));
                    self.enabled = enabled;
                }
                Some(_) => {}
                // Not a silent `false`: a misspelled master switch that reads as "off" is
                // indistinguishable from the mod being broken.
                None => rejections.push("enabled", raw),
            }
        }

        if let Some(raw) = doc.scalar("", "hotkey") {
            let before = keyboard_name(self.keyboard.code());
            match self.keyboard.apply(raw, parse_optional_chord) {
                BindingUpdate::Unchanged => self.keyboard_text = raw.to_owned(),
                BindingUpdate::Changed { .. } => {
                    self.keyboard_text = raw.to_owned();
                    update.keyboard_moved = Some((before, keyboard_name(self.keyboard.code())));
                }
                BindingUpdate::Rejected { value, error, kept } => {
                    update.keyboard_rejected =
                        Some((format!("{value:?}: {error}"), keyboard_name(kept)));
                }
            }
        }

        if let Some(raw) = doc.scalar("", "gamepad_hotkey") {
            Self::apply_pad(
                &mut self.gamepad,
                &mut self.gamepad_text,
                raw,
                &mut update.gamepad_moved,
                &mut update.gamepad_rejected,
            );
        }
        if let Some(raw) = doc.scalar("", "radial") {
            Self::apply_pad(
                &mut self.radial,
                &mut self.radial_text,
                raw,
                &mut update.radial_moved,
                &mut update.radial_rejected,
            );
        }

        // [target] is staged, never applied. The `on_disk` copy is rebuilt from the value in
        // force rather than from itself, so a table the player deleted returns to the built-in
        // default the same way every other absent setting does.
        let mut on_disk = self.target_in_force;
        on_disk.apply_from(&doc, &mut rejections);
        if on_disk != self.target_on_disk {
            self.target_on_disk = on_disk;
            if on_disk != self.target_in_force {
                update.target_staged = Some((self.target_in_force.summary(), on_disk.summary()));
            }
        }

        let before = self.tables.clone();
        self.tables.apply(&doc, &mut rejections);
        if self.tables != before {
            update.tables_moved = Some(self.tables.summary());
        }
        update.rejections = rejections;
        update
    }
}

/// The live settings plus the watcher that keeps them current.
///
/// The derived moveset file is a second `HotFile` field here when it lands, not a rewrite of this
/// struct -- see the module docs.
struct ConfigState {
    config: PossessConfig,
    hot: HotFile,
}

static CONFIG: OnceLock<Mutex<ConfigState>> = OnceLock::new();

/// Incremented by [`poll_reload`] every time a reload moved something. See [`generation`].
static GENERATION: AtomicUsize = AtomicUsize::new(0);

/// CWD-relative, which is the game directory: ME3 launches `eldenring.exe` from it, so this lands
/// beside `eldenring.exe` next to every other er-* DLL's config.
fn config_path() -> PathBuf {
    PathBuf::from(CONFIG_FILE_NAME)
}

fn state() -> MutexGuard<'static, ConfigState> {
    let state = CONFIG.get_or_init(|| {
        let path = config_path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => {
                let _ = fs::write(&path, DEFAULT_CONFIG_TOML);
                DEFAULT_CONFIG_TOML.to_owned()
            }
        };
        let mut hot = HotFile::new(path.clone());
        // Adopt what we just read -- including a default we just wrote -- so the first poll a
        // second from now is not a spurious reload of text nothing has touched. A reload resets
        // the key edge detectors, and one at that moment is a press nobody made.
        hot.adopt(text.clone());
        let mut config = PossessConfig::new(path);
        let update = config.apply(&text);
        if !update.is_quiet() {
            possess_log(format_args!("config: first read -- {}", describe(&update)));
        }
        Mutex::new(ConfigState { config, hot })
    });
    // A poisoned lock means a previous holder panicked. The settings inside are still a valid
    // config, and refusing to read them would disable the feature over a fault that has happened.
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Read the config, writing the commented default file when there is not one yet.
pub(crate) fn init_config() -> PossessConfig {
    state().config.clone()
}

/// The four values the per-frame input path needs.
pub(crate) fn bindings() -> LiveBindings {
    state().config.bindings()
}

/// Everything an engine needs to start a possession, plus the staged-`[target]` promotion that
/// happens at exactly that moment.
///
/// Doing both under one lock is the point: the request handed to the engine has to be the one
/// the log line describes, and a promotion that happened between the two would make them differ.
pub(crate) fn take_request() -> (PossessionRequest, Option<(String, String)>) {
    let mut guard = state();
    let adopted = guard.config.adopt_staged_target();
    (guard.config.request(), adopted)
}

/// The `[movement]` table in force, read fresh.
///
/// A per-frame accessor rather than a field on [`PossessionRequest`], because `[movement]` is a
/// live table: `speed_scale` is the one setting a player tunes by saving the file and watching
/// what changes, and snapshotting it at possession start would make it the one setting that
/// mysteriously needs a re-possess. `[target]` is snapshotted for the opposite reason -- see
/// [`PossessConfig::adopt_staged_target`].
/// The `[chr.cNNNN]` overrides for one creature, or `None` when the file names none.
///
/// Read at possession start rather than snapshotted into [`PossessionRequest`]: the override is a
/// growable `Vec` of pins and animation ids, and `PossessionRequest` is `Copy` on purpose so that
/// the log line describing a possession is a value rather than a borrow of a lock.
pub(crate) fn chr_override(chr_id: u32) -> Option<ChrOverride> {
    let wanted = format!("c{chr_id:04}");
    state()
        .config
        .tables
        .chr_overrides
        .iter()
        .find(|over| over.chr == wanted)
        .cloned()
}

pub(crate) fn movement() -> MovementSettings {
    state().config.tables.movement
}

/// The `[camera]` table in force.
pub(crate) fn camera() -> CameraSettings {
    state().config.tables.camera
}

/// How many reloads have moved something, since the process started.
///
/// The camera layer watches this rather than re-reading its two settings sixty times a second:
/// `[camera]` costs a lock and the per-character `camera_distance_scale` costs a `format!` and a
/// clone, and neither changes between edits to the file. One relaxed atomic load per frame buys
/// the same liveness for nothing. See `camera::Session::refresh`.
pub(crate) fn generation() -> usize {
    GENERATION.load(Ordering::Relaxed)
}

/// `[hud]`, read fresh so an edit to the file takes effect on the next frame like every other
/// live table.
pub(crate) fn hud() -> HudSettings {
    state().config.tables.hud
}

/// The `[picker]` table in force, read fresh. Live, like `[movement]`.
pub(crate) fn picker() -> PickerSettings {
    state().config.tables.picker
}

/// The chr id currently staged, for the picker's opening cursor.
pub(crate) fn staged_chr_id() -> Option<u32> {
    state().config.staged_chr_id()
}

/// Stage the creature the in-game picker chose. Returns the `(from, to)` summaries for the log.
pub(crate) fn pick_target(chr_id: u32) -> (String, String) {
    state().config.pick_target(chr_id)
}

/// Re-read the file if it changed, and report what moved.
///
/// `None` when nothing happened, which is the overwhelmingly common case and the one that must
/// stay silent in the log.
pub(crate) fn poll_reload() -> Option<ConfigUpdate> {
    let mut guard = state();
    let ConfigState { config, hot } = &mut *guard;
    match hot.poll()? {
        FileChange::Text(text) => {
            let update = config.apply(&text);
            if update.is_quiet() {
                return None;
            }
            // Bumped only when something actually moved, which is what makes this usable as a
            // "are my settings stale" test rather than a "was the file touched" one.
            GENERATION.fetch_add(1, Ordering::Relaxed);
            Some(update)
        }
        FileChange::Missing => {
            // Keep the settings that were working. A deleted config is not an instruction to
            // unbind the hotkey, and rewriting the default here would fight a player mid-edit.
            possess_log(format_args!(
                "config: {} disappeared; keeping the settings already in force",
                config.config_path.display()
            ));
            None
        }
    }
}

/// One line describing a reload, or nothing when nothing moved.
pub(crate) fn describe(update: &ConfigUpdate) -> String {
    let mut parts = Vec::new();
    if let Some((from, to)) = update.enabled_moved {
        parts.push(format!("enabled {from} -> {to}"));
    }
    for (label, moved) in [
        ("hotkey", &update.keyboard_moved),
        ("gamepad_hotkey", &update.gamepad_moved),
        ("radial", &update.radial_moved),
    ] {
        if let Some((from, to)) = moved {
            parts.push(format!("{label} {from} -> {to}"));
        }
    }
    for (label, rejected) in [
        ("hotkey", &update.keyboard_rejected),
        ("gamepad_hotkey", &update.gamepad_rejected),
        ("radial", &update.radial_rejected),
    ] {
        if let Some((value, kept)) = rejected {
            parts.push(format!("{label} {value}; STAYING ON {kept}"));
        }
    }
    if let Some((from, to)) = &update.target_staged {
        parts.push(format!(
            "[target] STAGED {from} -> {to} (takes effect at the next possession, not now)"
        ));
    }
    if let Some(summary) = &update.tables_moved {
        parts.push(format!("tables now {summary}"));
    }
    if !update.rejections.is_empty() {
        parts.push(format!(
            "unreadable values kept at their previous settings: {}",
            update.rejections.summary()
        ));
    }
    parts.join(" | ")
}

pub(crate) fn log_update(update: &ConfigUpdate) {
    if update.is_quiet() {
        return;
    }
    possess_log(format_args!("config: {}", describe(update)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Bucket, MappingModel, TargetMode, UnboundInputs};

    fn from_default() -> PossessConfig {
        let mut config = PossessConfig::new(PathBuf::from(CONFIG_FILE_NAME));
        config.apply(DEFAULT_CONFIG_TOML);
        config
    }

    /// The round trip. The file the DLL writes on first run must parse back to exactly the
    /// built-in defaults -- otherwise the shipped file silently means something other than what
    /// the code does, and the difference only shows up in somebody's session.
    #[test]
    fn the_shipped_default_file_parses_back_to_the_built_in_defaults() {
        let fresh = PossessConfig::new(PathBuf::from(CONFIG_FILE_NAME));
        let mut loaded = fresh.clone();
        let update = loaded.apply(DEFAULT_CONFIG_TOML);

        assert!(
            update.rejections.is_empty(),
            "the shipped file must not contain a value the shipped parser rejects: {}",
            update.rejections.summary()
        );
        assert!(
            update.is_quiet(),
            "reading the default file must move nothing: {}",
            describe(&update)
        );
        assert_eq!(loaded.enabled, fresh.enabled);
        assert_eq!(loaded.bindings().keyboard, fresh.bindings().keyboard);
        assert_eq!(loaded.bindings().gamepad, fresh.bindings().gamepad);
        assert_eq!(loaded.bindings().radial, fresh.bindings().radial);
        assert_eq!(loaded.target(), fresh.target());
        assert_eq!(loaded.tables, fresh.tables);
    }

    /// ...and those defaults are the ones the file's own text claims, read independently of the
    /// constants above. A default that drifts from its comment is a lie in the player's editor.
    #[test]
    fn the_shipped_default_file_says_what_it_means() {
        let config = from_default();
        assert!(config.enabled);
        assert_eq!(
            config.bindings().keyboard,
            Some(parse_chord("F9").expect("F9"))
        );
        assert_eq!(pad_chord_name(config.bindings().gamepad), "LS+RS");
        assert_eq!(pad_chord_name(config.bindings().radial), "DPad_Down");
        assert_eq!(config.target().mode, TargetMode::LockOn);
        assert_eq!(config.target().chr_id, 0);
        assert!(config.target().release_on_death);
        assert_eq!(config.tables.mapping.model, MappingModel::Context);
        assert_eq!(config.tables.mapping.combo_window_ms, 1200);
        assert_eq!(config.tables.mapping.bands_m, (4.0, 12.0));
        // On, and asserted rather than assumed: TAE event 304 is 100% of the 4000 animation band
        // and every boss grab in the game, so a `false` here silently removes the signature move
        // of most of what anybody would want to possess.
        assert!(config.tables.mapping.allow_grabs);
        assert_eq!(config.tables.mapping.unbound_inputs, UnboundInputs::Promote);
        assert_eq!(config.tables.mapping.watchdog_seconds, 4.0);
        assert_eq!(config.tables.buttons.r1, Bucket::Light);
        // The attack-set page keys, asserted rather than assumed for the same reason the picker's
        // arrows are: these two are the keys vanilla puts the armament swap on, and picking any
        // other pair would throw away the muscle memory that is the whole argument for them.
        assert_eq!(
            config.tables.buttons.page_right,
            Some(parse_chord("Right").expect("Right"))
        );
        assert_eq!(
            config.tables.buttons.page_left,
            Some(parse_chord("Left").expect("Left"))
        );
        assert_eq!(
            pad_chord_name(config.tables.buttons.pad_page_right),
            "DPad_Right"
        );
        assert_eq!(
            pad_chord_name(config.tables.buttons.pad_page_left),
            "DPad_Left"
        );
        assert_eq!(config.tables.movement.turn_deadzone_deg, 20.0);
        let picker = config.tables.picker;
        assert!(picker.enabled);
        assert_eq!(picker.visible_rows, 15);
        assert_eq!(picker.toggle, Some(parse_chord("F10").expect("F10")));
        // Arrows, not the keypad, and asserted rather than assumed: `KP_8` parses to
        // `VK_NUMPAD8`, which the numpad does not send with NumLock off, so a keypad default is
        // silently dead for anyone who leaves it off. See `settings::PickerSettings`.
        assert_eq!(picker.up, Some(parse_chord("Up").expect("Up")));
        assert_eq!(picker.down, Some(parse_chord("Down").expect("Down")));
        assert_eq!(picker.prev_group, Some(parse_chord("Left").expect("Left")));
        assert_eq!(
            picker.next_group,
            Some(parse_chord("Right").expect("Right"))
        );
        // Empty, and asserted rather than assumed. This DLL cannot take a key away from the game,
        // so a shipped pad default for "cursor down" would also swap the player's spell every
        // time they pressed it. See the `er-npc-possess` entry in scripts/me3-dll-conflicts.toml.
        for pad in [
            picker.pad_toggle,
            picker.pad_up,
            picker.pad_down,
            picker.pad_prev_group,
            picker.pad_next_group,
        ] {
            assert_eq!(
                pad,
                PadChord::default(),
                "every shipped pad binding is unbound"
            );
        }
        // The example per-chr table ships commented out, so a fresh install overrides nothing.
        assert!(config.tables.chr_overrides.is_empty());
    }

    /// A pick stages exactly what a hand edit of `[target]` would have staged, and goes through
    /// the same promotion. If these ever diverge the picker becomes a second way to choose a
    /// creature that behaves differently from the documented one.
    #[test]
    fn a_pick_stages_the_same_thing_a_hand_edit_would() {
        let mut picked = from_default();
        let (from, to) = picked.pick_target(4630);
        assert!(from.contains("mode=lock_on"), "{from}");
        assert!(to.contains("mode=chr_id"), "{to}");
        assert!(to.contains("chr_id=4630"), "{to}");

        let mut typed = from_default();
        typed.apply("[target]\nmode = \"chr_id\"\nchr_id = 4630\n");
        assert_eq!(picked.staged_target(), typed.staged_target());

        // ...and the pick is staged, not in force, exactly like the edit.
        assert_eq!(picked.target().mode, TargetMode::LockOn);
        assert!(picked.adopt_staged_target().is_some());
        assert_eq!(picked.target().mode, TargetMode::ChrId);
        assert_eq!(picked.target().chr_id, 4630);
        assert_eq!(picked.staged_target(), None, "nothing left staged");
    }

    /// The precedence rule the picker's doc comment states: the file wins on its next save.
    #[test]
    fn editing_the_file_replaces_a_pick() {
        let mut config = from_default();
        config.pick_target(4630);
        assert_eq!(config.staged_chr_id(), Some(4630));

        // Any edit re-reads `[target]`, and the shipped file always names mode and chr_id.
        config.apply(DEFAULT_CONFIG_TOML);
        assert_eq!(config.staged_chr_id(), None, "the file took it back");
    }

    /// `release_on_death` is a preference about how a possession ends; the picker has no opinion
    /// about it and must not quietly reset it.
    #[test]
    fn a_pick_carries_release_on_death_forward() {
        let mut config = from_default();
        config.apply("[target]\nrelease_on_death = false\n");
        config.pick_target(2120);
        let staged = config.staged_target().expect("staged");
        assert_eq!(staged.chr_id, 2120);
        assert!(!staged.release_on_death);
    }

    /// The picker opens on whatever is already chosen, so this has to read the staged value and
    /// has to say "nobody" for every mode that is not `chr_id`.
    #[test]
    fn the_staged_chr_id_is_only_reported_in_chr_id_mode() {
        let mut config = from_default();
        assert_eq!(config.staged_chr_id(), None, "lock_on names no id");

        config.apply("[target]\nmode = \"chr_id\"\nchr_id = 2120\n");
        assert_eq!(
            config.staged_chr_id(),
            Some(2120),
            "read before it is adopted"
        );

        config.apply("[target]\nmode = \"chr_id\"\nchr_id = 0\n");
        assert_eq!(config.staged_chr_id(), None, "zero names nobody");

        config.apply("[target]\nmode = \"nearest\"\nchr_id = 2120\n");
        assert_eq!(config.staged_chr_id(), None, "the id is inert in this mode");
    }

    /// A rebind is accepted, names both ends, and reports itself as a binding move -- which is
    /// what resets the edge detectors.
    #[test]
    fn a_rebind_is_accepted_and_reports_both_ends() {
        let mut config = from_default();
        let update = config.apply("hotkey = \"F8\"\ngamepad_hotkey = \"select+start\"\n");
        assert_eq!(
            update.keyboard_moved,
            Some(("F9".to_owned(), "F8".to_owned()))
        );
        assert_eq!(
            update.gamepad_moved,
            Some(("LS+RS".to_owned(), "Select+Start".to_owned()))
        );
        assert!(update.bindings_moved());
        assert_eq!(
            config.bindings().keyboard,
            Some(parse_chord("F8").expect("F8"))
        );
    }

    /// The rule that matters. A typo keeps the key that was working. It does not fall back to the
    /// built-in default, and it does not turn the binding off.
    #[test]
    fn a_rejected_binding_keeps_the_last_working_one() {
        let mut config = from_default();
        config.apply("hotkey = \"F8\"\ngamepad_hotkey = \"select+start\"\n");

        let update = config.apply("hotkey = \"Winkey\"\ngamepad_hotkey = \"select+turbo\"\n");
        assert!(!update.bindings_moved(), "a rejection is not a rebind");
        let (value, kept) = update.keyboard_rejected.expect("keyboard rejection");
        assert!(value.contains("Winkey"), "{value}");
        assert_eq!(
            kept, "F8",
            "F8 is the last one that WORKED, not the default"
        );
        let (value, kept) = update.gamepad_rejected.expect("gamepad rejection");
        assert!(value.contains("turbo"), "{value}");
        assert_eq!(kept, "Select+Start");
        assert_eq!(
            config.bindings().keyboard,
            Some(parse_chord("F8").expect("F8"))
        );
    }

    /// A value that means the same key is not a change. Reporting one resets the edge detector,
    /// and a key held at that instant fires without being pressed.
    #[test]
    fn respelling_a_binding_is_not_a_change() {
        let mut config = from_default();
        for spelling in ["F9", "f9", "  F9  "] {
            let update = config.apply(&format!("hotkey = \"{spelling}\"\n"));
            assert!(update.is_quiet(), "{spelling}: {}", describe(&update));
        }
        // Same for the pad, where order and case are both free.
        let update = config.apply("gamepad_hotkey = \"RS + LS\"\n");
        assert!(update.is_quiet(), "{}", describe(&update));
    }

    /// An empty binding is a real setting -- "no binding" -- and must not be read as a typo.
    #[test]
    fn an_empty_binding_disables_that_device_rather_than_being_rejected() {
        let mut config = from_default();
        let update = config.apply("hotkey = \"\"\ngamepad_hotkey = \"\"\n");
        assert!(update.keyboard_rejected.is_none());
        assert!(update.gamepad_rejected.is_none());
        assert_eq!(config.bindings().keyboard, None);
        assert!(!config.bindings().gamepad.is_bound());
    }

    /// The not-live table. An edit to `[target]` is staged and does not move what a possession
    /// starting now would use.
    #[test]
    fn a_target_edit_is_staged_and_does_not_take_effect_until_the_next_possession() {
        let mut config = from_default();
        let update = config.apply("[target]\nmode = \"chr_id\"\nchr_id = 45000\n");

        let (from, to) = update.target_staged.clone().expect("staged");
        assert!(from.contains("mode=lock_on"), "{from}");
        assert!(to.contains("mode=chr_id"), "{to}");
        assert!(
            !update.bindings_moved(),
            "staging a target must not reset a key edge detector"
        );
        // Still lock_on in force, which is what an engine would be handed.
        assert_eq!(config.target().mode, TargetMode::LockOn);
        assert_eq!(config.request().target.mode, TargetMode::LockOn);
        assert_eq!(
            config.staged_target().map(|t| t.mode),
            Some(TargetMode::ChrId)
        );

        // The next possession promotes it, and only then.
        let moved = config.adopt_staged_target().expect("adopted");
        assert!(moved.1.contains("chr_id=45000"), "{}", moved.1);
        assert_eq!(config.target().mode, TargetMode::ChrId);
        assert_eq!(config.target().chr_id, 45000);
        assert_eq!(config.staged_target(), None, "nothing left staged");
        assert_eq!(
            config.adopt_staged_target(),
            None,
            "and it does not re-fire"
        );
    }

    /// Editing `[target]` back to what is in force clears the staging rather than leaving a
    /// promotion queued that would be a no-op.
    #[test]
    fn a_target_edit_reverted_before_the_next_possession_stages_nothing() {
        let mut config = from_default();
        config.apply("[target]\nmode = \"nearest\"\n");
        assert!(config.staged_target().is_some());
        let update = config.apply("[target]\nmode = \"lock_on\"\n");
        assert_eq!(config.staged_target(), None);
        assert!(update.target_staged.is_none());
        assert_eq!(config.adopt_staged_target(), None);
    }

    /// Everything that is not `[target]` is live: it moves on the reload, with no re-possession.
    #[test]
    fn the_other_tables_are_live() {
        let mut config = from_default();
        let update = config.apply("[mapping]\nmodel = \"slots\"\ncombo_window_ms = 400\n");
        assert!(update.tables_moved.is_some());
        assert_eq!(config.tables.mapping.model, MappingModel::Slots);
        assert_eq!(config.tables.mapping.combo_window_ms, 400);
        assert!(
            !update.bindings_moved(),
            "a table edit must not reset the key edge detectors"
        );
    }

    /// A junk scalar anywhere is collected and named, and never silently applied.
    #[test]
    fn unreadable_values_are_collected_into_one_line() {
        let mut config = from_default();
        let update = config.apply("enabled = maybe\n[movement]\nspeed_scale = fast\n");
        assert!(config.enabled, "a misspelled master switch is not 'off'");
        let line = describe(&update);
        assert!(line.contains("enabled=\"maybe\""), "{line}");
        assert!(line.contains("movement.speed_scale=\"fast\""), "{line}");
    }

    /// A poll that changes nothing must produce no log line at all, or the log is a wall of
    /// "the config reloaded" that hides the one line that matters.
    #[test]
    fn re_reading_the_same_text_is_silent() {
        let mut config = from_default();
        for _ in 0..5 {
            let update = config.apply(DEFAULT_CONFIG_TOML);
            assert!(update.is_quiet(), "{}", describe(&update));
        }
    }

    /// The two file names the DLL owns, spelled once each and asserted so a rename cannot quietly
    /// leave the derived file pointing at the config.
    #[test]
    fn the_config_and_derived_files_are_distinct_and_cwd_relative() {
        assert_eq!(CONFIG_FILE_NAME, "er-npc-possess.toml");
        assert_eq!(DERIVED_CONFIG_FILE_NAME, "er-npc-possess.derived.toml");
        assert_ne!(CONFIG_FILE_NAME, DERIVED_CONFIG_FILE_NAME);
        assert_eq!(config_path(), PathBuf::from(CONFIG_FILE_NAME));
        assert!(
            config_path().is_relative(),
            "the game's own directory is the CWD; an absolute path would be somebody's machine"
        );
    }
}
