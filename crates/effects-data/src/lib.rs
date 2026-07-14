//! Shared schemas for effect metadata and user-provided effect catalogs.
//!
//! Runtime selector catalogs are external JSON files in `effect-catalogs/*.json`
//! next to `eldenring.exe`. `data/effects.json` remains a host-side curated list
//! used by validation/generation tooling, not a runtime built-in catalog.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Host-side curated effect list used by validation/generation tooling.
pub const EMBEDDED_EFFECTS_JSON: &str = include_str!("../../../data/effects.json");

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EffectsFile {
    pub calls: Vec<EffectCallSpec>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EffectHotkeysFile {
    pub hotkeys: Vec<EffectHotkeySpec>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EffectHotkeySpec {
    pub name: Option<String>,
    pub key: String,
    pub effect_id: i32,
    pub count: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EffectMasterCatalog {
    pub schema_version: u32,
    pub kind: String,
    pub source: EffectMasterCatalogSource,
    pub field_index: BTreeMap<String, EffectMasterField>,
    pub effects: Vec<EffectMasterEntry>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EffectMasterCatalogSource {
    pub param: String,
    pub binder_version: String,
    pub row_count: usize,
    pub regulation_file: String,
    pub paramdef_file: String,
    pub names_file: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EffectMasterField {
    pub r#type: String,
    pub display_name: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EffectMasterEntry {
    pub id: i32,
    pub name: String,
    pub row_name: Option<String>,
    pub community_name: Option<String>,
    pub curated_name: Option<String>,
    pub vfx: Vec<i32>,
    pub tags: Vec<String>,
    pub fields: BTreeMap<String, serde_json::Value>,
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

pub fn parse_effect_id_catalog_json(json: &str) -> Result<Vec<i32>, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn parse_effect_hotkeys_json(json: &str) -> Result<EffectHotkeysFile, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn parse_effect_master_catalog_json(
    json: &str,
) -> Result<EffectMasterCatalog, serde_json::Error> {
    serde_json::from_str(json)
}

/// Parses the compile-time embedded copy of `data/effects.json`.
pub fn embedded_effects() -> Result<EffectsFile, serde_json::Error> {
    parse_effects_json(EMBEDDED_EFFECTS_JSON)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_BUILT_IN_CALL_COUNT: usize = 594;
    #[test]
    fn embedded_effects_file_is_valid() {
        let effects = embedded_effects().expect("data/effects.json must parse");

        assert_eq!(effects.calls.len(), EXPECTED_BUILT_IN_CALL_COUNT);
        let player_all_black = effects
            .calls
            .iter()
            .find(|call| call.name == "Player all black")
            .expect("Player all black built-in call");
        assert_eq!(player_all_black.kind, EffectKindSpec::SpEffect);
        assert!(!player_all_black.enabled);
        assert!(effects.calls.iter().all(|call| !call.enabled));
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
    fn parses_effect_master_catalog_schema() {
        let master = parse_effect_master_catalog_json(
            r#"{
                "schema_version": 1,
                "kind": "sp_effect_master_catalog",
                "source": {
                    "param": "SpEffectParam",
                    "binder_version": "7",
                    "row_count": 1,
                    "regulation_file": "regulation.bin",
                    "paramdef_file": "SpEffect.xml",
                    "names_file": "SpEffectParam.txt"
                },
                "field_index": {
                    "sightSearchEnemyRate": {
                        "type": "f32",
                        "display_name": "Sight Search Enemy Rate",
                        "tags": ["ai.perception"]
                    }
                },
                "effects": [
                    {
                        "id": 20004380,
                        "name": "Stealth",
                        "row_name": null,
                        "community_name": null,
                        "curated_name": null,
                        "vfx": [],
                        "tags": ["ai.perception.zero"],
                        "fields": {"sightSearchEnemyRate": 0}
                    }
                ]
            }"#,
        )
        .expect("effect master catalog schema must parse");

        assert_eq!(master.schema_version, 1);
        assert_eq!(master.kind, "sp_effect_master_catalog");
        assert_eq!(master.source.param, "SpEffectParam");
        assert_eq!(master.effects.len(), 1);
        assert_eq!(master.effects[0].id, 20004380);
        assert_eq!(
            master.effects[0].fields.get("sightSearchEnemyRate"),
            Some(&serde_json::json!(0))
        );
        assert!(
            master.effects[0]
                .tags
                .iter()
                .any(|tag| tag == "ai.perception.zero")
        );
    }

    #[test]
    fn user_effect_catalogs_are_plain_id_lists() {
        let ids = parse_effect_id_catalog_json("[18570, 20004380]").expect("plain ID list parses");
        assert_eq!(ids, vec![18570, 20004380]);
    }

    #[test]
    fn parses_effect_hotkeys_file() {
        let parsed = parse_effect_hotkeys_json(
            r#"{
                "hotkeys": [
                    {
                        "name": "deathblight self test",
                        "key": "numpad_multiply",
                        "effect_id": 8355,
                        "count": 1
                    }
                ]
            }"#,
        )
        .expect("effect hotkeys file must parse");

        assert_eq!(parsed.hotkeys.len(), 1);
        assert_eq!(parsed.hotkeys[0].key, "numpad_multiply");
        assert_eq!(parsed.hotkeys[0].effect_id, 8355);
        assert_eq!(parsed.hotkeys[0].count, 1);
    }

    #[test]
    fn rejects_unknown_hotkey_fields() {
        assert!(
            parse_effect_hotkeys_json(
                r#"{"hotkeys":[{"key":"numpad_multiply","effect_id":8355,"count":1,"extra":true}]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_unknown_kinds() {
        const UNKNOWN_KIND_JSON: &str =
            r#"{ "calls": [ { "kind": "warp", "id": 1, "name": "x", "enabled": true } ] }"#;

        assert!(parse_effects_json(UNKNOWN_KIND_JSON).is_err());
    }
}
