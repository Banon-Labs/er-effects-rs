// Shared build-script support for generating detour-prologue byte constants with `iced-x86`.
//
// This file is `include!`d by the build script of every crate that byte-checks a game function's
// prologue before detouring or calling it. It exists so that no crate has to hand-type machine
// code: the expected bytes are produced by naming the instructions and letting the assembler
// encode them.
//
// # Why hand-typed prologues are a trap
//
// `mov rax, rsp` has two legal encodings. `CodeAssembler::mov(rax, rsp)` emits `48 89 e0`; the
// game ships `48 8b c4`. A prologue that differs by one byte fails its own install-time byte
// check, the hook disarms itself on every launch, and the feature looks built while doing
// nothing. That happened in this repo. So the rule is: name the instruction, and where the
// assembler has a choice, name the exact `Code` (see [`mov_r64_rm64`] and friends below).
//
// # What pins the result
//
// Two independent checks, both applied by [`generate`]:
//
// 1. **The pin.** Every spec carries the byte sequence the constant had before it was generated.
//    It is machine-independent and always runs, so a wrong encoding choice breaks the build
//    everywhere -- including a machine with no copy of the game.
// 2. **Ground truth.** When a readable copy of the image is found, the assembled bytes are
//    compared against the bytes actually living at that VA in the real binary. This is the
//    stronger check, but it can only run where the image exists, so it skips (with a
//    `cargo:warning` naming what went unverified) rather than failing when it is absent -- the
//    same shape as the corpus-gated tests in `crates/er-gfx/tests/common/mod.rs`.
//
// The pin is therefore not redundant with ground truth: it is the half that survives on a
// machine, or in CI, that has no game files.
//
// # one supported version per module, recorded here, checked before anything else
//
// Every image named below is pinned to exactly one build, and this workspace supports that build
// and no other. There is no candidate list, no "try these addresses and see which one takes", and
// no best-effort adaptation to whatever the machine happens to hold. The only question asked of an
// installed module is the boolean one: *is this the build we recorded, yes or no*.
//
// The two halves of an image's identity, and why they are not the same question:
//
// * **Which build is this file?** A game image answers by its NAME: `eldenring-deobf.bin` *is*
//   1.16.2 and `eldenring-deobf-1.17.bin` *is* 1.17, because this workspace produces them and
//   names them. `ersc.dll` cannot answer that way -- it is third-party, it always has the same
//   file name, and the user replaces it on their own schedule -- so it answers by CONTENT: the
//   `Seamless Co-op vX.Y.Z by Yui` banner Seamless builds into its own image.
//   [`ERSC_SUPPORTED_VERSION`] is the recorded answer, and it is the single value to change when
//   this workspace moves to a new Seamless build.
// * **Are our bytes right for it?** That is ground truth, and it may only be asked once the first
//   question has been answered yes. Comparing a pin against a different build's file is not ground
//   truth failing, it is ground truth being impossible: the diff it prints is a fact about a
//   module nobody claimed those addresses described.
//
// So [`generate`] identifies before it compares, and both answers are fatal when they are wrong:
//
// * **Wrong version installed.** The machine has Seamless, and it is not the build this tree was
//   measured against. A DLL built here would be inert on that machine at best -- the runtime gates
//   fail closed -- and its addresses name the wrong fields of a live multiplayer session at worst.
//   The build says so, in full, naming both versions and the ways out. It does not warn and carry
//   on: a `cargo:warning` scrolls past inside a green build, and the person who then pays for it
//   is a player wondering why a feature does nothing.
// * **Right version, wrong bytes.** The file is the recorded build and our constants disagree with
//   it. That is a defect in this repo -- the pins are wrong for the version they claim -- and it
//   panics for every image alike, `ersc.dll` included.
//
// **A module that is absent is not a mismatch.** No `ersc.dll`, no game image, no `ER_ERSC_DLL`:
// nothing to disagree with, so ground truth skips with a `cargo:warning` and the pin carries the
// verification alone. That is what keeps this buildable in CI and on a machine that has never
// installed Seamless, and it is the same line `scripts/check-game-version-supported.py` draws --
// "a missing game is not a failed gate, it is a machine without the game."
//
// # The escape hatch, and the gate that it cannot slip past
//
// `ER_ERSC_DLL` names the file to ground-truth against, so a developer whose install has moved on
// can still build by pointing it at a copy of the supported build. It is deliberately narrow: it
// changes which file the constants are checked against, and it is itself version-checked, so it
// cannot be used to wave a mismatch through. It also says nothing about what the machine will
// actually load -- that question belongs to `scripts/check-ersc-version-supported.py`, which reads
// the installed module and is not overridable by this variable.
//
// When the pinned VA does not hold, [`generate`] searches the module's real code sections for the
// assembled bytes under the generated mask, so the diagnostic carries the address the function
// moved to rather than only the fact that it moved. That search is deliberately not wired back
// into the constants: a build-machine-discovered RVA would bake one particular installed DLL into
// a DLL shipped to other machines. Locating at runtime is the consumer's job, and where it cannot
// be done safely the consumer fails closed -- see the module docs of
// `crates/er-invasion-warp/src/local_invasion_filter.rs`.

