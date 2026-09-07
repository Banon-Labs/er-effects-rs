//! Tier A: the one line that says which binary wrote a log.
//!
//! # The failure this exists to end
//!
//! A tester's log arrives with a symptom in it and the first question is always the same:
//! which build is that? On 2026-08-24 an `er-invasion-warp` log had to be dated by
//! string-matching its own format literals against the repo's source, because its opening
//! line was `loaded module_base=0x…` and nothing else -- no commit, no build time, no file
//! name. The alternative on offer was asking a human to courier DLLs across so a machine
//! could hash them. Both are archaeology in place of a field the writer already had.
//!
//! # Three facts, three different failure modes, all needed
//!
//! | fact | source | goes stale when |
//! |---|---|---|
//! | git sha + dirty | `build.rs`, baked at compile time | a tracked file is edited but never staged |
//! | PE timestamp | the DLL's own header, read at runtime | never -- the linker rewrites it on every relink |
//! | module file name | the loader, read at runtime | never |
//!
//! The PE timestamp is the anchor: it is per-module and exact, so a run mixing DLLs built
//! days apart (the ordinary case -- `er-crash-modules.txt` from 2026-08-23 carried a
//! bugfixes DLL from the 21st beside a crash logger from the 5th) shows that divergence
//! instead of hiding it behind one workspace-wide sha. It is also the same field
//! `er-crash-logging-core` prints for every loaded module, so a feature log and a crash dump can
//! be joined on it without anyone hashing anything.

/// Commit this crate was compiled from: a 12-hex-digit sha, `+dirty`-suffixed when tracked
/// files were modified relative to it, or [`UNKNOWN_SHA`] outside a git checkout.
///
/// Composed whole in `build.rs` rather than assembled here from a sha and a flag. Joining it at
/// runtime put the suffix behind a comparison of two compile-time constants, and the optimiser
/// resolved that branch and dropped the literal -- so a build whose tree was dirty shipped a
/// bare sha, which reads as a clean build. Verified by grepping the emitted DLL for the exact
/// string, which is only a meaningful check because there is exactly one of them.
pub const GIT_DESCRIPTION: &str = env!("ER_BUILD_GIT");

/// What [`GIT_DESCRIPTION`] holds when the build tree was not a git checkout.
pub const UNKNOWN_SHA: &str = "unknown";

/// `GetModuleHandleExW`: resolve the module containing an address rather than one named by
/// string. Passing the address of code in this crate is what makes the answer "the DLL that
/// is doing the logging" without any caller telling us its name.
#[cfg(windows)]
const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x0000_0004;

/// Resolve without taking a reference on the module. Omitting this would leak a refcount per
/// call and pin a DLL that might otherwise unload.
#[cfg(windows)]
const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x0000_0002;

/// `IMAGE_DOS_HEADER::e_lfanew` -- offset of the offset to the NT headers.
#[cfg(windows)]
const DOS_HEADER_LFANEW_OFFSET: usize = 0x3c;

/// `IMAGE_NT_HEADERS::FileHeader::TimeDateStamp`, past the 4-byte `PE\0\0` signature and the
/// two `IMAGE_FILE_HEADER` fields (`Machine`, `NumberOfSections`) that precede it.
#[cfg(windows)]
const NT_HEADERS_TIMESTAMP_OFFSET: usize = 8;

/// `MZ`, little-endian.
#[cfg(windows)]
const DOS_SIGNATURE: u16 = 0x5a4d;

/// `PE\0\0`, little-endian.
#[cfg(windows)]
const NT_SIGNATURE: u32 = 0x0000_4550;

/// Longest path `GetModuleFileNameW` is asked to return. A DLL loaded from deeper than this
/// degrades to the truncated tail rather than to nothing.
#[cfg(windows)]
const MODULE_PATH_BUFFER: usize = 512;

#[cfg(windows)]
unsafe extern "system" {
    fn GetModuleHandleExW(flags: u32, module_name: *const u16, module: *mut usize) -> i32;
    fn GetModuleFileNameW(module: usize, filename: *mut u16, size: u32) -> u32;
}

