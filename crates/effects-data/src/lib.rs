//! Shared schema for `data/effects.json`, the named effect-call list.
//!
//! The game DLL embeds this file at compile time and builds its overlay
//! entries from it; `er-param-inspect validate` reads the same file to check
//! every ID against `SpEffectParam` in a regulation archive. Editing
//! `data/effects.json` is the only step needed to change the seeded list.

use serde::Deserialize;

/// `data/effects.json`, embedded at compile time so the DLL and the
/// validation tooling always agree on the seeded list.
pub const EMBEDDED_EFFECTS_JSON: &str = include_str!("../../../data/effects.json");

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EffectsFile {
    pub calls: Vec<EffectCallSpec>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EffectCallSpec {
    /// Which kind of runtime call this entry maps to. Adding a new kind means
    /// extending this enum and the matching dispatch in the DLL's
    /// `EffectCallKind`; existing data files stay valid.
    pub kind: EffectKindSpec,
    pub id: i32,
    pub name: String,
    /// Whether the call starts selected in the overlay.
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EffectKindSpec {
    SpEffect,
}

pub fn parse_effects_json(json: &str) -> Result<EffectsFile, serde_json::Error> {
    serde_json::from_str(json)
}

/// Parses the compile-time embedded copy of `data/effects.json`.
pub fn embedded_effects() -> Result<EffectsFile, serde_json::Error> {
    parse_effects_json(EMBEDDED_EFFECTS_JSON)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_SEEDED_CALL_COUNT: usize = 3;
    const PLAYER_ALL_BLACK_SPEFFECT_ID: i32 = 4330;

    #[test]
    fn embedded_effects_file_is_valid() {
        let effects = embedded_effects().expect("data/effects.json must parse");

        assert_eq!(effects.calls.len(), EXPECTED_SEEDED_CALL_COUNT);
        let first = effects.calls.first().expect("first seeded call");
        assert_eq!(first.id, PLAYER_ALL_BLACK_SPEFFECT_ID);
        assert_eq!(first.kind, EffectKindSpec::SpEffect);
        assert_eq!(first.name, "Player all black");
        assert!(first.enabled);
    }

    #[test]
    fn embedded_ids_are_unique() {
        let effects = embedded_effects().expect("data/effects.json must parse");
        let mut ids: Vec<i32> = effects.calls.iter().map(|call| call.id).collect();
        ids.sort_unstable();
        ids.dedup();

        assert_eq!(ids.len(), effects.calls.len(), "duplicate effect IDs");
    }

    #[test]
    fn rejects_unknown_kinds() {
        const UNKNOWN_KIND_JSON: &str =
            r#"{ "calls": [ { "kind": "warp", "id": 1, "name": "x", "enabled": true } ] }"#;

        assert!(parse_effects_json(UNKNOWN_KIND_JSON).is_err());
    }
}
