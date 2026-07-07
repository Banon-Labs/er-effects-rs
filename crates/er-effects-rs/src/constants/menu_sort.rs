/// `CSMenuSystemSaveLoad::field_0x1440`: session-local remembered sort state array.
/// Static RE: `FUN_1408581c0` resolves the active sort criterion as
/// `field_0x1440[sort_menu_type] & 0x7fffffff`; the sign bit stores reverse/descending.
pub(crate) const MENU_SORT_STATE_ARRAY_OFFSET: usize = 0x1440;
pub(crate) const MENU_SORT_STATE_ENTRY_SIZE: usize = 4;
pub(crate) const MENU_SORT_DIRECTION_FLAG: u32 = 0x8000_0000;
pub(crate) const MENU_SORT_ID_MASK: u32 = 0x7fff_ffff;

/// GR_MenuText 6105 "Item Type" maps to comparator id 0x5141 in the sort-option tables.
pub(crate) const MENU_SORT_ITEM_TYPE_ID: u32 = 0x5141;
/// GR_MenuText 6190 "Order of Acquisition" maps to comparator id 0x5140.
pub(crate) const MENU_SORT_ORDER_OF_ACQUISITION_ID: u32 = 0x5140;

/// Sort-menu table types used by target categories:
/// - 4: Armaments (static MenuEquipTableData row 0x29, label 40550)
/// - 6: Armor (row 0x2a, label 40551; head/chest/arms/legs rows 0x20..0x23)
/// - 9: Talismans (SortMenu option list 0x143b35f50..0x143b35f80: Item Type / Order of Acquisition / Weight)
pub(crate) const MENU_SORT_TYPE_ARMAMENTS: usize = 4;
pub(crate) const MENU_SORT_TYPE_ARMOR: usize = 6;
pub(crate) const MENU_SORT_TYPE_TALISMANS: usize = 9;

pub(crate) const MENU_SORT_DEFAULTS_NOT_APPLIED: usize = 0;
pub(crate) const MENU_SORT_DEFAULTS_APPLIED: usize = 1;
pub(crate) static MENU_SORT_DEFAULTS_APPLIED_STATE: AtomicUsize =
    AtomicUsize::new(MENU_SORT_DEFAULTS_NOT_APPLIED);