/// Base address and full path of the module this code is linked into.
///
/// Returns `None` on host builds and on the (unobserved) failure of either call, so a caller
/// degrades a field at a time instead of vanishing.
///
/// `own_module` below keeps only the leaf, which is what an identity line wants. The directory
/// is what a DLL that ships beside its own data needs: `mushroom-man-runtime` is inert unless
/// the model binders it depends on sit next to it, and that question cannot be asked of a leaf.
#[cfg(windows)]
pub fn own_module_path() -> Option<(usize, String)> {
    let mut module: usize = 0;
    // SAFETY: `own_module_path` is code in this module, so its address is inside the mapped image
    // the loader is being asked about, and `module` is a live out-param for the duration.
    let resolved = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            own_module_path as *const u16,
            &mut module,
        )
    };
    if resolved == 0 || module == 0 {
        return None;
    }
    let mut buffer = [0u16; MODULE_PATH_BUFFER];
    // SAFETY: `module` is a handle the call above just produced, and the length passed is the
    // true capacity of `buffer`.
    let written = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) };
    if written == 0 {
        return Some((module, String::new()));
    }
    Some((
        module,
        String::from_utf16_lossy(&buffer[..written as usize]),
    ))
}

#[cfg(not(windows))]
pub fn own_module_path() -> Option<(usize, String)> {
    None
}

/// Base address and file name of the module this code is linked into.
///
/// Returns `None` on host builds and on the (unobserved) failure of either call, so the
/// identity line degrades a field at a time instead of vanishing.
pub fn own_module() -> Option<(usize, String)> {
    let (module, path) = own_module_path()?;
    // The leaf is what a tester actually has on disk and what `er-crash-modules.txt` lists;
    // the directory is the game install and says nothing.
    let name = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string();
    Some((module, name))
}

/// `IMAGE_FILE_HEADER::TimeDateStamp` of a mapped module: when its linker wrote it.
///
/// Reads are fault-safe, so a module whose headers are not mapped (`/FILEALIGN` games, a
/// manually mapped image) returns `None` rather than faulting the game thread.
///
/// The value is not always a date. MSVC's `/Brepro` replaces it with a content hash, which is
/// why callers print the raw field beside any decoded date -- several Windows system DLLs in
/// this game's module list carry obvious non-dates like `0xfb84bf00`.
#[cfg(windows)]
pub fn pe_timestamp(module_base: usize) -> Option<u32> {
    // SAFETY: every read below goes through the fault-safe reader, which validates the address
    // via `ReadProcessMemory` instead of dereferencing it.
    unsafe {
        if crate::mem::safe_read_u16(module_base)? != DOS_SIGNATURE {
            return None;
        }
        let lfanew = crate::mem::safe_read_i32(module_base + DOS_HEADER_LFANEW_OFFSET)?;
        if lfanew <= 0 {
            return None;
        }
        let nt = module_base.checked_add(lfanew as usize)?;
        let signature = crate::mem::safe_read_i32(nt)? as u32;
        if signature != NT_SIGNATURE {
            return None;
        }
        Some(crate::mem::safe_read_i32(nt + NT_HEADERS_TIMESTAMP_OFFSET)? as u32)
    }
}

#[cfg(not(windows))]
pub fn pe_timestamp(_module_base: usize) -> Option<u32> {
    None
}

/// Seconds in a day, for splitting a Unix timestamp into days and time-of-day.
const SECONDS_PER_DAY: i64 = 86_400;