use std::{
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use iced_x86::code_asm::*;
use iced_x86::{Code, Encoder, IcedError, Instruction, MemoryOperand, Register};

/// Mask byte meaning "this position must match exactly".
pub const PROLOGUE_BYTE_COMPARED: u8 = 0xff;
/// Mask byte meaning "ignore this position".
///
/// The only positions that ever get this value are the displacement bytes of a RIP-relative
/// memory operand. See [`rip_relative_mask`] for why, and for what is deliberately not masked.
pub const PROLOGUE_BYTE_IGNORED: u8 = 0x00;

/// MSVC emits a REX prefix with every bit clear ahead of some single-byte pushes (`40 55` for
/// `push rbp`). iced computes REX from the operands and exposes no way to request a redundant
/// one, so this prefix byte -- the only raw byte in the whole generator -- is named here and
/// emitted with `db`. [`rex_push`] is the only user.
pub const REDUNDANT_REX_PREFIX: u8 = 0x40;

/// The one Seamless Co-op build this workspace supports, and the only place that number is
/// written down.
///
/// # Why a single value and not a list
///
/// Every `ersc.dll` fact this repo holds -- an RVA, a struct field offset, a state code, a byte
/// signature -- was measured against one build, and the numbers do not survive an update: v2.0.0
/// moved `show` from `0x180022d30` to `0x1800241a0`, shifted the session object's state field from
/// `S+0x110` to `S+0x150`, and renumbered the session-state enum by `+1` throughout. A set of
/// addresses from one build used with the field offsets of another does not fail loudly at
/// runtime; it reads and writes the wrong members of a live multiplayer session. So there is no
/// version to fall back to and nothing to search: there is the recorded build, and everything
/// else.
///
/// # What changing it means
///
/// Moving to a new Seamless build is a re-measurement, not an edit to this line. Re-derive the
/// entry points (`uv run --with capstone python3 scripts/locate-ersc-entry-points.py`), read each
/// candidate rather than trusting a signature match -- v2.0.0's 19-byte `BUILD_LOBBY_KEY` pin
/// matches exactly one v2.0.0 address and it is the wrong function -- re-pin the constants and the
/// field offsets, and then set this. The value is checked against the file before any pin is
/// compared, so a repin that forgets this line fails the build rather than verifying against the
/// wrong module.
pub const ERSC_SUPPORTED_VERSION: &str = "2.0.1";

/// The version banner Seamless Co-op builds into its own image: `Seamless Co-op v2.0.1 by Yui`.
///
/// Read rather than inferred from a path, a file size or a timestamp, all of which a user can
/// change without changing the build. Measured 2026-09-06: the banner is UTF-16LE, NUL-terminated,
/// and occurs exactly once in the file (at `0x1e19fc` in the supported build), with no ASCII copy
/// of it anywhere in the file.
const ERSC_VERSION_BANNER: &str = "Seamless Co-op v";

/// How far past the banner prefix the walk to the NUL terminator may go, in bytes.
///
/// The banner is ~24 bytes of payload; this bound exists so a corrupt or hostile image cannot turn
/// "find the terminator" into a walk of the whole 13 MB file. Stripping NULs to fake an ASCII
/// search instead of walking code units is the mistake this avoids: the string pooled immediately
/// after the banner then merges into it, and both real builds report `v2.0.0 by YuiThis`.
const ERSC_VERSION_BANNER_LIMIT: usize = 128;

/// Which module a prologue lives in, and therefore which base its VA is relative to and which
/// file can ground-truth it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Image {
    /// `eldenring.exe` 1.16.2. Ground truth is `eldenring-deobf.bin`, a flat image at base
    /// `0x140000000` in which file offset == RVA for every section.
    EldenRing,
    /// `eldenring.exe` 1.17 -- the build the game has actually been since 2026-08-27. Ground
    /// truth is `eldenring-deobf-1.17.bin`, flat in the same way.
    ///
    /// # Why the version has to be part of the spec
    ///
    /// A prologue constant is compared against the bytes of the running game, and 1.17 moved
    /// things these bytes encode. The two save-request retractions are the clearest case: their
    /// whole body is `mov rax,[rip+disp]; mov byte [rax+0xb72],0; ret`, and `disp` reaches the
    /// GameMan singleton, which 1.17 moved by `+0x4070`. The instructions did not change and the
    /// field offset did not change, but four bytes of the encoding did -- so a signature
    /// generated against 1.16.2 fails its own byte check on 1.17, `call_verified_retract` fails
    /// closed, and the retraction silently never fires.
    ///
    /// Mapping the RVA does not fix that. The address translation gets the call to the right
    /// place; the check that decides whether to make the call is comparing against bytes from
    /// the previous build. Both halves have to move, which is why the version is named here
    /// rather than assumed.
    EldenRing1170,
    /// `ersc.dll` **v2.0.1**, the one build this workspace supports. The version is named
    /// rather than assumed: `ersc.dll` is third-party, always has the same file name, and the
    /// user replaces it on their own schedule, so [`Image::locate`] reads each candidate and
    /// matches [`Image::version_marker`] against the product string Seamless ships in its own
    /// resources. A file that does not carry that marker is not this image, whatever it is
    /// called and wherever it sits.
    Ersc201,
}

impl Image {
    pub fn base(self) -> u64 {
        match self {
            Self::EldenRing | Self::EldenRing1170 => 0x1_4000_0000,
            Self::Ersc201 => 0x1_8000_0000,
        }
    }

    /// Whether this image is a build of Seamless Co-op's `ersc.dll`.
    fn is_ersc(self) -> bool {
        matches!(self, Self::Ersc201)
    }

