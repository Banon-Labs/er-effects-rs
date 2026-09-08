#!/usr/bin/env bash
# Build the standalone save-disable DLL and emit an ME3 profile that loads only it.
#
# The profile is deliberately minimal -- no product DLL, no Seamless Co-op -- so the
# run measures the game plus this DLL and nothing else.
# Loading the product alongside it would confound the result: the product manipulates
# save-adjacent state during System->Quit (it clears CSMenuMan->disableSaveMenu at
# +0x13c), so any census taken with it loaded measures the pair, not the game.
#
#   bash scripts/build-save-census-profile.sh
#   OUT_DIR=/somewhere/else bash scripts/build-save-census-profile.sh
#
# Prints the absolute profile path on success. Every path is derived from the repo
# root or an env override; nothing is hard-coded to one machine or user.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 2

TARGET_TRIPLE="${TARGET_TRIPLE:-x86_64-pc-windows-msvc}"
OUT_DIR="${OUT_DIR:-$REPO/target/save-census}"
DLL="$REPO/target/$TARGET_TRIPLE/release/er_save_disable.dll"
HARNESS_DLL="$REPO/target/$TARGET_TRIPLE/release/er_input_harness.dll"
PROFILE="$OUT_DIR/save-census.me3"

# WITH_INPUT_HARNESS=1 adds the self-drive harness so the run reaches save moments the
# title screen never does. Booting alone only exercises the system-slot save; driving
# Continue -> in-world -> System->Quit is what exercises the return-to-title save, and a
# census that never reached those moments cannot claim trigger coverage.
# The product DLL is still deliberately absent either way.
WITH_INPUT_HARNESS="${WITH_INPUT_HARNESS:-0}"

PACKAGES=(er-save-disable)
DLLS=("$DLL")
if [[ "$WITH_INPUT_HARNESS" == "1" ]]; then
	PACKAGES+=(er-input-harness)
	DLLS+=("$HARNESS_DLL")
	echo "== building save-disable + input-harness (release, $TARGET_TRIPLE) =="
else
	echo "== building save-disable (release, $TARGET_TRIPLE) =="
fi

# Built through er-build-dlls.sh rather than a bare `cargo xwin build` because that wrapper
# records provenance for what it just linked -- a content hash over each package's compiled
# closure, taken while that tree was the one being compiled, which cannot be reconstructed
# afterwards. Without it the verification below could never pass, and a gate that can never pass
# is a gate people delete.
if ! ER_BUILD_TARGET="$TARGET_TRIPLE" bash "$REPO/scripts/er-build-dlls.sh" "${PACKAGES[@]}"; then
	echo "BUILD FAILED -- no profile written" >&2
	exit 1
fi

# Refuse to emit, rather than warn. This script's output is not a run, it is a FILE: the .me3
# below names the DLL by absolute path and is handed to `me3 launch -p save-census.me3` later --
# possibly much later, by someone who never saw this shell. A warning printed now is gone by
# then, me3 itself checks nothing, and the census the profile produces would be attributed to
# source that is not what was linked. So the failure has to be the absence of the profile, which
# is the one signal that survives to the moment the profile would have been used -- and that
# means removing the one a previous run left behind, or the refusal is invisible to anyone who
# just runs the path this script has always printed.
# shellcheck source=scripts/er-dll-freshness.sh
# shellcheck disable=SC1091
source "$REPO/scripts/er-dll-freshness.sh"
if ! require_fresh_dlls "${DLLS[@]}"; then
	echo "NO PROFILE WRITTEN -- it would have named a DLL that is not this source tree." >&2
	rm -f "$PROFILE"
	exit 1
fi

mkdir -p "$OUT_DIR" || exit 1
cat >"$PROFILE" <<EOF
profileVersion = "v1"
start_online = false

[[supports]]
game = "eldenring"

# Save-disable DLL: suppresses every save write at the SL submit choke point and
# reports success to the game, while the Win32 file-API census keeps watching for any
# write that escapes. Loads are untouched. Set ER_SAVE_DISABLE_CENSUS_ONLY=1 to disarm
# suppression for the positive-control run.
# See crates/er-save-suppress/src/lib.rs for the interception contract.
[[natives]]
path = '$DLL'
EOF

if [[ "$WITH_INPUT_HARNESS" == "1" ]]; then
	cat >>"$PROFILE" <<EOF

# Self-drive harness. Enabled by its mere presence in the profile; it re-derives all
# state from game memory, so it needs no product DLL. Drive mode comes from
# er-harness-drive-mode.txt in the game directory.
[[natives]]
path = '$HARNESS_DLL'
EOF
fi

echo "== profile: $PROFILE =="
echo "== dll:     $DLL =="
echo
echo "Runtime artifacts land next to eldenring.exe:"
echo "  er-save-disable-telemetry.json  THE ORACLE -- machine-readable, read this."
echo "                                  'escaped_write_sites' must be empty and"
echo "                                  'quit_phase_settle_events' must be non-zero."
echo "  er-save-disable.log             Human debug channel only; nothing parses it."
echo "                                  Repeated events are throttled to exponential"
echo "                                  milestones, so a short log is expected and is"
echo "                                  NOT evidence that few saves were suppressed --"
echo "                                  the count lives in the telemetry."
echo
echo "Pair with the offline witness to prove whether bytes reached disk:"
echo "  python3 scripts/save-write-witness.py snapshot --out before.json"
echo "  # run the game, trigger saves, quit"
echo "  python3 scripts/save-write-witness.py snapshot --out after.json"
echo "  python3 scripts/save-write-witness.py diff before.json after.json"
