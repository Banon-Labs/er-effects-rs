#!/usr/bin/env bash
# Rust format + build gate. Catches pre-existing rust breakage (format drift and a broken
# cross-compile of the actual DLL), not just the static python checks. Runs:
#   1. `cargo fmt --all -- --check`  (formatting must be clean)
#   2. a Windows-target build of the injectable DLL, cross-compiled from Linux via cargo-xwin
#      (preferred; falls back to a plain `cargo build` for the target only if cargo-xwin is
#      unavailable -- that path needs an MSVC toolchain on path and will otherwise fail at the
#      C-dependency link step).
#   3. a Windows-target `cargo check --tests` of the DLL crate, so its `#[cfg(test)]` modules are
#      compiled (a cdylib build alone never touches them)
#   4. the resulting Windows unit-test binaries, run under wine when wine is installed
#
# A build (not just `cargo check`) is used deliberately so the produced
# `target/x86_64-pc-windows-msvc/<profile>/er_quickload.dll` is proven to link, catching
# codegen/link regressions a metadata-only check would miss.
#
# Env:
#   CARGO_BUILD_PROFILE=release|dev   (default: release)  -- which profile to build.
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

# This gate builds and relinks every shell, then re-attests provenance for all of them. Same
# reason as the compile gate: yield from inside rather than trusting the caller to wrap it.
# shellcheck source=lib/cpu-courtesy.sh
# shellcheck disable=SC1091  # sourced at run time; shellcheck -x is not how this suite is linted.
. "$repo_root/scripts/lib/cpu-courtesy.sh"
cpu_courtesy check-rust-build
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

# The DLL crate's own unit tests are not built by the step above: a lib/cdylib build never
# compiles `#[cfg(test)]`, so until this step existed nothing in any gate had ever compiled --
# let alone run -- them. They rot silently when it does not. Compile them every run so a test
# module can never drift out of the build.
# CLIPPY on the Windows target. `cargo clippy` on the host cannot see a windows-only module: a
# `#[cfg(windows)]` file is not compiled there, so it is not linted there either, and a lint that
# only exists on the target the module actually builds for reaches CI untouched. That is exactly
# how `clippy::manual_is_multiple_of` in er-refill-all's runtime.rs got through a clean local run
# and failed CI (2026-08-25). Workspace-wide, because the gap applies to every DLL crate.
#
# --all-targets added 2026-09-03, closing a gap in the opposite direction. Without it this
# line lints lib and bin targets only, so `#[cfg(test)]` modules, benches and examples were
# linted by nothing -- and the moment scripts/check-committed-compiles.sh started linting
# with --all-targets at pre-push, it found three lint errors sitting on a green main that
# this gate had never been able to see: needless_update in er-build-export's test helper,
# manual_contains and assertions_on_constants in er-game-base.
#
# That is the worse shape of the two failures described above. Those are "CI catches what a
# local run misses"; this was "neither catches it, and the suite reports green". It also
# inverted the contract between the two: a pre-push hook is meant to be the fast subset of
# CI, never the only place a whole class of defect can be found.
if command -v cargo-xwin >/dev/null 2>&1; then
	echo "[check-rust-build] cargo xwin clippy --workspace --all-targets --target $target"
	cargo xwin clippy --workspace --all-targets --manifest-path "$repo_root/Cargo.toml" --target "$target"
else
	echo "[check-rust-build] cargo-xwin not found; skipping windows-target clippy" >&2
fi

