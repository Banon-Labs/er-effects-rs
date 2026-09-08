//! Bakes the commit this build came from into `er-game-base`, for `build_id`.
//!
//! # Why the git sha and not a version string
//!
//! Every crate in this workspace is `version = "0.1.0"`, so the manifest version identifies
//! nothing. The question a log has to answer is "which source produced the binary that wrote
//! this line", and the only honest answer is a commit id plus whether the tree was clean.
//!
//! # Why `dirty` is load-bearing
//!
//! A sha alone invites a false certainty: `git show <sha>` looks authoritative, but if the
//! build came from a tree with uncommitted edits, that diff is not what ran. `+dirty` is the
//! flag that stops someone reading the wrong source with total confidence.
//!
//! # The staleness this cannot fully close
//!
//! `rerun-if-changed` on `.git/HEAD`, the ref it names, and `.git/index` catches commits,
//! checkouts and `git add`. It does not catch a bare edit to a tracked file that is never
//! staged -- cargo has no reason to rebuild this crate for that, so the `+dirty` suffix can
//! be absent from a build whose tree had edits. That is why the runtime line pairs this with the DLL's
//! own PE timestamp, which is written by the linker on every relink and cannot go stale.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// How much of the sha to keep. Twelve hex digits is unambiguous in a repo of this size and
/// still fits a log line beside everything else.
const SHA_LENGTH: &str = "--short=12";

/// Stand-in when the tree is not a git checkout (a source archive, a vendored copy). Spelled
/// out rather than left empty so a log line reads as "we looked and there was nothing" instead
/// of looking like a formatting bug.
const UNKNOWN: &str = "unknown";

fn main() {
    let repo = repo_root();
    emit_address_map(&std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets this"));
    declare_rerun(&repo);

    let sha = git(&repo, &["rev-parse", SHA_LENGTH, "HEAD"]).unwrap_or_else(|| UNKNOWN.to_string());
    // `--untracked-files=no`: a stray scratch file in the working directory says nothing about
    // whether the code that built is the code at the sha, and treating it as dirty would make
    // the flag fire so often it stops being read.
    let dirty = git(&repo, &["status", "--porcelain", "--untracked-files=no"])
        .map(|out| !out.trim().is_empty())
        .unwrap_or(false);

    // One pre-composed string, not a sha plus a flag the runtime re-joins.
    //
    // The first cut emitted them separately and let `build_id` append "+dirty" behind an
    // `if GIT_DIRTY == "true"`. Both sides of that comparison are compile-time constants, so
    // the optimiser resolves the branch and drops the untaken literal -- and the failure is
    // silent and one-directional: a build that folded the wrong way ships a sha with no
    // warning attached, which reads as a clean build. Composing here means the binary either
    // contains the right string or contains nothing, with no branch in between to get this
    // wrong.
    let description = if dirty { format!("{sha}+dirty") } else { sha };
    println!("cargo::rustc-env=ER_BUILD_GIT={description}");
}

/// Workspace root: this crate lives at `<root>/crates/er-game-base`.
fn repo_root() -> PathBuf {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets this"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map(PathBuf::from)
        .unwrap_or(manifest)
}

/// Rebuild when the commit, the checked-out branch, or the index moves.
///
/// `.git/HEAD` covers a checkout; the ref file it names covers a commit on that branch;
/// `.git/index` covers a `git add`, which is what flips `DIRTY` back to clean.
fn declare_rerun(repo: &Path) {
    let git_dir = repo.join(".git");
    for relative in ["HEAD", "index"] {
        let path = git_dir.join(relative);
        if path.exists() {
            println!("cargo::rerun-if-changed={}", path.display());
        }
    }
    // `HEAD` normally holds `ref: refs/heads/<branch>`; a detached head holds a raw sha and
    // there is no second file to watch.
    if let Ok(head) = std::fs::read_to_string(git_dir.join("HEAD"))
        && let Some(reference) = head.trim().strip_prefix("ref: ")
    {
        let path = git_dir.join(reference);
        if path.exists() {
            println!("cargo::rerun-if-changed={}", path.display());
        }
    }
}

/// Run git in the repo, returning trimmed stdout only on a clean exit. Every failure mode --
/// git absent, not a checkout, a broken repository -- collapses to `None` so a build never
/// fails over provenance metadata.
fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
}