    /// The product string Seamless ships in its own version resource, as ASCII. It is compared
    /// UTF-16-encoded, which is how it appears in the file. This is what makes a candidate
    /// identifiable as a particular Seamless build rather than merely as "some ersc.dll".
    fn version_marker(self) -> Option<&'static str> {
        match self {
            Self::Ersc201 => Some("Seamless Co-op v2.0.1 by Yui"),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::EldenRing => "eldenring-deobf.bin",
            Self::EldenRing1170 => "eldenring-deobf-1.17.bin",
            Self::Ersc201 => "ersc.dll (Seamless Co-op v2.0.1)",
        }
    }

    fn env_override(self) -> &'static str {
        match self {
            Self::EldenRing => "ER_DEOBF_BIN",
            Self::EldenRing1170 => "ER_DEOBF_BIN_1170",
            // Two variables, because they list files to consider and the version marker
            // decides whether a file answers for this spec at all. Naming the version in the
            // variable instead would make the caller assert the very thing this code is able
            // to measure.
            Self::Ersc201 => "ER_ERSC_DLL / ER_ERSC_DLL_REFERENCE",
        }
    }

    /// The one build of this module the workspace supports, for an image that has to prove which
    /// build it is from its own content.
    ///
    /// `None` for the game images, and that is not a gap: they are version-named, so the file the
    /// build script opened has already answered the question by being called
    /// `eldenring-deobf.bin` rather than `eldenring-deobf-1.17.bin`. A flat dump carries no
    /// version resource to read anyway.
    fn supported_version(self) -> Option<&'static str> {
        match self {
            Self::EldenRing | Self::EldenRing1170 => None,
            Self::Ersc201 => Some(ERSC_SUPPORTED_VERSION),
        }
    }

    /// The version this image says it is, or `None` when it carries no version banner at all.
    ///
    /// `ersc.dll` can answer; the game dumps have nothing to read.
    fn declared_version(self, image: &[u8]) -> Option<String> {
        match self {
            Self::EldenRing | Self::EldenRing1170 => None,
            Self::Ersc201 => ersc_declared_version(image),
        }
    }

    /// The one-line command that re-measures this image's addresses by hand.
    fn remeasure_hint(self) -> &'static str {
        match self {
            Self::EldenRing | Self::EldenRing1170 => {
                "python3 scripts/map-rvas-1162-to-1170.py <va>"
            }
            // uv, because the body mapping needs capstone and there is no system pip here.
            Self::Ersc201 => "uv run --with capstone python3 scripts/locate-ersc-entry-points.py",
        }
    }

    /// Candidate locations, in order. Every one is env-overridable and none is required; a miss
    /// downgrades ground truth to a warning rather than breaking the build.
    fn locate(self, manifest_dir: &Path) -> Option<PathBuf> {
        match self {
            Self::EldenRing | Self::EldenRing1170 => {
                if let Some(explicit) = env::var_os(self.env_override()) {
                    let path = PathBuf::from(explicit);
                    return path.is_file().then_some(path);
                }
                manifest_dir
                    .ancestors()
                    .map(|ancestor| ancestor.join(self.label()))
                    .find(|candidate| candidate.is_file())
            }
            // Content, not position: every candidate is read and kept only if it carries the
            // supported build's version marker. A machine that has updated Seamless still holds
            // the previous build somewhere; it is skipped rather than ground-truthed against.
            Self::Ersc201 => {
                let marker: Vec<u8> = self
                    .version_marker()?
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect();
                ersc_candidates()
                    .into_iter()
                    .find(|candidate| match fs::read(candidate) {
                        Ok(image) => image
                            .windows(marker.len())
                            .any(|window| window == marker.as_slice()),
                        Err(_) => false,
                    })
            }
        }
    }

    /// The bytes the real module has at `va`, or `None` when the file cannot answer.
    fn bytes_at(self, image: &[u8], va: u64, len: usize) -> Option<Vec<u8>> {
        let rva = va.checked_sub(self.base())?;
        let offset = match self {
            Self::EldenRing | Self::EldenRing1170 => usize::try_from(rva).ok()?,
            Self::Ersc201 => pe_rva_to_offset(image, u32::try_from(rva).ok()?)?,
        };
        image
            .get(offset..offset.checked_add(len)?)
            .map(<[u8]>::to_vec)
    }

    /// Every VA in this module's real code where `bytes` occurs under `mask` -- i.e. where the
    /// function went when it stopped being at the address someone pinned.
    ///
    /// # Content, not position
    ///
    /// This is the half of the check that survives a third-party update. The pinned VA answers
    /// "is it still there"; this answers "where is it now", using only what the function is. The
    /// mask is the one [`rip_relative_mask`] derived, so a RIP-relative displacement -- which is
    /// guaranteed to re-encode when code moves -- does not defeat the search while opcodes,
    /// ModRM, field offsets, immediates and relative branches still have to match.
    ///
    /// # Why only executable, non-writable sections
    ///
    /// `ersc.dll` ships most of itself inside an Oreans WinLicense VM section (named `ERSC`)
    /// that is 11 MB of encrypted bytes. Scanning it would
    /// manufacture coincidental hits in ciphertext and report them as function addresses. That
    /// section is `CODE|EXECUTE|READ|WRITE`; a compiler-emitted `.text` is `CODE|EXECUTE|READ`
    /// with no write. Requiring executable-and-not-writable therefore selects exactly the
    /// plaintext code, and does it by section characteristics rather than by
    /// hard-coding a section name -- which would be another position assumption of the kind this
    /// whole function exists to remove.
    ///
    /// Flat game images have no section table at all, so the whole file is scanned.
    ///
    /// Returns at most [`MAX_CONTENT_MATCHES`] hits: a prologue shared by several functions (both
    /// ERSC option actions open with the same 14 bytes) must report as ambiguous, and a caller
    /// that sees the cap knows the signature does not identify anything on its own.
    fn find_by_content(self, image: &[u8], bytes: &[u8], mask: &[u8]) -> Vec<u64> {
        // `(file offset of the range, its length, the RVA that offset maps to)`. Carrying the
        // RVA rather than a precomputed offset->RVA delta is deliberate: a delta is a subtraction
        // that a malformed or repacked section table can make negative, and an underflow here
        // would abort a build script over a file this repo does not control.
        let ranges: Vec<(usize, usize, u64)> = match self {
            // Flat: file offset == RVA for every section, so one range covers the image.
            Self::EldenRing | Self::EldenRing1170 => vec![(0, image.len(), 0)],
            Self::Ersc201 => pe_sections(image)
                .into_iter()
                .filter(|section| {
                    section.characteristics & IMAGE_SCN_MEM_EXECUTE != 0
                        && section.characteristics & IMAGE_SCN_MEM_WRITE == 0
                })
                .filter_map(|section| {
                    let start = usize::try_from(section.raw_pointer).ok()?;
                    let len = usize::try_from(section.raw_size).ok()?;
                    let rva = u64::from(section.virtual_address);
                    (start.checked_add(len)? <= image.len()).then_some((start, len, rva))
                })
                .collect(),
        };
        let mut found = Vec::new();
        // [`rip_relative_mask`] asserts byte 0 is always compared, so it is a valid cheap anchor:
        // one comparison rejects ~99.6% of positions before the masked walk. It matters because
        // a game image is 98 MB and this runs in an unoptimised build script.
        let anchor = (mask.first() == Some(&PROLOGUE_BYTE_COMPARED)).then(|| bytes[0]);
        for (start, len, rva) in ranges {
            for at in start..=(start + len).saturating_sub(bytes.len()) {
                if anchor.is_some_and(|byte| image[at] != byte) {
                    continue;
                }
                let window = &image[at..at + bytes.len()];
                let hit = mask
                    .iter()
                    .zip(bytes)
                    .zip(window)
                    .all(|((&keep, expected), actual)| {
                        keep == PROLOGUE_BYTE_IGNORED || expected == actual
                    });
                if hit {
                    found.push(self.base() + rva + (at - start) as u64);
                    if found.len() >= MAX_CONTENT_MATCHES {
                        return found;
                    }
                }
            }
        }
        found
    }
}

/// `IMAGE_SCN_MEM_EXECUTE` / `IMAGE_SCN_MEM_WRITE`, the two section flags that separate
/// compiler-emitted code from a packer's writable VM section.
const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;

/// Hits reported by [`Image::find_by_content`] before it gives up and says "ambiguous". Small on
/// purpose: the answer a caller can act on is one address, and anything past a handful is a
/// signature that identifies a code shape rather than a function.
const MAX_CONTENT_MATCHES: usize = 8;

/// One PE section, in the terms the two callers here need.
struct Section {
    virtual_size: u32,
    virtual_address: u32,
    raw_pointer: u32,
    raw_size: u32,
    characteristics: u32,
}

