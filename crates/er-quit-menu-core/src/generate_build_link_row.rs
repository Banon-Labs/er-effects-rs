//! The System>Quit **Generate Build Link** row -- the exact inverse of the row above it.
//!
//! Load Build from URL takes a planner link and rewrites this character. This one takes this
//! character and writes a planner link: it reads the equipped loadout, the memorised spells and the
//! stats, encodes them into a self-contained `?i=` share URL, puts that URL on the clipboard and
//! asks the OS to open it.
//!
//! # Nothing is sent anywhere
//!
//! The planner has two share formats and only one of them is a server record. `?b=<id>` is an id
//! for a build stored on the API; creating one needs an account, which would mean minting a user
//! per player per link on one person's free hobby service. `?i=<payload>` carries the whole build
//! in the URL -- LZUTF8 over base64 over JSON -- and is decoded entirely in the player's browser.
//! This row emits the second. There is no request, no account, and no way for it to fail because
//! someone else's server is down.
//!
//! # Which half runs where
//!
//! The press latches a request and returns; it may arrive on any thread, inside a native
//! `PropertyNewButtonController` action, and reading `PlayerGameData` from there is not allowed.
//! The recurring `FrameBegin` task does the read -- a few dozen native getter calls -- and hands the
//! finished document to a worker, because `ShellExecuteW` spawns `winebrowser`, which spawns
//! `xdg-open`, which spawns a browser, and none of that belongs on the frame.
//!
//! # The row reports on itself
//!
//! Unlike the link field next door, this row opens no dialog, so when the export finishes there is
//! no other surface to say what happened. Its help line is a live buffer that becomes the outcome:
//! how long the link is, whether it was copied, whether a browser took it.

use std::sync::atomic::Ordering;

use er_telemetry_core::counters::{
    SYSTEM_QUIT_GENERATE_BUILD_LINK_ACTION_COUNT, SYSTEM_QUIT_GENERATE_BUILD_LINK_CLIPBOARD_COUNT,
    SYSTEM_QUIT_GENERATE_BUILD_LINK_ENCODED_COUNT, SYSTEM_QUIT_GENERATE_BUILD_LINK_FAILED_COUNT,
    SYSTEM_QUIT_GENERATE_BUILD_LINK_LAST_URL_LEN, SYSTEM_QUIT_GENERATE_BUILD_LINK_OPENED_COUNT,
    SYSTEM_QUIT_GENERATE_BUILD_LINK_REFUSED_COUNT, SYSTEM_QUIT_GENERATE_BUILD_LINK_REQUEST_COUNT,
    SYSTEM_QUIT_GENERATE_BUILD_LINK_STALE_LATCH_COUNT,
};

use crate::build_url_clipboard::set_clipboard_text;
use crate::host::append_autoload_debug;
use crate::row_text::{set_generate_build_link_row_help, wide_z};

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::PCWSTR;

use er_build_import_runtime::export::{self, ExportReport, Sinks};

/// What a row press did.
#[derive(Clone, Debug)]
pub enum GenerateLinkPress {
    /// An export was queued; the FrameBegin task will read the character.
    Started,
    /// A busy latch was found with nothing behind it, cleared, and the press honoured anyway.
    StaleLatchCleared,
    /// An export is genuinely still running.
    Refused(String),
}

impl GenerateLinkPress {
    fn label(&self) -> String {
        match self {
            GenerateLinkPress::Started => "STARTED".to_owned(),
            GenerateLinkPress::StaleLatchCleared => {
                "STARTED after clearing a stale latch".to_owned()
            }
            GenerateLinkPress::Refused(reason) => format!("REFUSED ({reason})"),
        }
    }
}

/// Put the URL on the Windows clipboard. Handed to the runtime as a function pointer so the OS
/// surface stays in the DLL that already declares the `windows` features backing it.
fn clipboard_sink(url: &str) -> bool {
    let copied = set_clipboard_text(url);
    if copied {
        SYSTEM_QUIT_GENERATE_BUILD_LINK_CLIPBOARD_COUNT.fetch_add(1, Ordering::SeqCst);
    }
    copied
}

