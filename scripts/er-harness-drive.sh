#!/usr/bin/env bash
# Drive the live Elden Ring menus through er-input-harness's command file, and read the state back
# from the product DLL's own log rather than from guesswork.
#
# Why this exists
# ---------------
# Every step of the first successful drive (2026-09-11) was retyped by hand, and two of them were
# wrong in ways that cost far more than the typing:
#
#   * the command file is a QUEUE KEYED ON A SEQUENCE NUMBER, and the number must parse as one.
#     `300d` does not. Three commands written with hex-ish sequences were silently ignored and read
#     as "the input channel does not work".
#   * `resource='02_000_IngameTop'` is NOT a state oracle. The line is repeat-suppressed, so it sits
#     unchanged in the log while the menu moves underneath it. A confirm that had worked was read as
#     a confirm that had not, and the drive was nearly abandoned on that reading. The state oracle is
#     `optionsetting-rows: active tab=N`, which this script reads.
#
# Usage
# -----
#   scripts/er-harness-drive.sh send 'key 0x12 12'   one command, waits for the harness to run it
#   scripts/er-harness-drive.sh tab                  the System menu's active tab, or 'none'
#   scripts/er-harness-drive.sh log [n]              the last n harness repl lines
#
# The sequence number lives in a file beside the command file, so consecutive `send`s cannot collide
# and a caller never has to track it.
#
# The drive that works, measured 2026-09-11 on a live session, in-world to the Quit tab:
#
#   send 'key 0x1 10'        Escape  -- opens 02_000_IngameTop, highlighting Equipment (cell 0)
#   send 'key 0xc8 6'        Up      -- wraps the column to System (cell 6); read it back with 'grid'
#   send 'key 0x12 10'       E       -- confirm; the System menu opens on Game Options, tab 0
#   send 'key 0x2c 8'        Z       -- tab left, wrapping 0 -> 8, which is the Quit tab
#
# Keys that are not what they look like, each one measured here rather than guessed:
#   0x10  Q   closes the menu outright. It is back/cancel, not tab-left.
#   0x1a 0x1b 0xc9 0xd1 0x0f   brackets, page up/down, Tab -- all no-ops on the tab strip.
#   force 0x2b / 0x34 / 0x16 / 0x0d-0x10   the menu-code query the surface polls; none switch tabs.
#   pad / padsweep   refuse with 'no menu pad device observed yet'. A controller can be present on
#                    the host and still never be observed by the game, so this channel is not a
#                    fallback for the keyboard one.
#
# The tab strip wraps, so one Z from tab 0 is the whole journey -- eight presses of the other
# direction reach the same place and are eight more chances to land on the wrong tab.
#
# The Quit tab is a 2x2 grid, not a list. Save Game starts highlighted:
#
#     [Save Game]         [Return to Desktop]
#     [Load Character]    [Load Character from File]
#
# So from the opening position Down reaches Load Character, not Return to Desktop; Return to
# Desktop is one Right. The cell indices match the row table the product logs
# ('system-quit-dup: cloned Quit rows ... row table [Save Game=#0 Return to Desktop=#1
# Load Character=#2 Load Character from File=#3]'), so the grid is numbered across then down.
#
#   send 'key 0xcd 6'        Right   -- Save Game -> Return to Desktop
#   send 'key 0xd0 6'        Down    -- Save Game -> Load Character
#   send 'key 0xc8 6'        Up
#   send 'key 0xcb 6'        Left

set -euo pipefail

game_dir() {
	printf '%s\n' "${ER_GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
}

cmd_file() { printf '%s/er-harness-cmd.txt\n' "$(game_dir)"; }
seq_file() { printf '%s/er-harness-cmd.seq\n' "$(game_dir)"; }
harness_log() { printf '%s/er-input-harness.log\n' "$(game_dir)"; }
product_log() { printf '%s/er-quit-rows-debug.log\n' "$(game_dir)"; }

next_seq() {
	local current=0
	[ -f "$(seq_file)" ] && current=$(cat "$(seq_file)")
	current=$((current + 1))
	printf '%s' "$current" > "$(seq_file)"
	printf '%s\n' "$current"
}

# Write one command and wait until the harness's log shows it ran.
#
# The wait is the acknowledgement line itself, not a sleep: `tail -f` replaying from the line the
# log had reached before the write cannot miss an acknowledgement that lands first, and `grep -m1`
# ends the pipeline the moment it arrives. `timeout` is the safety cap only -- a harness that is
# not polling must fail visibly rather than hang the caller.
send() {
	local command="$1"
	local seq
	seq=$(next_seq)
	local log before
	log=$(harness_log)
	before=$(wc -l < "$log")
	printf '%s\n%s\n' "$seq" "$command" > "$(cmd_file)"
	if timeout 25 tail -n "+$((before + 1))" -f "$log" | grep -q -m1 -F "repl: > $command"; then
		tail -n "+$((before + 1))" "$log" | grep 'repl:' | sed 's/^/  /'
		return 0
	fi
	echo "er-harness-drive: '$command' was never acknowledged in 25s -- is the harness loaded?" >&2
	return 1
}

# The System menu's active tab. Quit is tab 8; Game Options is tab 0.
tab() {
	local line
	line=$(grep -oE 'optionsetting-rows: active tab=[0-9]+' "$(product_log)" | tail -1 || true)
	if [ -z "$line" ]; then
		echo none
	else
		printf '%s\n' "${line##*=}"
	fi
}

case "${1:-}" in
send) send "$2" ;;
tab) tab ;;
log) grep 'repl:' "$(harness_log)" | tail -"${2:-10}" ;;
*)
	sed -n '2,30p' "$0"
	exit 2
	;;
esac
