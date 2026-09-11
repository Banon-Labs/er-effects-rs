#!/usr/bin/env bash
# Drive a running Elden Ring from the title through to a loaded world, pressing Continue.
#
# Why this exists
# ---------------
# With the `autoload` feature off, `er-quit-rows` no longer loads a character at boot, so getting
# to a world is an input problem again. AGENTS.md's standing order is that the agent drives every
# input, and the number of dialogs between the title and the menu is not fixed -- a patch notice, a
# cloud-save notice, a network warning -- so the loop presses confirm until the menu opens rather
# than assuming a count.
#
# The one dialog that must NOT be dismissed
# -----------------------------------------
# If the terms-of-service dialog builds, this run has no save to continue from: the game only shows
# it before the profile exists. Confirming through it would start a fresh character and quietly
# destroy the premise of whatever test asked for a Continue. So it is a HARD FAILURE, detected
# natively rather than by reading the screen -- the product logs `policy-oracle: TosTitle ctor
# 0x... built` from a detour on the dialog's own constructor (`POLICY_TOS_TITLE_CTOR_RVA`), which
# AGENTS.md requires in place of OCR.
#
# Usage
# -----
#   scripts/er-drive-to-continue.sh --arm      write the harness mode flag (BEFORE launching)
#   scripts/er-drive-to-continue.sh            watch the armed run through to an open menu
#   scripts/er-drive-to-continue.sh --to <row> ...and on to one Quit-tab row, which is one of
#                                              save-game, return-to-desktop, load-character,
#                                              load-character-from-file
#   scripts/er-drive-to-continue.sh --selftest check the log matching with no game running
#
# Requires `er_input_harness.dll` and a product shell in the running profile -- see
# ~/Elden/quit-rows-harness.me3 and scripts/er-harness-drive.sh, which this builds on.

set -uo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# Where the drive stops. Empty means "at the open pause menu", which is the handoff point for a
# caller that wants to drive something else from there.
DESTINATION=""
DRIVE="$REPO_ROOT/scripts/er-harness-drive.sh"

game_dir() {
	printf '%s\n' "${ER_GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
}
product_log() { printf '%s/er-quit-rows-debug.log\n' "$(game_dir)"; }
harness_log() { printf '%s/er-input-harness.log\n' "$(game_dir)"; }

# Native signals, each one a line the product already writes. Kept together because they are the
# whole contract between this script and the DLL: change a log string and the drive goes blind.
EULA_BUILT='policy-oracle: TosTitle ctor'
TITLE_MOVIE='title-resource-observer:.*label=05_000_title'
MENU_OPEN='title-open-menu: PASS-THROUGH native open_menu'
# Reaching the world is read from the HARNESS log, not the product's. `STEP_MoveMap_Update` and
# `EVENT T_controllable` were the obvious choices and are the wrong ones here: both are written by
# autoload-path telemetry, so in a build with the `autoload` feature compiled out they never appear
# and a watcher keyed on them waits out its cap on a world that is already up. Measured
# 2026-09-11 -- zero of either line in a run whose harness reported `world_sim=1`.
#
# The harness's phase log has no such dependency: it is the thing doing the driving, and it says
# `world_sim=1` when the simulation is running, which is also when the pause menu will open.
WORLD_REACHED='phase\[3\] wait_load_in ADVANCED'
CONTROLLABLE='world_sim=1'
# Every system message box the game builds, by id. The product logs one line per box, so the
# "unknown number of dialogs" is auditable after the fact instead of inferred from how many
# keys were sent -- a press that landed on nothing and a box that needed two are otherwise
# indistinguishable. Seamless Co-op contributes at least one; a profile without it may build
# none at all, which is not a failure.
MSGBOX='grsysmsg #[0-9]+: id='

# DirectInput scancodes, every one measured on a live session (see scripts/er-harness-drive.sh).
KEY_CONFIRM=0x12 # E
KEY_UP=0xc8
KEY_MENU=0x1      # Escape
KEY_DOWN=0xd0
KEY_LEFT=0xcb
KEY_RIGHT=0xcd
KEY_TAB_LEFT=0x2c # Z -- and not Q, which is back
# The System menu is up: the game acquires its own menu resource, once, when it opens.
OPTION_MENU="AcquireMenuResource.*02_040_Option"
# The Quit tab is up AND carries our rows. This is the one that matters -- it is written by the
# row cloner as the tab builds, so it proves the destination and the feature in the same line.
QUIT_ROWS='system-quit-dup: cloned Quit rows'

