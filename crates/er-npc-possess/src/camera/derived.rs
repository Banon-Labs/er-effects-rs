//! The `[camera.cNNNN]` block of `er-npc-possess.derived.toml`.
//!
//! Same contract as [`crate::moveset::derived`], which owns the rest of that file: it is output,
//! it is rewritten on every possession, and nothing reads it back. Corrections go in
//! `er-npc-possess.toml` under `[camera]` and `[chr.cNNNN].camera_distance_scale`.
//!
//! # Why the camera gets a block at all
//!
//! "The camera did not change" has half a dozen causes -- the feature is off, the build is one
//! nobody measured, the creature's `hitHeight` is zero, the row the player picked is referenced by
//! their regulation -- and from the chair they are indistinguishable. Every one of them is a
//! [`Refusal`] with a sentence next to it here. When it did change, the block prints the height it
//! read, the row it patched, the row it copied the untouched fields from, the two numbers it
//! wrote, and -- the part a player can actually check against what they can see -- where that puts
//! the top of the creature on screen and how much room is above it.

// Pure text generation; ungated so `cargo test` proves it on the host.
#![cfg_attr(not(windows), allow(dead_code))]

use std::fmt::Write as _;

use crate::camera::geometry::{self, Report};

/// Rendered into `er-npc-possess.derived.toml`, after the moveset block.
pub(crate) fn render(chr_id: u32, report: &Report) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "\n# THE CAMERA. Size comes from CSChrPhysicsModule+0x340 hitHeight, read off the\n\
         # creature itself; the numbers below are written into a free LockCamParam row in\n\
         # memory and ChrExFollowCam+0x468 is pointed at it for the length of the possession.\n\
         # Everything is put back on release. Tune with [camera] and\n\
         # [chr.c{chr_id:04}].camera_distance_scale in er-npc-possess.toml.\n\
         [camera.c{chr_id:04}]\n"
    );

    match (report.hit_height, report.hit_radius) {
        (Some(height), Some(radius)) => {
            let _ = writeln!(out, "hit_height_m = {height}");
            let _ = writeln!(out, "hit_radius_m = {radius}");
            let _ = writeln!(
                out,
                "# {:.2}x the player's own 1.5 m capsule (NpcParam row 0)",
                height / crate::camera::geometry::PLAYER_HIT_HEIGHT
            );
        }
        _ => {
            out.push_str("# the creature's size did not read\n");
        }
    }
    let _ = writeln!(out, "distance_scale = {}", report.distance_scale);

    match report.applied {
        Some(shape) => {
            let _ = writeln!(out, "patched_row = {}", report.row);
            if let Some(base) = report.base_row {
                let _ = writeln!(
                    out,
                    "# every other field in that row -- FOV, lock offset, chase rate, lock-on\n\
                     # radii -- was copied from row {base}, whatever the camera resolved a frame\n\
                     # before the possession started."
                );
            }
            let _ = writeln!(out, "cam_dist_target = {}", shape.distance);
            let _ = writeln!(out, "chr_org_offset_y = {}", shape.pivot_height);
            // Where that puts the creature. Two metres and a pivot height are not something
            // anybody can check against what they can see; "the top of your body sits here on
            // screen" is. The player's own body sits at +0.0296 with 1.0946 body-heights of sky
            // above it, and anything at or past 1.0 is cropped off the top edge.
            if let Some(height) = report.hit_height {
                let framing = geometry::framing(shape, height, 0.0);
                let _ = writeln!(out, "head_screen_y = {:.4}", framing.head_screen_y);
                let _ = writeln!(out, "headroom_heights = {:.4}", framing.headroom_heights);
                let _ = writeln!(
                    out,
                    "# the top of the body, in half-screen-heights above the centre of the\n\
                     # screen (1.0 is the top edge; the player gets +0.0296), and the gap above\n\
                     # it in body-heights (the player gets 1.0946). Both should be the player's.\n\
                     # A difference means the framing law was overridden -- by the clearance\n\
                     # floor over hit_radius_m, by [camera].distance_max, or by the\n\
                     # distance_scale above."
                );
            }
        }
        None => {
            let refusal = report.refusal;
            let name = refusal.map_or("unknown", |reason| reason.name());
            let _ = writeln!(out, "adapted = \"no: {name}\"");
            if let Some(reason) = refusal {
                let _ = writeln!(out, "# {}", reason.describe());
            }
            out.push_str(
                "# The camera is framing this creature with the parameters it would have\n\
                          # used for your own body, which on anything large puts it inside the\n\
                          # model.\n",
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::geometry::{Refusal, Shape};

    fn applied() -> Report {
        Report {
            hit_height: Some(12.0),
            hit_radius: Some(12.0),
            row: 1000,
            base_row: Some(0),
            distance_scale: 1.0,
            applied: Some(Shape {
                distance: 30.4,
                pivot_height: 11.6,
            }),
            refusal: None,
        }
    }

    #[test]
    fn an_applied_camera_prints_the_size_it_read_and_the_numbers_it_wrote() {
        let text = render(4500, &applied());
        assert!(text.contains("[camera.c4500]"), "{text}");
        assert!(text.contains("hit_height_m = 12"), "{text}");
        assert!(text.contains("patched_row = 1000"), "{text}");
        assert!(text.contains("cam_dist_target = 30.4"), "{text}");
        assert!(text.contains("chr_org_offset_y = 11.6"), "{text}");
        // ...and where that puts the creature, which is the form a player can check. This shape
        // is the law's own answer for a 12 m subject, so it must read back as the player's.
        assert!(text.contains("head_screen_y = 0.0296"), "{text}");
        assert!(text.contains("headroom_heights = 1.0946"), "{text}");
        assert!(text.contains("row 0"), "the base row is named: {text}");
    }

    /// A refusal must name itself and say what it means, because "the camera did not change" is
    /// the symptom every one of them shares.
    #[test]
    fn every_refusal_prints_its_name_and_its_sentence() {
        for reason in [
            Refusal::Disabled,
            Refusal::UnmeasuredBuild,
            Refusal::NoHeight,
            Refusal::ParamsNotReady,
            Refusal::RowInUse(1000),
            Refusal::RowMissing(4242),
            Refusal::BaseRowMissing,
            Refusal::NoFollowCam,
            Refusal::WriteFailed,
        ] {
            let text = render(4500, &Report::refused(1000, 1.0, reason));
            assert!(
                text.contains(&format!("adapted = \"no: {}\"", reason.name())),
                "{reason:?}: {text}"
            );
            assert!(text.contains(&reason.describe()), "{reason:?}: {text}");
            assert!(!text.contains("patched_row"), "{reason:?}: {text}");
        }
    }

    /// A row conflict has to name the row, or the player cannot tell which one to change.
    #[test]
    fn a_row_conflict_names_the_row() {
        let text = render(4500, &Report::refused(1000, 1.0, Refusal::RowInUse(1000)));
        assert!(
            text.contains("LockCamParam row 1000 is referenced"),
            "{text}"
        );
        assert!(text.contains("[camera].param_row"), "{text}");
    }

    /// The block has to survive this crate's own TOML reader: a player who copies a line out of it
    /// into the real config must not get a rejection they cannot explain.
    #[test]
    fn the_rendered_block_parses_as_toml() {
        let doc = crate::toml::Document::parse(&render(4500, &applied()));
        assert_eq!(doc.scalar("camera.c4500", "patched_row"), Some("1000"));
        assert_eq!(doc.scalar("camera.c4500", "cam_dist_target"), Some("30.4"));
        let refused = render(4500, &Report::refused(1000, 1.0, Refusal::Disabled));
        let doc = crate::toml::Document::parse(&refused);
        assert_eq!(doc.scalar("camera.c4500", "adapted"), Some("no: disabled"));
    }
}