/// The section table of a PE32+ image, or an empty list for anything that does not parse as one.
fn pe_sections(image: &[u8]) -> Vec<Section> {
    let read_u16 = |at: usize| -> Option<u16> {
        Some(u16::from_le_bytes(image.get(at..at + 2)?.try_into().ok()?))
    };
    let read_u32 = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(image.get(at..at + 4)?.try_into().ok()?))
    };
    let parse = || -> Option<Vec<Section>> {
        let pe = usize::try_from(read_u32(0x3c)?).ok()?;
        if image.get(pe..pe + 4)? != b"PE\0\0" {
            return None;
        }
        let count = usize::from(read_u16(pe + 6)?);
        let table = pe + 24 + usize::from(read_u16(pe + 20)?);
        (0..count)
            .map(|index| {
                let entry = table + 40 * index;
                Some(Section {
                    virtual_size: read_u32(entry + 8)?,
                    virtual_address: read_u32(entry + 12)?,
                    raw_size: read_u32(entry + 16)?,
                    raw_pointer: read_u32(entry + 20)?,
                    characteristics: read_u32(entry + 36)?,
                })
            })
            .collect()
    };
    parse().unwrap_or_default()
}

/// Every file that might be a build of `ersc.dll`, in the order they are tried.
///
/// Both env overrides and both install directories, because a version-named spec has to be able
/// to find its own build wherever it currently sits -- the Seamless launcher shuffles them
/// (`SeamlessCoop/` is the live one, `_SeamlessCoop/` is where the previous build is left), and
/// the user may downgrade. Which candidate answers for which spec is decided by
/// [`Image::version_marker`], never by which directory a file happens to be in.
fn ersc_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for variable in ["ER_ERSC_DLL", "ER_ERSC_DLL_REFERENCE"] {
        if let Some(explicit) = env::var_os(variable) {
            candidates.push(PathBuf::from(explicit));
        }
    }
    for root in steam_roots() {
        let game = root.join("steamapps/common/ELDEN RING/Game");
        candidates.push(game.join("SeamlessCoop/ersc.dll"));
        candidates.push(game.join("_SeamlessCoop/ersc.dll"));
    }
    candidates.retain(|candidate| candidate.is_file());
    candidates
}

fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(explicit) = env::var_os("ME3_STEAM_DIR") {
        roots.push(PathBuf::from(explicit));
    }
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        roots.push(home.join(".local/share/Steam"));
        roots.push(home.join(".steam/steam"));
    }
    roots
}

/// The `X.Y.Z` Seamless Co-op writes into its own [`ERSC_VERSION_BANNER`], or `None` when the
/// image carries no such banner.
///
/// Deliberately independent of the PE section table: the banner is a plain string constant, and
/// making its discovery depend on a parse that a repacked or corrupt image can defeat would turn
/// "which build is this" into "did the section table parse" -- two questions with very different
/// right answers. A whole-file search cannot be defeated that way and costs one pass.
///
/// The trailing ` by Yui` is dropped because it is an author credit, not part of the version, and
/// pinning it would make a future rename read as a version mismatch. What is not dropped is the
/// shape check: the token has to look like a dotted number, so a hit in compressed or encrypted
/// bytes cannot be reported as a version.
fn ersc_declared_version(image: &[u8]) -> Option<String> {
    let prefix: Vec<u8> = ERSC_VERSION_BANNER
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let start = image
        .windows(prefix.len())
        .position(|window| window == prefix)?;
    let body = start.checked_add(prefix.len())?;
    // `len() - 1` so the two-byte terminator read below always has a second byte.
    let limit = image
        .len()
        .saturating_sub(1)
        .min(body.saturating_add(ERSC_VERSION_BANNER_LIMIT));
    let mut end = body;
    while end < limit && image.get(end..end + 2) != Some(&[0, 0]) {
        end += 2;
    }
    let (pairs, _remainder) = image.get(body..end)?.as_chunks::<2>();
    let units: Vec<u16> = pairs.iter().copied().map(u16::from_le_bytes).collect();
    let version = String::from_utf16(&units)
        .ok()?
        .split_whitespace()
        .next()?
        .to_string();
    let dotted_number = !version.is_empty()
        && version
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.');
    dotted_number.then_some(version)
}

/// Minimal PE section walk: RVA -> file offset. Returns `None` for anything that does not parse
/// as a PE32+ image or for an RVA that falls outside every section.
fn pe_rva_to_offset(image: &[u8], rva: u32) -> Option<usize> {
    pe_sections(image).into_iter().find_map(|section| {
        let span = section.virtual_size.max(section.raw_size);
        (rva >= section.virtual_address && rva - section.virtual_address < span)
            .then(|| usize::try_from(section.raw_pointer + (rva - section.virtual_address)).ok())
            .flatten()
    })
}

/// How the generated constant is declared, chosen to match whatever the consuming module already
/// expects so that migrating a hand-written constant touches no call site.
#[derive(Clone, Copy)]
pub enum Shape {
    /// `const NAME: &[u8] = &[..];`
    Slice,
    /// `const NAME: [u8; N] = [..];`
    Array,
}

/// One generated prologue constant.
pub struct PrologueSpec {
    /// Constant name, exactly as the consuming module already spells it.
    pub name: &'static str,
    /// Doc comment for the generated constant, without the leading `///`. Empty for none.
    pub doc: &'static str,
    /// Visibility prefix: `""`, `"pub"` or `"pub(crate)"`.
    pub visibility: &'static str,
    pub shape: Shape,
    pub image: Image,
    /// Virtual address of the function entry, i.e. `image.base() + rva`.
    pub va: u64,
    /// How many of the assembled bytes the constant keeps. `0` keeps all of them. A shorter value
    /// is how a prologue that deliberately stops mid-instruction is expressed; the instructions
    /// are still named in full and only the tail is dropped.
    pub take: usize,
    /// The bytes this constant had before it was generated. See the module docs: this is the
    /// machine-independent half of the verification and must never be relaxed to make a change
    /// build.
    pub pin: &'static [u8],
}

/// The named-instruction body of a prologue.
pub type Assemble = fn(&mut CodeAssembler) -> Result<(), IcedError>;

// ---------------------------------------------------------------------------------------------
// Encoding helpers for the forms where the assembler would otherwise pick the other encoding.
// ---------------------------------------------------------------------------------------------

/// `push <reg>` behind MSVC's redundant REX prefix, e.g. `40 55`.
pub fn rex_push(asm: &mut CodeAssembler, register: AsmRegister64) -> Result<(), IcedError> {
    asm.db(&[REDUNDANT_REX_PREFIX])?;
    asm.push(register)
}

