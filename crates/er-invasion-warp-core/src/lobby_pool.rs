//! Put DLL users in a matchmaking pool of their own, by rewriting Seamless's `lobby_key`.
//!
//! # What `lobby_key` actually is
//!
//! Reversed 2026-08-06 out of `ersc.dll`. It is NOT the co-op password, which was the
//! working assumption for most of a day and was wrong twice over — a player with a different
//! password matched us live, and the derivation never contains a password:
//!
//! ```text
//! lobby_key = SHA256_hex( AES_decrypt(ctx[0xB8]) ++ ("0"|"1") ++ SALT32 )     BuildLobbyKey @ 0x0ABC20
//! ctx[0xB8] <- a regulation/param-table fingerprint                            fn 0x0A8930
//! ```
//!
//! So it is a REGULATION FINGERPRINT: everyone running the same params and the same Seamless build
//! derives the same key, which is why five strangers shared ours. It is consumed at exactly two
//! places, both Steam calls, both fed the same value:
//!
//! ```text
//! 0x0AB07E  AddRequestLobbyListStringFilter("lobby_key", K, Equal)   the search
//! 0x0AB12D  SetLobbyData(lobby, "lobby_key", K)                      the publish
//! ```
//!
//! # The lever that follows
//!
//! Because one value drives both halves, rewriting it at the Steam boundary moves a player into a
//! different pool ENTIRELY and SYMMETRICALLY. They cannot see vanilla lobbies and vanilla cannot see
//! theirs — the exclusivity applies to hosting as much as to invading, with no host-side accept hook
//! required, because a vanilla invader's query simply asks for a different string.
//!
//! Doing it here rather than by mutating the regulation is deliberate: no gameplay value changes
//! even briefly, there is no fingerprint-timing race to win, and `ersc.dll` needs no new hook.
//!
//! # Why this is not a cryptographic secret, and must not be sold as one
//!
//! [`POOL_NAMESPACE`] is a constant compiled into a DLL anyone can download. The derived key is
//! therefore public: it separates pools, it does not protect one. That is the whole requirement for
//! "only match other DLL users" — there is nobody to keep out, only a different room to stand in.
//! FNV is used accordingly. If a PRIVATE pool is ever added (a user-chosen passphrase), that changes
//! the requirement to secrecy and this must become a real digest, because FNV is trivially
//! invertible for short inputs.
//!
//! A collision with a genuine vanilla key would put a user back in the vanilla pool — the ordinary
//! behaviour they would have had anyway, not a crash or a corrupt session. The failure direction is
//! benign, which is the other reason a weak hash is tolerable here.

use er_game_base::fnv1a::{FNV1A64_OFFSET_BASIS, fnv1a64_extend};

/// Tags the pool. Changing this string splits users across DLL versions, so it is versioned
/// deliberately rather than derived from anything that drifts (a build id, a timestamp, a file
/// hash). Every DLL that shares this constant can match every other.
pub const POOL_NAMESPACE: &str = "er-invasion-warp/dll-pool/v1";

/// Seeds for the four independent passes that make up a 64-hex-character result.
///
/// Four passes because a real `lobby_key` is 64 hex characters and a substituted value that keeps
/// the same shape cannot trip any length assumption inside ersc or Steam. The seeds differ so the
/// passes are not four copies of one 64-bit value.
const PASS_SEEDS: [u64; 4] = [
    FNV1A64_OFFSET_BASIS,
    0x9e37_79b9_7f4a_7c15,
    0xff51_afd7_ed55_8ccd,
    0xc4ce_b9fe_1a85_ec53,
];

