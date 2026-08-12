use super::*;

fn valid_window_facts() -> er_save_picker::DriveStripWindowFacts {
    er_save_picker::DriveStripWindowFacts {
        hwnd_present: true,
        foreground_matches: true,
        same_process: true,
        client_geometry_valid: true,
        pointer_in_client: true,
    }
}

#[test]
fn router_moves_native_row_three_to_drive_row_before_pointer_commit() {
    let bounds = save_picker_drive_strip_pointer_bounds(0);
    let decision =
        er_save_picker::route_drive_strip_pointer_move(er_save_picker::DriveStripPointerFacts {
            window: valid_window_facts(),
            native_row: 3,
            drive_row: 0,
            controls_visible: true,
            cell_count: 3,
            pointer_position: 0x1234,
            last_pointer_position: None,
            stage_x: bounds.first_cell_left + bounds.cell_pitch + 0.1,
            stage_y: bounds.row_top + 0.1,
            bounds,
        })
        .expect("row-0 pointer movement is eligible even while row 3 starts selected");
    assert_eq!(decision.target, er_save_picker::DriveStripFocus::Cell(1));
    assert_eq!(decision.native_row_focus, Some(0));
    assert_eq!(decision.commit_pointer_position, 0x1234);
}

#[test]
fn router_rejects_background_and_out_of_y_band_pointer_facts() {
    let bounds = save_picker_drive_strip_pointer_bounds(0);
    let facts = er_save_picker::DriveStripPointerFacts {
        window: er_save_picker::DriveStripWindowFacts {
            foreground_matches: false,
            ..valid_window_facts()
        },
        native_row: 3,
        drive_row: 0,
        controls_visible: true,
        cell_count: 3,
        pointer_position: 0x1234,
        last_pointer_position: None,
        stage_x: bounds.first_cell_left + 0.1,
        stage_y: bounds.row_top + 0.1,
        bounds,
    };
    assert_eq!(er_save_picker::route_drive_strip_pointer_move(facts), None);
    assert_eq!(
        er_save_picker::route_drive_strip_pointer_move(er_save_picker::DriveStripPointerFacts {
            window: valid_window_facts(),
            stage_y: bounds.row_top - 0.1,
            ..facts
        }),
        None
    );
}

#[test]
fn production_composition_ignores_later_global_lbutton_state() {
    use std::cell::Cell;

    let row_input_gate = 1_u8;
    let gate = std::ptr::from_ref(&row_input_gate);
    let synthetic_global_lbutton_down = Cell::new(true);

    let keyboard = save_picker_compose_activation_provenance_with(
        gate,
        |_| true,
        |_| panic!("native Accept short-circuits the pointer predicate"),
        |_| panic!("physical/global pointer state cannot alter Accept provenance"),
    );
    assert!(synthetic_global_lbutton_down.get());
    assert_eq!(
        keyboard,
        er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept,
        "holding an unrelated synthetic LButton state cannot relabel native Accept"
    );

    // The exact composition seam used by the hook still trusts the event-bound pointer
    // predicate after an unrelated global LButton sample has observed release.
    synthetic_global_lbutton_down.set(false);
    let bounds = save_picker_drive_strip_pointer_bounds(0);
    let physical = save_picker_compose_activation_provenance_with(
        gate,
        |_| false,
        |_| {
            assert!(!synthetic_global_lbutton_down.get());
            true
        },
        |_| {
            er_save_picker::DriveStripActivationProvenance::physical_click(
                er_save_picker::route_drive_strip_native_click(
                    valid_window_facts(),
                    0,
                    0,
                    true,
                    3,
                    bounds.first_cell_left + bounds.cell_pitch + 0.1,
                    bounds.row_top + 0.1,
                    bounds,
                ),
            )
        },
    );
    assert_eq!(
        physical,
        er_save_picker::DriveStripActivationProvenance::AcceptedPhysicalClick(
            er_save_picker::DriveStripFocus::Cell(1),
        )
    );
    assert_eq!(
        er_save_picker::resolve_drive_strip_activation(
            physical,
            Some(er_save_picker::DriveStripFocus::CurrentPath),
        ),
        Some(er_save_picker::DriveStripActivation::SelectCell(1))
    );

    assert_eq!(
        save_picker_compose_activation_provenance_with(
            gate,
            |_| true,
            |_| true,
            |_| unreachable!("Accept precedence skips physical classification"),
        ),
        er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept,
        "native Accept wins when both logical predicates assert"
    );
    assert_eq!(
        save_picker_compose_activation_provenance_with(
            gate,
            |_| false,
            |_| false,
            |_| unreachable!("Unknown does not classify a physical target"),
        ),
        er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation
    );
}