/// `mov <r64>, <r64>` in the `8b` (r64 <- rm64) direction. `asm.mov(rax, rsp)` picks `89`
/// instead, which is the one-byte difference that silently disarms a hook.
pub fn mov_r64_rm64(
    asm: &mut CodeAssembler,
    destination: Register,
    source: Register,
) -> Result<(), IcedError> {
    asm.add_instruction(Instruction::with2(Code::Mov_r64_rm64, destination, source)?)
}

/// `mov <r32>, <r32>` in the `8b` direction, for the same reason as [`mov_r64_rm64`].
pub fn mov_r32_rm32(
    asm: &mut CodeAssembler,
    destination: Register,
    source: Register,
) -> Result<(), IcedError> {
    asm.add_instruction(Instruction::with2(Code::Mov_r32_rm32, destination, source)?)
}

/// `cmp <r32>, <r32>` in the `3b` direction; `asm.cmp` picks `39`.
pub fn cmp_r32_rm32(
    asm: &mut CodeAssembler,
    destination: Register,
    source: Register,
) -> Result<(), IcedError> {
    asm.add_instruction(Instruction::with2(Code::Cmp_r32_rm32, destination, source)?)
}

/// `xor <r32>, <r32>` in the `33` direction; `asm.xor` picks `31`.
pub fn xor_r32_rm32(
    asm: &mut CodeAssembler,
    destination: Register,
    source: Register,
) -> Result<(), IcedError> {
    asm.add_instruction(Instruction::with2(Code::Xor_r32_rm32, destination, source)?)
}

/// `mov <r64>, [<base> + displacement]`.
pub fn mov_r64_mem(
    asm: &mut CodeAssembler,
    destination: Register,
    base: Register,
    displacement: i64,
) -> Result<(), IcedError> {
    asm.add_instruction(Instruction::with2(
        Code::Mov_r64_rm64,
        destination,
        MemoryOperand::with_base_displ(base, displacement),
    )?)
}

/// `mov <r64>, [<base>]` with no displacement byte.
///
/// Distinct from `mov_r64_mem(.., 0)` on purpose: iced encodes an explicit zero displacement as
/// `mod=01, disp8=0` (`49 8b 40 00`), and the game ships the `mod=00` form (`49 8b 00`). The pin
/// catches the difference, but naming the two forms separately means it never has to.
pub fn mov_r64_mem_base(
    asm: &mut CodeAssembler,
    destination: Register,
    base: Register,
) -> Result<(), IcedError> {
    asm.add_instruction(Instruction::with2(
        Code::Mov_r64_rm64,
        destination,
        MemoryOperand::with_base(base),
    )?)
}

/// `mov <r32>, [<base> + displacement]`.
pub fn mov_r32_mem(
    asm: &mut CodeAssembler,
    destination: Register,
    base: Register,
    displacement: i64,
) -> Result<(), IcedError> {
    asm.add_instruction(Instruction::with2(
        Code::Mov_r32_rm32,
        destination,
        MemoryOperand::with_base_displ(base, displacement),
    )?)
}

/// `mov rax, [rip + ..]` written as the absolute address it resolves to. iced turns a `RIP`-based
/// memory operand's displacement into the rip-relative delta for the VA being assembled at, so
/// the singleton's address is named rather than the encoded offset.
pub fn mov_rax_rip_absolute(asm: &mut CodeAssembler, target: u64) -> Result<(), IcedError> {
    asm.add_instruction(Instruction::with2(
        Code::Mov_r64_rm64,
        Register::RAX,
        MemoryOperand::with_base_displ(Register::RIP, target as i64),
    )?)
}

// ---------------------------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------------------------

/// What [`assemble`] produces: the constant, and the longer sequence used to locate it.
///
/// The two differ because `PrologueSpec::take` truncates. `ANNOUNCE_UPDATE_PROLOGUE` keeps 8 of
/// 11 bytes and stops mid-`movaps`; `SHOW_PROLOGUE` keeps the eight pushes and drops the
/// `sub rsp,0x188` that follows. Truncation is right for a gate -- it is the window the runtime
/// check reads -- and wrong for a search, because the dropped bytes are the discriminating ones.
/// Measured on Seamless v2.0.0: `SHOW_PROLOGUE`'s 12 kept bytes are eight callee-saved pushes
/// that occur 1248 times in `ersc.dll`, while the same pushes plus the frame size occur exactly
/// once. Searching with `kept` reports "ambiguous, capped at 8"; searching with `full` reports
/// the one address that is actually the function.
struct Assembled {
    /// The constant: `take` bytes, and its gate mask.
    kept: Vec<u8>,
    mask: Vec<u8>,
    /// Every byte the named instructions encode to, and its mask. Never emitted; only searched.
    full: Vec<u8>,
    full_mask: Vec<u8>,
}

fn assemble(spec: &PrologueSpec, body: Assemble) -> Assembled {
    let mut asm = CodeAssembler::new(64).unwrap_or_else(|error| {
        panic!("{}: CodeAssembler::new failed: {error}", spec.name);
    });
    body(&mut asm).unwrap_or_else(|error| {
        panic!(
            "{}: assembling named instructions failed: {error}",
            spec.name
        );
    });
    // Captured before `assemble` because that call needs `&mut asm`. These are the same named
    // instructions the pin is generated from, which is what makes the mask derivable rather than
    // hand-marked.
    let instructions: Vec<Instruction> = asm.instructions().to_vec();
    let bytes = asm.assemble(spec.va).unwrap_or_else(|error| {
        panic!("{}: encoding at 0x{:x} failed: {error}", spec.name, spec.va);
    });
    let take = if spec.take == 0 {
        bytes.len()
    } else {
        spec.take
    };
    assert!(
        take <= bytes.len(),
        "{}: asked for {take} bytes but the named instructions encode to only {}",
        spec.name,
        bytes.len()
    );
    let kept = bytes[..take].to_vec();
    let mask = rip_relative_mask(spec, &instructions, &kept);
    let full_mask = rip_relative_mask(spec, &instructions, &bytes);
    Assembled {
        kept,
        mask,
        full: bytes,
        full_mask,
    }
}