/// Where the verified 1.16.2 -> 1.17 address pairs come from. Tracked, generated by
/// `scripts/verify-rva-map-1170.py`, and read here rather than transcribed: a hand-copied address
/// table is the exact failure this whole migration is about.
const VERIFIED_MAP: &str = "../../docs/recon/rva-map-1162-to-1170.verified.tsv";
/// How much of a body an `IDENTICAL` verdict must cover before it counts. `NEAR`, `DIVERGES` and
/// `IDENTICAL-SHORT` rows are left out on purpose: an address that merely looks similar is how a
/// detour lands mid-function.
///
/// # What it is a stand-in for
///
/// `IDENTICAL` is a claim about a prefix. The verifier's decode stopped for a reason unrelated to
/// where the function ends -- its instruction limit, or a `ret` in a body that runs on past it --
/// so nothing is known about the instruction after the last one compared, and this floor is a
/// proxy for "enough of it was seen to be worth something". It is a poor proxy in both
/// directions, and both failures were measured on 2026-08-30:
///
/// * it discards a function shorter than the floor even when the comparison was complete. Five
///   leaves of 3 to 13 instructions verified at ratio 1.000 over their entire bodies and were
///   dropped anyway, taking the Seamless null-container guard (`0x4f9940`) with them;
/// * it waves through a prefix of a long function. `STEP_MoveMap` matched over 120 instructions
///   of 975, cleared this floor comfortably, and was promoted to detour-safe -- while the two
///   instructions 1.17 inserted sat at index 873.
///
/// So it does not apply to a verdict in [`EXHAUSTIVE_VERDICTS`], where the comparison covered
/// every instruction of both bodies and found them the same length. That is not a longer prefix,
/// it is a different claim, and there is no next instruction for a floor to insure against.
const MIN_VERIFIED_INSNS: u32 = 12;

/// Verdicts that compared the whole of both functions, and so are exempt from
/// [`MIN_VERIFIED_INSNS`]. The list is duplicated as `EXHAUSTIVE_VERDICTS` in
/// `scripts/verify-rva-map-1170.py`, which writes these strings; the two drifting apart is how a
/// rescued row would quietly go back to being discarded, so change them together.
///
/// * `BYTE-IDENTICAL` -- both `.pdata` extents, byte for byte, unchanged.
/// * `IDENTICAL-WHOLE` -- both `.pdata` extents, normalised instruction streams equal over all of
///   them, and the two extents the same length.
/// * `IDENTICAL-LEAF` -- the same, for a function neither image declares in `.pdata` at all. The
///   x64 ABI omits unwind data for a function that allocates no stack and calls nothing, so
///   ELDEN RING's small getters simply have no entry and no extent can be read; the verifier
///   decodes the end instead, and three facts back that decode -- it stops on a real terminator
///   past every forward branch target, the two images decoded separately arrive at the same byte
///   length, and the streams agree over all of it.
///
///   `IDENTICAL-LEAF` is also the one verdict that carries its own detour licence. A leaf has no
///   `.pdata` entry, so it reaches [`DETOURABLE_ENTRY_EVIDENCE`] through the `NEITHER-ENTRY`
///   clause, which was written for a different situation and makes no claim about the entry. The
///   verifier therefore checks directly what MinHook actually needs, in both images, and
///   withholds the verdict when either half fails. That is the missing claim restored, not the
///   gate widened. Two halves:
///
///   * no branch inside the body targets the five bytes the patch overwrites
///     (`branch_into_prologue`);
///   * the body is at least those five bytes long (`leaf_fits_patch`). Added 2026-08-30 after
///     the first regeneration that admitted leaves put two three-byte bodies in
///     `DETOUR_SAFE_1162_TO_1170` -- `0x7add70` and `0x1c92f30`, the latter being
///     `xor eax,eax; ret`. `scripts/audit-1170-hook-targets.py` refuses both patch-unsafe with
///     the bytes to show for it: there is nowhere to put the jump. They stay CALLABLE.
const EXHAUSTIVE_VERDICTS: [&str; 3] = ["BYTE-IDENTICAL", "IDENTICAL-WHOLE", "IDENTICAL-LEAF"];

/// Verdicts where the two bodies are not the same, and the detour is licensed anyway because the
/// difference is nowhere near the five bytes MinHook writes.
///
/// Deliberately its own list rather than a fourth [`EXHAUSTIVE_VERDICTS`] entry: those three all
/// assert the normalised instruction streams are equal, and this one asserts they are not. Reading
/// a row as "the bodies match" when the verdict says "the bodies differ, elsewhere" is precisely
/// the confusion that would let this widen the gate by accident.
///
/// * `PATCH-SITE-IDENTICAL` -- both images' `.pdata` declare a function starting at the two
///   addresses; the comparison covered both bodies in full; MinHook's own trampoline walk (ported
///   from `vendor/minhook/src/trampoline.c`, run against both images) builds a trampoline at each
///   and consumes the same instructions doing it; nothing in either body branches into the bytes
///   the patch overwrites; and every instruction the two bodies disagree about lies strictly after
///   the last instruction that walk relocates -- so the instruction the trampoline returns into is
///   inside the equal prefix. `MAX_DRIFT_HUNKS`/`MAX_DRIFT_INSNS` in the verifier additionally cap
///   the difference at a localised edit, and any `replace` hunk refuses it outright.
///
/// Why this is not a prologue RE-check. A bare "the first bytes still match" test is what the
/// impostor at 1.16.2 `0x140aec480` would have passed: `IDENTICAL 1.000` over 56 instructions, and
/// `+0x360` inside a completely different function. That address is not a `.pdata` start in either
/// image, so it never reaches the byte comparison at all -- and a pair whose entry region did
/// change is refused by the position test however small the total difference is. The verdict is a
/// claim about the patch site and the relocated window, not about total body length.
///
/// Measured, 2026-08-30. Across all 128,602 pairs in `rva-map-1162-to-1170.functions.tsv` this
/// verdict admits 23 -- 0.018% -- and the only one in either ledger is `MOVEMAPSTEP_STEP_MOVEMAP`
/// (`0x140af7cf0 -> 0x140af9000`), whose `.pdata` extent grew by 8 bytes because 1.17 inserted
/// `mov rcx,rbx; call _UpdateHorseType` at instruction 873 of 975, 0x1055 bytes past a prologue
/// whose first 8 bytes are `48 8b c4 55 56 57 41 54` in both builds. Without it the autoload
/// gate-hold has no `STEP_MoveMap` detour and the feature is inert on 1.17.
const PATCH_SITE_VERDICTS: [&str; 1] = ["PATCH-SITE-IDENTICAL"];