#[test]
fn exact_drive_and_path_hit_bounds_come_from_shipped_layout() {
    let bounds = save_picker_drive_strip_pointer_bounds(0);
    assert_eq!(bounds.first_cell_left, -422.0);
    assert_eq!(bounds.cell_pitch, 32.0);
    assert_eq!(bounds.cell_width, 32.0);
    assert_eq!(bounds.path_left, -182.0);
    assert_eq!(bounds.path_width, 600.0);
    assert_eq!(bounds.row_top, -236.0);
    assert_eq!(bounds.row_height, 39.0);
    let y = bounds.row_top + 0.1;
    assert_eq!(
        bounds.classify(bounds.first_cell_left, y, DRIVE_STRIP_MAX_CELLS),
        Some(er_save_picker::DriveStripFocus::Cell(0))
    );
    assert_eq!(
        bounds.classify(
            bounds.first_cell_left + bounds.cell_pitch * 6.0,
            y,
            DRIVE_STRIP_MAX_CELLS,
        ),
        Some(er_save_picker::DriveStripFocus::Cell(6))
    );
    assert_eq!(
        bounds.classify(bounds.path_left, y, DRIVE_STRIP_MAX_CELLS),
        Some(er_save_picker::DriveStripFocus::CurrentPath)
    );
    assert_eq!(
        bounds.classify(
            bounds.path_left + bounds.path_width,
            y,
            DRIVE_STRIP_MAX_CELLS
        ),
        None
    );
    assert_eq!(
        bounds.classify(
            bounds.path_left,
            bounds.row_top - 0.1,
            DRIVE_STRIP_MAX_CELLS
        ),
        None
    );
}

#[test]
fn native_drive_buttons_render_only_the_drive_name_and_use_color_for_selection() {
    let selected = save_picker_drive_cell_html_utf16(">C:<");
    let idle = save_picker_drive_cell_html_utf16("[S:]");
    let selected = String::from_utf16(&selected[..selected.len() - 1]).expect("valid UTF-16");
    let idle = String::from_utf16(&idle[..idle.len() - 1]).expect("valid UTF-16");
    assert!(selected.contains("C:"));
    assert!(!selected.contains(">>C:<"));
    assert!(selected.contains("#d8a052"));
    assert!(idle.contains("S:"));
    assert!(!idle.contains(">[S:]"));
    assert!(idle.contains("#8f887a"));
}

#[test]
fn one_physical_arrow_source_produces_exactly_one_drive_action() {
    assert_eq!(drive_strip_nav_pressed_mask(0), 0);
    assert_eq!(
        drive_strip_nav_pressed_mask(crate::experiments::SAVE_PICKER_NAV_RIGHT_MASK),
        SAVE_PICKER_DRIVE_STRIP_RIGHT_MASK
    );
}