/// The comparison mask for `bytes`: `PROLOGUE_BYTE_IGNORED` at every byte that belongs to the
/// displacement of a RIP-relative memory operand, `PROLOGUE_BYTE_COMPARED` everywhere else.
///
/// # What is masked, and why exactly that
///
/// `mov rax, [rip+disp32]` encodes the delta from the end of the instruction to the global it
/// names. Both ends move when the game is patched, so the four displacement bytes are guaranteed
/// to re-encode across builds even when the function is byte-for-byte the same code doing the
/// same job. A pin that includes them is pinning a value that cannot survive, and it disarms its
/// hook on a target that translated perfectly. Measured on 1.17: three prologues
/// (`SAVE_REQUEST_RETRACT_B72_SIG`, `..._B73_SIG`, `QUIT_PHASE_SETTLE_SIG`) differ from their
/// 1.16.2 pin only inside that field.
///
/// # What is deliberately not masked
///
/// * **Opcode and ModRM bytes.** They are the instruction's identity; masking them would turn the
///   pin into "some instruction is here", which is not a pin at all.
/// * **Register-base memory displacements** (`[rax+0xb72]`). They are struct field offsets, and
///   they are the only thing distinguishing `SAVE_REQUEST_RETRACT_B72_SIG` from `..._B73_SIG`:
///   masking them would let each of those two pins accept the other function. `map-rvas`'s
///   search matcher masks them because a search wants candidates; a gate wants identity.
/// * **Immediates** (`cmp [rax+0xbc4], 2`) and **relative branch targets** (`jne +0xa`). A branch
///   inside the checked window is relative to the function's own layout, so it does not move when
///   the function moves -- it survived 1.17 unchanged, and it is evidence worth keeping.
///
/// # Fail-closed
///
/// Every uncertainty answers "compare this byte". If an instruction will not re-encode, or the
/// re-encoded stream does not reproduce the assembled bytes exactly, the mask is all-compared and
/// the pin behaves exactly as it did before this function existed.
fn rip_relative_mask(spec: &PrologueSpec, instructions: &[Instruction], bytes: &[u8]) -> Vec<u8> {
    let all_compared = vec![PROLOGUE_BYTE_COMPARED; bytes.len()];
    let mut mask = all_compared.clone();
    let mut encoder = Encoder::new(64);
    let mut stream: Vec<u8> = Vec::with_capacity(bytes.len());
    for instruction in instructions {
        let at = stream.len();
        let length = match encoder.encode(instruction, spec.va + at as u64) {
            Ok(length) => length,
            // A declared-data pseudo-instruction (`db`, used for MSVC's redundant REX prefix) or
            // anything else the encoder declines: give up on masking, keep every byte compared.
            Err(_) => return all_compared,
        };
        let encoded = encoder.take_buffer();
        if encoded.len() != length {
            return all_compared;
        }
        let offsets = encoder.get_constant_offsets();
        if instruction.is_ip_rel_memory_operand() && offsets.has_displacement() {
            let start = at + offsets.displacement_offset();
            for index in start..start + offsets.displacement_size() {
                if index < mask.len() {
                    mask[index] = PROLOGUE_BYTE_IGNORED;
                }
            }
        }
        stream.extend_from_slice(&encoded);
        if stream.len() >= bytes.len() {
            break;
        }
    }
    // Self-validation: the per-instruction re-encode must reproduce the block-assembled bytes.
    // If it does not, the offsets above describe a different byte stream than the pin, so the
    // mask would be aimed at the wrong positions.
    if stream.len() < bytes.len() || &stream[..bytes.len()] != bytes {
        return all_compared;
    }
    // Structural invariants. A mask that reaches byte 0, that masks everything, or that masks a
    // number of bytes no displacement field could account for, is a derivation bug, not a
    // relocation -- and it would silently weaken a gate. Break the build instead.
    let masked = mask.iter().filter(|&&m| m == PROLOGUE_BYTE_IGNORED).count();
    assert!(
        mask[0] == PROLOGUE_BYTE_COMPARED,
        "{}: derived mask would ignore the opening byte",
        spec.name
    );
    assert!(
        masked < mask.len() && masked % 4 == 0,
        "{}: derived mask ignores {masked} of {} bytes; a RIP disp32 field is 4 bytes",
        spec.name,
        mask.len()
    );
    mask
}

/// The message for "the pinned VA does not hold the bytes this spec describes".
///
/// This one is reached only once the file has proven it is the recorded build (see
/// [`describe_version_mismatch`], which runs first and stops the build if it has not), so it says
/// what it means: our constants are wrong for a version we claim to support. Three things every
/// reader needs are here rather than in someone's memory of a build that broke months ago: what
/// was expected, what is actually there, and where the expected bytes went.
fn describe_mismatch(
    spec: &PrologueSpec,
    path: &Path,
    bytes: &[u8],
    actual: &[u8],
    found: &[u64],
) -> String {
    let mut report = format!(
        "{}: assembled {} but {} has {} at 0x{:x}.\n",
        spec.name,
        hex(bytes),
        path.display(),
        hex(actual),
        spec.va
    );
    match found {
        [] => report.push_str(
            "  This function's opening occurs NOWHERE in that module's code, so nothing simply \
             moved: it is a build whose code this constant does not describe.\n",
        ),
        [single] => report.push_str(&format!(
            "  One CANDIDATE, from a unique content match: 0x{single:x}. A candidate is a shape \
             match, not an identification -- two functions can open identically, so READ it \
             before pinning it.\n"
        )),
        many => report.push_str(&format!(
            "  {} candidates ({}{}) -- this opening is a code SHAPE the module uses more than \
             once, so it identifies nothing on its own.\n",
            many.len(),
            many.iter()
                .map(|va| format!("0x{va:x}"))
                .collect::<Vec<_>>()
                .join(", "),
            if many.len() >= MAX_CONTENT_MATCHES {
                ", search capped"
            } else {
                ""
            },
        )),
    }
    if let Some(version) = spec.image.supported_version() {
        report.push_str(&format!(
            "  The file IS the supported build (v{version}), so this is not a Seamless update: \
             the constant is wrong for the version it claims to describe.\n"
        ));
    }
    report.push_str(&format!(
        "  To re-measure by hand: {}",
        spec.image.remeasure_hint()
    ));
    report
}

