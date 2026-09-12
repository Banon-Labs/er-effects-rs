mod tests {
    use super::boot_view_present_cover_failed;

    #[test]
    fn observed_draw_present_race_is_not_a_failure() {
        assert!(!boot_view_present_cover_failed(1, 0, 83, 83, 0));
    }

    #[test]
    fn attributed_present_path_draw_without_present_full_clear_is_a_failure() {
        assert!(boot_view_present_cover_failed(1, 0, 83, 82, 0));
    }

    #[test]
    fn present_full_clear_is_not_a_failure() {
        assert!(!boot_view_present_cover_failed(1, 0, 83, 82, 1));
    }

    #[test]
    fn non_handoff_stop_reason_is_not_a_failure() {
        assert!(!boot_view_present_cover_failed(2, 0, 83, 82, 0));
    }

    #[test]
    fn stopped_boot_view_is_not_a_failure() {
        assert!(!boot_view_present_cover_failed(1, 1, 83, 82, 0));
    }
}
