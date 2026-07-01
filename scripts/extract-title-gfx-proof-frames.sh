#!/usr/bin/env bash
set -euo pipefail

# Stable/overwrite-by-default frame extraction for the latest title GFX proof video.
# Usage:
#   scripts/extract-title-gfx-proof-frames.sh                 # all frames
#   scripts/extract-title-gfx-proof-frames.sh 14 15           # time range [14s,15s]

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ARTIFACT_DIR=${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/title-gfx-proof-latest}
OUT_DIR=${OUT_DIR:-$ARTIFACT_DIR/frames}
VIDEO=${VIDEO:-}

if [[ -z "$VIDEO" ]]; then
  shopt -s nullglob
  candidates=("$ARTIFACT_DIR"/wf-*fps.mkv "$ARTIFACT_DIR"/wf-*fps.mp4 "$ARTIFACT_DIR"/fast-*fps.mp4)
  shopt -u nullglob
  if (( ${#candidates[@]} == 0 )); then
    echo "no proof video found in $ARTIFACT_DIR" >&2
    exit 1
  fi
  VIDEO=${candidates[0]}
fi

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

if (( $# >= 2 )); then
  start=$1
  end=$2
  ffmpeg -hide_banner -loglevel error -y -ss "$start" -to "$end" -i "$VIDEO" -vsync 0 "$OUT_DIR/frame-%04d.png"
else
  ffmpeg -hide_banner -loglevel error -y -i "$VIDEO" -vsync 0 "$OUT_DIR/frame-%04d.png"
fi

count=$(find "$OUT_DIR" -maxdepth 1 -type f -name 'frame-*.png' | wc -l)
echo "video=$VIDEO"
echo "out=$OUT_DIR"
echo "frames=$count"