/// Every `PATCH-SITE-IDENTICAL` row in either ledger, pinned to the difference a human read.
///
/// # The hole this closes
///
/// [`PATCH_SITE_VERDICTS`] is the one verdict that admits a pair whose bodies differ to
/// `DETOUR_SAFE_1162_TO_1170`, and it decides that on machine caps alone: no `replace` hunk, at
/// most `MAX_DRIFT_HUNKS` places, at most `MAX_DRIFT_INSNS` instructions, all of it after the
/// window MinHook relocates. Those caps are about the patch site. They say nothing about what the
/// inserted code does, and our detours are not only five bytes at an entry -- they read and write
/// the object the function is stepping. `STEP_MoveMap`'s after-original detour clears the advance
/// gate at `MoveMapStep+0x4b8`; the insertion 1.17 made lands two instructions before the native
/// read of that same field. It was benign, and nothing in the pipeline had to look to find that
/// out, which is the actual defect: a second insertion would be admitted just as quietly.
///
/// So a row may carry this verdict only if it is named here. A new one fails the build until
/// somebody reads the two bodies and writes down what changed.
///
/// # What is machine-checked and what is not
///
/// CHECKED: the pair still exists, still carries this verdict, still names this constant, and its
/// `.pdata` extents are still the two lengths recorded. That is enough to catch the next
/// insertion, because clause 4 of `patch_site_drift` refuses any `replace` hunk -- a body that
/// reaches this verdict differs by pure insertion and deletion, which moves the extent.
///
/// Not checked HERE: the `note`. It is prose, and build.rs has no image to compare it against
/// (`eldenring-deobf*.bin` are untracked, 94 MB, and no build may depend on them). Reproduce it
/// with `python3 scripts/diff-function-bodies-1162-1170.py <old> <new>`.
const PATCH_SITE_ACKNOWLEDGED: [(&str, &str, &str, &str, &str); 1] = [(
    "0x140af7cf0",
    "0x140af9000",
    "MOVEMAPSTEP_STEP_MOVEMAP_RVA",
    "PDATA:0x120b/0x1213+8",
    "1.17 inserts `mov rcx,rbx; call 0x140b02ef0` at instruction 873 of 975, 0x1055 bytes past \
     the prologue and 2 instructions before the native read of `MoveMapStep+0x4b8`. The callee is \
     `CS::MoveMapStep::_UpdateHorseType`, new in 1.17 -- its prologue signature has 0 hits in \
     eldenring-deobf.bin and 1 in eldenring-deobf-1.17.bin, and the whole-image pairing's local \
     delta steps 0x1490 -> 0x16a0 across it, which is its 514 bytes plus alignment. It re-applies \
     the mount type after a map move (event flags 0x1a2c..0x1a2f -> RemoveChrIns + \
     CreatePlayersHorse + SetHP), is idempotent behind PlayerGameData+0x960, and never \
     dereferences the MoveMapStep: `rcx` is a dead store, clobbered before any read in both the \
     callee and the first thing the callee calls, and `rbx` is reloaded from GameDataMan. So the \
     fields our detour holds (+0x100, +0x270, +0x4b8, +0x4c, +0x50) are untouched by it.",
)];

