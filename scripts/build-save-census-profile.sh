#!/usr/bin/env bash
# Build the standalone save-disable DLL and emit an ME3 profile that loads ONLY it.
#
# The profile is deliberately minimal -- no product DLL, no Seamless Co-op -- because
# the census is meant to establish where a near-vanilla ELDEN RING writes save data.
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
PROFILE="$OUT_DIR/save-census.me3"

echo "== building er-save-disable-dll (release, $TARGET_TRIPLE) =="
if ! cargo xwin build --release -p er-save-disable-dll --target "$TARGET_TRIPLE"; then
	echo "BUILD FAILED -- no profile written" >&2
	exit 1
fi

if [[ ! -f "$DLL" ]]; then
	echo "build reported success but $DLL is missing" >&2
	exit 1
fi

mkdir -p "$OUT_DIR" || exit 1
cat >"$PROFILE" <<EOF
profileVersion = "v1"
start_online = false

[[supports]]
game = "eldenring"

# Census-only build: this DLL observes save writes and suppresses nothing.
# See crates/er-save-disable-dll/src/lib.rs for the phase contract.
[[natives]]
path = '$DLL'
EOF

echo "== profile: $PROFILE =="
echo "== dll:     $DLL =="
echo
echo "Runtime artifacts land next to eldenring.exe:"
echo "  er-save-disable.log             (install + hook status)"
echo "  er-save-disable-telemetry.json  (census; 'escaped_write_sites' is the oracle)"
echo
echo "Pair with the offline witness to prove whether bytes reached disk:"
echo "  python3 scripts/save-write-witness.py snapshot --out before.json"
echo "  # run the game, trigger saves, quit"
echo "  python3 scripts/save-write-witness.py snapshot --out after.json"
echo "  python3 scripts/save-write-witness.py diff before.json after.json"