# How many confirms to spend clearing dialogs. A bound, not an expectation: the loop stops the
# moment the menu opens, and a run that needs more than this has something wrong that pressing
# again will not fix.
MAX_DIALOG_PRESSES=${ER_MAX_DIALOG_PRESSES:-12}

# Whether `pattern` appears in the product log at or after line `from`.
seen_since() {
	local pattern="$1" from="$2"
	tail -n "+$from" "$(product_log)" 2>/dev/null | grep -qE "$pattern"
}

log_lines() { wc -l < "$(product_log)" 2>/dev/null || echo 0; }

# Block until `pattern` appears, or the cap expires. The cap is a safety backstop; the readiness is
# the line itself.
wait_for() { wait_in "$(product_log)" "$@"; }

wait_in() {
	local file="$1" pattern="$2" cap="$3" from="$4"
	# `grep -q` exits the moment it matches, which kills `tail -f` with SIGPIPE (141). Under
	# `pipefail` that becomes the pipeline's status, so a successful match reported failure and the
	# first live drive sat out its whole 180s cap on a milestone that was already in the log.
	# grep's own verdict is the only one that means anything here.
	local verdict
	verdict=$(timeout "$cap" tail -n "+$from" -f "$file" 2>/dev/null |
		{ grep -qE -m1 "$pattern" && echo matched || echo missed; })
	[ "$verdict" = matched ]
}

press() { bash "$DRIVE" send "key $1 10" >/dev/null 2>&1; }

mode_flag() { printf '%s/er-harness-drive-mode.txt\n' "$(game_dir)"; }

# Arm the harness's boot drive. Separate from the watch because the harness reads this file once, at
# attach: armed after launch it does nothing, and the run looks like broken input rather than a flag
# that arrived late.
arm() {
	printf 'boot\n' > "$(mode_flag)"
	echo "er-drive-to-continue: armed $(mode_flag) = boot -- launch the game now, then run this with no argument"
}

# The terms-of-service check, run after every press. Fails the whole drive rather than returning,
# because every later step would be operating on a game that is creating a character.
abort_if_eula() {
	local from="$1"
	if seen_since "$EULA_BUILT" "$from"; then
		echo "er-drive-to-continue: HARD FAILURE -- the terms-of-service dialog was built." >&2
		echo "  That dialog only appears when there is no profile to continue from, so this session" >&2
		echo "  has no save and confirming through it would start a new character." >&2
		tail -n "+$from" "$(product_log)" | grep -E "$EULA_BUILT" | tail -1 >&2
		echo "  Tearing the session down rather than leaving a mis-set-up run on screen." >&2
		python3 "$REPO_ROOT/scripts/er-teardown.py" --reason eula-built-no-save-to-continue >&2
		exit 3
	fi
}