/// Render a Unix timestamp as `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Hand-rolled because this crate is a zero-dependency leaf on purpose (see its manifest) and
/// pulling a date crate into every mini-DLL to format one line per run is not a trade worth
/// making. The civil-date split is Howard Hinnant's `civil_from_days`, which is exact for the
/// whole range a `u32` timestamp can express.
pub fn format_utc(unix_seconds: u32) -> String {
    let seconds = i64::from(unix_seconds);
    let days = seconds.div_euclid(SECONDS_PER_DAY);
    let time_of_day = seconds.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Days since the Unix epoch -> `(year, month, day)`.
///
/// Shifts the era to start in March so the leap day lands at the end of a year and needs no
/// special case; `719468` is the day count from `0000-03-01` to `1970-01-01`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    const DAYS_FROM_MARCH_ZERO_TO_EPOCH: i64 = 719_468;
    const DAYS_PER_ERA: i64 = 146_097;
    const YEARS_PER_ERA: i64 = 400;

    let shifted = days + DAYS_FROM_MARCH_ZERO_TO_EPOCH;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - (DAYS_PER_ERA - 1)
    } / DAYS_PER_ERA;
    let day_of_era = shifted - era * DAYS_PER_ERA;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let year = year_of_era + era * YEARS_PER_ERA;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_position + 2) / 5 + 1) as u32;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The identity line every fresh log opens with.
///
/// Deliberately one line and deliberately first: a reader who scrolls to the top of a log
/// they were sent must be able to answer "which build is this" without reading further, and a
/// harness can pick it up with a single prefix match.
///
/// Every field degrades independently. A host build prints the sha alone; a module whose
/// headers cannot be read prints everything but the timestamp. The line is never absent.
pub fn identity_line() -> String {
    let mut line = format!("build git={GIT_DESCRIPTION}");
    match own_module() {
        Some((base, name)) => {
            if !name.is_empty() {
                line.push_str(&format!(" module={name}"));
            }
            line.push_str(&format!(" base=0x{base:x}"));
            if let Some(stamp) = pe_timestamp(base) {
                line.push_str(&format!(" pe=0x{stamp:08x} ({})", format_utc(stamp)));
            }
        }
        None => line.push_str(" module=<host build, not a loaded module>"),
    }
    line
}

/// Symbol every DLL in this workspace exports, because every DLL links this crate.
///
/// One definition here becomes an export of each `cdylib` that depends on `er-game-base`, so a
/// mod cannot ship without an identity the way it cannot ship without a log header. Any module
/// in the process can therefore ask any other "what are you" with a single `GetProcAddress`,
/// which is the whole mechanism behind [`loaded_mod_identities`].
///
/// Returns a NUL-terminated `name|sha|pe_timestamp` triple. The pointer is to a process-lifetime
/// `OnceLock`, so a caller may hold it indefinitely.
///
/// # Safety
///
/// Called across a DLL boundary by address. The returned pointer must be read as a NUL-terminated
/// byte string and never freed.
#[unsafe(no_mangle)]
pub extern "C" fn er_build_identity_v1() -> *const u8 {
    static ENCODED: std::sync::OnceLock<std::ffi::CString> = std::sync::OnceLock::new();
    ENCODED
        .get_or_init(|| {
            let (name, stamp) = match own_module() {
                Some((base, name)) => (name, pe_timestamp(base).unwrap_or(0)),
                None => (String::new(), 0),
            };
            // `|` cannot occur in a Windows file name, in a hex sha, or in a decimal stamp, so
            // no escaping is needed and a split can never be ambiguous.
            let encoded = format!("{name}|{GIT_DESCRIPTION}|{stamp}");
            std::ffi::CString::new(encoded).unwrap_or_else(|_| c"|unknown|0".to_owned())
        })
        .as_ptr()
        .cast()
}

/// What one loaded module answered when asked its identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModIdentity {
    /// The DLL's own file name, e.g. `er_invasion_warp.dll`.
    pub module: String,
    /// The commit it was built from, `+dirty`-suffixed when the tree was not clean.
    pub sha: String,
    /// `IMAGE_FILE_HEADER::TimeDateStamp`. Orders builds when their shas disagree.
    pub pe_timestamp: u32,
}