/// The message for "this file is a different build of the module than the one we support".
///
/// A separate report from [`describe_mismatch`] because it is a different fact with a different
/// audience and a different fix. A byte mismatch is addressed to whoever changed this repo; this
/// one is usually addressed to someone whose Seamless Co-op updated itself, and telling them
/// "assembled `f3 0f 1e fa ...` but the file has `00 48 8b 85 ...`" answers a question they did
/// not ask. It says which build they have, which one this tree supports, and the three ways out.
///
/// `found` is `None` when the file carries no version banner at all -- an unreadable identity is
/// treated as an unsupported one, the same call `er_game_base::game_build::is_supported_build`
/// makes for a game whose version resource will not parse: "cannot tell" and "wrong build" have
/// the same consequence for an address measured against a different one.
fn describe_version_mismatch(image: Image, path: &Path, want: &str, found: Option<&str>) -> String {
    let label = image.label();
    let is = match found {
        Some(version) => format!("is v{version}"),
        None => "carries no version banner, so it cannot be identified at all".to_string(),
    };
    format!(
        "{label} at {} {is}; this workspace supports v{want} and no other version.\n\
         \n\
         Every {label} address, struct field offset and state code in this tree was measured \
         against v{want}. They do not carry across an update: v2.0.0 moved the session object's \
         state field from S+0x110 to S+0x150 and renumbered the state enum by +1, so a correct \
         address from one build used with the field offsets of another reads and writes the wrong \
         members of a live multiplayer session. That is why this stops the build instead of \
         guessing.\n\
         \n\
         Three ways forward, in the order you probably want them:\n\
         \n\
         1. Re-measure this tree onto the installed build:\n\
         \x20     {}\n\
         \x20  Then re-pin the constants AND the field offsets, and set ERSC_SUPPORTED_VERSION\n\
         \x20  in build-support/prologue_build.rs to the new version. A signature match is a\n\
         \x20  candidate, never an identification -- read each one before pinning it.\n\
         2. Point the build at a copy of the supported build:\n\
         \x20     {}=<path to a v{want} {label}>\n\
         \x20  That verifies the constants and nothing else. It does not change what the game\n\
         \x20  will load, and scripts/check-ersc-version-supported.py reads the INSTALLED\n\
         \x20  module and is not overridable by it.\n\
         3. Install v{want} of the module.\n",
        path.display(),
        image.remeasure_hint(),
        image.env_override(),
    )
}