if command -v cargo-xwin >/dev/null 2>&1; then
	echo "[check-rust-build] cargo xwin check --tests --target $target"
	cargo xwin check --tests --manifest-path "$repo_root/Cargo.toml" --target "$target"
	# er-telemetry-core is a workspace member but not a default-member, so the line above (which
	# honours default-members = er-quickload) never compiles its test modules. It owns the
	# load-count consistency logic, so keep its tests building for the shipping target; check.sh
	# runs them on the host.
	echo "[check-rust-build] cargo xwin check --tests -p er-telemetry-core --target $target"
	cargo xwin check --tests -p er-telemetry-core --manifest-path "$repo_root/Cargo.toml" --target "$target"
	# The Scaleform hook owner is a library, not a default member and not yet linked by a
	# product until R24 moves the first hook family. Keep its Windows-only er-hook edge and
	# test module compiling from the R23 skeleton onward.
	echo "[check-rust-build] cargo xwin check --tests -p er-scaleform-hooks --target $target"
	cargo xwin check --tests -p er-scaleform-hooks --manifest-path "$repo_root/Cargo.toml" --target "$target"
	# Save-picker split crates (docs/plans/save-picker-crate-extraction.md). None is a
	# default-member, and the two DLL shells are not depended on by anything, so without
	# this line nothing in any gate would compile them for the shipping target.
	echo "[check-rust-build] cargo xwin check --tests -p er-save-picker-core -p er-save-picker -p er-quit-menu-core -p er-quit-menu --target $target"
	cargo xwin check --tests \
		-p er-save-picker-core -p er-save-picker -p er-quit-menu-core -p er-quit-menu \
		--manifest-path "$repo_root/Cargo.toml" --target "$target"
	# The ProfileSummary crate split. Not a default-member, so without this line its
	# `#[cfg(windows)]` test module -- the runtime `ChrAsm` image reassembly, which is the one
	# place a foreign save's bytes are rearranged into the layout the native copy expects --
	# would compile for no target at all. The host `cargo test` in check.sh cannot see it.
	echo "[check-rust-build] cargo xwin check --tests -p er-profile-summary-core --target $target"
	cargo xwin check --tests -p er-profile-summary-core \
		--manifest-path "$repo_root/Cargo.toml" --target "$target"
	# World-map invasion-spawn warp crates (docs/plans/world-map-invasion-warp.md). Same
	# situation as the save-picker split: neither is a default-member and the DLL shell is
	# not depended on by anything, so without this line nothing in any gate would compile
	# them for the shipping target -- including the `cfg(windows)`-only `CSAutoInvadePoint`
	# read, which the host `cargo test` never sees.
	echo "[check-rust-build] cargo xwin check --tests -p er-invasion-warp-core -p er-invasion-warp --target $target"
	cargo xwin check --tests \
		-p er-invasion-warp-core -p er-invasion-warp \
		--manifest-path "$repo_root/Cargo.toml" --target "$target"
	# Link the cdylib, do not merely type-check it. `cargo xwin check` stops at metadata and
	# never invokes the linker, so a shell that cannot link -- a missing `#[no_mangle]`
	# DllMain, a bad crate-type, an unresolved import from a `cfg(windows)` block -- passed
	# every gate above while being unloadable. Found when the DLL had to be built by hand
	# with an ad-hoc `-p` invocation to run it at all (bd er-effects-rs-5es review).
	# `er-invasion-warp` is not a default-member and nothing depends on it, so this is
	# the only step in any gate that produces the artifact a profile can load.
	# Every ME3-LOADABLE shell must link. `cargo xwin check` stops at metadata and never
	# invokes the linker, and the bare `cargo xwin build` above builds only `default-members`
	# (= crates/er-quickload). So before this step, 13 of the 15 cdylibs that export a
	# `DllMain` -- i.e. every DLL a user can list in an me3 `[[natives]]` entry except the
	# product itself -- were never linked by any gate. A shell that cannot link (missing
	# `#[no_mangle] DllMain`, wrong crate-type, an unresolved import inside a `cfg(windows)`
	# block) passed the whole suite while being unloadable. Found when er-invasion-warp
	# had to be built by hand with an ad-hoc `-p` to run it at all (bd er-effects-rs-5es).
	#
	# Keep this list in sync with the cdylibs that define `DllMain`. Measured cost: well under
	# a second per shell incrementally, which is cheap enough to run unconditionally. (The
	# count is deliberately not written here any more -- it said "all 13" while the list held
	# 22, and the echo below prints `${#me3_shells[@]}` at runtime anyway.)
	# `package:artifact`. Since the `-dll` suffix removal every ER shell's artifact is its
	# package name with dashes swapped for underscores -- but the pair is still written out
	# rather than derived, because `mushroom-man-runtime` produces mushroom_man.dll and
	# er-ags-stub produces amd_ags_x64.dll. Deriving the filename would silently skip those,
	# which is how four overridden `[lib] name`s went unchecked before.
	me3_shells=(
		er-armament-icons:er_armament_icons
		er-better-refills:er_better_refills
		er-build-import:er_build_import
		er-enemynpc-effects:er_enemynpc_effects
		er-convenient-deaths:er_convenient_deaths
		er-crash-logging:er_crash_logging
		er-hotkey-conflicts:er_hotkey_conflicts
		er-death-persist:er_death_persist
		er-diag-harness:er_diag_harness
		er-input-harness:er_input_harness
		er-focus-input:er_focus_input
		er-build-watermark:er_build_watermark
		er-invasion-path:er_invasion_path
		er-invasion-warp:er_invasion_warp
		er-inventory-sort:er_inventory_sort
		er-refill-all:er_refill_all
		er-loading-bar:er_loading_bar
		er-loading-portrait:er_loading_portrait
		er-net-effects:er_net_effects
		er-npc-possess:er_npc_possess
		er-player-name-filter:er_player_name_filter
		er-quit-menu:er_quit_menu
		er-reload-trace:er_reload_trace
		er-save-disable:er_save_disable
		er-save-picker:er_save_picker
		er-seamless-bugfixes:er_seamless_bugfixes
		er-telemetry:er_telemetry
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
	# was produced -- and note a bare `rm` of the artifact does not force a relink, because
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

	# RE-attest what we just RELINKED. This step overwrites the very artifacts every `.me3`
	# profile's `[[natives]]` entries point at (they reference target/$target/release/*.dll
	# directly -- there is no separate staging copy), so leaving their `.provenance.json`
	# sidecars behind describing the previous bytes turns `er-dll-provenance.py verify` into a
	# liar in the safe direction: every DLL reads STALE/"ARTIFACT REPLACED" even though it was
	# just built from this exact tree. Measured 2026-08-31: sidecars said built=17:23 while the
	# binaries' PE stamps said 17:36, and all 26 shells verified stale purely because the gate
	# had relinked them. `er-run-branch.py` refuses to launch on that, so the gate was training
	# everyone to distrust or bypass the freshness check that exists to stop a stale-DLL run.
	# Writing provenance here is the same call `er-build-dlls.sh` makes after its own cargo
	# invocation, for the same reason: provenance is a claim about the tree that was compiled,
	# and only the builder knows that.
	prov_status=0
	for shell in "${me3_shells[@]}" "er-quickload:er_quickload"; do
		python3 "$repo_root/scripts/er-dll-provenance.py" write \
			--package "${shell%%:*}" \
			--artifact "$repo_root/target/$target/release/${shell##*:}.dll" >/dev/null || prov_status=1
	done
	if [[ "$prov_status" != "0" ]]; then
		echo "[check-rust-build] FAIL: could not record provenance for a relinked shell" >&2
		exit 1
	fi
	echo "[check-rust-build] recorded provenance for $(( ${#me3_shells[@]} + 1 )) artifacts"

	# LINT parity with ../fromsoftware-rs (2026-08-21). The parent project's entire
	# strictness is `RUSTFLAGS=-Dwarnings` around `cargo clippy --all-targets`, and the
	# user requires this workspace be at least as strict. The root `[workspace.lints]`
	# table carries the deny levels, but a `cargo build` only enforces the RUSTC half --
	# clippy's own lints exist only under `cargo clippy`, so without this step the clippy
	# half of parity is declared in the manifest and never actually run.
	#
	# NOTE the deliberate absence of `--no-deps`, which upstream passes. Upstream's
	# default-members covers its whole workspace so `--no-deps` still lints everything
	# there; here it would restrict linting to the shells and skip every library crate
	# they depend on (er-game-base, er-telemetry-core, er-title-flow, er-quit-menu-core, ...).
	# Registry dependencies are unaffected either way -- cargo applies `--cap-lints allow`
	# to them automatically -- so dropping the flag costs nothing and closes that hole.
	#
	# `scripts/check-lint-parity.py` asserts the configuration; this asserts the code.
	echo "[check-rust-build] cargo xwin clippy --all-targets (lint parity with ../fromsoftware-rs)"
	cargo xwin clippy --release "${shell_pkg_args[@]}" -p er-quickload --all-targets \
		--manifest-path "$repo_root/Cargo.toml" --target "$target"
	# Feature matrix. `er-quit-menu-core` takes `er-save-picker-core` with `default-features = false`
	# so a standalone quit-menu DLL links the OS-native fallback surface without the boot
	# missing-save flow. Cargo unifies features across a build graph, so the line above
	# only ever exercises the union of the two; this one proves the reduced build compiles
	# on its own and cannot rot.
	echo "[check-rust-build] cargo xwin check -p er-save-picker-core --no-default-features --target $target"
	cargo xwin check -p er-save-picker-core --no-default-features \
		--manifest-path "$repo_root/Cargo.toml" --target "$target"
fi

# Run them when a Windows-binary runner is available. The tests are pure logic (row math, config
# parsing, path mapping) with no game dependency, so wine executes them fine; skip cleanly where
# wine is absent rather than failing a gate on an optional tool. Running them on the real target
# matters: the first run of this step found a config-path assertion that only held under host
# `std::path` separator semantics, never under the windows target the crate actually ships to.
if command -v cargo-xwin >/dev/null 2>&1 && command -v wine >/dev/null 2>&1; then
	echo "[check-rust-build] cargo xwin test --lib --target $target (via wine)"
	# `-p` is explicit rather than relying on default-members (= er-quickload): the
	# ProfileSummary crate's windows-only tests moved out of er-quickload, and a bare
	# `--lib` would have quietly stopped running them.
	# The list is the gate. `-p` is explicit rather than relying on default-members
	# (= er-quickload), and every crate here is one whose tests cannot run on the host:
	# they live under `#[cfg(windows)]`, so a host `cargo test` compiles them out of
	# existence and then reports OK over the ones that remain. Measured 2026-08-31, before
	# the six additions below -- each pair is (what the host runs / what actually exists):
	#
	#   er-quit-menu-core        43 / 73   -- 30 windows-only tests, none of them ever run
	#   er-invasion-path         73 / 90   -- 17 behind a file-level `#![cfg(windows)]`
	#   er-better-refills         0 /  3   -- host build was broken outright until today
	#   er-build-import-runtime   0 /  2   -- whole lib.rs is `#![cfg(windows)]`
	#   er-invasion-warp-core   283 / 284
	#   er-loading-portrait-core 81 / 82
	#
	# er-quit-menu-core is the one that shows why crate-level bookkeeping is not enough:
	# check.sh has run it on the host for weeks, printing "ok. 43 passed", while 30 tests
	# next to them had never been built. scripts/check-test-target-coverage.py holds this
	# list complete from here.
	CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine WINEDEBUG="${WINEDEBUG:--all}" \
		cargo xwin test --lib \
		-p er-quickload -p er-profile-summary-core \
		-p er-quit-menu-core -p er-invasion-path -p er-invasion-warp-core \
		-p er-loading-portrait-core -p er-better-refills -p er-build-import-runtime \
		--manifest-path "$repo_root/Cargo.toml" --target "$target"

	# er-game-base's `pgd` module is `#[cfg(all(windows, feature = "game-types"))]`, and
	# `game-types` is not a default feature -- so its 2 tests are invisible to every command
	# in this repo: the host `cargo test -p er-game-base --lib` in check.sh compiles neither
	# the windows half nor the feature, and the windows run above uses default features. A
	# separate invocation because `--features` is not per-package when several `-p` are
	# selected. Measured: 62 tests without the feature, 64 with it.
	CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine WINEDEBUG="${WINEDEBUG:--all}" \
		cargo xwin test --lib -p er-game-base --features game-types \
		--manifest-path "$repo_root/Cargo.toml" --target "$target"
else
	echo "[check-rust-build] wine not found; skipping the windows-target unit-test RUN (compile check above still ran)" >&2
fi

echo "[check-rust-build] ok"
