//! Shared schema for `data/effects.json`, the named effect-call list.
//!
//! The game DLL embeds this file at compile time and builds its overlay
//! entries from it; `er-param-inspect validate` reads the same file to check
//! every ID against `SpEffectParam` in a regulation archive. Editing
//! `data/effects.json` is the only step needed to change the built-in catalog.

use std::collections::BTreeMap;

use serde::Deserialize;

/// `data/effects.json`, embedded at compile time so the DLL and the
/// validation tooling always agree on the built-in catalog.
pub const EMBEDDED_EFFECTS_JSON: &str = include_str!("../../../data/effects.json");

/// Rich metadata keyed by `SpEffectParam` ID. Selector/user catalogs should
/// reference this by ID instead of duplicating field data.
pub const EMBEDDED_EFFECT_MASTER_CATALOG_JSON: &str =
    include_str!("../../../data/effect-master-catalog.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltInEffectCatalog {
    pub file_name: &'static str,
    pub json: &'static str,
}

pub const BUILT_IN_EFFECT_CATALOGS: &[BuiltInEffectCatalog] = &[
    BuiltInEffectCatalog {
        file_name: "ai-perception.json",
        json: include_str!("../../../data/effect-catalogs/ai-perception.json"),
    },
    BuiltInEffectCatalog {
        file_name: "ai-targeting-experimental.json",
        json: include_str!("../../../data/effect-catalogs/ai-targeting-experimental.json"),
    },
    BuiltInEffectCatalog {
        file_name: "casting-and-consumption.json",
        json: include_str!("../../../data/effect-catalogs/casting-and-consumption.json"),
    },
    BuiltInEffectCatalog {
        file_name: "damage-output.json",
        json: include_str!("../../../data/effect-catalogs/damage-output.json"),
    },
    BuiltInEffectCatalog {
        file_name: "darkness-and-light.json",
        json: include_str!("../../../data/effect-catalogs/darkness-and-light.json"),
    },
    BuiltInEffectCatalog {
        file_name: "defense-resistance.json",
        json: include_str!("../../../data/effect-catalogs/defense-resistance.json"),
    },
    BuiltInEffectCatalog {
        file_name: "fp-mp-changes.json",
        json: include_str!("../../../data/effect-catalogs/fp-mp-changes.json"),
    },
    BuiltInEffectCatalog {
        file_name: "hearing-reduced.json",
        json: include_str!("../../../data/effect-catalogs/hearing-reduced.json"),
    },
    BuiltInEffectCatalog {
        file_name: "hearing-zero.json",
        json: include_str!("../../../data/effect-catalogs/hearing-zero.json"),
    },
    BuiltInEffectCatalog {
        file_name: "hides-from-npcs.json",
        json: include_str!("../../../data/effect-catalogs/hides-from-npcs.json"),
    },
    BuiltInEffectCatalog {
        file_name: "hp-changes.json",
        json: include_str!("../../../data/effect-catalogs/hp-changes.json"),
    },
    BuiltInEffectCatalog {
        file_name: "instant-or-default-duration.json",
        json: include_str!("../../../data/effect-catalogs/instant-or-default-duration.json"),
    },
    BuiltInEffectCatalog {
        file_name: "item-drop-and-souls.json",
        json: include_str!("../../../data/effect-catalogs/item-drop-and-souls.json"),
    },
    BuiltInEffectCatalog {
        file_name: "movement-timing.json",
        json: include_str!("../../../data/effect-catalogs/movement-timing.json"),
    },
    BuiltInEffectCatalog {
        file_name: "nonmechanical-visual-sfx.json",
        json: include_str!("../../../data/effect-catalogs/nonmechanical-visual-sfx.json"),
    },
    BuiltInEffectCatalog {
        file_name: "permanent-effects.json",
        json: include_str!("../../../data/effect-catalogs/permanent-effects.json"),
    },
    BuiltInEffectCatalog {
        file_name: "sight-and-hearing-zero.json",
        json: include_str!("../../../data/effect-catalogs/sight-and-hearing-zero.json"),
    },
    BuiltInEffectCatalog {
        file_name: "sight-reduced.json",
        json: include_str!("../../../data/effect-catalogs/sight-reduced.json"),
    },
    BuiltInEffectCatalog {
        file_name: "sight-zero.json",
        json: include_str!("../../../data/effect-catalogs/sight-zero.json"),
    },
    BuiltInEffectCatalog {
        file_name: "stamina-changes.json",
        json: include_str!("../../../data/effect-catalogs/stamina-changes.json"),
    },
    BuiltInEffectCatalog {
        file_name: "status-ailments.json",
        json: include_str!("../../../data/effect-catalogs/status-ailments.json"),
    },
    BuiltInEffectCatalog {
        file_name: "target-clear.json",
        json: include_str!("../../../data/effect-catalogs/target-clear.json"),
    },
    BuiltInEffectCatalog {
        file_name: "target-priority-high.json",
        json: include_str!("../../../data/effect-catalogs/target-priority-high.json"),
    },
    BuiltInEffectCatalog {
        file_name: "target-priority-low.json",
        json: include_str!("../../../data/effect-catalogs/target-priority-low.json"),
    },
    BuiltInEffectCatalog {
        file_name: "team-change-experimental.json",
        json: include_str!("../../../data/effect-catalogs/team-change-experimental.json"),
    },
    BuiltInEffectCatalog {
        file_name: "timed-effects.json",
        json: include_str!("../../../data/effect-catalogs/timed-effects.json"),
    },
    BuiltInEffectCatalog {
        file_name: "visual-vfx.json",
        json: include_str!("../../../data/effect-catalogs/visual-vfx.json"),
    },
];

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EffectsFile {
    pub calls: Vec<EffectCallSpec>,
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

pub fn parse_effect_master_catalog_json(
    json: &str,
) -> Result<EffectMasterCatalog, serde_json::Error> {
    serde_json::from_str(json)
}

/// Parses the compile-time embedded copy of `data/effects.json`.
pub fn embedded_effects() -> Result<EffectsFile, serde_json::Error> {
    parse_effects_json(EMBEDDED_EFFECTS_JSON)
}

/// Parses the compile-time embedded copy of `data/effect-master-catalog.json`.
pub fn embedded_effect_master_catalog() -> Result<EffectMasterCatalog, serde_json::Error> {
    parse_effect_master_catalog_json(EMBEDDED_EFFECT_MASTER_CATALOG_JSON)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_BUILT_IN_CALL_COUNT: usize = 594;
    const EXPECTED_MASTER_EFFECT_COUNT: usize = 11325;
    const EXPECTED_BUILT_IN_CATALOG_COUNT: usize = 27;

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
    fn embedded_master_catalog_is_valid() {
        let master =
            embedded_effect_master_catalog().expect("data/effect-master-catalog.json must parse");

        assert_eq!(master.schema_version, 1);
        assert_eq!(master.kind, "sp_effect_master_catalog");
        assert_eq!(master.source.param, "SpEffectParam");
        assert_eq!(master.effects.len(), EXPECTED_MASTER_EFFECT_COUNT);
        assert_eq!(master.source.row_count, EXPECTED_MASTER_EFFECT_COUNT);

        let stealth = master
            .effects
            .iter()
            .find(|effect| effect.id == 20004380)
            .expect("known sight/hearing-zero effect");
        assert_eq!(
            stealth.fields.get("sightSearchEnemyRate"),
            Some(&serde_json::json!(0))
        );
        assert_eq!(
            stealth.fields.get("hearingSearchEnemyRate"),
            Some(&serde_json::json!(0))
        );
        assert!(stealth.tags.iter().any(|tag| tag == "ai.perception.zero"));
    }

    #[test]
    fn built_in_effect_catalogs_are_plain_id_lists() {
        assert_eq!(
            BUILT_IN_EFFECT_CATALOGS.len(),
            EXPECTED_BUILT_IN_CATALOG_COUNT
        );
        let master = embedded_effect_master_catalog().expect("master catalog must parse");
        let valid_ids = master
            .effects
            .iter()
            .map(|effect| effect.id)
            .collect::<std::collections::BTreeSet<_>>();
        for catalog in BUILT_IN_EFFECT_CATALOGS {
            let ids = parse_effect_id_catalog_json(catalog.json).unwrap_or_else(|error| {
                panic!(
                    "{} must parse as a JSON array of IDs: {error}",
                    catalog.file_name
                )
            });
            assert!(!ids.is_empty(), "{} must not be empty", catalog.file_name);
            let mut unique = ids.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(
                unique.len(),
                ids.len(),
                "{} has duplicate IDs",
                catalog.file_name
            );
            for id in ids {
                assert!(
                    valid_ids.contains(&id),
                    "{} references unknown SpEffect ID {id}",
                    catalog.file_name
                );
            }
        }
    }

    #[test]
    fn rejects_unknown_kinds() {
        const UNKNOWN_KIND_JSON: &str =
            r#"{ "calls": [ { "kind": "warp", "id": 1, "name": "x", "enabled": true } ] }"#;

        assert!(parse_effects_json(UNKNOWN_KIND_JSON).is_err());
    }
}
