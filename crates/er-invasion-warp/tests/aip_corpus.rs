//! Corpus proof for the `.aip` decoder against the REAL shipped auto-invade-point data.
//!
//! Game-derived bytes are never versioned in this repo, so this test reads the local
//! extraction corpus and SKIPs when it is absent (same contract as
//! `crates/er-gfx/tests/common/mod.rs`). What it proves when the corpus IS present is that
//! `er_invasion_warp::aip` decodes every one of the 365 shipped `.aip` files -- 7073 points --
//! exactly, and that the fingerprints compiled into the crate still describe those bytes.
//!
//! Roots (both overridable, because an extraction path embeds its own timestamp):
//! * `ER_AIP_CORPUS_ROOT`    -- the directory holding the UNPACKED `autoinvadepoint*-aipbnd-dcx`
//!   entry directories (a WitchyBND recursive unpack of `other/`).
//! * `ER_AIP_CONTAINER_ROOT` -- the directory holding the COMPRESSED `.aipbnd.dcx` containers.
//!
//! Producing the corpus is offline asset work, not a game run: WitchyBND (which needs a PTY
//! and its bundled Oodle library, because these containers are DCX/KRAK) unpacks
//! `other/autoinvadepoint.aipbnd.dcx` in one step. See docs/plans/world-map-invasion-warp.md.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use er_game_base::fnv1a::fnv1a64;
use er_invasion_warp::{
    AIP_FINGERPRINT_BASE, AIP_FINGERPRINT_DLC02, AipContainerFingerprint, BlockKey, parse_aip,
};

const DEFAULT_CORPUS_ROOT: &str =
    "/home/banon/er-extract/LOOK_HERE_WITCHY_RECURSIVE_20260713/sharded/other";
const DEFAULT_CONTAINER_ROOT: &str = "/home/banon/er-extract/LOOK_HERE_ALL_ASSETS_20260713/other";

fn root(var: &str, default: &str) -> PathBuf {
    match std::env::var(var) {
        Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
        _ => PathBuf::from(default),
    }
}

/// The unpacked-entry directory WitchyBND produces for a container: `foo.aipbnd.dcx` ->
/// `foo-aipbnd-dcx`.
fn entry_dir(corpus_root: &Path, container_name: &str) -> PathBuf {
    corpus_root.join(container_name.replace('.', "-"))
}

/// `name -> bytes` for every `.aip` in an unpacked container directory, or `None` to SKIP.
fn read_entries(dir: &Path) -> Option<BTreeMap<String, Vec<u8>>> {
    if !dir.is_dir() {
        eprintln!("SKIP: aip corpus {} not present", dir.display());
        return None;
    }
    let mut entries = BTreeMap::new();
    for entry in std::fs::read_dir(dir).expect("read corpus dir") {
        let path = entry.expect("corpus dir entry").path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".aip") {
            continue;
        }
        entries.insert(
            name.to_owned(),
            std::fs::read(&path).expect("read aip entry"),
        );
    }
    Some(entries)
}

