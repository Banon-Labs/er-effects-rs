# er-menu-sort-dll

Isolated Elden Ring startup menu-sort DLL for the armament, armor/protector, and talisman menu categories.

This crate intentionally duplicates the menu-sort startup behavior from the product DLL instead of depending on `er-effects-rs`. It is only a workspace member; the DLL does not call into the product DLL or any other repo-local feature crate.

## Build

```bash
cargo xwin build -p er-menu-sort-dll --release --target x86_64-pc-windows-msvc
```

Output:

```text
target/x86_64-pc-windows-msvc/release/er_menu_sort_dll.dll
```

## Configuration

By default all supported categories are changed from the game's startup `Item Type` value to `Order of Acquisition` once `CSMenuSystemSaveLoad` is available.

Environment variables override config files:

- `ER_EFFECTS_MENU_SORT_ARMAMENTS`
- `ER_EFFECTS_MENU_SORT_ARMOR`
- `ER_EFFECTS_MENU_SORT_TALISMANS`

Config files are read from the game directory, with `er-menu-sort.toml` preferred and `er-effects.toml` accepted for compatibility:

```toml
menu_sort.armaments = "order_of_acquisition"
menu_sort.armor = "order_of_acquisition"
menu_sort.talismans = "order_of_acquisition"
```

Accepted values are `order_of_acquisition`, `item_type`, and `preserve`.

`menu_sort.protectors` is accepted as an alias for `menu_sort.armor` in this isolated DLL.
