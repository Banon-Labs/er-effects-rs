use eldenring::cs::GameDataMan;
use fromsoftware_shared::FromStatic;

use crate::{
    log::Log,
    process_memory::{GameAddress, ProcessMemory},
};

use super::preferences::{MenuSortDefault, SortMenuKind};

const GAME_DATA_MAN_MENU_SAVELOAD_OFFSET: usize = 0x60;
const SORT_STATE_ARRAY_OFFSET: usize = 0x1440;
const SORT_STATE_ENTRY_SIZE: usize = 4;
const SORT_STATE_REVERSE_FLAG: u32 = 0x8000_0000;
const SORT_STATE_ID_MASK: u32 = 0x7fff_ffff;

pub(super) const ITEM_TYPE: SortCriterion = SortCriterion(0x5141);
const ORDER_OF_ACQUISITION: SortCriterion = SortCriterion(0x5140);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SortCriterion(u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SortStateValue(u32);

impl SortStateValue {
    pub(super) const fn for_default(default: MenuSortDefault) -> Option<Self> {
        match default {
            MenuSortDefault::Preserve => None,
            MenuSortDefault::ItemType => Some(Self::reversed(ITEM_TYPE)),
            MenuSortDefault::OrderOfAcquisition => Some(Self::reversed(ORDER_OF_ACQUISITION)),
        }
    }

    const fn reversed(criterion: SortCriterion) -> Self {
        Self(SORT_STATE_REVERSE_FLAG | criterion.0)
    }

    pub(super) const fn criterion(self) -> SortCriterion {
        SortCriterion(self.0 & SORT_STATE_ID_MASK)
    }

    pub(super) const fn raw(self) -> u32 {
        self.0
    }
}

impl From<i32> for SortStateValue {
    fn from(value: i32) -> Self {
        Self(value as u32)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MenuSystemSaveLoad {
    address: GameAddress,
}

impl MenuSystemSaveLoad {
    pub(super) fn resolve(memory: ProcessMemory) -> Option<Self> {
        let game_data_man = GameDataManAddress::resolve()?;
        let address = memory.read_address(game_data_man.menu_save_load_slot())?;
        Some(Self { address })
    }

    pub(super) const fn address(self) -> GameAddress {
        self.address
    }

    pub(super) const fn sort_slot(self, kind: SortMenuKind) -> SortStateSlot {
        SortStateSlot {
            address: self
                .address
                .offset(SORT_STATE_ARRAY_OFFSET + kind.state_index() * SORT_STATE_ENTRY_SIZE),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GameDataManAddress {
    address: GameAddress,
}

impl GameDataManAddress {
    fn resolve() -> Option<Self> {
        let ptr = GameDataMan::instance_ptr().ok()?;
        Some(Self {
            address: GameAddress::from_ptr(ptr)?,
        })
    }

    const fn menu_save_load_slot(self) -> GameAddress {
        self.address.offset(GAME_DATA_MAN_MENU_SAVELOAD_OFFSET)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SortStateSlot {
    address: GameAddress,
}

impl SortStateSlot {
    pub(super) const fn address(self) -> GameAddress {
        self.address
    }

    pub(super) fn read(self, memory: ProcessMemory) -> Option<SortStateValue> {
        let state = memory.read_i32(self.address).map(SortStateValue::from);
        if state.is_none() {
            Log::write(format_args!(
                "menu-sort: deferred; could not read addr=0x{:x}",
                self.address.value()
            ));
        }
        state
    }

    pub(super) fn write(self, memory: ProcessMemory, value: SortStateValue) -> bool {
        memory.write_u32(self.address, value.raw())
    }
}