#[test]
fn pending_native_click_target_encodes_without_cell_path_aliases() {
    for cell in 0..3 {
        let target = er_save_picker::DriveStripFocus::Cell(cell);
        assert_eq!(
            save_picker_decode_pending_drive_strip_target(
                save_picker_encode_pending_drive_strip_target(target),
                3,
            ),
            Some(target)
        );
    }
    assert_eq!(
        save_picker_decode_pending_drive_strip_target(
            SAVE_PICKER_DRIVE_STRIP_PATH_EDITOR_PENDING,
            3,
        ),
        Some(er_save_picker::DriveStripFocus::CurrentPath)
    );
    assert_eq!(
        save_picker_decode_pending_drive_strip_target(SAVE_PICKER_DRIVE_STRIP_NO_PENDING_CELL, 3,),
        None
    );
    assert_eq!(
        save_picker_decode_drive_strip_activation_provenance(
            SAVE_PICKER_DRIVE_STRIP_NO_PENDING_CELL,
            3,
        ),
        er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation
    );
    assert_eq!(
        save_picker_decode_drive_strip_activation_provenance(
            SAVE_PICKER_DRIVE_STRIP_KEYBOARD_ACCEPT_PENDING,
            3,
        ),
        er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept
    );
    assert_eq!(
        save_picker_decode_drive_strip_activation_provenance(
            SAVE_PICKER_DRIVE_STRIP_UNKNOWN_ACTIVATION_PENDING,
            3,
        ),
        er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation
    );
    assert_eq!(
        save_picker_decode_drive_strip_activation_provenance(
            SAVE_PICKER_DRIVE_STRIP_ORDINARY_PHYSICAL_PENDING,
            3,
        ),
        er_save_picker::DriveStripActivationProvenance::OrdinaryRowPhysicalActivation
    );
    assert_eq!(
        save_picker_decode_drive_strip_activation_provenance(
            SAVE_PICKER_DRIVE_STRIP_REJECTED_PHYSICAL_PENDING,
            3,
        ),
        er_save_picker::DriveStripActivationProvenance::RejectedPhysicalClick
    );
    assert_eq!(
        save_picker_decode_drive_strip_activation_provenance(
            SAVE_PICKER_DRIVE_STRIP_CONSUMED_ACTIVATION_PENDING,
            3,
        ),
        er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation
    );
    assert_eq!(
        save_picker_decode_drive_strip_activation_provenance(
            save_picker_encode_pending_drive_strip_target(er_save_picker::DriveStripFocus::Cell(2),),
            3,
        ),
        er_save_picker::DriveStripActivationProvenance::AcceptedPhysicalClick(
            er_save_picker::DriveStripFocus::Cell(2),
        )
    );
    assert_eq!(
        save_picker_decode_drive_strip_activation_provenance(3, 3),
        er_save_picker::DriveStripActivationProvenance::RejectedPhysicalClick,
        "a stale physical cell may not degrade into keyboard Accept"
    );
}

#[test]
fn consumed_physical_provenance_cannot_replay_as_keyboard_accept() {
    for provenance in [
        er_save_picker::DriveStripActivationProvenance::AcceptedPhysicalClick(
            er_save_picker::DriveStripFocus::Cell(2),
        ),
        er_save_picker::DriveStripActivationProvenance::RejectedPhysicalClick,
        er_save_picker::DriveStripActivationProvenance::OrdinaryRowPhysicalActivation,
    ] {
        save_picker_clear_pending_drive_strip_target();
        save_picker_arm_drive_strip_activation_provenance(provenance);
        assert_eq!(
            save_picker_take_drive_strip_activation_provenance(3),
            provenance
        );
        assert_eq!(
            save_picker_take_drive_strip_activation_provenance(3),
            er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation,
            "a second callback in the same native forward is ignored, not keyboard fallback"
        );
        save_picker_clear_pending_drive_strip_target();
        assert_eq!(
            save_picker_take_drive_strip_activation_provenance(3),
            er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation,
            "clear leaves no rejected/physical residue or keyboard fallback"
        );
    }

    save_picker_clear_pending_drive_strip_target();
    save_picker_arm_drive_strip_activation_provenance(
        er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept,
    );
    assert_eq!(
        save_picker_take_drive_strip_activation_provenance(3),
        er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept
    );
    assert_eq!(
        save_picker_take_drive_strip_activation_provenance(3),
        er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation,
        "explicit keyboard provenance is consumed once"
    );

    save_picker_clear_pending_drive_strip_target();
    save_picker_arm_drive_strip_activation_provenance(
        er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation,
    );
    assert_eq!(
        save_picker_take_drive_strip_activation_provenance(3),
        er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation,
        "explicit Unknown provenance is armed and consumed fail-closed"
    );

    save_picker_clear_pending_drive_strip_target();
    assert_eq!(
        save_picker_take_drive_strip_activation_provenance(3),
        er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation,
        "absence is fail-closed and never implicit keyboard Accept"
    );
}

