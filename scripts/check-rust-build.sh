#!/usr/bin/env bash
# Rust format + build gate. Catches pre-existing rust breakage (format drift AND a broken
# cross-compile of the actual DLL), not just the static python checks. Runs:
#   1. `cargo fmt --all -- --check`  (formatting must be clean)
#   2. a Windows-target BUILD of the injectable DLL, cross-compiled from Linux via cargo-xwin
#      (preferred; falls back to a plain `cargo build` for the target only if cargo-xwin is
#      unavailable -- that path needs an MSVC toolchain on PATH and will otherwise fail at the
#      C-dependency link step).
#
# A BUILD (not just `cargo check`) is used deliberately so the produced
# `target/x86_64-pc-windows-msvc/<profile>/er_effects_rs.dll` is proven to link, catching
# codegen/link regressions a metadata-only check would miss.
#
# Env:
#   CARGO_BUILD_PROFILE=release|dev   (default: release)  -- which profile to build.
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
target="x86_64-pc-windows-msvc"
profile="${CARGO_BUILD_PROFILE:-release}"

echo "[check-rust-build] cargo fmt --all -- --check"
cargo fmt --all --manifest-path "$repo_root/Cargo.toml" -- --check

profile_flag=()
if [ "$profile" = "release" ]; then
	profile_flag=(--release)
fi

if command -v cargo-xwin >/dev/null 2>&1; then
	echo "[check-rust-build] cargo xwin build ${profile_flag[*]} --target $target"
	cargo xwin build "${profile_flag[@]}" --manifest-path "$repo_root/Cargo.toml" --target "$target"
else
	echo "[check-rust-build] cargo-xwin not found; falling back to cargo build --target $target" >&2
	cargo build "${profile_flag[@]}" --manifest-path "$repo_root/Cargo.toml" --target "$target"
fi

echo "[check-rust-build] ok"