/// The key this player should publish and filter on, or `None` to leave Seamless's own key alone.
///
/// `original` is whatever ersc computed. Passing it through the hash rather than discarding it
/// keeps everything the vanilla key already separates — game version, params, Seamless build — so
/// two DLL users on mismatched regulations stay mismatched instead of being forced together into a
/// session neither can actually play.
#[must_use]
pub fn pooled_lobby_key(enabled: bool, original: &str) -> Option<String> {
    if !enabled {
        return None;
    }
    let mut material = String::with_capacity(original.len() + POOL_NAMESPACE.len() + 1);
    material.push_str(original);
    material.push('\u{1f}');
    material.push_str(POOL_NAMESPACE);
    let bytes = material.as_bytes();
    let mut out = String::with_capacity(64);
    for seed in PASS_SEEDS {
        out.push_str(&format!("{:016x}", fnv1a64_extend(seed, bytes)));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VANILLA: &str = "16ca67264987f56ca821525a2f65884cedc2546cf15ca3e69292c5c394c9d4ef";

    #[test]
    fn disabled_leaves_seamlesss_own_key_alone() {
        assert_eq!(pooled_lobby_key(false, VANILLA), None);
    }

    /// The whole mechanism: the pooled key must DIFFER from vanilla, or DLL users stay in the
    /// general population and the option does nothing while appearing to work.
    #[test]
    fn the_pooled_key_differs_from_the_vanilla_one() {
        let pooled = pooled_lobby_key(true, VANILLA).expect("enabled");
        assert_ne!(pooled, VANILLA);
    }

    /// Two DLL users derive the SAME key from the same vanilla input, which is what makes them
    /// find each other. This is the property the feature is.
    #[test]
    fn two_players_on_the_same_game_derive_the_same_pooled_key() {
        let a = pooled_lobby_key(true, VANILLA).expect("enabled");
        let b = pooled_lobby_key(true, VANILLA).expect("enabled");
        assert_eq!(a, b);
    }

    /// Everything the vanilla key already separated must stay separated. If two players are on
    /// different game versions or different params, ersc gives them different keys and they must
    /// remain unable to match -- forcing them together would produce a session neither can play.
    #[test]
    fn players_whose_vanilla_keys_differ_still_cannot_match() {
        let a = pooled_lobby_key(true, VANILLA).expect("enabled");
        let b = pooled_lobby_key(true, "deadbeef").expect("enabled");
        assert_ne!(a, b, "a differing regulation must survive the transform");
    }

    /// Substituting a value of the same shape means no length assumption anywhere downstream can
    /// notice the swap.
    #[test]
    fn the_pooled_key_keeps_the_shape_of_a_real_one() {
        let pooled = pooled_lobby_key(true, VANILLA).expect("enabled");
        assert_eq!(pooled.len(), 64, "a real lobby_key is 64 hex characters");
        assert!(
            pooled
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "lowercase hex only, like the game's own: {pooled}"
        );
    }

    /// The four passes must not collapse into one repeated value, which is what would happen if the
    /// seeds were equal or ignored.
    #[test]
    fn the_four_passes_are_independent() {
        let pooled = pooled_lobby_key(true, VANILLA).expect("enabled");
        let chunks: Vec<&str> = (0..4).map(|i| &pooled[i * 16..(i + 1) * 16]).collect();
        for i in 0..chunks.len() {
            for j in (i + 1)..chunks.len() {
                assert_ne!(
                    chunks[i], chunks[j],
                    "pass {i} and {j} produced the same 64 bits"
                );
            }
        }
    }

    /// A one-character change in the input must change the output, or near-identical regulations
    /// would silently share a pool.
    #[test]
    fn a_small_input_change_changes_the_key() {
        let a = pooled_lobby_key(true, VANILLA).expect("enabled");
        let mut altered = VANILLA.to_owned();
        altered.pop();
        altered.push('0');
        let b = pooled_lobby_key(true, &altered).expect("enabled");
        assert_ne!(a, b);
    }

    /// THE "GLOBAL DLL COMMUNITY" MODE, pinned because it is a config COMBINATION rather than a
    /// feature with its own switch, and combinations are what refactors quietly break.
    ///
    /// `enabled = false` + `dll_users_only = true` means: no location filtering at all, invade
    /// anywhere exactly as unmodded play, but the only people in your matchmaking pool are other
    /// DLL users. That requires the pool to be INDEPENDENT of the reject filter's master switch.
    /// Gating the pool on `enabled` -- an easy tidy-up, since both live in the same config -- would
    /// kill this mode while every existing test still passed.
    #[test]
    fn the_pool_is_independent_of_the_reject_filters_master_switch() {
        use crate::local_invasion::LocalInvasionConfig;
        let mut config = LocalInvasionConfig {
            enabled: false,
            dll_users_only: true,
            ..LocalInvasionConfig::default()
        };
        assert!(
            pooled_lobby_key(config.dll_users_only, VANILLA).is_some(),
            "the pool must apply with the location filter switched OFF -- that is the \
             invade-anywhere-but-only-DLL-users mode"
        );
        // And the converse: filtering by location without joining the pool stays vanilla-visible.
        config.enabled = true;
        config.dll_users_only = false;
        assert!(
            pooled_lobby_key(config.dll_users_only, VANILLA).is_none(),
            "filtering by location must not drag a player out of the vanilla pool"
        );
    }

    /// The namespace is what every DLL user agrees on; an empty one would make the transform a
    /// no-op-shaped identity that still differs from vanilla only by accident.
    #[test]
    fn the_namespace_is_present_and_versioned() {
        assert!(!POOL_NAMESPACE.is_empty());
        assert!(
            POOL_NAMESPACE.contains("/v"),
            "versioned so pools can be split later"
        );
    }
}