/// Split the wire form `name|sha|stamp` produced by [`er_build_identity_v1`].
///
/// Separated from the FFI so the parse is testable on the host, where no module exports
/// anything: the round trip is the part that can silently rot, not the `GetProcAddress`.
pub fn parse_identity(encoded: &str) -> Option<ModIdentity> {
    let mut parts = encoded.split('|');
    let module = parts.next()?.to_string();
    let sha = parts.next()?.to_string();
    let pe_timestamp = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(ModIdentity {
        module,
        sha,
        pe_timestamp,
    })
}

/// How a loaded build stands relative to `main`, and therefore how loudly it is drawn.
///
/// The distinction that matters is not "does this sha equal the newest one" -- it is whether the
/// build is behind. A commit that was never pushed is not behind anything; it is the ordinary
/// state of working locally, and shouting about it would train everyone to ignore the watermark.
/// A build that is an older published release is the one case where somebody is running code we
/// have already moved past, which is exactly the situation that cost a day of crash triage on
/// 2026-08-24.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Standing {
    /// This build is `main`'s tip, or local work on top of it. Barely drawn.
    AtMain,
    /// Built from a commit the release list has never heard of -- a local branch, an unpushed
    /// commit. Not behind, so drawn as quietly as [`Standing::AtMain`].
    Local,
    /// A published release that is not the newest: this build is genuinely behind `main`, and
    /// someone reporting a bug against it may be reporting one already fixed. Drawn red.
    BehindMain,
    /// The release list is not known yet. Never red -- see [`Standing::BehindMain`] for what red
    /// is reserved to mean.
    Unknown,
}

impl Standing {
    /// Opacity this standing is drawn at, in percent.
    ///
    /// 1% is deliberately almost invisible: the watermark's job in the ordinary case is to be
    /// available when someone screenshots it, not to be read. 25% is loud enough to notice
    /// without obscuring the game, and is reserved for the one actionable state.
    pub const fn opacity_percent(self) -> u8 {
        match self {
            Standing::BehindMain => 25,
            Standing::AtMain | Standing::Local | Standing::Unknown => 1,
        }
    }

    /// Whether this standing is the one the reader must act on.
    pub const fn is_behind(self) -> bool {
        matches!(self, Standing::BehindMain)
    }
}

/// Releases are tagged `main-<40 hex>`; the tag is the commit, so no asset has to be opened to
/// learn what a published build was cut from.
pub const RELEASE_TAG_PREFIX: &str = "main-";

/// Pull the commit out of a `main-<sha>` release tag.
///
/// Rejects anything else -- the semver tags (`0.1.6`) name a hand-cut release whose assets are
/// months older than any `main-*` pre-release, so admitting one into the ordered list would put
/// a stale commit at a position it does not hold on `main`.
pub fn sha_from_release_tag(tag: &str) -> Option<&str> {
    let sha = tag.strip_prefix(RELEASE_TAG_PREFIX)?;
    let hex = sha.len() >= 12 && sha.chars().all(|c| c.is_ascii_hexdigit());
    hex.then_some(sha)
}

/// Where this build stands against the published history of `main`.
///
/// `published` is every `main-*` release sha, newest first -- the order GitHub's releases API
/// already returns. `local` is a [`GIT_DESCRIPTION`]: a 12-hex prefix, optionally `+dirty`.
///
/// A dirty tree is [`Standing::AtMain`] when its sha is the tip, not stale. Local edits on top of
/// current `main` are how the work gets done, and the previous rule -- which called every dirty
/// build stale -- would have painted the developer's own screen red permanently.
///
/// The comparison is prefix-wise because the two sides are deliberately different lengths: 12 hex
/// digits is what fits on a watermark, and lengthening it would trade legibility for certainty
/// nobody needs at 48 bits.
pub fn standing_against_main(local: &str, published: &[String]) -> Standing {
    if published.is_empty() || local == UNKNOWN_SHA {
        return Standing::Unknown;
    }
    let clean = local.strip_suffix("+dirty").unwrap_or(local);
    let matches = |candidate: &String| {
        let width = clean.len().min(candidate.len());
        width >= 12 && clean.get(..width) == candidate.get(..width)
    };
    match published.iter().position(matches) {
        Some(0) => Standing::AtMain,
        Some(_) => Standing::BehindMain,
        // Not published at all: a local branch or an unpushed commit. Explicitly not behind.
        None => Standing::Local,
    }
}

