#!/usr/bin/env bash
# Rust format + build gate. Catches pre-existing rust breakage (format drift AND a broken
# cross-compile of the actual DLL), not just the static python checks. Runs:
#   1. `cargo fmt --all -- --check`  (formatting must be clean)
#   2. a Windows-target BUILD of the injectable DLL, cross-compiled from Linux via cargo-xwin
#      (preferred; falls back to a plain `cargo build` for the target only if cargo-xwin is
#      unavailable -- that path needs an MSVC toolchain on PATH and will otherwise fail at the
#      C-dependency link step).
#   3. a Windows-target `cargo check --tests` of the DLL crate, so its `#[cfg(test)]` modules are
#      compiled (a cdylib build alone never touches them)
#   4. the resulting Windows unit-test binaries, RUN under wine when wine is installed
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

# The DLL crate's own unit tests are NOT built by the step above: a lib/cdylib build never
# compiles `#[cfg(test)]`, so until this step existed nothing in any gate had ever compiled --
# let alone run -- them. They rot silently when it does not. Compile them every run so a test
# module can never drift out of the build.
if command -v cargo-xwin >/dev/null 2>&1; then
	echo "[check-rust-build] cargo xwin check --tests --target $target"
	cargo xwin check --tests --manifest-path "$repo_root/Cargo.toml" --target "$target"
fi

# RUN them when a Windows-binary runner is available. The tests are pure logic (row math, config
# parsing, path mapping) with no game dependency, so wine executes them fine; skip cleanly where
# wine is absent rather than failing a gate on an optional tool. Running them on the real target
# matters: the first run of this step found a config-path assertion that only held under HOST
# `std::path` separator semantics, never under the windows target the crate actually ships to.
if command -v cargo-xwin >/dev/null 2>&1 && command -v wine >/dev/null 2>&1; then
	echo "[check-rust-build] cargo xwin test --lib --target $target (via wine)"
	CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine WINEDEBUG="${WINEDEBUG:--all}" \
		cargo xwin test --lib --manifest-path "$repo_root/Cargo.toml" --target "$target"
else
	echo "[check-rust-build] wine not found; skipping the windows-target unit-test RUN (compile check above still ran)" >&2
fi

echo "[check-rust-build] ok"
