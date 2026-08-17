#!/usr/bin/env bash
# Build named ME3 shells AND record what they were built from.
#
# WHY THIS EXISTS RATHER THAN A BARE CARGO CALL
# ---------------------------------------------
# `cargo xwin build --release --target x86_64-pc-windows-msvc` honours
#     default-members = ["crates/er-effects-rs"]
# so it builds ONLY the product and exits 0 in a fraction of a second having compiled none of
# the other fifteen shells. That is indistinguishable from a successful incremental build,
# and the stale DLL from last week stays exactly where it was. So every package is named with
# an explicit `-p`, taken from scripts/me3-dll-list.py (the single source of truth for which
# cdylibs this workspace ships, including the four whose artifact name is NOT the package name
# with dashes swapped for underscores).
#
# Provenance is written HERE because it cannot be reconstructed afterwards: proving a DLL came
# from a given source tree needs a content hash taken while that tree was the one being
# compiled. scripts/er-run-branch.py refuses to launch an artifact without it.
#
# Usage:
#   scripts/er-build-dlls.sh er-effects-rs er-armament-icons     # named packages
#   scripts/er-build-dlls.sh --all                               # every shipped shell
#   scripts/er-build-dlls.sh --closure closure.json              # packages from a closure
#
# A cold cross-compile takes minutes -- run this detached, not in a foreground shell.
set -euo pipefail

REPO_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
TARGET="${ER_BUILD_TARGET:-x86_64-pc-windows-msvc}"
PROFILE_DIR="$REPO_ROOT/target/$TARGET/release"

usage() { sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'; }

packages=()
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  --all)
    mapfile -t packages < <(python3 "$REPO_ROOT/scripts/me3-dll-list.py" --pairs | cut -d: -f1)
    ;;
  --closure)
    [[ -f "${2:-}" ]] || { echo "er-build-dlls: no such closure file: ${2:-}" >&2; exit 1; }
    mapfile -t packages < <(python3 -c "
import json,sys
print('\n'.join(json.load(open(sys.argv[1]))['packages']))" "$2")
    ;;
  "")
    echo "er-build-dlls: name at least one package, or pass --all / --closure FILE" >&2
    exit 1
    ;;
  *) packages=("$@") ;;
esac

if [[ ${#packages[@]} -eq 0 ]]; then
  echo "er-build-dlls: nothing to build" >&2
  exit 1
fi

# package -> artifact stem, from the same authoritative array. Deriving the filename by
# swapping dashes for underscores silently skips the four crates that override [lib] name.
declare -A artifact_of
while IFS=: read -r pkg artifact; do
  artifact_of["$pkg"]="$artifact"
done < <(python3 "$REPO_ROOT/scripts/me3-dll-list.py" --pairs)

cargo_args=()
for pkg in "${packages[@]}"; do
  [[ -n "${artifact_of[$pkg]:-}" ]] || {
    echo "er-build-dlls: '$pkg' is not an ME3-loadable shell (see scripts/me3-dll-list.py --pairs)" >&2
    exit 1
  }
  cargo_args+=(-p "$pkg")
done

echo "er-build-dlls: building ${#packages[@]} package(s): ${packages[*]}"
( cd "$REPO_ROOT" && cargo xwin build --release --target "$TARGET" "${cargo_args[@]}" )

status=0
for pkg in "${packages[@]}"; do
  dll="$PROFILE_DIR/${artifact_of[$pkg]}.dll"
  if [[ ! -f "$dll" ]]; then
    echo "er-build-dlls: BUILD REPORTED SUCCESS BUT $dll DOES NOT EXIST ($pkg)" >&2
    status=1
    continue
  fi
  python3 "$REPO_ROOT/scripts/er-dll-provenance.py" write --package "$pkg" --artifact "$dll"
done

exit "$status"