#[test]
fn complete_path_hit_target_is_distinct_from_drive_cells() {
    let bounds = save_picker_drive_strip_pointer_bounds(0);
    let y = bounds.row_top + 0.1;
    assert_eq!(
        bounds.classify(bounds.path_left, y, DRIVE_STRIP_MAX_CELLS),
        Some(er_save_picker::DriveStripFocus::CurrentPath)
    );
    assert_eq!(
        bounds.classify(
            bounds.path_left + bounds.path_width - 0.1,
            y,
            DRIVE_STRIP_MAX_CELLS,
        ),
        Some(er_save_picker::DriveStripFocus::CurrentPath)
    );
    assert_eq!(
        bounds.classify(
            bounds.path_left + bounds.path_width,
            y,
            DRIVE_STRIP_MAX_CELLS,
        ),
        None
    );
}

#[test]
fn accept_activates_only_the_explicitly_focused_subtarget() {
    assert_eq!(
        er_save_picker::resolve_drive_strip_activation(
            er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept,
            Some(er_save_picker::DriveStripFocus::Cell(2)),
        ),
        Some(er_save_picker::DriveStripActivation::SelectCell(2))
    );
    assert_eq!(
        er_save_picker::resolve_drive_strip_activation(
            er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept,
            Some(er_save_picker::DriveStripFocus::CurrentPath),
        ),
        Some(er_save_picker::DriveStripActivation::OpenCurrentPath)
    );
}

#[test]
fn live_profile_select_cursor_is_the_model_row_index() {
    assert_eq!(save_picker_model_row_from_native_cursor(-1), None);
    assert_eq!(save_picker_model_row_from_native_cursor(0), Some(0));
    assert_eq!(save_picker_model_row_from_native_cursor(1), Some(1));
    assert_eq!(save_picker_model_row_from_native_cursor(9), Some(9));
    assert_eq!(save_picker_model_row_from_native_cursor(10), None);
}

fn client_x_for_stage_x(stage_x: f32, client_width: f32, client_height: f32) -> f32 {
    let movie_aspect = PROFILE_SELECT_MOVIE_WIDTH_PX / PROFILE_SELECT_MOVIE_HEIGHT_PX;
    let client_aspect = client_width / client_height;
    if client_aspect > movie_aspect {
        let content_w = client_height * movie_aspect;
        ((client_width - content_w) * 0.5)
            + ((stage_x + PROFILE_SELECT_MOVIE_WIDTH_PX * 0.5) / PROFILE_SELECT_MOVIE_WIDTH_PX)
                * content_w
    } else {
        ((stage_x + PROFILE_SELECT_MOVIE_WIDTH_PX * 0.5) / PROFILE_SELECT_MOVIE_WIDTH_PX)
            * client_width
    }
}

#[test]
fn live_cursor_mapping_uses_fixed_movie_stage_not_user_resolution() {
    let (hit_left, pitch, _) = drive_strip_hit_geometry();
    let second_cell_x = hit_left + pitch + 0.1;
    for (client_width, client_height) in [
        (1920.0, 1080.0),
        (2560.0, 1440.0),
        (3440.0, 1440.0),
        (1024.0, 768.0),
    ] {
        let client_x = client_x_for_stage_x(second_cell_x, client_width, client_height);
        let client_y = client_height * 0.5;
        let (stage_x, stage_y) = save_picker_client_point_to_movie_stage(
            client_x,
            client_y,
            client_width,
            client_height,
        )
        .expect("point should lie inside the fitted movie stage");
        assert!(
            (stage_x - second_cell_x).abs() < 0.02,
            "client {client_width}x{client_height} mapped x={stage_x}, not the fixed movie-stage boundary {second_cell_x}"
        );
        assert!(stage_y.abs() < 0.02);
        let bounds = save_picker_drive_strip_pointer_bounds(0);
        assert_eq!(
            bounds.classify(stage_x, bounds.row_top + 0.1, DRIVE_STRIP_MAX_CELLS),
            Some(er_save_picker::DriveStripFocus::Cell(1))
        );
    }
}

#[test]
fn live_cursor_mapping_rejects_pillarbox_margin() {
    assert_eq!(
        save_picker_client_point_to_movie_stage(100.0, 720.0, 3440.0, 1440.0),
        None,
        "ultrawide pillarbox margin must not be treated as movie coordinates"
    );
}
