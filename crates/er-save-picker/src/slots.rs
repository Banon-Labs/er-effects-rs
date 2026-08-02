//! Save-container character slot discovery shared by the boot picker and quit-menu browse rows.

/// One active character slot parsed straight from a picked save's bytes: the slot index,
/// character name, and level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveSlotInfo {
    pub slot: usize,
    pub name: String,
    pub level: i32,
}

/// Parse the ACTIVE character slots out of a save container's bytes, returning slot index, name,
/// and level for each occupied slot in order.
///
/// Empty means either the bytes are not a readable save or the save contains no active character
/// slots matching the same real-character filter the autoload path uses.
pub fn parse_save_character_slots(bytes: &[u8]) -> Vec<SaveSlotInfo> {
    er_save_loader::bnd4::active_character_slots(bytes)
        .map(|slots| {
            slots
                .into_iter()
                .map(|slot| SaveSlotInfo {
                    slot: slot.slot,
                    name: slot.name,
                    level: slot.level,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_bytes_have_no_slots() {
        assert!(parse_save_character_slots(b"not a bnd4 save").is_empty());
    }
}
