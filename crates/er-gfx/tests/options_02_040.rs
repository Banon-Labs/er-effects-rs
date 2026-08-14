//! Proof gates for the runtime-derived 4-button System->Quit OptionSetting movie.
//!
//! No game-derived `.gfx` is versioned in the repo. These tests read the real
//! vanilla Windows `02_040_optionsetting.gfx` from the local extraction corpus and
//! skip when it is absent; the DLL uses the same transform at runtime against the
//! game's own Scaleform MemoryFile.

mod common;

use er_game_base::fnv1a::fnv1a64;
use er_gfx::options_02_040::{
    QUIT4_GRID_CELL_NAMES, QUIT4_WIN_FNV1A64, QUIT4_WIN_LEN, Quit4Error, VANILLA_WIN_FNV1A64,
    VANILLA_WIN_LEN, grid_horizontal_axis_enabled, grid_item_index, grid_vertical_axis_enabled,
    is_known_vanilla_win, measure_grid, quit4,
};
use er_gfx::{Movie, Tag};

/// The `MENU_FL_QuitGame` sprite the Quit tab's `GridControl` measures its geometry from.
const QUIT_GAME_SPRITE_ID: u16 = 138;

/// Instance names of every named `PlaceObject2` child of one sprite, in tag order.
fn placed_names(movie: &Movie, sprite_id: u16) -> Vec<String> {
    movie
        .tags
        .iter()
        .filter_map(|tag| match tag {
            Tag::DefineSprite { id, tags, .. } if *id == sprite_id => Some(tags),
            _ => None,
        })
        .flatten()
        .filter_map(|child| match child {
            Tag::PlaceObject2 { name, .. } => name.clone(),
            _ => None,
        })
        .collect()
}

/// Does the derived movie contain the cell the native measure loop probes for?
fn has_cell(names: &[String], row: u32, col: u32) -> bool {
    names.iter().any(|n| n == &format!("Item_{row}_{col}"))
}

fn read_vanilla_or_skip() -> Option<Vec<u8>> {
    common::read_vanilla_or_skip(
        "win/02_040_optionsetting.gfx",
        VANILLA_WIN_LEN,
        VANILLA_WIN_FNV1A64,
        fnv1a64,
        is_known_vanilla_win,
    )
}

#[test]
fn quit4_of_vanilla_matches_validated_fingerprint() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = quit4(&vanilla).expect("quit4 edit must apply cleanly to the known vanilla movie");
    assert_eq!(out.len(), QUIT4_WIN_LEN);
    assert_eq!(fnv1a64(&out), QUIT4_WIN_FNV1A64);
}

#[test]
fn quit4_of_already_edited_movie_fails_closed() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let edited = quit4(&vanilla).expect("quit4 edit must apply cleanly to the known vanilla movie");
    match quit4(&edited) {
        Err(Quit4Error::Edit(_)) => {}
        other => panic!("expected Edit error on already-edited input, got {other:?}"),
    }
}

/// The whole navigation and hover model of the patched Quit tab: the two added cells must extend the
/// native pair into a SECOND ROW, because `GridControl` measures its geometry from these names and
/// enables the vertical axis only at `rows >= 2` while hit-testing exactly `cols * rows` cells.
#[test]
fn the_derived_movie_measures_a_two_by_two_grid() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let vanilla_movie = Movie::parse(&vanilla).expect("vanilla movie parses");
    let vanilla_names = placed_names(&vanilla_movie, QUIT_GAME_SPRITE_ID);
    assert_eq!(
        vanilla_names,
        vec!["Item_0_0", "Item_0_1", "PlayerInfo"],
        "vanilla sprite {QUIT_GAME_SPRITE_ID} children"
    );
    assert_eq!(
        measure_grid(|row, col| has_cell(&vanilla_names, row, col)),
        (2, 1),
        "vanilla measures one horizontal row, which is why up/down does nothing there"
    );

    let out = quit4(&vanilla).expect("quit4 edit must apply cleanly to the known vanilla movie");
    let derived = Movie::parse(&out).expect("derived movie parses");
    let names = placed_names(&derived, QUIT_GAME_SPRITE_ID);
    for cell in QUIT4_GRID_CELL_NAMES {
        assert!(
            names.iter().any(|n| n == cell),
            "missing cell {cell} in {names:?}"
        );
    }
    for stale in ["Item_0_2", "Item_0_3"] {
        assert!(
            !names.iter().any(|n| n == stale),
            "{stale} would measure a 4x1 grid, disabling the vertical axis"
        );
    }
    let (cols, rows) = measure_grid(|row, col| has_cell(&names, row, col));
    assert_eq!((cols, rows), (2, 2));
    assert!(
        grid_vertical_axis_enabled(cols, rows),
        "up/down must walk rows"
    );
    assert!(
        grid_horizontal_axis_enabled(cols, rows),
        "left/right must walk columns"
    );
    assert_eq!(cols * rows, 4, "all four cells must be inside the hit test");
    // Item index order must match the order the DLL appends the property rows:
    // 0 Save Game, 1 Return to Desktop, 2 Load Character, 3 Load Character from File.
    // (Rows 2 and 3 were "Load Profile" / "Load Save Profiles" before 2026-07-31; the ORDER is
    // what this test pins, and the relabel did not move anything.)
    for (index, cell) in QUIT4_GRID_CELL_NAMES.iter().enumerate() {
        let (row, col) = (index as u32 / cols, index as u32 % cols);
        assert_eq!(grid_item_index(row, col, cols), index as u32);
        assert_eq!(&format!("Item_{row}_{col}"), cell);
    }
}

/// The measure loop's own arithmetic, independent of any movie.
#[test]
fn measure_grid_matches_the_native_loop() {
    // No cells at all: the constructor's 1x1 survives.
    assert_eq!(measure_grid(|_, _| false), (1, 1));
    // A single horizontal strip -- vanilla's shape, and vertical-only navigation is impossible.
    assert_eq!(measure_grid(|row, col| row == 0 && col < 4), (4, 1));
    assert!(!grid_vertical_axis_enabled(4, 1));
    assert!(grid_horizontal_axis_enabled(4, 1));
    // A single column of four -- horizontal navigation is impossible.
    assert_eq!(measure_grid(|row, col| col == 0 && row < 4), (1, 4));
    assert!(grid_vertical_axis_enabled(1, 4));
    assert!(!grid_horizontal_axis_enabled(1, 4));
    // A row that is missing its column 0 ends the walk, so a gap truncates the grid.
    assert_eq!(
        measure_grid(|row, col| (row == 0 || row == 2) && col < 2),
        (2, 1)
    );
    // Both caps hold.
    assert_eq!(measure_grid(|_, _| true), (32, 64));
}

#[test]
fn quit4_of_garbage_fails_closed() {
    assert!(matches!(
        quit4(b"not a gfx movie"),
        Err(Quit4Error::Parse(_))
    ));
    assert!(matches!(quit4(&[]), Err(Quit4Error::Parse(_))));
}