/// Every `main-*` release commit, newest first, once something has looked them up.
///
/// A list rather than just the newest, because "behind main" is a position in that list and an
/// inequality against one sha cannot express it: a local commit that was never pushed and a
/// published commit from last week both differ from the tip, and only one of them is behind.
///
/// Held here rather than passed around because the two sides never meet -- whoever performs the
/// lookup (a background thread doing an HTTPS request) and whoever needs the answer (the render
/// thread, mid-Present) are different threads at different times.
static PUBLISHED_MAIN_SHAS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Record the published `main-*` commits, newest first. Idempotent; last writer wins.
pub fn set_published_main_shas(shas: Vec<String>) {
    if let Ok(mut slot) = PUBLISHED_MAIN_SHAS.lock() {
        *slot = shas;
    }
}

/// The published `main-*` commits, newest first; empty until the lookup succeeds.
///
/// Uses `try_lock`: this is read from the render thread inside Present, and a watermark is never
/// worth blocking a frame for. A contended read reports empty for that frame, which renders as
/// [`Standing::Unknown`] -- one quiet frame, not a stall.
pub fn published_main_shas() -> Vec<String> {
    PUBLISHED_MAIN_SHAS
        .try_lock()
        .map(|shas| shas.clone())
        .unwrap_or_default()
}

/// One line naming every one of this workspace's DLLs in the process, and how each compares to
/// the newest published release.
///
/// This is the watermark's data in text form, and it exists separately because the watermark is
/// drawn: if the overlay is ever blank, this line is what says whether the roster was empty or
/// the pixels were lost, which are different bugs with different fixes.
pub fn roster_line(published: &[String]) -> String {
    let identities = loaded_mod_identities();
    if identities.is_empty() {
        return "build roster: EMPTY -- no module answered er_build_identity_v1. Either this is \
                the only mod loaded and it failed to export the symbol, or enumeration failed."
            .to_string();
    }
    let mut line = format!("build roster: {} mod(s)", identities.len());
    for identity in &identities {
        let standing = match standing_against_main(&identity.sha, published) {
            Standing::AtMain => "at-main",
            Standing::Local => "local",
            Standing::BehindMain => "BEHIND-MAIN",
            Standing::Unknown => "unknown",
        };
        line.push_str(&format!(
            " [{} {} {} pe={}]",
            identity.module,
            identity.sha,
            standing,
            format_utc(identity.pe_timestamp)
        ));
    }
    line
}

/// Every module handle the loader currently has mapped.
///
/// Split out of [`loaded_mod_identities`] because asking "which modules export X" is a question
/// with more than one X: the identity symbol answers "which of these are ours", and the overlay
/// host registration asks a different one of the same list.
#[cfg(windows)]
pub fn loaded_module_handles() -> Vec<usize> {
    const MAX_MODULES: usize = 512;
    let mut modules = [0usize; MAX_MODULES];
    let mut needed: u32 = 0;
    // SAFETY: the buffer and its byte length are consistent, and `needed` is a live out-param.
    let ok = unsafe {
        K32EnumProcessModules(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            modules.as_mut_ptr(),
            std::mem::size_of_val(&modules) as u32,
            &mut needed,
        )
    };
    if ok == 0 {
        return Vec::new();
    }
    let count = (needed as usize / std::mem::size_of::<usize>()).min(MAX_MODULES);
    modules[..count].to_vec()
}

#[cfg(not(windows))]
pub fn loaded_module_handles() -> Vec<usize> {
    Vec::new()
}