/// Verdicts admitted to [`VERIFIED_1162_TO_1170`] -- Call and read -- and to nothing else.
///
/// A third list rather than a longer second one, and the separation is the whole feature.
/// [`EXHAUSTIVE_VERDICTS`] and [`PATCH_SITE_VERDICTS`] are both read by `detourable_pairs`, which
/// is the only function `emit_address_map` builds the detour table from; this list is read by
/// `callable_only_pairs`, which the detour table never calls. The two paths do not merely apply
/// different rules to the same rows -- they are different functions, and the one that feeds
/// `DETOUR_SAFE_1162_TO_1170` does not mention this constant at all. `emit_address_map` then
/// asserts the result, so the claim is measured on every build rather than argued here.
///
/// * `IDENTICAL-LEAF-NOPATCH` -- everything `IDENTICAL-LEAF` claims, minus the hook. Neither
///   image declares the address in `.pdata`, so both extents were DECODED; the two decodes agreed
///   on the byte length; the normalised streams are equal over all of both bodies; nothing
///   branches into the bytes a patch would overwrite -- and the body is shorter than the five
///   bytes MinHook writes, with the ported `CreateTrampolineFunction` refusing the site in both
///   images rather than a length test standing in for it.
///
/// Why it had to exist. Until 2026-08-30 the call map was seeded from `detourable_pairs`, so
/// "too short to hook" and "unsafe to call or compare" were one decision, and a three-byte
/// function proved byte-for-byte identical in both builds was thrown out of both tables. Two rows
/// were paying for it, each with a live consumer that never wanted a hook:
///
/// * `0x7add70 -> 0x7aebf0` -- `CS::MenuItem`'s constant-false accept predicate, `xor eax,eax;
///   ret`. er-quickload compares a menu row's `+0xf8` against it to tell a disabled Continue from
///   an enabled one; unmapped, the comparison is against 0 and every row reads disabled. It is a
///   `.pdata`-less leaf that nothing calls -- its address is only ever taken -- so the whole-image
///   function-table alignment cannot see it and the caller-vote tools could not either until they
///   learned to count `lea`s. The evidence is a unanimous 1-of-1: each image holds exactly one
///   reference to its address, both `48 8d 05 44 0d 00 00` at byte +0xa5 of `0x7acf80 ->
///   0x7ade00`, itself an `identical-whole` pair over 151 instructions.
/// * `0x1c92f30 -> 0x1c94d30` -- `CTRL_SUBOBJECT_RELEASE_RVA`, body `ret 0`. er-invasion-path
///   calls it while tearing a spawned effect down, and its `let (Some(..), ..) = (..) else
///   { return; }` abandons the entire teardown when one address will not resolve, so an unmapped
///   three-byte stub leaks every effect the crate spawns. 185 of 189 caller votes carry it, the
///   three runners-up at 2/1/1 votes and unrelated deltas.
const CALLABLE_ONLY_VERDICTS: [&str; 1] = ["IDENTICAL-LEAF-NOPATCH"];
/// Mappings held back despite verifying, because the handler at that address is what turned out
/// to be stale. See the file's own header for the distinction it exists to record.
const QUARANTINE: &str = "../../docs/recon/rva-1170-quarantine.tsv";
/// The second, weaker source of pairs, selected by `scripts/select-needed-1170-rows.py` from a
/// whole-image alignment of both `.pdata` function tables.
///
/// # Why a weaker source is worth having
///
/// The verified map covers 27 addresses, all of them hook targets, because each row costs a
/// byte-by-byte instruction comparison. The calls are the larger exposure: 159 sites reach the
/// game through a raw `transmute(base + *_RVA)`, and an unmapped one is not a degraded feature
/// but a dead process -- `0x1405eefb0` on 1.17 is the second byte of a five-byte `call`, and the
/// `9a` it lands on is a far call, invalid in long mode.
///
/// # What it is allowed to decide
///
/// These pairs come from masked-signature identity, calibrated against 41 pairs derived without
/// it (62 of the bundle's 96 fields come from RTTI class names, which involve no pattern matching
/// at all) with zero disagreements. That is strong enough to call. It is not the same claim as
/// the verified map, so the verified map wins wherever both cover an address, and a detour still
/// owes `audit-1170-hook-targets.py` a check that the destination has room for the patch.
const FUNCTION_MAP: &str = "../../docs/recon/rva-map-1162-to-1170.needed.tsv";
/// The third source: globals, vtables and tables, which the function map structurally cannot
/// cover because a datum has no content to compare -- at rest a global is eight zero bytes like
/// every other global.
///
/// They moved too, and not by a constant: most `.data` globals shifted +0x4070 between 2.6.2.0
/// and 2.7.0.0, `runtime_heap_allocator` +0x4080, `multiplay_properties` +0x4000, and
/// `cs_system_step` +0x4080 while its neighbours went +0x4070. Reading a stale one is quiet and
/// then fatal: `GLOBAL_TEX_REPOSITORY_RVA` went unread-and-unnoticed into `CreateTpfResCap`,
/// which divided by zero and took the game down 894ms after load -- with a correctly translated
/// function address one frame up, which is what made it look like the translation's fault.
///
/// Each row is carried by the code that references it, and only rows where independent
/// references agree are used; see the file's own header.
const DATA_MAP: &str = "../../docs/recon/rva-map-1162-to-1170.data.tsv";
/// The needed map, put through the same byte comparison the verified map gets.
///
/// This is what makes a detour promotable. `FUNCTION_MAP` says where a signature re-occurs, which
/// is enough to call; this file says the normalised instruction sequences agree over the body and
/// what each image's own `.pdata` declares about the two endpoints. Both claims are required
/// before a row reaches `DETOUR_SAFE_1162_TO_1170`.
///
/// Measured, and the reason the byte comparison is not optional: of the five detours
/// er-armament-icons installed when the game died at the first overlay draw on 2026-08-29, four
/// are the same function in 1.17 and one -- `HUD_WEAPON_SLOT_UPDATE`, 1.16.2 `0x8d2110` -- was
/// paired by signature with a function sharing 18% of its instruction shape. The entry-and-
/// prologue audit passed it. This comparison rejects it, and `scripts/verify-rva-map-1170.py
/// --selftest` asserts that separation so it cannot quietly regress.
const NEEDED_VERIFIED_MAP: &str = "../../docs/recon/rva-map-1162-to-1170.needed-verified.tsv";

