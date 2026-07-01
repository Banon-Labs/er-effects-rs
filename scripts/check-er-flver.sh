#!/usr/bin/env bash
# Verify the er-flver crate + its migration: unit tests, the wgpu-gated path, and the
# downstream consumers (er-objectkit re-export alias, the viewer) still build.
# Host-only; never touches the Windows cdylib. Run from anywhere.
set -u
cd "$(dirname "$0")/.." || exit 2

echo "===== cargo test -p er-flver ====="
cargo test -p er-flver 2>&1 | tail -35
echo "EXIT_TEST=${PIPESTATUS[0]}"

echo "===== cargo check -p er-flver --features wgpu ====="
cargo check -p er-flver --features wgpu 2>&1 | tail -15
echo "EXIT_WGPU=${PIPESTATUS[0]}"

echo "===== cargo check -p er-objectkit ====="
cargo check -p er-objectkit 2>&1 | tail -25
echo "EXIT_OBJECTKIT=${PIPESTATUS[0]}"

echo "===== cargo check -p er-shader-viewer ====="
cargo check -p er-shader-viewer 2>&1 | tail -25
echo "EXIT_VIEWER=${PIPESTATUS[0]}"

echo "===== DONE ====="