/// Resolve an exported symbol in a module handle from [`loaded_module_handles`].
///
/// Returns the raw address, or `None` when the module does not export that name.
///
/// # Safety
///
/// `name` must be NUL-terminated. The caller is responsible for the signature it transmutes the
/// returned address to.
#[cfg(windows)]
pub unsafe fn module_export(module: usize, name: &[u8]) -> Option<usize> {
    // SAFETY: `module` is a loader handle and `name` is NUL-terminated by contract.
    let symbol = unsafe { GetProcAddress(module, name.as_ptr().cast()) };
    (symbol != 0).then_some(symbol)
}

/// Ask every loaded module for its identity; the ones that answer are this workspace's DLLs.
///
/// A module that does not export the symbol is not one of ours and is skipped silently -- the
/// game's own DLLs, Seamless, Steam and the CRT all fall out here without a list to maintain.
#[cfg(windows)]
pub fn loaded_mod_identities() -> Vec<ModIdentity> {
    let mut found = Vec::new();
    for module in loaded_module_handles() {
        // SAFETY: `module` is a handle the enumeration just returned.
        let symbol = unsafe { GetProcAddress(module, c"er_build_identity_v1".as_ptr().cast()) };
        if symbol == 0 {
            continue;
        }
        // SAFETY: the symbol is ours by name and ABI; it takes nothing and returns a pointer to
        // a process-lifetime NUL-terminated string.
        let describe: extern "C" fn() -> *const u8 = unsafe { std::mem::transmute(symbol) };
        let encoded = describe();
        if encoded.is_null() {
            continue;
        }
        // SAFETY: read fault-safely rather than trusting the pointer, for the same reason the
        // lobby hooks do: a module that exports this name is probably ours, but "probably" is
        // not a basis for walking a pointer looking for a NUL.
        let Some(bytes) = (unsafe { crate::mem::safe_read_cstr(encoded as usize, 256) }) else {
            continue;
        };
        if let Ok(text) = std::str::from_utf8(&bytes)
            && let Some(identity) = parse_identity(text)
        {
            found.push(identity);
        }
    }
    found.sort_by(|a, b| a.module.cmp(&b.module));
    found
}

#[cfg(not(windows))]
pub fn loaded_mod_identities() -> Vec<ModIdentity> {
    Vec::new()
}

/// `-1` as a handle: the current-process pseudo-handle, same convention as [`crate::mem`].
#[cfg(windows)]
const CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;