/// Entry evidence a row must carry before it may hold a detour, from the verdict table's last
/// column.
///
/// `BOTH-ENTRIES` is the ordinary case: both images' `.pdata` declare a function starting there.
/// `NEITHER-ENTRY` is accepted too, and deliberately: `SCALEFORM_HANDLER_CTOR_RVA` (0x11a8870)
/// sits 0x20 bytes before the entry Ghidra names, in both builds, and that symmetry is itself
/// evidence the pair is aligned. What is refused is an asymmetric verdict -- a source that is an
/// entry paired with a destination that is not, or the reverse -- because that is precisely the
/// shape of a mapping that landed mid-function.
///
/// The hole in `NEITHER-ENTRY`, and why `IDENTICAL-LEAF` does not fall through it. The clause was
/// written for a deliberate fixed offset into a known function, and it makes no claim about the
/// bytes MinHook is about to overwrite; it is safe there because the offset is identical in both
/// builds and a human chose it. A leaf also lands on `NEITHER-ENTRY`, for the unrelated reason
/// that the x64 ABI emits no unwind data for it -- so admitting leaves on this clause alone would
/// widen the gate by accident, on a technicality rather than on evidence. It does not happen:
/// `verify-rva-map-1170.py` issues `IDENTICAL-LEAF` only after checking, in both images, that no
/// branch inside the body targets the patched bytes, which is the claim a `.pdata` entry stands
/// in for. A leaf that fails that check falls back to `IDENTICAL`/`IDENTICAL-SHORT` and is held
/// to the floor like any other prefix.
const DETOURABLE_ENTRY_EVIDENCE: [&str; 2] = ["BOTH-ENTRIES", "NEITHER-ENTRY"];

/// 1.16.2 RVAs a verdict table says are paired with the wrong 1.17 function.
///
/// This is not the same as an unverified row. The whole-image signature map carries hundreds of
/// pairs nobody has compared, and they stay -- an unexamined pair is the ordinary case and the
/// calls need the coverage. A `DIVERGES` verdict is different: the comparison ran and disagreed,
/// so the row is positive evidence of a wrong address, and a wrong address reached as a call is
/// how `0x1405eefb0` took a boot down. Those are dropped from the call map as well as the detour
/// one.
fn refuted_sources(path: &Path) -> Vec<u32> {
    println!("cargo:rerun-if-changed={}", path.display());
    let mut out = Vec::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 3 || fields[2] != "DIVERGES" {
            continue;
        }
        if let Ok(va) = u64::from_str_radix(fields[0].trim().trim_start_matches("0x"), 16) {
            out.push((va - 0x140000000) as u32);
        }
    }
    out
}

/// Pairs from a `verify-rva-map-1170.py` verdict table that are good enough to detour.
fn detourable_pairs(path: &Path) -> Vec<(u32, u32)> {
    println!("cargo:rerun-if-changed={}", path.display());
    let mut rows = Vec::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return rows;
    };
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        // Seven columns: the entry evidence is the seventh, and a table written before it existed
        // is refused rather than assumed safe.
        if fields.len() < 7 {
            continue;
        }
        match fields[2] {
            // The whole of both functions was compared. There is no minimum to impose: the
            // comparison covered everything there was, and a 21-byte function proved that way is
            // proved harder than a 4-KB one sampled over its first 120 instructions.
            verdict if EXHAUSTIVE_VERDICTS.contains(&verdict) => {}
            // The bodies differ and the patch site does not. Like the exhaustive verdicts this
            // takes no instruction floor -- and for a stronger reason: the floor is a proxy for
            // "was enough of the body seen", and this verdict is issued only after all of both
            // bodies were seen and the difference was located relative to the bytes MinHook
            // writes. A count of compared instructions has nothing left to add.
            verdict if PATCH_SITE_VERDICTS.contains(&verdict) => {}
            // Normalised instruction sequences agree over a prefix of unknown remainder. How much
            // of the body they agree over is the whole question, hence the floor.
            "IDENTICAL" => {
                if fields[4].trim().parse::<u32>().unwrap_or(0) < MIN_VERIFIED_INSNS {
                    continue;
                }
            }
            _ => continue,
        }
        if !DETOURABLE_ENTRY_EVIDENCE.contains(&fields[6].trim()) {
            continue;
        }
        let parse = |s: &str| u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok();
        if let (Some(old), Some(new)) = (parse(fields[0]), parse(fields[1])) {
            // Stored as RVAs: the table has to survive a relocated image base.
            rows.push(((old - 0x140000000) as u32, (new - 0x140000000) as u32));
        }
    }
    rows
}

