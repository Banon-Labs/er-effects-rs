//! Tests for the local invasion filter.
//!
//! In their own file since 2026-09-02, when the parent crossed the 3200-line limit. That is not
//! purely cosmetic: several of these tests read the parent's source with `include_str!` and assert
//! on what is and is not in a particular function's body, and a test that scans the file it lives
//! in finds its own assertion text. That had already bitten twice, which is why the needles below
//! are assembled at runtime rather than written out; from here the scanned file no longer contains
//! them at all.

use super::*;

/// Every option that changes behaviour has to show up in the `config loaded` line.
///
/// Not a style rule -- it is the difference between a hot-reload you can verify and one you can
/// only hope about. Measured cost of the gap, 2026-08-06: `dll_users_only` was toggled mid-run,
/// the line reprinted (so the file had certainly been re-read), and it still did not say what
/// the option was now set to. The A/B could not be confirmed until a `lobby-pool` line happened
/// to appear on the next query. "The config reloaded" is a fact nobody needs; "here is what is
/// in force" is the one they do.
///
/// Checked against the struct's real field list rather than a copy of it, so adding a field and
/// forgetting the log line fails here instead of silently on someone's live run.
#[test]
fn every_behaviour_changing_option_is_named_in_the_config_line() {
    let config_source = include_str!("../../../er-invasion-warp-core/src/local_invasion.rs");
    let start = config_source
        .find("pub struct LocalInvasionConfig")
        .expect("the config struct");
    let body = &config_source[start..];
    let end = body.find("\n}").expect("end of the struct");
    let fields: Vec<&str> = body[..end]
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub "))
        // A field declaration, not the `pub struct ... {` header the scrape starts on: it must
        // carry a type, and its name must be a plain identifier.
        .filter(|rest| rest.contains(':'))
        .filter_map(|rest| rest.split(':').next())
        .map(str::trim)
        .filter(|name| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        .collect();
    assert!(
        fields.len() >= 8,
        "field scrape found only {fields:?} -- the struct's shape changed and this test is no \
         longer reading it"
    );

    let source = include_str!("../local_invasion_filter.rs");
    let line_start = source
        .find("local-invasion: config loaded")
        .expect("the config-loaded log line");
    // The format string plus its whole argument list, bounded by the call's own closing `));`
    // rather than by a character count -- a fixed window silently truncates the moment a
    // comment is added to the argument list, and then this test starts failing on fields that
    // are in fact present.
    let call_end = source[line_start..]
        .find("));")
        .expect("the log call must be closed");
    // Comments are stripped, and the check is for the read itself rather than the bare name.
    // Both guards earn their place: a first version of this test looked for the bare field name
    // anywhere in the call, and a negative control proved it toothless -- the explanatory
    // comment in the argument list mentions `dll_users_only`, so deleting the actual argument
    // left the test passing on prose alone. A gate that cannot fail is worse than no gate,
    // because it is mistaken for coverage.
    let call: String = source[line_start..line_start + call_end]
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    for field in fields {
        // The two keybinds are reported by name rather than by number, so they appear as
        // `key_name(outcome.config.mark_key)` -- still the read, just rendered for a human.
        assert!(
            call.contains(&format!("outcome.config.{field}")),
            "config option `{field}` never reaches the `config loaded` line, so a user who \
             changes it cannot tell from the log whether it took effect"
        );
    }
}

/// The dwell figure exists to be compared against the game's real frame rate; one that rounds
/// to zero on a plausible interval would read as "the task stopped ticking" and send the next
/// reader hunting a hang that never happened.
#[test]
fn a_dwell_at_a_normal_frame_rate_reports_that_frame_rate() {
    // 600 ticks over 10s -- the shape of the measured `0x11` retry dwell at 60fps.
    assert_eq!(implied_fps(600, 10_000), Some(60));
    // The same wall-clock dwell at half the frame rate: Half the ticks. This is the frame-vs-
    // clock discriminator in miniature -- if the retry is a clock, this is what the log shows.
    assert_eq!(implied_fps(300, 10_000), Some(30));
    // The same tick dwell at half the frame rate: twice the wall clock. If the retry is a frame
    // counter, this is what the log shows instead. The two are distinguishable only because
    // both columns are recorded.
    assert_eq!(implied_fps(600, 20_000), Some(30));
}

/// Degenerate intervals must decline to divide rather than emit a misleading number: two
/// transitions inside one tick say nothing about the frame rate.
#[test]
fn an_interval_too_short_to_divide_reports_nothing_rather_than_zero() {
    assert_eq!(implied_fps(0, 10_000), None, "no ticks elapsed");
    assert_eq!(implied_fps(600, 0), None, "no time elapsed");
    assert_eq!(implied_fps(0, 0), None);
}

/// A dwell shorter than a millisecond per tick must not be reported as zero fps -- it is a very
/// fast interval, and zero would read as a stall.
#[test]
fn a_sub_millisecond_dwell_never_reports_as_a_stalled_task() {
    assert_ne!(implied_fps(1, 1), Some(0));
    assert_eq!(implied_fps(1, 1), Some(1000));
}

#[test]
fn the_default_mark_keys_are_insert_and_delete_and_are_distinct_from_the_warp_keys() {
    let defaults = er_invasion_warp_core::local_invasion::LocalInvasionConfig::default();
    assert_eq!(defaults.mark_key, 0x2d, "VK_INSERT");
    assert_eq!(defaults.unmark_key, 0x2e, "VK_DELETE");
    // Sharing a key with the warp driver would make the two pollers eat each other's
    // GetAsyncKeyState "pressed since last call" edge.
    //
    // Only the defaults can be checked here: the keys are configurable now, so a player is free
    // to name a warp key and collide on purpose. That is their choice to make and the log line
    // reports which keys are live, but the shipped defaults must not collide out of the box.
    for warp_key in [
        crate::drive::VK_WARP_NEAREST,
        crate::drive::VK_WARP_NEXT,
        crate::drive::VK_WARP_OTHER_AREA,
    ] {
        assert_ne!(defaults.mark_key, warp_key);
        assert_ne!(defaults.unmark_key, warp_key);
    }
}

/// A player who names a key must be able to see which key is live, or a typo that parsed into
/// some other valid key is indistinguishable from the feature being broken.
#[test]
fn the_configured_keys_render_names_a_player_would_recognise() {
    let defaults = er_invasion_warp_core::local_invasion::LocalInvasionConfig::default();
    assert_eq!(
        er_invasion_warp_core::keybind::key_name(defaults.mark_key),
        "Insert"
    );
    assert_eq!(
        er_invasion_warp_core::keybind::key_name(defaults.unmark_key),
        "Delete"
    );
}

#[test]
fn this_module_installs_exactly_four_detours_and_all_three_seamless_ones_are_read_only() {
    // The budget, made explicit so growing it is a decision rather than a drift:
    //   ORIG_SET_JOIN_DATA    -- the game's SetMultiplayJoinData, where matches are judged.
    //   ORIG_SHOW             -- ersc's menu builder, observation only, because OSM has no
    //                            static to read it out of (see NEXT_OBJECT_OFFSET's docs).
    //   ORIG_BUILD_LOBBY_KEY  -- ersc's lobby-key builder, observation only. Grown from two to
    //                            three deliberately: the key is the single value that decides
    //                            whether two Seamless players can see each other, it exists
    //                            only as a stack `std::string` inside one function, and no
    //                            field holds it afterwards -- so there is nothing to read
    //                            passively and a hook is the only way to observe it.
    //   ORIG_INVADE_ACTION    -- ersc's "Invade world" action, observation only. Grown from
    //                            three to four on 2026-09-08 for a measured reason: `show` runs
    //                            only when the player opens Seamless's menu, and an invasion
    //                            item does not open it. Run `br-20260908-230004-d163` judged 13
    //                            matches with zero `captured Seamless's option-menu object`
    //                            lines, so every one of them ended `NOT cancelled`. This action
    //                            is the seam that sees the object on the item path.
    // The cancel action stays un-hooked: it reads `rcx` only and nothing calls it but us, so
    // calling it with `(OSM, 0, 1, 1)` needs no captured arguments and therefore no detour.
    let source = filter_module_code();
    let orig_slots = source.matches("\nstatic ORIG_").count();
    assert_eq!(orig_slots, 4, "detour budget is four trampolines");
    assert!(source.contains("\nstatic ORIG_SET_JOIN_DATA"));
    assert!(source.contains("\nstatic ORIG_SHOW"));
    assert!(source.contains("\nstatic ORIG_BUILD_LOBBY_KEY"));
    assert!(source.contains("\nstatic ORIG_INVADE_ACTION"));
    assert!(
        !source.contains("static ORIG_CANCEL_ACTION"),
        "the cancel action must stay un-hooked -- we are its only caller, so a detour on it \
         would only ever observe ourselves"
    );
    // The three Seamless seams are observers: each one runs the original with its arguments
    // untouched and alters nothing. The invade one is the newest, so it is the one a future
    // edit is most likely to turn into a driver by accident.
    assert!(
        source.contains("unsafe { core::mem::transmute::<usize, ErscActionFn>(orig)(a, b, c, d) }"),
        "the invade observer must pass every argument through unchanged"
    );
}

/// A session that reads idle while a join is in flight is proven wrong, and must be dropped.
///
/// Measured live, run `br-20260909-000018-d691`: the scan reported `found at 0x309c0038 ... owner
/// 0x0`, the player invaded, and the one rejection came back `cannot cancel (WrongBlock) -- the
/// session is in state 0x1`. The player was mid-search, so the real session read `state_searching`
/// and the cached object could not have been it. Without this the same wrong pointer refuses every
/// rejection for the rest of the run, which is exactly what that run did: one reject, one
/// `NOT cancelled`, the invasion proceeded, no banner.
#[test]
fn a_session_reading_idle_during_a_join_is_dropped_rather_than_waited_out() {
    let source = filter_module_code();
    let body = source
        .split_once("fn cancel_match(")
        .expect("cancel_match exists")
        .1;
    // Anchored on the reading itself rather than on the old refusal arm: the cancel-row check
    // became a report on 2026-09-08 (it was a UI rule refusing a state measured to cancel), and
    // the idle drop is now its own arm just after it.
    let refusal = body
        .split_once("cancel_row_refusal(")
        .expect("the cancel-row reading is still taken")
        .1;
    // Bounded, so a call to the same function anywhere else in `cancel_match` cannot satisfy this.
    // Three earlier gates in this file matched their own assertion text; scoping the window is
    // what stops that class of false pass.
    let arm = &refusal[..refusal.len().min(2_500)];
    assert!(
        arm.contains("JOIN_IN_FLIGHT.load("),
        "the refusal must distinguish an idle session mid-join from one that is merely idle"
    );
    assert!(
        arm.contains("state_idle"),
        "it is specifically the idle reading that proves the pointer wrong"
    );
    assert!(
        arm.contains("session_scan::invalidate_cached_session()"),
        "a pointer proven wrong must be dropped, or every later rejection refuses on the same \
         reading"
    );
    // And the row reading itself must stay a report. It is ERSC's hide-predicate -- when Seamless
    // draws the Cancel row -- while the cancel action at ersc+0x258d0 has no state precondition at
    // all. Obeying it refused `state 0x16` on 2026-09-08, the same state a Frida-driven cancel had
    // succeeded from eight times that evening (22 -> 35 every time, no crash).
    // `arm` already begins after the call, so the row-reading block is everything up to the first
    // line that closes it. Splitting on the call name again finds nothing and would hand back the
    // whole window, which then matches the idle drop's own `return false` -- a false fail this
    // test produced on its first run.
    let row_reading = arm.split_once("\n    }").map_or(arm, |(inside, _)| inside);
    assert!(
        !row_reading.contains("return false"),
        "the cancel-row reading must not refuse: it describes the row Seamless draws, not what \
         the action accepts"
    );
}

/// Turning the filter off must not turn the banner off.
///
/// `enabled = false` means "judge nothing", not "say nothing": the player still wants to be told
/// where the server sent them, stated as the server's choice rather than as something this mod
/// approved. The switch that governs the banner is `reject_notice`, and conflating the two would
/// make a player who only wanted the filter paused go silent as well.
///
/// Asserted structurally, on the order of the two: the announcement has to come before the
/// `enabled` early return, or it can never run in the disabled case.
#[test]
fn disabling_the_filter_leaves_the_arrival_banner_speaking() {
    let source = filter_module_code();
    let judge = source
        .split_once("fn judge_incoming_match(")
        .expect("judge_incoming_match exists")
        .1;
    let disabled = judge
        .split_once("if !config.enabled {")
        .expect("the disabled arm exists")
        .1;
    let arm = &disabled[..disabled.len().min(600)];
    assert!(
        arm.contains("banner::announce_arrival(config.reject_notice"),
        "a disabled filter must still announce the arrival, gated on reject_notice rather than on \
         enabled"
    );
    // And it must be the arrival wording, not a verdict: nothing was judged, so claiming a
    // rejection or a success there would be a lie about what the mod did.
    for verdict in ["announce_rejection", "announce_success"] {
        assert!(
            !arm.contains(verdict),
            "the disabled arm must not claim a verdict it never reached: {verdict}"
        );
    }
}

/// The lobby-key observer must stay an observer. Altering `lobby_key` changes what every other
/// Seamless client's filter matches, which is not this DLL's to do.
///
/// Amended 2026-08-06, deliberately and narrowly. The original banned `SetLobbyData(` and
/// `AddRequestLobbyListStringFilter(` outright, which was broader than its own stated reason:
/// the harm it names is to `lobby_key`, and publishing a separate namespaced key does not touch
/// it. Measured that day: a host publishes 7 keys and an invader filters on 5 of them, and a
/// lobby lacking a filtered key is excluded (baseline 13 lobbies -> 0 with a filter on an
/// unpublished key, reproduced). So one extra key is exactly how a location filter can exist,
/// and it is invisible to vanilla Seamless players -- they never query it, and their own
/// matching is on `lobby_key`, which stays untouched.
///
/// What remains absolute: `lobby_key` itself is never written, and no filter is ever added on
/// `lobby_key` or `lobby_type`. Those two decide who can see whom at all.
#[test]
fn the_lobby_key_is_never_published_or_altered() {
    let code = product_code();
    // The keys that decide mutual visibility. Writing or filtering on either changes what other
    // players match, so they stay banned by name rather than by call.
    for reserved in ["lobby_key", "lobby_type"] {
        assert!(
            !code.contains(&format!("\"{reserved}\"")),
            "{reserved}: this key decides who can see whom -- publishing or filtering on it \
             changes what every other Seamless player matches"
        );
    }
    // And the observer calls the original before reading, so a fault in our read can never
    // change the key Seamless publishes.
    let source = include_str!("../local_invasion_filter.rs");
    let observer = source
        .split("unsafe extern \"system\" fn build_lobby_key_observer")
        .nth(1)
        .expect("the observer exists");
    let body = &observer[..observer.find("\n}\n").unwrap_or(observer.len())];
    let call_at = body.find("ErscActionFn").expect("calls the trampoline");
    let read_at = body
        .find("read_std_string")
        .expect("reads the produced string");
    assert!(
        call_at < read_at,
        "the original must run before the string is read"
    );
}

/// This module's shipping code, with comments and the test module removed.
///
/// Both exclusions are load-bearing and were learned by the guards failing on themselves:
/// * comments, because these names appear throughout the documentation, where describing what
///   Seamless does is the entire point. A check that cannot tell prose from a call site either
///   fails on its own docs or forces the docs to go quiet about the mechanism.
/// * the test module, because a ban list is written in code -- `["lobby_key", ...]` is a string
///   literal, so a guard scanning the whole file trips on the very list that defines it. Both
///   new guards failed exactly that way on first run.
fn product_code() -> String {
    let source = include_str!("../local_invasion_filter.rs");
    let shipping = source
        .split_once("#[cfg(test)]")
        .map_or(source, |(before, _)| before);
    shipping
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The session scanner's shipping code, comments and test module removed.
///
/// A second reader rather than a widened `product_code`, because these guards pin the shape of one
/// function each and a reader that concatenated both files would let a string in either satisfy a
/// guard written for the other.
/// The filter's own source plus the submodules split out of it, as one string.
///
/// Every gate in this file that scans `local_invasion_filter.rs` for a pattern went silently blind
/// the moment a block moved into a submodule -- the needle was simply absent and the assertion
/// read as satisfied, or the `expect` fired against correct code. Both happened on 2026-09-08 when
/// the file crossed the 3200-line limit and `capture_osm` moved to `menu_object.rs`. Concatenating
/// here means a future split costs one line in this function rather than a gate nobody notices.
fn filter_module_code() -> String {
    [
        include_str!("../local_invasion_filter.rs"),
        include_str!("menu_object.rs"),
        include_str!("banner.rs"),
        include_str!("menu_seams.rs"),
    ]
    .join("\n")
}

fn session_scan_code() -> String {
    let source = include_str!("session_scan.rs");
    let shipping = source
        .split_once("#[cfg(test)]")
        .map_or(source, |(before, _)| before);
    shipping
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// No invasion target may ever be chosen by who the other player is.
///
/// # The line, and why it is drawn here rather than left to judgement
///
/// Measured 2026-08-06: a lobby query returns a real candidate set (13 lobbies), ersc picks one
/// (index 14, then 7), and `GetLobbyOwner` / `GetLobbyMemberByIndex` are both called on the
/// result. So selecting a candidate by SteamID is not merely conceivable -- every primitive it
/// needs is already in the process, it needs nothing from the host, and it would work today.
///
/// That is exactly what makes it unacceptable. Its one advantage and its one abuse are the same
/// property: needing nothing from the target. Any consent check would require the target to run
/// this DLL, which removes the advantage entirely, so there is no version of it that is both
/// useful and safe. And a location is somewhere a player chose to stand; an account is the
/// player, everywhere, forever.
///
/// The principle this encodes, which generalises past this one call: filtering may decline, it
/// may never select a person. Declining removes options from ourselves -- the worst case is not
/// invading someone, which the user could do by hand. Selecting imposes on somebody who never
/// opted in. `er_map` passes because a host is findable by location only if that host chose to
/// broadcast it; consent is structural rather than a policy someone can quietly drop.
#[test]
fn no_invasion_target_is_ever_chosen_by_steam_id() {
    let code = product_code();
    // The primitives that turn a candidate set into a named person. Reading an owner to log it
    // would be equally targetable once the value exists, so the call itself is the line.
    for banned in [
        "GetLobbyOwner",
        "GetLobbyMemberByIndex",
        "GetNumLobbyMembers",
        "GetLobbyByIndex",
    ] {
        assert!(
            !code.contains(&format!("{banned}(")),
            "{banned}: choosing a lobby by who is in it targets a person who never opted in -- \
             filtering may decline, it may never select a person"
        );
    }
    // And no allowlist of accounts, however it is spelled.
    for banned in [
        "steam_id_allow",
        "steamid_allow",
        "target_steam_id",
        "friend_steam_id",
    ] {
        assert!(
            !code.contains(banned),
            "{banned}: an account allowlist is person-targeting with extra steps"
        );
    }
}

/// The watched window must actually cover the fields the static read identified, or a VM write
/// to one of them is invisible and the whole point of the tracing is lost.
#[test]
fn the_session_watch_window_covers_every_known_field() {
    let begin = SESSION_WATCH_BEGIN;
    let end = SESSION_WATCH_BEGIN + SESSION_WATCH_WORDS * 8;
    // Every build's state field, not just the installed one's: the window is a compile-time
    // constant and the same code traces whichever build is loaded, so a window that covers
    // a stale build's state offset but not the supported one's would trace nothing at all.
    for abi in ersc::SUPPORTED {
        assert!(
            abi.session_state_offset >= begin && abi.session_state_offset < end,
            "{}'s state field at {:#x} is outside the watched window {begin:#x}..{end:#x}",
            abi.version,
            abi.session_state_offset,
        );
        assert!(
            abi.session_guard_offset >= begin && abi.session_guard_offset < end,
            "{}'s guard field at {:#x} is outside the watched window {begin:#x}..{end:#x}",
            abi.version,
            abi.session_guard_offset,
        );
    }
    for (name, offset) in [
        ("lobby id", 0x178),
        ("lobby owner", 0x180),
        ("offer block start", 0x190),
        ("seek flag", 0x1d4),
        ("seek latch", 0x1f0),
        ("offer block end", 0x220),
        ("lobby-key flag byte", 0x229),
    ] {
        assert!(
            offset >= begin && offset < end,
            "{name} at {offset:#x} is outside the watched window {begin:#x}..{end:#x}"
        );
    }
}

/// The window is read every frame, so it must stay small enough to be free.
#[test]
fn the_session_watch_window_stays_small() {
    const {
        assert!(
            SESSION_WATCH_WORDS * 8 <= 0x200,
            "a per-frame read of this size is no longer negligible"
        )
    };
}

/// The digest is SHA-256 hex. A probe that expected the 16-character `%016llX` intermediate
/// would print a truncation of the wrong string and answer "did the key change" with noise --
/// which is the one question this probe exists to answer.
#[test]
fn the_lobby_key_is_a_sha256_hex_digest_not_the_sixteen_char_intermediate() {
    assert_eq!(ersc::LOBBY_KEY_HEX_LEN, 64);
    assert_ne!(ersc::LOBBY_KEY_HEX_LEN, 16);
}

/// `mov dword ptr [rdi + displacement], immediate` -- `C7 /0` with a disp32 on `rdi`.
///
/// The option-action pins run through the state write, so this is how a test asks "does the
/// pin actually encode the offset and the code this [`ersc::Abi`] claims". It is the check
/// that keeps the table honest: the bytes come from `build.rs` and were ground-truthed against
/// a real copy of that Seamless build, so a hand-edited field number stops agreeing with them.
fn mov_dword_rdi(displacement: usize, immediate: u32) -> Vec<u8> {
    let mut bytes = vec![0xc7, 0x87];
    bytes.extend_from_slice(&(displacement as u32).to_le_bytes());
    bytes.extend_from_slice(&immediate.to_le_bytes());
    bytes
}

/// `cmp dword ptr [rdi + displacement], imm8` -- `83 /7`, the form both builds use for the
/// small idle sentinel.
fn cmp_dword_rdi_imm8(displacement: usize, immediate: u8) -> Vec<u8> {
    let mut bytes = vec![0x83, 0xbf];
    bytes.extend_from_slice(&(displacement as u32).to_le_bytes());
    bytes.push(immediate);
    bytes
}

/// `cmp dword ptr [rdi + displacement], imm32` -- `81 /7`, used for the guard poison value,
/// which does not fit an imm8.
fn cmp_dword_rdi_imm32(displacement: usize, immediate: u32) -> Vec<u8> {
    let mut bytes = vec![0x81, 0xbf];
    bytes.extend_from_slice(&(displacement as u32).to_le_bytes());
    bytes.extend_from_slice(&immediate.to_le_bytes());
    bytes
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Every declared offset and state code must appear, encoded, inside the pin it describes.
///
/// This is the load-bearing test of the whole re-pin. The pins are generated by `build.rs` from
/// named instructions and verified there against a real copy of each Seamless build, located by
/// the version string inside the file. So if `session_state_offset` or `state_searching` is
/// ever edited without re-measuring, the number stops matching the bytes and this fails --
/// rather than the module quietly writing a renumbered code into a neighbouring field of a
/// live multiplayer session.
#[test]
fn every_abi_number_is_encoded_in_the_bytes_that_were_measured_for_it() {
    for abi in ersc::SUPPORTED {
        let invade = abi.invade_prologue;
        let cancel = abi.cancel_prologue;
        assert!(
            contains(
                invade,
                &mov_dword_rdi(abi.session_state_offset, abi.state_searching)
            ),
            "{}: the invade pin does not write {:#x} to S+{:#x}",
            abi.version,
            abi.state_searching,
            abi.session_state_offset,
        );
        assert!(
            contains(
                cancel,
                &mov_dword_rdi(abi.session_state_offset, abi.state_cancelling)
            ),
            "{}: the cancel pin does not write {:#x} to S+{:#x}",
            abi.version,
            abi.state_cancelling,
            abi.session_state_offset,
        );
        assert!(
            contains(
                invade,
                &cmp_dword_rdi_imm8(abi.session_state_offset, abi.state_idle as u8)
            ),
            "{}: the invade pin does not gate on S+{:#x} == {:#x}",
            abi.version,
            abi.session_state_offset,
            abi.state_idle,
        );
        for (name, pin) in [("invade", invade), ("cancel", cancel)] {
            assert!(
                contains(
                    pin,
                    &cmp_dword_rdi_imm32(abi.session_guard_offset, ersc::SESSION_GUARD_POISON)
                ),
                "{}: the {name} pin does not check the guard at S+{:#x}",
                abi.version,
                abi.session_guard_offset,
            );
        }
    }
}

/// One row of [`the_pinned_abis_are_the_ones_measured_out_of_the_two_seamless_builds`], named
/// rather than left as a four-element tuple so the numbers say which is which.
struct Measured {
    version: &'static str,
    /// `show`, invade action, cancel action, `BuildLobbyKey`.
    rvas: [usize; 4],
    /// Session state, session guard.
    fields: [usize; 2],
    /// Idle, searching, cancelling, offer-received.
    states: [u32; 4],
}

/// The exact addresses and numbers, so a change to any of them is a deliberate edit to a test
/// that says where its value came from -- not a silent drift.
#[test]
fn the_pinned_abi_is_the_one_measured_out_of_the_supported_seamless_build() {
    // The supported build, measured in place -- see this module's `ersc` docs for what
    // identifies each address; none was taken from a byte match alone. There is one build here
    // and there is never more than one: a build that is no longer latest leaves nothing behind.
    let expected = [Measured {
        version: "2.0.1",
        rvas: [0x2_41a0, 0x2_5850, 0x2_58d0, 0xa_d6e0],
        fields: [0x150, 0x14c],
        states: [0x01, 0x0e, 0x23, 0x13],
    }];
    assert_eq!(ersc::SUPPORTED.len(), expected.len());
    for (
        abi,
        Measured {
            version,
            rvas,
            fields,
            states,
        },
    ) in ersc::SUPPORTED.iter().zip(&expected)
    {
        assert_eq!(&abi.version, version);
        assert_eq!(
            [
                abi.show_rva,
                abi.invade_action_rva,
                abi.cancel_action_rva,
                abi.build_lobby_key_rva
            ],
            *rvas,
            "{version} addresses"
        );
        assert_eq!(
            [abi.session_state_offset, abi.session_guard_offset],
            *fields,
            "{version} session fields"
        );
        assert_eq!(
            [
                abi.state_idle,
                abi.state_searching,
                abi.state_cancelling,
                abi.state_offer_received
            ],
            *states,
            "{version} state codes"
        );
    }
}

/// Each pin has to identify a function rather than a code shape.
///
/// The cross-build half of this test went with the retired table -- there is no second build to
/// tell this one apart from any more. What remains is the property the code still depends on, and
/// the one the original fourteen-byte pins actually failed: five different option actions share
/// those fourteen bytes, so a pin that stops near them proves only that some option action lives
/// at the address. The measurement that proves each pin unique needs the DLL and so cannot run
/// here; what runs here is the shape requirement that makes such a pin possible.
#[test]
fn the_version_discriminator_actually_discriminates() {
    const SHARED_OPTION_ACTION_OPENING: usize = 14;
    for abi in ersc::SUPPORTED {
        assert!(
            abi.invade_prologue.len() > SHARED_OPTION_ACTION_OPENING * 3,
            "{}: a pin that stops near the shared opening identifies a code shape, not a function",
            abi.version
        );
        // Within one build the four entry points must be four different addresses.
        let mut rvas = [
            abi.show_rva,
            abi.invade_action_rva,
            abi.cancel_action_rva,
            abi.build_lobby_key_rva,
        ];
        rvas.sort_unstable();
        let unique = rvas.windows(2).all(|pair| pair[0] != pair[1]);
        assert!(unique, "{}: two entry points share an address", abi.version);
        // And invade must be distinguishable from cancel, which the old fourteen-byte pins
        // were not -- they were byte-identical.
        assert_ne!(
            abi.invade_prologue, abi.cancel_prologue,
            "{}: invade and cancel must not share a pin",
            abi.version
        );
    }
}

/// The refusal is fail-closed and says which builds it knows, so an unrecognised Seamless
/// produces an actionable line rather than a filter that silently does nothing.
#[test]
fn an_unrecognised_seamless_build_is_refused_rather_than_guessed_at() {
    let source = include_str!("../local_invasion_filter.rs");
    let resolver = source
        .split_once("fn resolve_ersc_abi(")
        .expect("the version gate exists")
        .1
        .split_once("\n}")
        .expect("gate body")
        .0;
    // Exactly one match. Zero is an unknown build; two would mean the discriminator does not
    // discriminate. Both refuse.
    assert!(
        resolver.contains("ambiguous"),
        "a build matching two ABIs must be refused, not resolved to whichever came first"
    );
    assert!(
        resolver.contains("ABI_REFUSED"),
        "the refusal has to be remembered, or it is re-reported every tick"
    );
    assert!(
        resolver.contains("invade_prologue"),
        "the discriminator must be the invade action -- `show` is byte-identical between the \
         builds AND is hooked by this module"
    );
    assert!(
        !resolver.contains("show_prologue"),
        "`show` cannot discriminate: it is the same code at a different address in both builds"
    );
    // Every refusal path has to leave the caller with nothing to call.
    assert!(
        resolver.contains("None"),
        "an unrecognised build must yield no ABI at all"
    );
}

#[test]
fn capturing_the_menu_object_is_not_gated_on_the_seamless_tag() {
    // Regression, 2026-08-05. `show_observer` used to store OSM only if `+0x68` held the ASCII
    // `seamless` tag. The tag had been measured once in one live frida session; as a
    // precondition it never matched, OSM was never stored, and the feature failed exactly
    // where it was supposed to work -- the live log read `REJECT ...` immediately followed by
    // `cannot cancel -- session is not resolvable`. A single observation is evidence, not a
    // gate. The tag is now a diagnostic string and nothing branches on it.
    let source = filter_module_code();
    // Both observers hand their pointer to the same helper, so the gate could only reappear
    // there. Checking the helper covers `show` and the invade action at once.
    let observer = source
        .as_str()
        .split_once("fn capture_osm(")
        .expect("capture_osm exists")
        .1
        .split_once("\n}")
        .expect("helper body")
        .0;
    assert!(
        observer.contains("OSM.swap(osm,"),
        "the helper must store the pointer it was handed"
    );
    // The tag is still read, once, as a diagnostic in the log line that follows the store -- so
    // the property to assert is order, not absence. Nothing before the store may consult it.
    let before_store = observer
        .split_once("OSM.swap(osm,")
        .expect("the store is in the helper")
        .0;
    assert!(
        !before_store.contains("osm_tag_matches"),
        "storing OSM must not be conditional on the tag"
    );
    // And the resolver must validate the shape it actually depends on instead.
    let resolver = source
        .split_once("fn resolve_session(")
        .expect("resolve_session exists")
        .1
        .split_once("\n}")
        .expect("resolver body")
        .0;
    assert!(
        !resolver.contains("osm_tag_matches"),
        "no tag gate in the resolver either"
    );
    assert!(
        resolver.contains("read_session_state"),
        "validate the state field instead"
    );
}

#[test]
fn self_recovery_cannot_resume_a_search_after_a_kept_match() {
    // The hazard, and why this is a test rather than a comment. Restarting whenever the session
    // is idle is what makes the loop survive a Seamless stall -- but a successful invasion
    // unwinds through exactly the same states as a failed one. Measured 2026-08-06: `KEEP` was
    // followed by `0x15 -> 0x22 -> 0x23 -> 0x00 IDLE`, identical to a rejection. So idle alone
    // cannot tell "the attempt died" from "you are standing in their world", and a restart on
    // the latter would yank the player back out into a search they never asked for.
    //
    // What separates them is that `Verdict::Keep` DISARMS the loop. That makes the disarm check
    // load-bearing rather than incidental, which is precisely the kind of thing a later tidy-up
    // reorders without noticing.
    let source = include_str!("../local_invasion_filter.rs");
    let recovery = source
        .split_once("fn arm_self_recovery(")
        .expect("self-recovery exists")
        .1
        .split_once("\n}")
        .expect("self-recovery body")
        .0;
    assert!(
        recovery.contains("AUTO_SEARCH_ARMED"),
        "self-recovery must consult the armed flag -- it is the ONLY thing distinguishing a \
         dead attempt from a successful invasion, both of which sit at idle"
    );
    let armed_at = recovery.find("AUTO_SEARCH_ARMED").expect("checked above");
    let invade_at = recovery.find("PENDING_REINVADE.store(true");
    assert!(
        invade_at.is_none_or(|at| armed_at < at),
        "the armed check must come BEFORE arming a restart, or a kept match restarts once \
         before the guard is consulted"
    );
    // And the disarm on Keep is the other half of the same invariant.
    let keep = source
        .split_once("Verdict::Keep(reason) => {")
        .expect("keep arm exists")
        .1;
    assert!(
        keep[..keep.find("Verdict::Reject").unwrap_or(keep.len())]
            .contains("AUTO_SEARCH_ARMED.store(false"),
        "a kept match must disarm the loop; self-recovery relies on it"
    );
}

#[test]
fn the_stall_detector_never_times_the_searching_state() {
    // Searching means "nobody has matched yet" and is unbounded by nature -- three consecutive
    // live queries on 2026-08-06 returned 0, 0 and 1 lobbies. Timing it would cancel a healthy
    // search in a quiet bracket, which is the single most obvious way to get a stall detector
    // wrong. The rule lives in `stall_watchdog::is_transient`; this pins that the caller does
    // not reintroduce a timer of its own alongside it.
    let source = include_str!("../local_invasion_filter.rs");
    let watcher = source
        .split_once("fn watch_for_stall(")
        .expect("stall watcher exists")
        .1
        .split_once("\n}")
        .expect("watcher body")
        .0;
    assert!(
        !watcher.contains("SESSION_STATE_SEARCHING"),
        "the watcher must not special-case SEARCHING -- the detector decides what is timed, \
         and it deliberately never times a state that is unbounded by nature"
    );
    assert!(
        watcher.contains("observe("),
        "the watcher feeds the detector rather than deciding staleness itself"
    );
    // The regression this pins, 2026-08-06: the watchdog cancelled a match it had just kept,
    // five seconds after accepting it, because the session dwells in 0x15 while loading into
    // the host's world. No choice of timed states fixes that -- a successful join walks 0x22
    // and 0x23 exactly like a cancel does -- so the arming gate is the whole defence.
    let armed_at = watcher
        .find("AUTO_SEARCH_ARMED")
        .expect("the watcher must only run while the hunt is armed");
    let observe_at = watcher.find("observe(").expect("checked above");
    assert!(
        armed_at < observe_at,
        "the armed check must come BEFORE any observation, or a kept match is timed as a \
         stalled handshake and the invasion is cancelled out from under the player"
    );
}

#[test]
fn the_auto_search_arms_on_the_invade_transition_not_on_merely_being_busy() {
    // Regression, 2026-08-05, caught live. Arming used to be "state != idle, so a search must
    // be running". That made standing down when the menu opened useless: you open the menu
    // during a search, the loop stands down, and one frame later the session is still non-idle
    // so it re-arms. The log read `stood down` and then `0x11 -> 0x0d` with another automatic
    // restart immediately after.
    //
    // Arming now keys on the transition into searching, which is sound because the static scan
    // found `S+0x110 = 0x0d` written at exactly one site in the whole unpacked .text.
    let source = include_str!("../local_invasion_filter.rs");
    // Assembled, not written out: a test that scans its own file finds its own assertion text.
    // That has now bitten twice in this module, so every needle here is built at runtime.
    assert!(
        !source.contains(&format!("fn observe_{}", "user_search")),
        "the not-idle heuristic must stay gone, not sit alongside the replacement"
    );
    let tracer = source
        .split_once("fn trace_session_state(")
        .expect("tracer exists")
        .1
        .split_once("\n}")
        .expect("tracer body")
        .0;
    assert!(
        tracer.contains("state_searching") && tracer.contains("AUTO_SEARCH_ARMED"),
        "arming belongs on the SEARCHING transition"
    );
    assert!(
        !tracer.contains("!= abi.state_idle") && !tracer.contains("!= session.abi.state_idle"),
        "not-idle must never again stand in for a search having been started"
    );
    // Our own restart writes the same value the option does, so it has to be claimed first or
    // the tracer credits the user for it.
    assert!(source.contains("fn note_state_after_our_action"));
    let reinvade = source
        .split_once("fn drive_pending_reinvade(")
        .expect("re-invade exists")
        .1
        .split_once("\n}")
        .expect("re-invade body")
        .0;
    assert!(
        reinvade.contains("note_state_after_our_action"),
        "an unclaimed restart is indistinguishable from the user pressing Invade world"
    );
}

#[test]
fn starting_or_ending_an_invasion_attempt_invalidates_the_pin_cache() {
    // The coupling that makes the dim appear on a map that is already open. `restyle_live_pins`
    // early-returns when this signature is unchanged, so without the attempt state mixed in the
    // pins would keep their idle frames for the whole search and only dim on the next map open
    // -- i.e. never, for the player who noticed by trying to click one.
    //
    // Restores the latch afterwards: it is process-global and every other test in this binary
    // reads it through the same accessor.
    let restore = er_invasion_warp_core::warp::invasion_attempt_in_flight();
    er_invasion_warp_core::warp::set_invasion_attempt_in_flight(false);
    let idle = pin_choice_signature();
    er_invasion_warp_core::warp::set_invasion_attempt_in_flight(true);
    let searching = pin_choice_signature();
    er_invasion_warp_core::warp::set_invasion_attempt_in_flight(false);
    let idle_again = pin_choice_signature();
    er_invasion_warp_core::warp::set_invasion_attempt_in_flight(restore);
    assert_ne!(
        idle, searching,
        "an attempt starting must invalidate the pin cache or the live map never dims"
    );
    assert_eq!(
        idle, idle_again,
        "and it must go back, or the map would never un-dim"
    );
}

#[test]
fn nothing_writes_one_icon_id_across_every_param_row() {
    // Regression, 2026-08-05, caught live. The map's param rows are built with a per-location
    // icon and were then re-stamped with a single id over every row, so the tiers were
    // computed correctly (`chosen=3` then `chosen=96` in the log as marks were added) and then
    // flattened before anything rendered. Every write of the icon field must come from the
    // per-appearance helper.
    let source = include_str!("../map_hooks.rs");
    assert!(
        source.contains("invasion_pin_icon_id_for("),
        "the map must choose icons per location"
    );
    // Anchored on a string unique to the re-stamp. `PARAM_ICON_ID_OFFSET;` is not: an earlier
    // `use` of the same constant matched first and put the window over unrelated code.
    let restamp = source
        .split_once("let stamp_signature")
        .expect("re-stamp block")
        .1;
    let restamp = &restamp[..restamp.len().min(2_000)];
    assert!(
        !restamp.contains(&format!("invasion_pin_icon_id{}", "(")),
        "the re-stamp must not use the single-icon helper -- that is what flattened the tiers"
    );
    assert!(
        restamp.contains("for (index, row) in rows.iter_mut().enumerate()"),
        "the re-stamp has to be per row, with the index that identifies the location"
    );
}

#[test]
fn the_recurring_build_fingerprint_never_reads_a_function_this_module_hooks() {
    // Regression, 2026-08-05. `resolve_session` fingerprinted ersc.dll by comparing `show`'s
    // opening bytes -- and this module hooks `show`. MinHook overwrote those bytes with its
    // jump, so from the first install onward the check compared Seamless against our own
    // detour, failed, and reported `ErscUnrecognised`. A live invasion was judged, rejected,
    // and then not cancelled because of it. Whatever the recurring check reads must be
    // something nothing patches.
    let source = include_str!("../local_invasion_filter.rs");
    let resolver = source
        .split_once("fn resolve_session(")
        .expect("resolve_session exists")
        .1
        .split_once("\n}")
        .expect("resolver body")
        .0;
    // No prologue at all. The fingerprint moved off code entirely on 2026-09-08 after being
    // broken a third time: `show` (our detour, 2026-08-05), then `invade` (our detour), then
    // `cancel` (a Frida Interceptor attached beside us for twenty minutes), each time reading a
    // trampoline and reporting `ErscUnrecognised` on a live rejection. Any function may be hooked
    // by anyone, so naming a currently-unhooked one just schedules the next occurrence.
    for prologue in ["show_prologue", "invade_prologue", "cancel_prologue"] {
        assert!(
            !resolver.contains(prologue),
            "the recurring fingerprint must not read {prologue} -- any function's bytes can be \
             replaced by a hook, ours or someone else's"
        );
    }
    assert!(
        resolver.contains("module_identity_holds("),
        "fingerprint the mapped file rather than the state of its code"
    );
    // And that check must read the PE header rather than reaching back to a prologue.
    let identity = source
        .split_once("fn module_identity_holds(")
        .expect("module_identity_holds exists")
        .1;
    let identity = &identity[..identity.len().min(2_000)];
    assert!(
        identity.contains("PE_TIME_DATE_STAMP") && identity.contains("PE_SIZE_OF_IMAGE"),
        "the identity must come from PE-header fields, which no hook rewrites"
    );
    assert!(
        !identity.contains("_prologue"),
        "the identity check must not fall back to reading code"
    );
}

#[test]
fn the_ersc_action_prologues_are_the_bytes_read_out_of_the_shipped_dlls() {
    // Read from the shipped Seamless DLL in place. If these ever need changing, re-read the
    // DLL; do not adjust them to make a hook install.
    for abi in ersc::SUPPORTED {
        for (name, pin) in [
            ("invade", abi.invade_prologue),
            ("cancel", abi.cancel_prologue),
        ] {
            assert_eq!(
                &pin[..4],
                &[0xf3, 0x0f, 0x1e, 0xfa],
                "{}: the {name} action opens with endbr64",
                abi.version
            );
        }
        // The two functions this module does not call open with the eight callee-saved pushes
        // instead, so a pin swapped between the two groups fails here rather than at runtime.
        for (name, pin) in [
            ("show", abi.show_prologue),
            ("lobby-key builder", abi.build_lobby_key_prologue),
        ] {
            assert_ne!(
                &pin[..4],
                &[0xf3, 0x0f, 0x1e, 0xfa],
                "{}: {name} has no endbr64",
                abi.version
            );
            assert_eq!(
                &pin[..2],
                &[0x55, 0x41],
                "{}: {name} pushes rbp, r15",
                abi.version
            );
        }
    }
}

/// The address that broke the filter must not be accepted as a session, and the one that is a real
/// session must still be.
///
/// # This is a regression test with a date and a log line behind it
///
/// 2026-09-06, live run `br-20260906-212236-d9b7`: `scan_for_session` reported
/// `session resolved WITHOUT hooking Seamless -- found at 0x3dfadb ... owner 0x0`. Read back out of
/// the running process through `/proc/<pid>/mem`, `0x3dfadb` is a three-byte-misaligned window into
/// a table of Wine pointers whose bytes at `+0x150` read `01 00 00 00` -- this build's `state_idle`
/// exactly, and permanently. The run judged four invasions, rejected all four, cancelled none, and
/// logged `ersc=0x01` on all 53 `join-progress` samples because the "session state" it was reading
/// was a fragment of somebody else's pointer.
///
/// The positive case matters as much as the negative one: the check must be weak enough that it
/// cannot reject a real session. `0x1801b2560` is the session this module actually resolved on this
/// machine, recorded in `scan_for_session`'s own comment.
#[test]
fn the_measured_false_positive_is_not_a_plausible_session_pointer() {
    assert!(
        !plausible_session_pointer(0x3d_fadb),
        "0x3dfadb is not 8-aligned, so it cannot be an object carrying a mutex and pointers -- \
         accepting it cost a live run every cancellation it should have made"
    );
    assert!(
        plausible_session_pointer(0x1_801b_2560),
        "the session this module really resolved must still pass, or the gate has replaced a false \
         positive with a false negative"
    );
    // The null page and the low reservations, which no allocation reaches.
    assert!(!plausible_session_pointer(0));
    assert!(!plausible_session_pointer(0x150));
    // Aligned but too low is still refused; the alignment check is not the only one.
    assert!(!plausible_session_pointer(0x8000));
    // Every misalignment is refused, not merely the one that was measured.
    for offset in 1..8usize {
        assert!(
            !plausible_session_pointer(0x1_801b_2560 + offset),
            "a session pointer off by {offset} byte(s) is not a session pointer"
        );
    }
}

/// The identity check must consult the pointer before it consults the field.
///
/// `identifies_a_session` answers "is this object a session", and until 2026-09-06 it only ever
/// looked at what `+0x150` contained. Three false positives were each answered by narrowing the
/// permitted values -- zero, then any byte, then the four reversed codes -- and the third one
/// proved the value was never the whole question, because the address it came from could not have
/// held an object at all. If this call is ever dropped the scan is back to matching coincidences.
#[test]
fn the_session_identity_check_rejects_implausible_pointers_first() {
    let code = product_code();
    let body = code
        .split_once("fn identifies_a_session(")
        .expect("identifies_a_session must exist")
        .1;
    let body = body.split_once("\n}").expect("a function body").0;
    assert!(
        body.contains("plausible_session_pointer("),
        "identifies_a_session must gate on the pointer as well as the state field, or a misaligned \
         fragment of unrelated memory reading 0x01 is a session again"
    );
}

/// The identity check must ask for the session's mutex, not just its state field.
///
/// # The run that bought this
///
/// br-20260908-193258-27ba: the scan resolved `0x860f90f8`, the filter judged one match, and
/// `ersc_owner_or_refuse` then declined to cancel it because `_Type` at `session+0x100` was not a
/// shape MSVC's mutex constructors write. The refusal was correct and it came too late --
/// `cached_scan_for_session` had already latched that pointer, and it revalidates through this
/// same function, so the wrong answer held for the whole process and the rejected invasion
/// proceeded.
///
/// The discriminator existed; it ran at the wrong end of the run. This pins it to the scan.
#[test]
fn the_session_identity_check_requires_a_mutex_where_a_session_carries_one() {
    let code = product_code();
    let body = code
        .split_once("fn identifies_a_session(")
        .expect("identifies_a_session must exist")
        .1;
    let body = body.split_once("\n}").expect("a function body").0;
    assert!(
        body.contains("mutex_shape_identifies_a_session("),
        "identifies_a_session must require an _Mtx_internal_imp_t at session+0x100 -- four small \
         integers at known offsets is a signature the scan has already false-positived on four \
         times, and the mutex is the check that caught the fourth one after the fact"
    );
}

/// Judging a match must not throw away the session that would cancel it.
///
/// # The run that bought this
///
/// br-20260908-212740-ea7c is the first run in which the drive worked end to end: the session
/// resolved at `0x55080038`, and ERSC's own cancel executed through a synthesized owner, moving
/// the state `0x0e SEARCHING -> 0x23 CANCELLING`. The very next rejection -- `reject 0x3c2b1f00
/// (WrongBlock)` -- then reported `MenuNeverOpened` and let the invasion proceed.
///
/// Nothing about Seamless changed in between. `judge_incoming_match` called
/// `invalidate_cached_session()` two lines before it judged, so the session was cleared by the
/// arrival of the event that needed it, and the sweeper that would replace it runs on a fifteen
/// second interval on another thread. The cache is revalidated on every read by
/// `cached_scan_for_session`, so a dead session is already discarded on evidence; clearing it
/// here only ever discarded a live one.
#[test]
fn judging_a_match_keeps_the_session_it_will_need_to_cancel_with() {
    let code = product_code();
    let body = code
        .split_once("fn judge_incoming_match(")
        .expect("judge_incoming_match must exist")
        .1;
    let body = body.split_once("\nfn ").expect("a function body").0;
    assert!(
        !body.contains("invalidate_cached_session("),
        "judge_incoming_match must not clear the cached session -- the rejection it is about to \
         make is the one thing that session exists to enforce, and the sweeper takes seconds to \
         find another while the join takes ten"
    );
}

/// The join-in-flight idle refusal is a discovery rule and must not be applied to retention.
///
/// # Why the same predicate cannot answer both questions
///
/// While a join is in flight the real session is never idle, which is a sound way to shrink a
/// haystack of a million candidates: `identifies_a_session` uses it to refuse the ocean of memory
/// that merely happens to hold `0x01`. Applied to the pointer already in hand it inverts --
/// br-20260908-212740-ea7c drove ERSC's cancel through the cached session and then could not
/// resolve it for the next rejection, because the join that made the cancel necessary is what
/// made the rule refuse the pointer that would have performed it.
///
/// So the caller says which question it is asking. If `discovering` ever stops gating this, the
/// cached session becomes unresolvable for the whole duration of every match.
#[test]
fn the_idle_refusal_applies_only_while_choosing_a_new_candidate() {
    let code = product_code();
    let body = code
        .split_once("fn identifies_a_session(")
        .expect("identifies_a_session must exist")
        .1;
    let body = body.split_once("\n}").expect("a function body").0;
    assert!(
        body.contains("if discovering && JOIN_IN_FLIGHT.load("),
        "the idle refusal must be gated on `discovering`, or revalidating the cached session \
         fails for exactly as long as a match is in flight"
    );
    let scan = session_scan_code();
    assert!(
        scan.contains("identifies_a_session(abi, session, false)"),
        "cached_scan_for_session revalidates an answer it already holds, so it must ask the \
         retention question, not the discovery one"
    );
    assert!(
        scan.contains("identifies_a_session(abi, candidate, true)"),
        "the sweep is choosing between candidates, so it must ask the discovery question"
    );
}

/// The first reading of a session must not arm the auto-search loop.
///
/// # The run that bought this
///
/// br-20260908-212740-ea7c resolved a session whose very first read was `0x0e SEARCHING`. The
/// tracer treated that as a transition, logged "you started a search", and armed the hunt --
/// before the player had used an invasion item. Five seconds later the stall watchdog called the
/// handshake stuck and cancelled it, parking the session at `0x23`, which is outside the set
/// ERSC's hide-predicate draws its Cancel row for. So the genuine rejection later in that run
/// could not have been enforced even with a session in hand.
///
/// `usize::MAX` is the sentinel `LAST_SESSION_STATE` starts at, and it separates "this is what the
/// session reads" from "this is what the session just became".
#[test]
fn the_first_reading_of_a_session_is_a_baseline_not_a_search_the_player_started() {
    let code = product_code();
    let body = code
        .split_once("fn trace_session_state(")
        .expect("trace_session_state must exist")
        .1;
    let body = body.split_once("\n}").expect("a function body").0;
    assert!(
        body.contains("previous != usize::MAX"),
        "arming the hunt must require a real transition -- armed off a first reading, the stall \
         watchdog cancels a search the player never started and parks the session outside the \
         states ERSC will cancel from"
    );
}

/// The scan's mutex signature must reject a bare `_Mtx_plain`.
///
/// # Why this is stricter than the refusal it shares code with
///
/// `lock_shape_refusal` lets `_Mtx_plain` through, correctly: `mtx_do_lock` cannot report a plain
/// mutex busy, so it is no reason to decline an action. As a signature it is worthless -- the
/// value is `1`.
///
/// Live proof, run br-20260908-200726-15d4. With the budget finally covering all of ersc.dll the
/// scan reached `ersc+0x62c908` and accepted the session `0x451200`; read back out of the running
/// process, that address is a table with stride `0x50` whose every boundary holds
/// `01 00 00 00 00 00 00 00`. The state field sits at `+0x150` and the mutex at `+0x100` -- one
/// stride apart -- so the state check and the mutex check read the same repeating `1` and
/// corroborated each other about an object that is neither a session nor a mutex. Its `_Count`
/// reads `0x6fff`.
#[test]
fn the_scans_mutex_signature_rejects_a_bare_plain_type() {
    let code = include_str!("lock_report.rs");
    let body = code
        .split_once("fn mutex_shape_identifies_a_session(")
        .expect("the scan's mutex signature must exist")
        .1;
    let body = body.split_once("\n}").expect("a function body").0;
    assert!(
        body.contains("(kind & MTX_TRY) == 0"),
        "the scan's signature must require the _Mtx_try bit MSVC's std::mutex constructor writes; \
         accepting _Mtx_plain accepts the literal value 1, which is what a stride-0x50 table of \
         ones already defeated once"
    );
    assert!(
        body.contains("MTX_COUNT_OFFSET"),
        "the signature must also read _Count -- a non-recursive std::mutex only goes 0 -> 1, and \
         the object that defeated the previous version read 0x6fff there"
    );
}

/// The scan's budget has to cross the module it is scanning.
///
/// Measured from the installed Seamless v2.0.1 PE section table: the writable sections total
/// `0xafa0a5` = 10.98 MB, of which the `ERSC` section alone is 10.94 MB. The budget was `1 << 18`
/// qwords = 2.00 MB, so the scan crossed 18.2% of the image and then stopped without saying so --
/// a scan that gave up 8.9 MB early was indistinguishable from a scan that looked everywhere and
/// found nothing. An owner-bearing global past that cut was simply unreachable.
///
/// A byte figure rather than a repeat of the constant, so a future shrink has to argue with the
/// measurement instead of with a number that agrees with itself.
#[test]
fn the_session_scan_budget_covers_all_of_seamlesss_writable_data() {
    /// `0xafa0a5`, rounded up: what ersc.dll v2.0.1's writable sections actually total.
    const ERSC_WRITABLE_BYTES: usize = 11 * 1024 * 1024;
    let covered = super::session_scan::SESSION_SCAN_QWORD_BUDGET * 8;
    assert!(
        covered >= ERSC_WRITABLE_BYTES,
        "the scan budget covers {covered} bytes but ersc.dll has {ERSC_WRITABLE_BYTES} bytes of \
         writable sections, so the scan would stop short and report the miss as an absence"
    );
}

/// A missing OSM must no longer refuse the action.
///
/// # What reading the function settled
///
/// Both driven actions were read end to end out of the installed ersc.dll v2.0.1 with
/// `scripts/disas-ersc.py --whole`. They are 0x64 and 0x75 bytes, and in each one `rcx` is read
/// exactly once:
///
/// ```text
/// cancel ersc+0x258d0:  mov rdi, [rcx + 0x58]   then everything via rdi
/// invade ersc+0x25850:  mov rdi, [rcx + 0x58]   then everything via rdi
/// ```
///
/// So `this` is a box with a session pointer at `+0x58`, and whose box it is does not matter to
/// them. `synthesized_owner` supplies one we own, which cannot be a misidentification and cannot
/// be freed under a call -- strictly safer than the object the scan spent four false positives,
/// two killed processes and a whole-address-space sweep failing to identify.
///
/// This guards the direction of the change: a null OSM must produce a shim, never a refusal.
#[test]
fn a_missing_osm_is_answered_with_a_synthesized_owner_rather_than_a_refusal() {
    let code = product_code();
    let body = code
        .split_once("fn ersc_owner_or_refuse(")
        .expect("the owner gate must exist")
        .1;
    let body = body.split_once("\n}").expect("a function body").0;
    assert!(
        body.contains("synthesized_owner(session.session)"),
        "a session with no OSM must be driven through a synthesized owner -- the actions read rcx \
         only at +0x58, so Seamless's own object was never required"
    );
    let shim = code
        .split_once("fn synthesized_owner(")
        .expect("the shim must exist")
        .1;
    let shim = shim.split_once("\n}").expect("a function body").0;
    assert!(
        shim.contains("ersc::NEXT_OBJECT_OFFSET"),
        "the shim must write the session at the offset the actions read, derived from the ABI \
         rather than spelled again"
    );
    assert!(
        shim.contains("Ordering::Acquire"),
        "the allocation must be published once and shared, not remade per call"
    );
}

/// The game thread must never sweep.
///
/// # The freeze this closes
///
/// Measured on run br-20260908-204210-6513: a 3.5-second stall every 11 seconds -- six in sixty
/// -- during which the game task advanced between one and five ticks. Read out of
/// `er-telemetry-timeseries.jsonl` as gaps in a stream whose median sample spacing is 218 ms. The
/// frame-delta oracle in the same file saturates at exactly 50.0 ms, so it cannot report a stall
/// this size at all and the gaps are the measurement rather than the samples.
///
/// Reading pages in bulk fixed the wrong half of the cost. Crossing 10.98 MB is 2,812 calls, but
/// `identifies_a_session` asks the kernel about each candidate -- state, then `_Type`, then
/// `_Count` -- and those pointers are on the heap, outside the page in hand, so they cannot be
/// served from the buffer. In 1.4 million qwords that is hundreds of thousands of round trips.
///
/// None of it needs the game thread, so none of it may run there.
#[test]
fn the_game_thread_reads_the_cache_and_never_sweeps() {
    let code = session_scan_code();
    let body = code
        // The name only. Anchoring on `(base: usize` broke the moment rustfmt wrapped the
        // signature across lines, and a source-scanning test that cannot find its subject reports
        // a fail against code that is correct.
        .split_once("fn cached_scan_for_session(")
        .expect("the windows cached_scan_for_session must exist")
        .1;
    let body = body.split_once("\n}").expect("a function body").0;
    assert!(
        !body.contains("scan_for_session(base, abi)"),
        "the game-thread entry point must not call the sweep -- it stalled the game task for 3.5s \
         every 11s. It may only validate the cache and ask the worker for another pass"
    );
    assert!(
        body.contains("request_sweep("),
        "it must still ask for a sweep, or a session that appears later is never found"
    );
}

/// A session found without an owner is provisional, and the sweeper keeps looking.
///
/// `owner == 0` is the shape that lets the filter judge and log while `ersc_owner_or_refuse`
/// declines every cancel -- open issue er-effects-rs-9i0g, and the state every live run has ended
/// in so far. Settling for it permanently would stop the search that is meant to end it.
#[test]
fn a_session_found_without_an_owner_keeps_the_sweeper_looking() {
    let code = session_scan_code();
    let body = code
        // The name only. Anchoring on `(base: usize` broke the moment rustfmt wrapped the
        // signature across lines, and a source-scanning test that cannot find its subject reports
        // a fail against code that is correct.
        .split_once("fn cached_scan_for_session(")
        .expect("the windows cached_scan_for_session must exist")
        .1;
    let body = body.split_once("\n}").expect("a function body").0;
    assert!(
        body.contains("owner == 0"),
        "a cached session with no owner must still request a sweep, or the bare answer becomes \
         permanent and the owner is never looked for again"
    );
}

/// The sweeper is bounded, and blocks between passes.
///
/// A worker thread cannot stall a frame, but it can still burn a core forever. The thing it looks
/// for arrives when Seamless builds it -- a human-scale event -- so every iteration of the loop
/// ends in a blocking wait, and the loop stops rather than running for the life of the process.
///
/// The wait is a condvar rather than a sleep, which `scripts/check-no-timeouts.py` requires and
/// which is also the better mechanism: the two callers that raise a request are the two moments the
/// answer is wanted, so the sweeper starts its pass then instead of up to fifteen seconds later.
#[test]
fn the_sweeper_is_bounded_and_paces_itself() {
    let code = session_scan_code();
    let body = code
        .split_once("fn sweep_until_answered(")
        .expect("the sweeper must exist")
        .1;
    let body = body.split_once("\n}").expect("a function body").0;
    assert!(
        body.contains("SESSION_SCAN_MAX_SWEEPS"),
        "the sweeper loop must be bounded"
    );
    assert!(
        body.contains("await_sweep_request(&mut seen, Pacing::AtMostOnePassPerInterval)"),
        "a pass that found nothing must block until the next request, capped by the interval, \
         rather than spinning a core"
    );
    assert!(
        body.contains("await_sweep_request(&mut seen, Pacing::UntilRequested)"),
        "with no passes owed the sweeper must park on a request, not wake on a timer to re-read a \
         budget nothing has touched"
    );
    assert!(
        !body.contains("thread::sleep"),
        "the sweeper must not sleep -- readiness here is the request, and the interval is only its \
         cap"
    );
    const {
        assert!(
            super::session_scan::SESSION_SCAN_MAX_SWEEPS <= 16,
            "the cap has to be small enough that the worst case is a handful of passes, not a habit"
        );
    }
}

/// Every refill of the sweep budget must also wake the sweeper.
///
/// The budget and the wake-up are two halves of one event. A refill that does not raise a request
/// leaves the sweeper parked in `await_sweep_request` with passes owed and nothing coming to end
/// the wait -- which is worse than the sleep this replaced, because that at least woke on its own.
#[test]
fn refilling_the_sweep_budget_always_wakes_the_sweeper() {
    let code = session_scan_code();
    for owner in ["fn invalidate_cached_session(", "fn request_sweep_now("] {
        let body = code.split_once(owner).expect("the refill must exist").1;
        let body = body.split_once("\n}").expect("a function body").0;
        assert!(
            body.contains("SWEEP_BUDGET.store(SESSION_SCAN_MAX_SWEEPS")
                && body.contains("raise_sweep_request()"),
            "{owner} refills the budget, so it must raise a request too"
        );
    }
}

/// A bare session find may never veto an owner find.
///
/// # The shape of the bug this pins
///
/// `scan_for_session` produces two shapes. The bare one -- a global pointing straight at something
/// that identifies itself as a session -- names no owner, so it can only ever yield `owner: 0`, and
/// `ersc_owner_or_refuse` then declines to drive Seamless for the rest of the process. The owned
/// one additionally hands back the OSM, and it is the only shape that lets a rejected match be
/// cancelled.
///
/// The owned arm used to carry `&& found.map(|(_, session)| session == next).unwrap_or(true)`: an
/// owner was accepted only if it pointed at the session a previous bare hit had already accepted.
/// One wrong bare hit therefore vetoed every real owner in the image, because a real OSM points at
/// the real session and the real session was not the wrong one. Measured 2026-09-06: four
/// rejections, zero cancellations, `owner 0x0` for the whole run.
///
/// Corroboration is still used -- it is a reason to stop scanning early. It is never a reason to
/// reject.
#[test]
fn a_bare_session_find_cannot_veto_an_owner_find() {
    let code = session_scan_code();
    let body = code
        .split_once("fn scan_for_session(base: usize")
        .expect("the windows scan_for_session must exist")
        .1;
    let body = body.split_once("\n}").expect("a function body").0;
    assert!(
        !body.contains("unwrap_or(true)"),
        "the owner arm must not be conditional on a previous bare find agreeing with it -- that is \
         the veto that left a live run unable to cancel anything"
    );
    let returns_owned_first = body.find("owned.or_else(");
    assert!(
        returns_owned_first.is_some(),
        "the scan must prefer the shape that carries an owner over the shape that cannot act"
    );
    // A bare hit must be remembered rather than returned on sight, or the scan stops before it can
    // ever reach the owner it needs.
    assert!(
        body.contains("bare = Some((slot, candidate));"),
        "a bare session must be recorded and the scan continued, not returned immediately"
    );
}

/// A rejection that was not enforced has to be counted, not merely logged.
///
/// The user-visible failure on 2026-09-06 was "I didn't only invade locally. It might be disabled?"
/// -- and the filter was armed, judging correctly, and enforcing nothing. Every other state this
/// module can be in shows up in the heartbeat; that one showed up only as four lines buried in 408,
/// so the run read as healthy right up until somebody read the log by hand.
///
/// The counter is the oracle: above zero means a match this module rejected proceeded anyway.
#[test]
fn an_unenforced_rejection_is_counted_and_reported() {
    let code = product_code();
    assert!(
        code.contains("UNENFORCED_REJECTS.fetch_add(1, Ordering::SeqCst);"),
        "the path that declines to cancel a rejected match must increment the counter, or an inert \
         filter is indistinguishable from a working one"
    );
    assert!(
        code.contains("UNENFORCED_REJECTS.load(Ordering::SeqCst),"),
        "the counter must be published through `tallies`, or nothing outside this module can see it"
    );
    let heartbeat = include_str!("../drive.rs");
    assert!(
        heartbeat.contains("UNENFORCED={unenforced}"),
        "the heartbeat must print the unenforced count -- it is the one number that says whether \
         the filter worked, as opposed to whether it ran"
    );
}

#[test]
fn a_rejection_arms_the_pending_cancel_rather_than_driving_it_where_it_is_judged() {
    // The verdict is reached inside `set_join_data_hook`, a detour on the game's own
    // `SetMultiplayJoinData`. Driving from there calls into ersc.dll in the state the server's
    // offer left behind, which is outside the set ERSC draws its own Cancel row for.
    super::arm_pending_cancel(0x3d2f2c00, RejectReason::WrongBlock, true);
    let armed = {
        let guard = super::PENDING_CANCEL.lock();
        let guard = match guard {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard
    };
    let armed = armed.expect("the rejection is armed for the game task");
    assert_eq!(armed.destination, 0x3d2f2c00);

    // One slot, not a queue: the server is waiting on the newest offer, so a second rejection
    // replaces the first rather than queueing a cancel for a match already superseded.
    super::arm_pending_cancel(0x12000000, RejectReason::WrongPlaceName, false);
    let armed = {
        let guard = super::PENDING_CANCEL.lock();
        let mut guard = match guard {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.take()
    };
    let armed = armed.expect("the newer rejection replaced the older one");
    assert_eq!(armed.destination, 0x12000000);
    assert!(!armed.notice);
}