#[cfg(windows)]
unsafe extern "system" {
    fn K32EnumProcessModules(
        process: isize,
        modules: *mut usize,
        size_bytes: u32,
        needed: *mut u32,
    ) -> i32;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact field the crash logger printed for `er_seamless_bugfixes.dll` in the
    /// 2026-08-23 dump, and the date it was independently decoded to. This is the join
    /// between a feature log and a crash dump, so it is pinned rather than trusted.
    #[test]
    fn a_real_module_timestamp_decodes_to_its_known_date() {
        assert_eq!(format_utc(0x6a88_7a31), "2026-08-21T16:17:53Z");
    }

    /// Boundaries the civil-date split is most likely to get wrong: the epoch itself, a leap
    /// day, and the first instant after one.
    #[test]
    fn civil_dates_hold_at_the_epoch_and_across_a_leap_day() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(format_utc(1_709_251_199), "2024-02-29T23:59:59Z");
        assert_eq!(format_utc(1_709_251_200), "2024-03-01T00:00:00Z");
    }

    /// The wire form must survive a round trip, because the parse runs in a different DLL from
    /// the one that encoded it -- there is no shared type to keep them honest, only this test.
    #[test]
    fn an_identity_round_trips_through_its_wire_form() {
        let parsed = parse_identity("er_invasion_warp.dll|c57b5c1bf04a+dirty|1787368671")
            .expect("well-formed triple");
        assert_eq!(parsed.module, "er_invasion_warp.dll");
        assert_eq!(parsed.sha, "c57b5c1bf04a+dirty");
        assert_eq!(parsed.pe_timestamp, 1_787_368_671);
    }

    /// Malformed input comes from another module's memory, so it is refused rather than
    /// half-accepted: a watermark that renders a truncated sha is worse than one that omits it.
    #[test]
    fn a_malformed_identity_is_refused_rather_than_guessed() {
        assert!(parse_identity("only_a_name").is_none());
        assert!(parse_identity("name|sha").is_none());
        assert!(parse_identity("name|sha|not_a_number").is_none());
        assert!(parse_identity("name|sha|1|extra").is_none());
    }

    /// The real tag from 2026-08-25, and the semver tag that must not be mistaken for a build.
    #[test]
    fn only_a_main_release_tag_names_a_commit() {
        assert_eq!(
            sha_from_release_tag("main-ba46f81cc9306253958013affa1c916f980e1162"),
            Some("ba46f81cc9306253958013affa1c916f980e1162")
        );
        // `0.1.6` is a hand-cut release from 2026-07-08 whose only asset is months old; admitting
        // it would put a stale commit at a position it does not hold on `main`.
        assert!(sha_from_release_tag("0.1.6").is_none());
        assert!(sha_from_release_tag("main-nothexadecimal").is_none());
        assert!(
            sha_from_release_tag("main-abc").is_none(),
            "too short to compare"
        );
    }

    /// The real release list from 2026-08-25, newest first.
    fn published() -> Vec<String> {
        [
            "ba46f81cc9306253958013affa1c916f980e1162",
            "c57b5c1bf04abcf567e841333032fdaa76f0e2ca",
            "c1adc6c89a49107160897c9e3dd7e1eff1415419",
            "341883cff9845b715c41c729f60480a7663d4a2f",
        ]
        .iter()
        .map(|sha| sha.to_string())
        .collect()
    }

    /// The tip of `main` is quiet, and so is a dirty tree on top of it. Local edits are how the
    /// work gets done -- painting the developer's own screen red permanently is how a warning
    /// stops being read.
    #[test]
    fn the_tip_of_main_is_quiet_even_with_local_edits() {
        assert_eq!(
            standing_against_main("ba46f81cc930", &published()),
            Standing::AtMain
        );
        assert_eq!(
            standing_against_main("ba46f81cc930+dirty", &published()),
            Standing::AtMain
        );
        assert_eq!(Standing::AtMain.opacity_percent(), 1);
        assert!(!Standing::AtMain.is_behind());
    }

    /// Deto's DLL: a published release that is not the newest. This is the one actionable state,
    /// and the only one drawn red.
    #[test]
    fn an_older_published_release_is_behind_main_and_loud() {
        let standing = standing_against_main("c1adc6c89a49", &published());
        assert_eq!(standing, Standing::BehindMain);
        assert!(standing.is_behind());
        assert_eq!(standing.opacity_percent(), 25);
    }

    /// A commit that was never pushed is not behind anything. It is the ordinary state of working
    /// on a branch, and is drawn as quietly as the tip.
    #[test]
    fn an_unpublished_commit_is_local_rather_than_behind() {
        let standing = standing_against_main("0123456789ab", &published());
        assert_eq!(standing, Standing::Local);
        assert!(
            !standing.is_behind(),
            "a local branch must never be painted red"
        );
        assert_eq!(standing.opacity_percent(), 1);
    }

    /// No network, no releases, no checkout: none of these is "behind".
    #[test]
    fn not_knowing_the_release_list_is_never_reported_as_behind() {
        assert_eq!(
            standing_against_main("ba46f81cc930", &[]),
            Standing::Unknown
        );
        assert_eq!(
            standing_against_main(UNKNOWN_SHA, &published()),
            Standing::Unknown
        );
        assert!(!Standing::Unknown.is_behind());
        assert_eq!(Standing::Unknown.opacity_percent(), 1);
    }

    /// A log line whose build field is empty is the failure this module exists to prevent, so
    /// the line must carry a sha on every platform -- including the host, where there is no
    /// module to name.
    #[test]
    fn the_identity_line_always_names_a_build() {
        let line = identity_line();
        assert!(line.starts_with("build git="), "unexpected shape: {line}");
        assert!(
            !line.contains("git= "),
            "the sha field came out empty: {line}"
        );
    }
}