/// Pairs a verdict table admits to the call map and to nothing else.
///
/// A separate function, not a flag on [`detourable_pairs`], and that is the point rather than a
/// style choice. The detour table is built only from `detourable_pairs`, whose match arms do not
/// mention [`CALLABLE_ONLY_VERDICTS`] -- so a row admitted here cannot reach a detour by any
/// route through this file, and the separation survives someone editing either rule without
/// reading the other. `emit_address_map` additionally asserts it on the finished tables.
///
/// The instruction floor has nothing to do here for the same reason it has nothing to do for an
/// exhaustive verdict: it is a proxy for "was enough of the body seen", and the verifier issues
/// `IDENTICAL-LEAF-NOPATCH` only after seeing all of both.
///
/// Entry evidence is still required, from the same list. Its name says `DETOURABLE` and the check
/// is worth making anyway: what that list refuses is an asymmetric verdict -- a source the linker
/// declares a function start paired with a destination it does not, or the reverse -- which is
/// the shape of a pair that landed mid-function, and reading four bytes out of the middle of the
/// wrong 1.17 function is not made safe by never hooking it.
fn callable_only_pairs(path: &Path) -> Vec<(u32, u32)> {
    println!("cargo:rerun-if-changed={}", path.display());
    let mut rows = Vec::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return rows;
    };
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 7 {
            continue;
        }
        if !CALLABLE_ONLY_VERDICTS.contains(&fields[2]) {
            continue;
        }
        if !DETOURABLE_ENTRY_EVIDENCE.contains(&fields[6].trim()) {
            continue;
        }
        let parse = |s: &str| u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok();
        if let (Some(old), Some(new)) = (parse(fields[0]), parse(fields[1])) {
            rows.push(((old - 0x140000000) as u32, (new - 0x140000000) as u32));
        }
    }
    rows
}

/// 1.16.2 RVAs excluded from translation.
fn quarantined(root_dir: &str) -> Vec<u32> {
    let path = Path::new(root_dir).join(QUARANTINE);
    println!("cargo:rerun-if-changed={}", path.display());
    let mut out = Vec::new();
    if let Ok(text) = std::fs::read_to_string(&path) {
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            if let Some(rva) = line.split('\t').next()
                && let Ok(value) = u32::from_str_radix(rva.trim().trim_start_matches("0x"), 16)
            {
                out.push(value);
            }
        }
    }
    out
}

/// Refuse to build while a `PATCH-SITE-IDENTICAL` row is not pinned in [`PATCH_SITE_ACKNOWLEDGED`].
///
/// Both directions, and the second is the one that rots quietly: an unpinned row means a body
/// changed under a detour and nobody read it, and a pin with no row means the evidence the pin
/// describes is gone while the pin still reads as current.
///
/// Skipped entirely when neither ledger can be read -- a source archive without `docs/recon` is
/// the same situation `detourable_pairs` already returns empty for, and turning that into a build
/// failure would be a different rule than this one.
fn assert_patch_sites_acknowledged(root_dir: &str) {
    let mut readable = false;
    let mut found: Vec<(String, String, String, String)> = Vec::new();
    for ledger in [VERIFIED_MAP, NEEDED_VERIFIED_MAP] {
        let path = Path::new(root_dir).join(ledger);
        println!("cargo:rerun-if-changed={}", path.display());
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        readable = true;
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 3 || !PATCH_SITE_VERDICTS.contains(&fields[2].trim()) {
                continue;
            }
            // The extent column is the machine-checked half of the pin, so a row that carries
            // this verdict without one cannot be checked at all -- say that instead of skipping.
            assert!(
                fields.len() >= 8,
                "{ledger} row `{line}` carries {} with only {} column(s); the extent column is \
                 what pins the difference, so this row cannot be acknowledged",
                fields[2].trim(),
                fields.len()
            );
            found.push((
                fields[0].trim().to_ascii_lowercase(),
                fields[1].trim().to_ascii_lowercase(),
                fields[5].trim().to_string(),
                fields[7].trim().to_string(),
            ));
        }
    }
    if !readable {
        return;
    }
    for (old, new, name, extent) in &found {
        let pin = PATCH_SITE_ACKNOWLEDGED
            .iter()
            .find(|(o, n, ..)| o.to_ascii_lowercase() == *old && n.to_ascii_lowercase() == *new);
        let Some((_, _, pinned_name, pinned_extent, _)) = pin else {
            panic!(
                "{old} -> {new} ({name}) carries PATCH-SITE-IDENTICAL, which admits a pair whose \
                 BODIES DIFFER to the detour table, and it is not in PATCH_SITE_ACKNOWLEDGED. \
                 Read both bodies -- `python3 scripts/diff-function-bodies-1162-1170.py {old} \
                 {new}` -- work out what the difference does to whatever our detour reads and \
                 writes on that object, then pin it. Do not add the pin first"
            )
        };
        assert!(
            pinned_name == name && pinned_extent == extent,
            "{old} -> {new} is pinned in PATCH_SITE_ACKNOWLEDGED as ({pinned_name}, \
             {pinned_extent}) and the ledger now says ({name}, {extent}). A changed extent is a \
             changed body: this verdict refuses every `replace` hunk, so a pair that still holds \
             it differs by pure insertion and deletion and cannot move its extent without new \
             code. Re-read both bodies before touching this pin"
        );
    }
    for (old, new, name, ..) in PATCH_SITE_ACKNOWLEDGED {
        assert!(
            found
                .iter()
                .any(|(o, n, ..)| o == &old.to_ascii_lowercase() && n == &new.to_ascii_lowercase()),
            "PATCH_SITE_ACKNOWLEDGED pins {old} -> {new} ({name}), and no ledger row carries \
             PATCH-SITE-IDENTICAL for it any more. The pin now describes evidence that is gone; \
             delete it, or find out why the row lost the verdict"
        );
    }
}