/// Ask the OS to open the URL.
///
/// Under Proton this reaches the host browser through `winebrowser.exe` -> `xdg-open`. Measured
/// rather than assumed, and measured at LENGTH: `ShellExecuteW` returned success and `xdg-open`
/// received the URL complete and untruncated at 51, 1543, 2043, 2143, 3043 and 5043 characters, so
/// the classic 2083-character `INTERNET_MAX_URL_LENGTH` does not bound this path. That matters
/// because a share link for a fully-kitted character is around three thousand characters.
///
/// A return value of 32 or less is a failure code, not a handle -- that is the documented
/// `ShellExecute` convention and the only thing distinguishing "the browser opened" from "nothing
/// happened at all".
fn open_sink(url: &str) -> bool {
    /// `ShellExecute` returns an HINSTANCE-shaped value; anything <= 32 is an error code.
    const SHELL_EXECUTE_MIN_SUCCESS: isize = 32;

    let operation = wide_z("open");
    let url_w = wide_z(url);
    // Safety: both strings are NUL-terminated and outlive the call; the shell copies what it needs.
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::from_raw(operation.as_ptr()),
            PCWSTR::from_raw(url_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = result.0 as isize;
    let opened = code > SHELL_EXECUTE_MIN_SUCCESS;
    if opened {
        SYSTEM_QUIT_GENERATE_BUILD_LINK_OPENED_COUNT.fetch_add(1, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "system-quit-generate-link: ShellExecuteW(open) returned {code} ({}) for a {}-character URL",
        if opened { "opened" } else { "FAILED" },
        url.chars().count()
    ));
    opened
}

fn sinks() -> Sinks {
    Sinks {
        clipboard: Some(clipboard_sink),
        open: Some(open_sink),
    }
}

/// Handle a confirmed press of the Generate Build Link row.
///
/// A press that finds a busy latch it cannot prove is live clears it and proceeds. That direction
/// is deliberate: the worst case of being wrong this way is a second browser tab, and the worst
/// case of the other way is a row that never works again for the rest of the session. The link
/// field next door went dead for three consecutive presses on exactly that failure.
pub fn system_quit_start_build_export() -> GenerateLinkPress {
    SYSTEM_QUIT_GENERATE_BUILD_LINK_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
    drain_build_export_outcome();

    let mut cleared = false;
    if export::phase() != export::Phase::Idle && export::export_latch_is_stale() {
        SYSTEM_QUIT_GENERATE_BUILD_LINK_STALE_LATCH_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-generate-link: STALE export latch on a press ({}); clearing it and \
             exporting anyway -- a dead latch must not outrank the player",
            export::export_latch_state()
        ));
        export::reset();
        cleared = true;
    }

    match export::request() {
        Ok(()) => {
            SYSTEM_QUIT_GENERATE_BUILD_LINK_REQUEST_COUNT.fetch_add(1, Ordering::SeqCst);
            set_generate_build_link_row_help("Reading this character...");
            if cleared {
                GenerateLinkPress::StaleLatchCleared
            } else {
                GenerateLinkPress::Started
            }
        }
        Err(err) => {
            SYSTEM_QUIT_GENERATE_BUILD_LINK_REFUSED_COUNT.fetch_add(1, Ordering::SeqCst);
            // Only reachable when the latch was proven live, since a stale one was cleared above.
            append_autoload_debug(format_args!(
                "system-quit-generate-link: press refused -- {err} ({})",
                export::export_latch_state()
            ));
            set_generate_build_link_row_help("Still working on the last link...");
            GenerateLinkPress::Refused(err.to_string())
        }
    }
}

/// Log a press outcome, so both routing hooks report it identically.
pub fn system_quit_log_build_export_press(site: &str, press: &GenerateLinkPress) {
    append_autoload_debug(format_args!(
        "system-quit-generate-link: row press at {site} -> {}; the read itself runs on the \
         FrameBegin task and the browser on a worker",
        press.label()
    ));
}

/// Report a finished export, once, on whichever surface is available.
fn drain_build_export_outcome() {
    if let Some(report) = export::take_report() {
        SYSTEM_QUIT_GENERATE_BUILD_LINK_ENCODED_COUNT.fetch_add(1, Ordering::SeqCst);
        SYSTEM_QUIT_GENERATE_BUILD_LINK_LAST_URL_LEN.store(report.url_len, Ordering::SeqCst);
        crate::arm::append_build_row_oracle_line("generate-link-outcome");
        set_generate_build_link_row_help(&row_help_for(&report));
        append_autoload_debug(format_args!(
            "system-quit-generate-link: export complete for {:?} -- {}{}",
            report.character,
            report.summary(),
            if report.unnamed == 0 {
                String::new()
            } else {
                format!(
                    "; {} equipped slot(s) held an item the message repository could not name and \
                     were left OUT rather than guessed at",
                    report.unnamed
                )
            }
        ));
    }
    if let Some(reason) = export::take_error() {
        SYSTEM_QUIT_GENERATE_BUILD_LINK_FAILED_COUNT.fetch_add(1, Ordering::SeqCst);
        set_generate_build_link_row_help("Could not generate a link - see er-build-import.log");
        append_autoload_debug(format_args!(
            "system-quit-generate-link: the export FAILED -- {reason}"
        ));
    }
}

/// The row's help line after a completed export. The clipboard is mentioned first because it is the
/// half the player can act on if the browser did not appear.
fn row_help_for(report: &ExportReport) -> String {
    match (report.clipboard, report.opened) {
        (true, true) => format!("Link copied and opened ({} characters)", report.url_len),
        (true, false) => format!(
            "Link copied ({} characters) - no browser opened, paste it yourself",
            report.url_len
        ),
        (false, true) => format!(
            "Link opened in your browser ({} characters)",
            report.url_len
        ),
        (false, false) => "The link was built but could not be copied or opened".to_owned(),
    }
}

/// One frame of the exporter, driven from the product's recurring `FrameBegin` task.
///
/// Safe to call every frame from boot: the runtime's `tick` returns immediately unless a press has
/// queued a request.
///
/// # Safety
///
/// Game task thread only -- the context the character read requires.
pub unsafe fn system_quit_build_export_tick() {
    // Safety: the caller's contract (FrameBegin game task) carries through.
    unsafe { export::tick(sinks()) };
    drain_build_export_outcome();
}
