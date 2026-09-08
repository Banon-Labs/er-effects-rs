//! Where "behind main" comes from: the published `main-*` release tags, newest first.
//!
//! # Why GitHub at all
//!
//! A DLL cannot know its position in history from its own bytes. It knows the commit it was
//! built from -- that is baked in -- but "is that commit still the tip" is a question only the
//! repository can answer, and the release list is the cheapest form of the answer: the tags are
//! the commits (`main-<40 hex>`), so nothing has to be downloaded or opened to read them.
//!
//! # One request, off the game thread, failing open
//!
//! A single get on a background thread at startup. If it fails -- offline, rate-limited, GitHub
//! down -- the list stays empty and every build renders as `Standing::Unknown`, which is drawn
//! quietly and never red. A watermark that turned red because a network call failed would be
//! worse than no watermark: it would teach the reader that red means nothing.

/// Host GitHub's rest API answers on.
#[cfg(windows)]
const API_HOST: &str = "api.github.com";

/// Releases, newest first. 30 is the API default and far more history than "is this the tip"
/// needs; asking for fewer would risk a burst of same-day pre-releases hiding an older one that
/// somebody is actually running.
#[cfg(windows)]
const RELEASES_PATH: &str = "/repos/Banon-Labs/er-mods-rs/releases?per_page=12";

/// GitHub rejects requests with no User-Agent outright, so this is required, not decoration.
#[cfg(windows)]
const USER_AGENT: &str = "er-mods-rs-build-watermark";

/// Body cap for the releases response.
///
/// The shared default is 1 MB, sized for a build-planner document. A releases page is nothing
/// like that: each `main-*` release publishes ~43 assets and every one is a JSON record, so the
/// first live run came back `TooLarge` and every build rendered as unknown. Twelve releases at
/// that density is comfortably under this.
#[cfg(windows)]
const MAX_RELEASES_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Pull the `main-<sha>` commits out of a GitHub releases response, in the order they appear.
///
/// Hand-scanned rather than deserialized because the whole question is one repeated field, and a
/// JSON dependency in every shipped DLL to read `tag_name` would cost more than it explains. The
/// API returns releases newest first, and that order is the entire meaning of the result -- see
/// `er_game_base::build_id::standing_against_main`, where position 0 is the tip and everything
/// after it is behind.
pub fn parse_release_tags(body: &str) -> Vec<String> {
    const KEY: &str = "\"tag_name\":";
    let mut shas = Vec::new();
    let mut rest = body;
    while let Some(at) = rest.find(KEY) {
        rest = &rest[at + KEY.len()..];
        let Some(open) = rest.find('"') else { break };
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('"') else {
            break;
        };
        let tag = &after_open[..close];
        rest = &after_open[close + 1..];
        // Non-`main-*` tags (the hand-cut semver releases) are skipped rather than kept, because
        // admitting one would give it a position on `main` that it does not hold.
        if let Some(sha) = er_game_base::build_id::sha_from_release_tag(tag) {
            shas.push(sha.to_string());
        }
    }
    shas
}

/// Fetch the release list and publish it, on a background thread.
///
/// Returns immediately. Never runs on the game thread: the get is blocking, and a TLS handshake
/// inside `Present` or a frame task would be a visible hitch at best.
#[cfg(windows)]
pub fn spawn_lookup(log: fn(std::fmt::Arguments<'_>)) {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("er-build-watermark-releases".to_string())
            .spawn(move || match er_game_base::http::get_with_limit(
                API_HOST,
                RELEASES_PATH,
                USER_AGENT,
                MAX_RELEASES_BODY_BYTES,
            ) {
                Ok(body) => {
                    let shas = parse_release_tags(&body);
                    let count = shas.len();
                    let tip = shas.first().cloned().unwrap_or_default();
                    er_game_base::build_id::set_published_main_shas(shas);
                    log(format_args!(
                        "build-watermark: release lookup ok -- {count} main-* release(s), tip={tip}"
                    ));
                }
                Err(error) => {
                    // Deliberately not retried. The answer changes at the rate somebody cuts a
                    // release, so a run that starts offline is a run that stays Unknown, and
                    // Unknown is drawn quietly rather than as a warning.
                    log(format_args!(
                        "build-watermark: release lookup FAILED ({error:?}); every build will \
                         render as unknown, which is drawn quiet and never red"
                    ));
                }
            });
        if spawned.is_err() {
            log(format_args!(
                "build-watermark: could not spawn the release lookup thread"
            ));
        }
    });
}

#[cfg(not(windows))]
pub fn spawn_lookup(_log: fn(std::fmt::Arguments<'_>)) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed copy of the real 2026-08-25 response shape, including the semver release that
    /// must not be admitted.
    const SAMPLE: &str = r#"[
      {"tag_name":"main-ba46f81cc9306253958013affa1c916f980e1162","prerelease":true},
      {"tag_name":"main-c57b5c1bf04abcf567e841333032fdaa76f0e2ca","prerelease":true},
      {"tag_name":"0.1.6","prerelease":false},
      {"tag_name":"main-c1adc6c89a49107160897c9e3dd7e1eff1415419","prerelease":true}
    ]"#;

    /// Order is the whole meaning of the result: position 0 is `main`'s tip.
    #[test]
    fn tags_come_back_newest_first_with_semver_releases_excluded() {
        let shas = parse_release_tags(SAMPLE);
        assert_eq!(shas.len(), 3, "the semver tag was admitted: {shas:?}");
        assert_eq!(shas[0], "ba46f81cc9306253958013affa1c916f980e1162");
        assert_eq!(shas[2], "c1adc6c89a49107160897c9e3dd7e1eff1415419");
    }

    /// The parsed list has to feed the comparison it exists for, so the join is asserted rather
    /// than assumed: the tip is quiet, an older published release is behind.
    #[test]
    fn the_parsed_list_drives_the_standing_it_exists_for() {
        use er_game_base::build_id::{Standing, standing_against_main};
        let shas = parse_release_tags(SAMPLE);
        assert_eq!(
            standing_against_main("ba46f81cc930", &shas),
            Standing::AtMain
        );
        assert_eq!(
            standing_against_main("c1adc6c89a49", &shas),
            Standing::BehindMain
        );
    }

    /// Garbage in must not panic and must not invent history.
    #[test]
    fn a_broken_body_yields_no_history_rather_than_a_panic() {
        assert!(parse_release_tags("").is_empty());
        assert!(parse_release_tags("not json at all").is_empty());
        assert!(parse_release_tags(r#"{"tag_name":"#).is_empty());
        assert!(parse_release_tags(r#"{"tag_name":"unterminated"#).is_empty());
    }
}
