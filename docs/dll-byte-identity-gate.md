# The refactor/move DLL byte-identity gate

`.github/workflows/refactor-byte-identical.yml` builds all 16 shipped cdylibs twice --
once from the merge-base, once from the PR head -- and fails when any of them differ.
It arms itself on PRs that present as a refactor or a move.

This document records what was measured before it was built, because the measurements
decide how to read a red run.

## What it is scoped to

| Signal | Arms the gate |
| --- | --- |
| Branch name contains `refactor` | yes |
| PR title contains `refactor` | yes |
| PR title contains `move` | yes |
| Anything else | no |

Matching is case-insensitive substring, implemented in `scripts/pr-refactor-scope.sh`
and tested by `scripts/test-pr-refactor-scope.sh`.

Both signals are author-chosen. Renaming the branch or retitling the PR removes the
gate. That is inherent to what it keys on -- a green run means *this* refactor moved no
bytes, never that no refactor reaches `main` unchecked.

Substring matching over-triggers, deliberately and measurably: across the last 400
commit subjects, 37 (9%) would arm it, and 7 of those 37 match only because `remove`
contains `move`. Those are deletions, which change the DLLs by definition and will fail.
The known over-triggers are pinned as test cases rather than left to be rediscovered.

## What "byte-identical" had to be defined as

Two clean builds of **identical source** are not byte-identical. Measured on
`crates/er-crash-logging-dll` (`scripts/probe-dll-build-determinism.sh`):

```
A vs B (identical source, two clean builds): 10 differing bytes
```

All ten sit in three fields, none derived from the code:

- the COFF header `TimeDateStamp` (wall-clock link time)
- `IMAGE_DEBUG_DIRECTORY.TimeDateStamp` (same)
- the RSDS GUID + Age in the CodeView debug record (LLD reseeds per link)

`scripts/check-dll-byte-identical.py` zeroes exactly those, plus the OptionalHeader
`CheckSum` (a function of the bytes just zeroed). Without that, the gate would be red on
an empty diff -- measuring the linker's clock, not the change. Nothing else is
normalized: no allowlist, no tolerance, no per-file exemption.

## The limit that matters: a pure move does not preserve the bytes

Moving `DllMain` verbatim into a submodule -- same code, same statics, no behaviour
change whatsoever:

```
A vs C (pure code move): 41,038 differing bytes in 10,977 runs -- 16.9% of the image
by section: .rdata 37786, .text 1321, .reloc 1218, .pdata 694, .data 9
```

Cause: rustc embeds the crate-relative source path **and line number** of every panic
site (`core::panic::Location`) and derives symbol hashes from module paths.
`er_effects_rs.dll` currently carries 56 distinct project source paths in `.rdata`, e.g.
`crates/er-effects-rs/src/experiments/startup_hooks/loading_cover/profile_table_gfx_files.rs`.
Move a file and the string changes; move a function inside a file and the line numbers
change; rename a module and the mangled hashes change. `--remap-path-prefix` does not
help -- the embedded paths are already crate-relative.

**So this gate passes for PRs that touch only comments, docs, tests, formatting, or
non-DLL crates, and fails for essentially every genuine code move.** A red run on a real
refactor is not a bug in the gate and is not fixable by changing the code.

### How much of that 17% is layout cascade

Almost all of it. Two one-word edits to the same string literal in the same crate:

| Change | Differing bytes | Share of image | Sections touched |
| --- | --- | --- | --- |
| `"standalone loaded"` -> `"standalone active"` (same length) | 479 | 0.2% | `.text` 356, `.rdata` 112, `.pdata` 11 |
| `"standalone loaded"` -> `"standalone attached"` (2 chars longer) | 41,264 | 17.0% | all of them |

A length change shifts every `.rdata` address after the literal, and the shift
cascades into code immediates, unwind data and relocations. So the size of a diff says
nothing about the size of the behaviour change: 0.2% and 17.0% here are the same edit,
differing only in whether the replacement word happened to be the same length.

Practical consequence for reading a failure: the per-section breakdown and the printable
context in the report are informative for a change that preserves layout, and near
useless once anything shifts a section. Do not read "17% of the image changed" as
"something big happened".

Two things did survive the pure move unchanged, and are the honest invariants if a
future gate wants one that a refactor can actually satisfy:

- the export table (`['DllMain']` before and after)
- `.text` size (159,542 bytes before and after; `.rdata` grew 16, `.reloc` shrank 4)

One data point on one small crate, so neither is proven general.

## Cost

Two serialized release builds of 16 cdylibs per armed PR, sharing one `target/` and the
main-scoped `rust-cache`.

## Maintenance surface

- `scripts/pr-refactor-scope.sh` -- trigger, tested by `scripts/test-pr-refactor-scope.sh`
- `scripts/check-dll-byte-identical.py` -- PE normalizer + comparator, tested by
  `scripts/test-dll-byte-identical.py` (asserts link noise is ignored **and** that a
  single flipped `.text` byte still fails)
- `scripts/me3-dll-list.py` -- the compared DLL set, reusing the parser already owned by
  `scripts/check-me3-shell-coverage.py` so the array has one parser, not two
- `scripts/probe-dll-build-determinism.sh` -- reproduces every number above

Both halves of the gate run in `scripts/check.sh` and in the `check` workflow, because a
gate that only executes on the rare armed PR is a gate that rots between firings.

## If you want equivalence proof instead of byte proof

Behavioural equivalence for a refactor is what `AGENTS.md` already requires: a live
runtime smoke. No amount of CI hashing substitutes for it.