drive() {
	# The whole log, not the tail of it. The product opens a fresh log per process (its second line
	# says so), so every line in the file belongs to this run -- and the title is usually up before
	# a caller gets here. Starting from "wherever the log has reached now" made the first version
	# wait 180 seconds for a milestone that had been written a minute earlier.
	local start=1
	[ "$(log_lines)" -eq 0 ] && {
		echo "er-drive-to-continue: no product log at $(product_log) -- is the game running?" >&2
		exit 2
	}

	echo "er-drive-to-continue: waiting for the title"
	if ! wait_for "$TITLE_MOVIE|$MENU_OPEN|$EULA_BUILT" 180 "$start"; then
		echo "er-drive-to-continue: the title never came up within 180s" >&2
		exit 2
	fi
	abort_if_eula "$start"

	# The title is NOT driven from here, and the first version of this script spent two live boots
	# learning that. `er-input-harness`'s own `drive.rs` says it, from bd
	# `title-continue-is-accept-byte-not-keystate`: press any button -> menu -> Continue is the
	# global accept byte, not the `inputmgr+0x90` keystate bitmap that the repl's `key` verb writes.
	# Twelve confirms at the title moved nothing because none of them was ever read.
	#
	# So the harness drives it, through the mode flag armed before launch: `boot` is
	# `DriveMode::BootContinueOnly`, whose phases are Startup -> PressAnyButton -> Continue ->
	# WaitLoadIn, and whose popup-accept cadence answers dialog-OK every frame -- which is what makes
	# "an unknown number of dialogs" a non-problem rather than a counting exercise.
	echo "er-drive-to-continue: harness is driving the title (mode flag 'boot')"
	if ! wait_in "$(harness_log)" "$WORLD_REACHED" 300 1; then
		echo "er-drive-to-continue: the harness never reached a world within 300s" >&2
		echo "  Check that er-harness-drive-mode.txt read 'boot' BEFORE the game started -- the" >&2
		echo "  harness reads it once, at attach." >&2
		exit 2
	fi
	abort_if_eula "$start"

	local boxes
	boxes=$(tail -n "+$start" "$(product_log)" | grep -cE "$MSGBOX")
	echo "er-drive-to-continue: $boxes message box(es) built on the way in"
	tail -n "+$start" "$(product_log)" | grep -oE "$MSGBOX[0-9-]+" | sed 's/^/  accepted /'

	echo "er-drive-to-continue: world is stepping -- Continue loaded"

	# Continue is the trigger, not the destination. What a caller actually wants is a session it can
	# open the menu on, and the wait for that is the whole reason this step exists: the game enables
	# input some seconds after the map is up, so a menu key pressed on the strength of
	# `STEP_MoveMap_Update` alone is swallowed and the drive reads as broken input.
	if ! wait_in "$(harness_log)" "$WORLD_REACHED.*$CONTROLLABLE" 60 1; then
		echo "er-drive-to-continue: the world loaded but the simulation never started running" >&2
		exit 2
	fi
	echo "er-drive-to-continue: simulation running -- the menu will take input"

	# The menu is read from the game's own controls, not from a log line. `MenuWindowJob::Run
	# resource='02_000_IngameTop'` looked like the obvious signal and is not written at all in a
	# build with the `autoload` feature off, so a watcher keyed on it declares a menu that is open on
	# screen to be shut. The harness's `grid` verb enumerates every live `CS::GridControl`, and the
	# pause menu brings its own -- so the count rising is the menu opening, whatever the log says.
	local grids_before grids_after
	grids_before=$(grid_count)
	press "$KEY_MENU"
	grids_after=$(grid_count)
	if [ "$grids_after" -le "$grids_before" ]; then
		echo "er-drive-to-continue: the pause menu did not open ($grids_before grid(s) before, $grids_after after)" >&2
		echo "  If the harness reported delivered=true, check that er_focus_input.dll is in the" >&2
		echo "  profile: Elden Ring skips its input update entirely on an unfocused frame." >&2
		exit 2
	fi
	echo "er-drive-to-continue: pause menu open ($grids_before -> $grids_after grids)"

	[ -z "$DESTINATION" ] && return 0
	to_quit_tab || exit 2
	take_row "$DESTINATION" || exit 2
	return 0
}

# The System menu's active tab, for reporting only. Quit is tab 8, Game Options is tab 0. This is
# deliberately not used as a gate: the line is written when rows build, not when a tab opens, so
# it reads `none` for a menu that is plainly on screen.
tab() {
	local line
	line=$(grep -oE 'optionsetting-rows: active tab=[0-9]+' "$(product_log)" | tail -1 || true)
	[ -z "$line" ] && { echo none; return; }
	printf '%s\n' "${line##*=}"
}

# Pause menu -> System -> the Quit tab. Every key here was measured on a live session; two of them
# are not what they look like, which is why each step reads its result back.
to_quit_tab() {
	# The pause menu opens on Equipment. System is the last entry in the column, so one Up wraps to
	# it -- six Downs reach the same place and are six more chances to stop on the wrong row.
	local before
	before=$(log_lines)
	press "$KEY_UP"
	press "$KEY_CONFIRM"
	if ! wait_for "$OPTION_MENU" 20 "$before"; then
		echo "er-drive-to-continue: the System menu did not open" >&2
		return 1
	fi
	# `Z`, not `Q`. Q is back/cancel and closes the menu outright. The tab strip wraps, so one Z
	# from tab 0 lands on the Quit tab.
	before=$(log_lines)
	press "$KEY_TAB_LEFT"
	if ! wait_for "$QUIT_ROWS" 20 "$before"; then
		echo "er-drive-to-continue: the Quit tab never built its rows" >&2
		return 1
	fi
	echo "er-drive-to-continue: on the Quit tab, rows cloned"
}