fn check_container(fingerprint: &AipContainerFingerprint) {
    let dir = entry_dir(
        &root("ER_AIP_CORPUS_ROOT", DEFAULT_CORPUS_ROOT),
        fingerprint.container_name,
    );
    let Some(entries) = read_entries(&dir) else {
        return;
    };

    assert_eq!(
        entries.len(),
        fingerprint.entry_count,
        "{} entry count drifted",
        fingerprint.container_name
    );

    let mut digest_input: Vec<u8> = Vec::new();
    let mut total_points = 0usize;
    let mut yaw_min = f32::INFINITY;
    let mut yaw_max = f32::NEG_INFINITY;

    for (name, bytes) in &entries {
        digest_input.extend_from_slice(name.as_bytes());
        digest_input.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        digest_input.extend_from_slice(bytes);

        let parsed = parse_aip(bytes).unwrap_or_else(|error| panic!("{name}: {error}"));

        // The BND entry name is the block's map name -- so the header block id and the file
        // name are two independent statements of the same fact, and they must agree. This is
        // what catches a byte-order mistake in BlockKey::from_disk_bytes.
        assert_eq!(
            format!("{}.aip", parsed.block),
            *name,
            "{name}: header block id disagrees with the entry name"
        );
        assert_eq!(
            parsed.block.area(),
            fingerprint.area,
            "{name}: unexpected map area"
        );

        total_points += parsed.targets.len();
        for (index, target) in parsed.targets.iter().enumerate() {
            assert_eq!(target.point_index, index as u32);
            assert_eq!(target.block, parsed.block);
            assert!(
                target.position.iter().all(|c| c.is_finite()) && target.yaw.is_finite(),
                "{name}: non-finite point {index}"
            );
            yaw_min = yaw_min.min(target.yaw);
            yaw_max = yaw_max.max(target.yaw);
            // Every heading must land in (-pi, pi] after wrapping, which is what the map pin
            // needs; roughly half the shipped values start outside it.
            let heading = target.heading_radians();
            assert!(
                heading > -std::f32::consts::PI && heading <= std::f32::consts::PI,
                "{name}: yaw {} wrapped to {heading}",
                target.yaw
            );
        }
    }

    assert_eq!(
        total_points, fingerprint.point_count,
        "{} point count drifted",
        fingerprint.container_name
    );
    assert_eq!(
        fnv1a64(&digest_input),
        fingerprint.entries_fnv1a64,
        "{} entry bytes drifted from the fingerprint the RE claims were made against",
        fingerprint.container_name
    );

    // Yaw is RADIANS in (-2pi, 0], not degrees and not +/-pi. The authored extreme is -6.28
    // (two-decimal authoring, so not exactly -2pi).
    assert!(
        yaw_min >= -std::f32::consts::TAU && yaw_max <= 0.0,
        "{}: yaw range [{yaw_min}, {yaw_max}] is not the authored (-2pi, 0]",
        fingerprint.container_name
    );
}

#[test]
fn the_base_container_decodes_exactly() {
    check_container(&AIP_FINGERPRINT_BASE);
}

#[test]
fn the_dlc02_container_decodes_exactly() {
    check_container(&AIP_FINGERPRINT_DLC02);
}

#[test]
fn the_compressed_containers_match_their_recorded_lengths() {
    let root = root("ER_AIP_CONTAINER_ROOT", DEFAULT_CONTAINER_ROOT);
    for fingerprint in [&AIP_FINGERPRINT_BASE, &AIP_FINGERPRINT_DLC02] {
        let path = root.join(fingerprint.container_name);
        if !path.is_file() {
            eprintln!("SKIP: container {} not present", path.display());
            continue;
        }
        let bytes = std::fs::read(&path).expect("read container");
        assert_eq!(
            bytes.len(),
            fingerprint.container_len,
            "{} length drifted",
            fingerprint.container_name
        );
        // DCX/KRAK: the container is Oodle-compressed, which is why the crate decodes the
        // UNPACKED entries and never tries to read the container itself.
        assert_eq!(&bytes[0..4], b"DCX\0", "not a DCX container");
        assert_eq!(&bytes[0x28..0x2c], b"KRAK", "container is not Oodle/KRAK");
    }
}

#[test]
fn every_shipped_block_key_round_trips_through_the_bcd_index_packing() {
    // A corpus-free structural check: both shipped areas are inside the BCD range, so the
    // packing is on the path for every real block key, and it must be lossless there.
    for area in [AIP_FINGERPRINT_BASE.area, AIP_FINGERPRINT_DLC02.area] {
        for index in 0u8..100 {
            let key = BlockKey::from_parts(area, 40, 40, index);
            assert!(key.uses_bcd_index());
            assert_eq!(key.index(), index);
        }
    }
}