/// Emit the translation table `er-hook` consults when the running build is not 1.16.2.
fn emit_address_map(root_dir: &str) {
    assert_patch_sites_acknowledged(root_dir);
    let verified_path = Path::new(root_dir).join(VERIFIED_MAP);
    let verified_detourable: Vec<(u32, u32)> = detourable_pairs(&verified_path);
    // The call map is seeded from the detourable rows plus the callable-only ones. Those were the
    // same set until 2026-08-30, and that is what made "too short to hook" mean "unsafe to call":
    // a three-byte function proved byte-for-byte identical in both builds was refused a detour,
    // correctly, and thereby refused a comparison too. See [`CALLABLE_ONLY_VERDICTS`].
    let callable_only: Vec<(u32, u32)> = callable_only_pairs(&verified_path);
    let mut rows: Vec<(u32, u32)> = verified_detourable.clone();
    rows.extend(callable_only.iter().copied());
    // Both verdict tables feed the detour set. They differ only in which candidate map they were
    // run over -- the byte-search one and the whole-image one -- and a row from either has passed
    // the same two tests, so there is no reason to trust one and not the other.
    //
    // Built from `detourable_pairs` alone, on purpose: `rows` is no longer the same set, so
    // cloning it here would hand every callable-only row a detour licence in one line.
    let mut detour_safe: Vec<(u32, u32)> = verified_detourable;
    detour_safe.extend(detourable_pairs(
        &Path::new(root_dir).join(NEEDED_VERIFIED_MAP),
    ));
    let verified: Vec<u32> = rows.iter().map(|(old, _)| *old).collect();
    let function_map = Path::new(root_dir).join(FUNCTION_MAP);
    println!("cargo:rerun-if-changed={}", function_map.display());
    if let Ok(text) = std::fs::read_to_string(&function_map) {
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 2 {
                continue;
            }
            let parse = |s: &str| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok();
            if let (Some(old), Some(new)) = (parse(fields[0]), parse(fields[1]))
                && !verified.contains(&old)
            {
                rows.push((old, new));
            }
        }
    }
    let data_map = Path::new(root_dir).join(DATA_MAP);
    println!("cargo:rerun-if-changed={}", data_map.display());
    if let Ok(text) = std::fs::read_to_string(&data_map) {
        let known: Vec<u32> = rows.iter().map(|(old, _)| *old).collect();
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 2 {
                continue;
            }
            let parse = |s: &str| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok();
            if let (Some(old), Some(new)) = (parse(fields[0]), parse(fields[1]))
                && !known.contains(&old)
            {
                rows.push((old, new));
            }
        }
    }
    let mut held_back = quarantined(root_dir);
    // A comparison that ran and disagreed is evidence against the pair, wherever it would be used.
    held_back.extend(refuted_sources(
        &Path::new(root_dir).join(NEEDED_VERIFIED_MAP),
    ));
    held_back.extend(refuted_sources(&Path::new(root_dir).join(VERIFIED_MAP)));
    rows.retain(|(old, _)| !held_back.contains(old));
    rows.sort_unstable();
    rows.dedup_by_key(|(old, _)| *old);
    // A fifth ledger used to be declared here and parked under `let _ = AUDITED_DETOURS;`.
    //
    // `docs/recon/rva-1170-detour-audited.tsv` was deleted on 2026-08-31, with the reason recorded
    // rather than the file: 89 of the 448 rows it promoted rested on its "unwindless leaf" clause,
    // and all 89 destinations are in non-EXECUTABLE sections of the 1.17 image. `.pdata` declares
    // no function containing a `.data` global for the same reason it declares none containing a
    // leaf, so the clause could not tell the two apart and never once fired on real code. Four of
    // those rows are 24 bytes of zeros in both images, described as `6B relocatable`.
    //
    // Nothing read it -- this `let _` was the whole of its wiring -- so it had no consequence and
    // no way to be caught being wrong. `scripts/check-ledger-section-kind.py` now enforces what it
    // violated (a ledger whose rows license a hook may only name executable memory) and refuses
    // the file's return, since `audit-1170-hook-targets.py --promote` can still write it.
    //
    // What replaced it as evidence answers both questions better. Semantic identity comes from the
    // byte comparison in NEEDED_VERIFIED_MAP; entry evidence comes from that same file's last
    // column, read out of each image's `.pdata` rather than counted from forward references.
    let mut detour_rows = detour_safe;
    detour_rows.retain(|(old, _)| !held_back.contains(old));
    detour_rows.sort_unstable();
    detour_rows.dedup_by_key(|(old, _)| *old);

    // The separation, measured on the finished tables rather than argued from the code above.
    // `CALLABLE_ONLY_VERDICTS` is disjoint from the two detour lists and `detourable_pairs` never
    // reads it, so this cannot fire from a verdict-list edit alone -- what it catches is the other
    // way in: the same 1.16.2 address admitted callable-only by one ledger and detourable by
    // another. That is not a policy question to resolve at build time; two verdict tables
    // disagreeing about whether MinHook can install at an address is a contradiction, and the
    // build stops rather than picking the permissive half.
    for verdict in CALLABLE_ONLY_VERDICTS {
        assert!(
            !EXHAUSTIVE_VERDICTS.contains(&verdict) && !PATCH_SITE_VERDICTS.contains(&verdict),
            "{verdict} is in CALLABLE_ONLY_VERDICTS and in a detour list; it would license the \
             hook it exists to refuse"
        );
    }
    for (old, new) in &callable_only {
        assert!(
            !detour_rows.contains(&(*old, *new)),
            "{old:#x} -> {new:#x} is admitted CALL-only by {VERIFIED_MAP} and detourable by \
             another ledger. One of the two verdicts is wrong about whether MinHook can write \
             five bytes there; resolve the ledgers, do not let the build choose"
        );
    }

    let mut out = String::from(
        "// GENERATED by er-game-base/build.rs. Do not edit by hand.\n\
         //\n\
         // TWO tables, because being the right address is not the same claim as being a safe\n\
         // place to write five bytes.\n\
         //\n\
         // VERIFIED_1162_TO_1170 is every pair from all three sources: byte-verified, whole-image\n\
         // signature pairing, and code-reference carrying. Good enough to CALL or to READ.\n\
         //\n\
         // It also holds rows that are PROVED and deliberately un-hookable -- the verifier\'s\n\
         // IDENTICAL-LEAF-NOPATCH: a function whose whole body is identical in both builds and\n\
         // which is too short for MinHook to patch, confirmed by MinHook\'s own ported rules. A\n\
         // three-byte `ret 0` is a fine thing to call and nowhere to put a five-byte jmp. Those\n\
         // rows appear HERE and never below; build.rs asserts that on every build.\n\
         //\n\
         // DETOUR_SAFE_1162_TO_1170 is the subset carrying BOTH claims a detour needs: the\n\
         // normalised instruction sequences agree over the body (same function), and each image\'s\n\
         // own .pdata agrees about where the two endpoints sit relative to a function start\n\
         // (somewhere MinHook may write). MEASURED on 2026-08-29: when rows holding only the\n\
         // second claim were allowed to carry detours, er-armament-icons installed five of them\n\
         // and the game died ~2.0s in, at the first overlay draw -- one of the five is paired with\n\
         // a function sharing 18% of its instruction shape. Nothing about a signature match, or\n\
         // about a safe place to land, says the code there does the same job.\n",
    );
    out.push_str(&format!(
        "pub(crate) const VERIFIED_1162_TO_1170: [(u32, u32); {}] = [\n",
        rows.len()
    ));
    for (old, new) in &rows {
        out.push_str(&format!("    ({old:#x}, {new:#x}),\n"));
    }
    out.push_str("];\n");
    out.push_str(&format!(
        // Only the Windows resolver reads it; a host build still compiles the file.
        "#[allow(dead_code)]\npub(crate) const DETOUR_SAFE_1162_TO_1170: [(u32, u32); {}] = [\n",
        detour_rows.len()
    ));
    for (old, new) in &detour_rows {
        out.push_str(&format!("    ({old:#x}, {new:#x}),\n"));
    }
    out.push_str("];\n");
    let out_dir = env::var("OUT_DIR").unwrap();
    std::fs::write(Path::new(&out_dir).join("address_map_1170.rs"), out).unwrap();
}