# Move to one of the Quit tab's four rows and confirm it.
#
# The tab is a 2x2 grid, not a list, and the cell numbers match the row table the product logs:
#
#     0 Save Game          1 Return to Desktop
#     2 Load Character     3 Load Character from File
#
# Save Game is highlighted when the tab opens, so Down reaches Load Character and Return to Desktop
# is one Right -- the mistake worth spelling out, because Down is the intuitive guess for the row
# printed second.
take_row() {
	case "$1" in
	save-game) ;;
	return-to-desktop) press "$KEY_RIGHT" ;;
	load-character) press "$KEY_DOWN" ;;
	load-character-from-file)
		press "$KEY_DOWN"
		press "$KEY_RIGHT"
		;;
	*)
		echo "er-drive-to-continue: unknown row '$1'" >&2
		return 1
		;;
	esac
	echo "er-drive-to-continue: confirming '$1'"
	press "$KEY_CONFIRM"
}

# How many `CS::GridControl` instances the game currently has. The pause menu adds its own, so this
# is a menu-open oracle that needs no log line and no screenshot.
grid_count() {
	local before
	before=$(wc -l < "$(harness_log)")
	bash "$DRIVE" send "grid" >/dev/null 2>&1
	# `grid vtable=0x... -> N instance(s)` carries the count, and it is written with the header
	# rather than after the per-instance lines. Counting `GridControl` lines instead reads whatever
	# has arrived by the time the command is acknowledged, which is none of them -- the result trails
	# the acknowledgement by a few hundred milliseconds, and a pause menu that was open on screen
	# read as zero grids twice because of it.
	local line
	line=$(timeout 8 tail -n "+$((before + 1))" -f "$(harness_log)" 2>/dev/null |
		{ grep -m1 -oE 'grid vtable=0x[0-9a-f]+ -> [0-9]+ instance' || true; })
	# Exactly one integer on stdout, whatever happened. An earlier version ended the pipeline with
	# `|| echo 0`, which fired alongside a successful match and handed the caller two lines.
	line=${line##* -> }
	line=${line%% *}
	case "$line" in
	'' | *[!0-9]*) echo 0 ;;
	*) echo "$line" ;;
	esac
}

# Check the matching logic against text rather than against a game: every one of these patterns is
# a promise about a line the DLL writes, and a typo in one is invisible until a live run wastes a
# boot discovering it.
selftest() {
	local tmp
	tmp=$(mktemp)
	cat > "$tmp" <<'FIXTURE'
[+12186ms] dll:a2b17ba3 title-resource-observer: Scaleform file-open title-memory label=05_000_title logo_hit=0 total=68
[+14885ms] dll:a2b17ba3 title-open-menu: PASS-THROUGH native open_menu #1 (dialog=0x24575480) suppressed_so_far=0
[000042 +1724592336ms] phase[3] wait_load_in ADVANCED after 319f (pause_menu=0 menu_id=-1 world_sim=1 save_state=0 title_state=6)
[+99999ms] dll:a2b17ba3 policy-oracle: TosTitle ctor 0x1409b5970 built object=0x12345678
FIXTURE
	local failures=0
	local name pattern
	for pair in "title:$TITLE_MOVIE" "title-menu:$MENU_OPEN" "world:$WORLD_REACHED" "controllable:$CONTROLLABLE" "eula:$EULA_BUILT"; do
		name=${pair%%:*}
		pattern=${pair#*:}
		if grep -qE "$pattern" "$tmp"; then
			echo "  ok    $name"
		else
			echo "  FAIL  $name -- pattern does not match its own fixture line: $pattern" >&2
			failures=$((failures + 1))
		fi
	done
	# A clean boot must not read as an end-user licence prompt.
	if grep -vE "$EULA_BUILT" "$tmp" | grep -qE "$EULA_BUILT"; then
		echo "  FAIL  eula pattern matches a line it should not" >&2
		failures=$((failures + 1))
	else
		echo "  ok    eula pattern is specific"
	fi
	rm -f "$tmp"
	[ "$failures" -eq 0 ] || return 1
	echo "er-drive-to-continue --selftest: ok"
}

case "${1:-}" in
--selftest) selftest ;;
--arm) arm ;;
"") drive ;;
--to)
	DESTINATION="${2:-}"
	drive
	;;
*)
	sed -n '2,28p' "$0"
	exit 2
	;;
esac
