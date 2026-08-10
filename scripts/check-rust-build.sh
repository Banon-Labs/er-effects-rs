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
	# er-telemetry is a workspace member but NOT a default-member, so the line above (which
	# honours default-members = er-effects-rs) never compiles its test modules. It owns the
	# load-count consistency logic, so keep its tests building for the shipping target; check.sh
	# RUNS them on the host.
	echo "[check-rust-build] cargo xwin check --tests -p er-telemetry --target $target"
	cargo xwin check --tests -p er-telemetry --manifest-path "$repo_root/Cargo.toml" --target "$target"
	# Save-picker split crates (docs/plans/save-picker-crate-extraction.md). None is a
	# default-member, and the two DLL shells are not depended on by anything, so without
	# this line nothing in any gate would compile them for the shipping target.
	echo "[check-rust-build] cargo xwin check --tests -p er-save-picker -p er-save-picker-dll -p er-quit-menu -p er-quit-menu-dll --target $target"
	cargo xwin check --tests \
		-p er-save-picker -p er-save-picker-dll -p er-quit-menu -p er-quit-menu-dll \
		--manifest-path "$repo_root/Cargo.toml" --target "$target"
	# World-map invasion-spawn warp crates (docs/plans/world-map-invasion-warp.md). Same
	# situation as the save-picker split: neither is a default-member and the DLL shell is
	# not depended on by anything, so without this line nothing in any gate would compile
	# them for the shipping target -- including the `cfg(windows)`-only `CSAutoInvadePoint`
	# read, which the host `cargo test` never sees.
	echo "[check-rust-build] cargo xwin check --tests -p er-invasion-warp -p er-invasion-warp-dll --target $target"
	cargo xwin check --tests \
		-p er-invasion-warp -p er-invasion-warp-dll \
		--manifest-path "$repo_root/Cargo.toml" --target "$target"
	# LINK the cdylib, do not merely type-check it. `cargo xwin check` stops at metadata and
	# never invokes the linker, so a shell that cannot link -- a missing `#[no_mangle]`
	# DllMain, a bad crate-type, an unresolved import from a `cfg(windows)` block -- passed
	# every gate above while being unloadable. Found when the DLL had to be built by hand
	# with an ad-hoc `-p` invocation to run it at all (bd er-effects-rs-5es review).
	# `er-invasion-warp-dll` is not a default-member and nothing depends on it, so this is
	# the only step in any gate that produces the artifact a profile can load.
	# EVERY ME3-LOADABLE SHELL MUST LINK. `cargo xwin check` stops at metadata and never
	# invokes the linker, and the bare `cargo xwin build` above builds only `default-members`
	# (= crates/er-effects-rs). So before this step, 13 of the 15 cdylibs that export a
	# `DllMain` -- i.e. every DLL a user can list in an me3 `[[natives]]` entry except the
	# product itself -- were never linked by ANY gate. A shell that cannot link (missing
	# `#[no_mangle] DllMain`, wrong crate-type, an unresolved import inside a `cfg(windows)`
	# block) passed the whole suite while being unloadable. Found when er-invasion-warp-dll
	# had to be built by hand with an ad-hoc `-p` to run it at all (bd er-effects-rs-5es).
	#
	# Keep this list in sync with the cdylibs that define `DllMain`. Measured cost: ~17s
	# incremental for all 13, which is cheap enough to run unconditionally.
	# `package:artifact`. The artifact is NOT always the package name with dashes swapped for
	# underscores -- four of these override `[lib] name`, so er-better-refills-dll produces
	# er_better_refills.dll, er-inventory-sort-dll -> er_inventory_sort.dll,
	# er-save-disable-dll -> er_save_disable.dll, mushroom-man-runtime -> mushroom_man.dll.
	# Deriving the filename instead of listing it silently skipped those four.
	me3_shells=(
		er-armament-icons:er_armament_icons
		er-better-refills-dll:er_better_refills
		er-crash-logging-dll:er_crash_logging_dll
		er-input-harness-dll:er_input_harness_dll
		er-invasion-warp-dll:er_invasion_warp_dll
		er-inventory-sort-dll:er_inventory_sort
		er-loading-bar-dll:er_loading_bar_dll
		er-loading-portrait-dll:er_loading_portrait_dll
		er-net-effects-dll:er_net_effects_dll
		er-quit-menu-dll:er_quit_menu_dll
		er-reload-trace-dll:er_reload_trace_dll
		er-save-disable-dll:er_save_disable
		er-save-picker-dll:er_save_picker_dll
		er-telemetry-dll:er_telemetry_dll
		mushroom-man-runtime:mushroom_man
	)
	shell_pkg_args=()
	for shell in "${me3_shells[@]}"; do
		shell_pkg_args+=(-p "${shell%%:*}")
	done
	echo "[check-rust-build] cargo xwin build --release ${#me3_shells[@]} me3 shells --target $target (LINK)"
	cargo xwin build --release "${shell_pkg_args[@]}" \
		--manifest-path "$repo_root/Cargo.toml" --target "$target"
	# Assert the artifact exists. `cargo build` succeeding is not by itself proof the cdylib
	# was produced -- and note a bare `rm` of the artifact does NOT force a relink, because
	# cargo restores it from `target/.../deps` with its original mtime. Use
	# `cargo clean -p <crate> --release --target <target>` to force a genuine link locally.
	for shell in "${me3_shells[@]}"; do
		shell_dll="$repo_root/target/$target/release/${shell##*:}.dll"
		if [[ ! -f "$shell_dll" ]]; then
			echo "[check-rust-build] FAIL: cdylib did not link: $shell_dll (package ${shell%%:*})" >&2
			exit 1
		fi
	done
	echo "[check-rust-build] linked ${#me3_shells[@]} me3 shells + the product DLL"
	# FEATURE MATRIX. `er-quit-menu` takes `er-save-picker` with `default-features = false`
	# so a standalone quit-menu DLL links the OS-native fallback surface WITHOUT the boot
	# missing-save flow. Cargo unifies features across a build graph, so the line above
	# only ever exercises the union of the two; this one proves the reduced build compiles
	# on its own and cannot rot.
	echo "[check-rust-build] cargo xwin check -p er-save-picker --no-default-features --target $target"
	cargo xwin check -p er-save-picker --no-default-features \
		--manifest-path "$repo_root/Cargo.toml" --target "$target"
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