/// `[(start, len)]` for each masked run, for the generated doc comment.
fn masked_runs(mask: &[u8]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut start = None;
    for (index, &byte) in mask.iter().enumerate() {
        match (byte == PROLOGUE_BYTE_IGNORED, start) {
            (true, None) => start = Some(index),
            (false, Some(from)) => {
                runs.push((from, index - from));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        runs.push((from, mask.len() - from));
    }
    runs
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn declaration(spec: &PrologueSpec, name: &str, bytes: &[u8], out: &mut String) {
    let space = if spec.visibility.is_empty() { "" } else { " " };
    match spec.shape {
        Shape::Slice => writeln!(out, "{}{space}const {}: &[u8] = &[", spec.visibility, name),
        Shape::Array => writeln!(
            out,
            "{}{space}const {}: [u8; {}] = [",
            spec.visibility,
            name,
            bytes.len()
        ),
    }
    .expect("write to String");
    for chunk in bytes.chunks(12) {
        out.push_str("    ");
        for (index, byte) in chunk.iter().enumerate() {
            if index != 0 {
                out.push_str(", ");
            }
            write!(out, "0x{byte:02x}").expect("write to String");
        }
        out.push_str(",\n");
    }
    out.push_str("];\n");
}

fn render(spec: &PrologueSpec, bytes: &[u8], mask: &[u8]) -> String {
    let mut out = String::new();
    for line in spec.doc.lines().filter(|line| !line.is_empty()) {
        writeln!(out, "/// {line}").expect("write to String");
    }
    writeln!(
        out,
        "/// Generated from named `iced-x86` instructions; see this crate's `build.rs`. Do not"
    )
    .expect("write to String");
    writeln!(out, "/// hand-edit -- edit the instructions instead.").expect("write to String");
    declaration(spec, spec.name, bytes, &mut out);
    out.push('\n');

    let runs = masked_runs(mask);
    writeln!(
        out,
        "/// Comparison mask for [`{}`]: 0xff = compare, 0x00 = ignore.",
        spec.name
    )
    .expect("write to String");
    writeln!(out, "///").expect("write to String");
    if runs.is_empty() {
        writeln!(
            out,
            "/// Nothing is ignored -- this prologue has no RIP-relative operand, so every byte is"
        )
        .expect("write to String");
        writeln!(
            out,
            "/// an exact-match byte and the comparison is identical to a plain `==`."
        )
        .expect("write to String");
    } else {
        let listed: Vec<String> = runs
            .iter()
            .map(|(start, length)| format!("+{start}..+{}", start + length))
            .collect();
        writeln!(
            out,
            "/// IGNORED: {}. Each run is the displacement field of a RIP-relative memory operand,",
            listed.join(", ")
        )
        .expect("write to String");
        writeln!(
            out,
            "/// which re-encodes on every build because both the instruction and the global it"
        )
        .expect("write to String");
        writeln!(
            out,
            "/// names move. Opcode, ModRM, register-base displacements, immediates and relative"
        )
        .expect("write to String");
        writeln!(
            out,
            "/// branch targets are all still compared. See `build-support/prologue_build.rs`."
        )
        .expect("write to String");
    }
    writeln!(out, "#[allow(dead_code)]").expect("write to String");
    declaration(spec, &format!("{}_MASK", spec.name), mask, &mut out);
    out
}

/// One image the specs name, with its file and contents once it has passed identification.
/// `None` means the file is not on this machine, which is not a failure -- see [`generate`].
type LoadedImage = (Image, Option<(PathBuf, Vec<u8>)>);

/// Locate, read and identify every distinct image the specs name, once each.
///
/// Once each matters twice over. It is where "fail early" comes from -- identification happens
/// before a single spec is compared, so a machine holding an unsupported Seamless build gets one
/// report about versions rather than four reports about bytes, in a build that then succeeds. And
/// it is the difference between reading `ersc.dll` once and reading its 13 MB once per spec, which
/// is what the per-spec `fs::read` this replaced actually did.
///
/// Panics only on the case it exists to catch: a file that is present, readable, and a build this
/// workspace does not support. Absent and unreadable both answer `None`, because neither is a
/// disagreement -- they are a machine that cannot answer the question.
fn load_images(specs: &[(PrologueSpec, Assemble)], manifest_dir: &Path) -> Vec<LoadedImage> {
    let mut loaded: Vec<LoadedImage> = Vec::new();
    for (spec, _) in specs {
        if loaded.iter().any(|(image, _)| *image == spec.image) {
            continue;
        }
        let Some(path) = spec.image.locate(manifest_dir) else {
            loaded.push((spec.image, None));
            continue;
        };
        // The image is a build input from here on: without this, swapping `ersc.dll` leaves a
        // stale verdict cached and neither the skip warning nor the version failure reappears.
        println!("cargo:rerun-if-changed={}", path.display());
        let Ok(bytes) = fs::read(&path) else {
            // Present but unreadable: a permission or I/O problem on the developer's machine, not
            // a statement about which build it is. Same verdict as absent.
            loaded.push((spec.image, None));
            continue;
        };
        if let Some(want) = spec.image.supported_version() {
            let found = spec.image.declared_version(&bytes);
            assert!(
                found.as_deref() == Some(want),
                "{}",
                describe_version_mismatch(spec.image, &path, want, found.as_deref())
            );
        }
        loaded.push((spec.image, Some((path, bytes))));
    }
    loaded
}

/// The file and bytes of `image`, or `None` when this machine does not have it.
fn loaded(images: &[LoadedImage], image: Image) -> Option<(&Path, &[u8])> {
    images
        .iter()
        .find(|(candidate, _)| *candidate == image)?
        .1
        .as_ref()
        .map(|(path, bytes)| (path.as_path(), bytes.as_slice()))
}

/// The `SUPPORTED_VERSION` constant emitted alongside the prologues, so the version a consuming
/// crate names in a refusal cannot drift from the one the build script checked.
///
/// Emitted only for a file whose specs name an image with a recorded version, and at most one such
/// image per file: two would make `SUPPORTED_VERSION` ambiguous, and a constant that silently
/// describes whichever image happened to come first is worse than not having one.
fn render_supported_version(specs: &[(PrologueSpec, Assemble)]) -> String {
    let mut versioned: Vec<Image> = Vec::new();
    for (spec, _) in specs {
        if spec.image.supported_version().is_some() && !versioned.contains(&spec.image) {
            versioned.push(spec.image);
        }
    }
    match versioned.as_slice() {
        [] => String::new(),
        [image] => {
            let label = image.label();
            let version = image
                .supported_version()
                .expect("only images with a recorded version are collected above");
            format!(
                "/// The one build of `{label}` every constant in this file was measured against,\n\
                 /// and which this crate's `build.rs` verified the file on this machine to be.\n\
                 ///\n\
                 /// Generated so that a runtime refusal can NAME the supported version without a\n\
                 /// second copy of the number going stale beside the first. See\n\
                 /// `build-support/prologue_build.rs`.\n\
                 #[allow(dead_code)]\n\
                 pub const SUPPORTED_VERSION: &str = \"{version}\";\n\n"
            )
        }
        many => panic!(
            "{} images with recorded versions in one generated file ({}); SUPPORTED_VERSION \
             could only name one of them",
            many.len(),
            many.iter()
                .map(|image| image.label())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Assemble every spec, verify it, and write the constants to `OUT_DIR/<out_file>`.
///
/// Order matters and is the point of the function: every image is identified before any spec is
/// compared against it. A file that is present but is a build this workspace does not support
/// stops the build there, with a report about versions; only a file that has proven it is the
/// recorded build gets its bytes read, and then a disagreement is a defect in this repo and
/// panics for every image alike.
///
/// The one thing that is not a failure is absence. No image, no `ER_ERSC_DLL`, no game dump:
/// nothing to disagree with, so ground truth skips with a `cargo:warning` and the pin carries the
/// verification on its own. See the module docs.
pub fn generate(specs: &[(PrologueSpec, Assemble)], out_file: &str) {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR for build scripts"),
    );
    let mut generated = String::new();
    let mut unverified: Vec<&'static str> = Vec::new();
    let images = load_images(specs, &manifest_dir);

    for (spec, body) in specs {
        let Assembled {
            kept: bytes,
            mask,
            full,
            full_mask,
        } = assemble(spec, *body);
        assert_eq!(
            bytes.as_slice(),
            spec.pin,
            "{}: the named instructions encode to {} but the pinned bytes are {} -- the \
             assembler picked a different encoding than the one the game ships at 0x{:x}",
            spec.name,
            hex(&bytes),
            hex(spec.pin),
            spec.va
        );
        match loaded(&images, spec.image) {
            Some((path, image)) => match spec.image.bytes_at(image, spec.va, bytes.len()) {
                Some(actual) if actual == bytes => {}
                Some(actual) => {
                    // Searched with `full`, not `bytes`: see [`Assembled`]. The constant is
                    // truncated for the runtime gate, and the truncated part is what tells
                    // this function apart from every other one with the same opening.
                    let found = spec.image.find_by_content(image, &full, &full_mask);
                    panic!("{}", describe_mismatch(spec, path, &bytes, &actual, &found));
                }
                // The pinned VA is not inside this file at all: past the end of a flat dump, or
                // outside every section of a PE. For a version-checked image that is the loudest
                // possible disagreement -- the file has proven it is the build we support, and
                // the address we pinned is not in it -- so it fails rather than skipping.
                None if spec.image.supported_version().is_some() => panic!(
                    "{}: 0x{:x} is not inside {}, which has already identified itself as the \
                     supported build. The pin does not describe this module.\n  To re-measure by \
                     hand: {}",
                    spec.name,
                    spec.va,
                    path.display(),
                    spec.image.remeasure_hint()
                ),
                // A version-named image that cannot answer: a truncated or unreadable dump. Same
                // verdict as not having it.
                None => unverified.push(spec.name),
            },
            None => unverified.push(spec.name),
        }
        generated.push_str(&render(spec, &bytes, &mask));
        generated.push('\n');
    }

    generated.insert_str(0, &render_supported_version(specs));

    if !unverified.is_empty() {
        let images: Vec<&str> = {
            let mut seen: Vec<&str> = specs
                .iter()
                .map(|(spec, _)| spec.image.env_override())
                .collect();
            seen.dedup();
            seen
        };
        println!(
            "cargo:warning=prologue ground truth skipped for {} ({}); set {} to verify against \
             the real module -- the pinned bytes still apply",
            out_file,
            unverified.join(", "),
            images.join(" / ")
        );
    }

    let out_dir =
        PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR for build scripts"));
    fs::write(out_dir.join(out_file), generated)
        .unwrap_or_else(|error| panic!("writing {out_file}: {error}"));
}

/// Tell cargo to re-run when the build script or this shared support file changes, and when an
/// image override moves.
pub fn declare_rerun(build_script_relative_support_path: &str) {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={build_script_relative_support_path}");
    println!("cargo:rerun-if-env-changed=ER_DEOBF_BIN");
    println!("cargo:rerun-if-env-changed=ER_DEOBF_BIN_1170");
    println!("cargo:rerun-if-env-changed=ER_ERSC_DLL");
    println!("cargo:rerun-if-env-changed=ME3_STEAM_DIR");
}
