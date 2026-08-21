use std::time::Duration;

use super::*;

/// Build a model with a fixed listing, bypassing the filesystem enumeration `open` does.
///
/// Each file carries ONE character whose name encodes the file's own index (`char{idx}`), and a
/// modification time of epoch + `idx` minutes, so a test can assert that the character info AND
/// the timestamp a row renders belong to that row's OWN file rather than a neighbour's -- the
/// exact confusion `row_file_characters` had.
///
/// `drives` is left EMPTY, so these models have no drive cycler row: the drive-row tests opt
/// in explicitly via `with_drives`, and every other test keeps the no-drive-row layout.
fn model_with(intent: PickerIntent, dir: &str, files: usize) -> SavePickerModel {
    SavePickerModel {
        current_dir: PathBuf::from(dir),
        extension: "sl2".to_owned(),
        extensions: vec!["sl2".to_owned()],
        entries: (0..files)
            .map(|idx| PickerEntry::File {
                name: format!("save{idx}.sl2"),
                path: PathBuf::from(dir).join(format!("save{idx}.sl2")),
                modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(idx as u64 * 60)),
                chars: vec![crate::slots::SaveSlotInfo {
                    slot: 0,
                    name: format!("char{idx}"),
                    level: 10 + idx as i32,
                }],
            })
            .collect(),
        scroll_offset: 0,
        cursor: 0,
        drive_strip_offset: 0,
        status_message: None,
        rejected_path_text: None,
        drives: Vec::new(),
        last_dir_per_drive: HashMap::new(),
        intent,
        drive_strip_path_focused: false,
    }
}

/// Attach mounted drives so the stable drive/path row exists (one or more) or deliberately does not.
fn with_drives(mut model: SavePickerModel, drives: &[&str]) -> SavePickerModel {
    model.drives = drives.iter().map(PathBuf::from).collect();
    model
}

/// The character name a row's stats text would render, or `None` for a non-file row.
fn row_char_name(model: &SavePickerModel, row: usize) -> Option<String> {
    model
        .row_file_characters(row)
        .and_then(|chars| chars.first())
        .map(|info| info.name.clone())
}

/// The file stem a row's LABEL would render, or `None` for a non-file row.
fn row_label_file(model: &SavePickerModel, row: usize) -> Option<String> {
    match model.row_meaning(row) {
        PickerRow::File(path) => Some(
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned(),
        ),
        _ => None,
    }
}

fn label_of(model: &SavePickerModel, row: usize) -> String {
    String::from_utf16(&model.row_label_utf16(row)).expect("row label is valid UTF-16")
}

fn destination(dir: &str, files: usize) -> SavePickerModel {
    destination_loading(dir, files, "Z:\\elsewhere\\ER0000.sl2")
}

/// A destination browse whose LOADED save is `loaded`, so the `[CURRENT]` marker has something
/// to point at. The default `destination` deliberately loads a save that is not in the listing.
fn destination_loading(dir: &str, files: usize, loaded: &str) -> SavePickerModel {
    model_with(
        PickerIntent::SaveDestination {
            loaded_file_name: "ER0000.sl2".to_owned(),
            loaded_path: PathBuf::from(loaded),
        },
        dir,
        files,
    )
}

// -----------------------------------------------------------------------------------------
// SYNTHETIC SAVE CONTAINERS. Deterministic generators, never captured game bytes (repo rule:
// no game-derived binaries in tree, test fixtures included). They reproduce only the fields
// the readers under test actually parse: the BND4 header/entry index, `USER_DATA010`'s
// active-slot bytes, and a PlayerGameData block placed where the FACE-anchored locator scans.
// -----------------------------------------------------------------------------------------

/// PlayerGameData field offsets the plausibility core reads (`loading_cover_save_slot.rs`).
const PGD_HEALTH: usize = 0x08;
const PGD_MAX_HEALTH: usize = 0x0c;
const PGD_BASE_MAX_HEALTH: usize = 0x10;
const PGD_STAT_BASE: usize = 0x34;
const PGD_STAT_COUNT: usize = 8;
const PGD_LEVEL: usize = 0x60;
const PGD_NAME: usize = 0x94;
const PGD_GENDER: usize = 0xb6;
const PGD_MAX_CRIMSON: usize = 0xf9;
const PGD_MAX_CERULEAN: usize = 0xfa;
/// The locator finds `FACE` and scans back over `[face-0xa600, face-0xa000]`; putting the
/// block at exactly `face - 0xa000` makes the accepted offset the only plausible one, because
/// every other candidate reads zeros and fails the name/level checks.
const PGD_AT: usize = 0x100;
const FACE_AT: usize = PGD_AT + 0xa000;
const SLOT_BODY_BYTES: usize = FACE_AT + 0x10;
/// `USER_DATA010` body: a zero-length `CSMenuSystemSaveLoad` blob at `0x150`, so the 10
/// active-slot bytes sit at `0x154`.
const SYSTEM_BODY_BYTES: usize = 0x200;
const SYSTEM_MENU_SAVE_LOAD_LEN: usize = 0x150;
const SYSTEM_ACTIVE_SLOTS: usize = 0x154;

/// One character slot's plaintext body carrying a locatable, plausible character.
fn synthetic_slot_body(name: &str, level: u32) -> Vec<u8> {
    let mut body = vec![0_u8; SLOT_BODY_BYTES];
    body[FACE_AT..FACE_AT + 4].copy_from_slice(b"FACE");
    let put32 = |body: &mut Vec<u8>, at: usize, v: u32| {
        body[PGD_AT + at..PGD_AT + at + 4].copy_from_slice(&v.to_le_bytes());
    };
    put32(&mut body, PGD_HEALTH, 1000);
    put32(&mut body, PGD_MAX_HEALTH, 1000);
    put32(&mut body, PGD_BASE_MAX_HEALTH, 1000);
    for index in 0..PGD_STAT_COUNT {
        put32(&mut body, PGD_STAT_BASE + index * 4, 10);
    }
    put32(&mut body, PGD_LEVEL, level);
    body[PGD_AT + PGD_GENDER] = 0;
    body[PGD_AT + PGD_MAX_CRIMSON] = 4;
    body[PGD_AT + PGD_MAX_CERULEAN] = 3;
    for (index, unit) in name.encode_utf16().take(15).enumerate() {
        let at = PGD_AT + PGD_NAME + index * 2;
        body[at..at + 2].copy_from_slice(&unit.to_le_bytes());
    }
    body
}

/// A structurally complete BND4 save container. `slots[i]` is `Some((name, level))` for an
/// ACTIVE character slot and `None` for an inactive one; a `USER_DATA010` entry always carries
/// the resulting active-slot bytes.
fn synthetic_save_container(slots: &[Option<(&str, u32)>]) -> Vec<u8> {
    const HEADER_LEN: usize = 0x40;
    const ENTRY_STRIDE: usize = 0x20;
    const MD5_LEN: usize = 0x10;
    let mut bodies: Vec<(String, Vec<u8>)> = slots
        .iter()
        .enumerate()
        .filter_map(|(slot, present)| {
            present.map(|(name, level)| {
                (
                    format!("USER_DATA{slot:03}"),
                    synthetic_slot_body(name, level),
                )
            })
        })
        .collect();
    let mut system = vec![0_u8; SYSTEM_BODY_BYTES];
    system[SYSTEM_MENU_SAVE_LOAD_LEN..SYSTEM_MENU_SAVE_LOAD_LEN + 4]
        .copy_from_slice(&0_u32.to_le_bytes());
    for (slot, present) in slots.iter().enumerate().take(PICKER_ROW_COUNT) {
        system[SYSTEM_ACTIVE_SLOTS + slot] = u8::from(present.is_some());
    }
    bodies.push(("USER_DATA010".to_owned(), system));

    let names_at = HEADER_LEN + bodies.len() * ENTRY_STRIDE;
    let name_blobs: Vec<Vec<u8>> = bodies
        .iter()
        .map(|(name, _)| {
            let mut out: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
            out.extend_from_slice(&[0, 0]);
            out
        })
        .collect();
    let data_at = names_at + name_blobs.iter().map(Vec::len).sum::<usize>();
    let total = data_at
        + bodies
            .iter()
            .map(|(_, body)| MD5_LEN + body.len())
            .sum::<usize>();
    let mut out = vec![0_u8; total];
    out[..4].copy_from_slice(b"BND4");
    out[0x0c..0x10].copy_from_slice(&(bodies.len() as i32).to_le_bytes());
    out[0x10..0x18].copy_from_slice(&(HEADER_LEN as i64).to_le_bytes());
    out[0x20..0x28].copy_from_slice(&(ENTRY_STRIDE as i64).to_le_bytes());
    let mut name_cursor = names_at;
    let mut data_cursor = data_at;
    for (index, ((_, body), name_blob)) in bodies.iter().zip(&name_blobs).enumerate() {
        let entry = HEADER_LEN + index * ENTRY_STRIDE;
        let entry_size = MD5_LEN + body.len();
        out[entry + 0x08..entry + 0x10].copy_from_slice(&(entry_size as i64).to_le_bytes());
        out[entry + 0x10..entry + 0x14].copy_from_slice(&(data_cursor as i32).to_le_bytes());
        out[entry + 0x14..entry + 0x18].copy_from_slice(&(name_cursor as i32).to_le_bytes());
        out[name_cursor..name_cursor + name_blob.len()].copy_from_slice(name_blob);
        name_cursor += name_blob.len();
        let body_at = data_cursor + MD5_LEN;
        out[body_at..body_at + body.len()].copy_from_slice(body);
        data_cursor += entry_size;
    }
    out
}

/// An empty temp directory of our own, so a listing test sees exactly the files it wrote.
fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("er-save-picker-accepts-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
    dir
}

fn write_file(dir: &Path, leaf: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(leaf);
    std::fs::write(&path, bytes).expect("temp file must be writable");
    path
}

const SL2: &[&str] = &["sl2"];

fn load(path: &Path) -> Result<Vec<crate::slots::SaveSlotInfo>, PickRejection> {
    save_picker_accepts(path, &PickerIntent::LoadSource, SL2)
}

fn dest(path: &Path) -> Result<Vec<crate::slots::SaveSlotInfo>, PickRejection> {
    save_picker_accepts(path, &dest_intent("Z:\\elsewhere\\ER0000.sl2"), SL2)
}

/// A destination intent loading `loaded`. The leaf is always `ER0000.sl2` because that is what
/// `[ new ]` writes; only the folder differs between these tests.
fn dest_intent(loaded: &str) -> PickerIntent {
    PickerIntent::SaveDestination {
        loaded_file_name: "ER0000.sl2".to_owned(),
        loaded_path: PathBuf::from(loaded),
    }
}

/// The generator has to actually produce a container the shipping reader accepts, or every
/// assertion built on it is vacuous.
#[test]
fn the_synthetic_container_parses_as_a_real_loadable_save() {
    let bytes = synthetic_save_container(&[Some(("Tarnished", 42)), None, Some(("Second", 7))]);
    assert!(er_save_loader::bnd4::parse_entries(&bytes).is_ok());
    let chars = crate::slots::parse_save_character_slots(&bytes);
    assert_eq!(
        chars
            .iter()
            .map(|info| (info.slot, info.name.as_str(), info.level))
            .collect::<Vec<_>>(),
        vec![(0, "Tarnished", 42), (2, "Second", 7)]
    );
}

#[test]
fn the_load_intent_rejects_everything_that_is_not_a_loadable_container() {
    let dir = scratch_dir("load-rejects");
    assert_eq!(load(&dir), Err(PickRejection::NotAFile));
    assert_eq!(
        load(&write_file(&dir, "notes.txt", b"not a save")),
        Err(PickRejection::WrongExtension)
    );
    assert_eq!(
        load(&write_file(&dir, "ER0000.bak", b"right name, wrong flavor")),
        Err(PickRejection::WrongExtension)
    );
    assert_eq!(
        load(&dir.join("absent.sl2")),
        Err(PickRejection::NotAFile),
        "a path that does not exist is not a load source"
    );
    let mut truncated = synthetic_save_container(&[Some(("Tarnished", 42))]);
    truncated.truncate(0x20);
    assert_eq!(
        load(&write_file(&dir, "truncated.sl2", &truncated)),
        Err(PickRejection::NotBnd4)
    );
    let slotless = synthetic_save_container(&[None, None]);
    assert_eq!(
        load(&write_file(&dir, "slotless.sl2", &slotless)),
        Err(PickRejection::NoLoadableCharacter)
    );
    let level_zero = synthetic_save_container(&[Some(("Deleted", 0))]);
    assert_eq!(
        load(&write_file(&dir, "level0.sl2", &level_zero)),
        Err(PickRejection::NoLoadableCharacter),
        "a level-0 leftover fails the autoload's own real-character fingerprint"
    );
    let real = synthetic_save_container(&[Some(("Tarnished", 42))]);
    let chars = load(&write_file(&dir, "real.sl2", &real)).expect("a real save is a load source");
    assert_eq!(chars.len(), 1);
    assert_eq!(chars[0].name, "Tarnished");
}

/// THE INTENT ASYMMETRY, pinned. A destination is an overwrite target: it needs no loadable
/// character (hiding a slotless file would let `[ new ]` clobber it silently) and it need not
/// exist at all (`[ new ]` and Save-As both name a file that does not). Its FOLDER must exist.
#[test]
fn the_destination_intent_accepts_what_the_load_intent_refuses() {
    let dir = scratch_dir("dest-accepts");
    let slotless = write_file(
        &dir,
        "slotless.sl2",
        &synthetic_save_container(&[None, None]),
    );
    assert_eq!(load(&slotless), Err(PickRejection::NoLoadableCharacter));
    assert_eq!(
        dest(&slotless),
        Ok(Vec::new()),
        "a slotless existing container is a legal overwrite target"
    );
    assert_eq!(
        dest(&dir.join("brand-new.sl2")),
        Ok(Vec::new()),
        "a leaf that does not exist yet in an existing folder is a legal destination"
    );
    assert_eq!(
        dest(&dir.join("gone").join("brand-new.sl2")),
        Err(PickRejection::ParentMissing)
    );
    assert_eq!(
        dest(&dir.join("wrong.co2")),
        Err(PickRejection::WrongExtension),
        "the flavor filter still applies to a destination"
    );
    assert_eq!(dest(&dir), Err(PickRejection::NotAFile));
}

/// CONTRACT 7: there is not a second notion of "valid save". Whatever the in-game listing shows
/// is exactly what the predicate accepts, in both intents -- so the OS dialog, which calls the
/// same predicate, cannot load a container the browser would have hidden.
#[test]
fn the_listing_and_the_predicate_agree_file_for_file() {
    let dir = scratch_dir("listing-agreement");
    let candidates = [
        write_file(
            &dir,
            "real.sl2",
            &synthetic_save_container(&[Some(("Ranni", 5))]),
        ),
        write_file(&dir, "slotless.sl2", &synthetic_save_container(&[None])),
        write_file(&dir, "garbage.sl2", b"not bnd4 at all"),
        write_file(&dir, "notes.txt", b"ignored"),
    ];
    for (intent, model) in [
        (
            PickerIntent::LoadSource,
            SavePickerModel::open_with_extensions(&dir, SL2),
        ),
        (
            dest_intent("Z:\\elsewhere\\ER0000.sl2"),
            SavePickerModel::open_destination(
                &dir,
                SL2,
                "ER0000.sl2",
                Path::new("Z:\\elsewhere\\ER0000.sl2"),
            ),
        ),
    ] {
        let mut listed: Vec<PathBuf> = model
            .entries
            .iter()
            .filter(|entry| matches!(entry, PickerEntry::File { .. }))
            .map(|entry| entry.path().to_path_buf())
            .collect();
        let mut accepted: Vec<PathBuf> = candidates
            .iter()
            .filter(|path| save_picker_accepts(path, &intent, SL2).is_ok())
            .cloned()
            .collect();
        listed.sort();
        accepted.sort();
        assert_eq!(
            listed, accepted,
            "the {intent:?} listing and the predicate disagree about which files are saves"
        );
    }
}

/// A directory that genuinely exists, so the drive-resume tests exercise the real existence
/// filter instead of an invented path. Returns `(created_dir, its drive root)`.
fn real_dir_and_root(tag: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("er-save-picker-{tag}"));
    std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
    let mut root = dir.as_path();
    while let Some(parent) = root.parent() {
        if parent.as_os_str().is_empty() {
            break;
        }
        root = parent;
    }
    (dir.clone(), root.to_path_buf())
}

/// `[ new ]` remains above the parent row (moved 2026-07-31, when the Save Game row press
/// started opening this list with no confirm in front of it). With no drive strip it is still
/// row 0; when the drive strip exists, that stable location control is the sole row above it and
/// the explicit initial-cursor rule still starts on `[ new ]`.
#[test]
fn destination_keeps_new_file_above_parent_but_below_the_drive_row() {
    let model = destination("Z:\\saves", 3);
    // No drive row here, and `Z:\saves` has a parent: `[ new ]` at 0, up at 1, entries from 2.
    assert_eq!(model.new_file_row(), Some(0));
    assert_eq!(model.parent_row(), Some(1));
    assert_eq!(model.drive_row(), None);
    assert_eq!(
        model.row_meaning(0),
        PickerRow::NewFile(PathBuf::from("Z:\\saves").join("ER0000.sl2"))
    );
    assert_eq!(model.row_meaning(1), PickerRow::ParentDir);
    for (offset, expected) in (0..3).enumerate() {
        assert_eq!(
            model.row_meaning(2 + offset),
            PickerRow::File(PathBuf::from("Z:\\saves").join(format!("save{expected}.sl2")))
        );
    }
    assert_eq!(model.row_meaning(5), PickerRow::Empty);
    assert_eq!(model.visible_row_count(), 5);
    // At a single-drive root there is no drive or parent row, so `[ new ]` is still 0 and
    // entries follow it directly.
    let root = destination("Z:\\", 2);
    assert_eq!(root.new_file_row(), Some(0));
    assert_eq!(root.parent_row(), None);
    assert_eq!(
        root.row_meaning(1),
        PickerRow::File(PathBuf::from("Z:\\").join("save0.sl2"))
    );
}

/// REGRESSION: the per-row character info was read at `row - 1` while the row LABEL came from
/// `entry_row_base()`, so in destination intent every row showed the character info of the file
/// one entry further down and the pinned `[ new ]` row showed the first file's info. Pin the
/// invariant that matters -- the text a row renders describes that row's OWN file -- across
/// BOTH intents AND with the drive row present, which is the layout shift most likely to
/// reintroduce it.
#[test]
fn row_character_info_belongs_to_that_rows_own_file() {
    for model in [
        destination("Z:\\saves", 3),
        model_with(PickerIntent::LoadSource, "Z:\\saves", 3),
        with_drives(destination("Z:\\saves", 3), &["C:\\", "Z:\\"]),
        with_drives(
            model_with(PickerIntent::LoadSource, "Z:\\saves", 3),
            &["C:\\", "Z:\\"],
        ),
        // At a drive root there is no up row, so the entry base shifts down by one.
        with_drives(
            model_with(PickerIntent::LoadSource, "Z:\\", 3),
            &["C:\\", "Z:\\"],
        ),
    ] {
        let mut file_rows = 0;
        for row in 0..PICKER_ROW_COUNT {
            match row_label_file(&model, row) {
                Some(file) => {
                    // "save2.sl2" must render "char2", never a neighbour's character.
                    let idx = file
                        .trim_start_matches("save")
                        .trim_end_matches(".sl2")
                        .to_owned();
                    assert_eq!(
                        row_char_name(&model, row),
                        Some(format!("char{idx}")),
                        "row {row} labelled {file} rendered another file's character"
                    );
                    file_rows += 1;
                }
                // Every non-file row (up, drive, [ new ], page cycler, placeholder) must render
                // no character info at all, or it shows junk borrowed from a real file.
                None => assert_eq!(
                    row_char_name(&model, row),
                    None,
                    "non-file row {row} rendered character info"
                ),
            }
        }
        assert_eq!(file_rows, 3, "expected all three files to occupy rows");
    }
}

fn assert_visible_labels_fit_profile_summary_budget(model: &SavePickerModel) {
    for row in 0..model.visible_row_count() {
        let label = model.row_label_utf16(row);
        assert!(
            !label.is_empty(),
            "visible row {row} ({:?}) must have a non-empty ProfileSummary label",
            model.row_meaning(row)
        );
        assert!(
            label.len() <= PICKER_ROW_NAME_UTF16_MAX,
            "visible row {row} ({:?}) spent {} UTF-16 units in the ProfileSummary name",
            model.row_meaning(row),
            label.len()
        );
    }
}

/// Load-source browse rows keep their 16-unit native names short and move explanatory
/// navigation text into the two auxiliary stats lines. File rows still own those lines for real
/// character summaries, so they deliberately have no auxiliary replacement.
#[test]
fn load_source_auxiliary_lines_describe_navigation_without_name_budget_drift() {
    let model = with_drives(
        model_with(PickerIntent::LoadSource, r"Z:\saves", 2),
        &[r"C:\", r"Z:\"],
    );
    assert_visible_labels_fit_profile_summary_budget(&model);

    assert_eq!(model.row_meaning(0), PickerRow::DriveCycle);
    assert_eq!(label_of(&model, 0), "DRIVES");
    assert_eq!(model.row_auxiliary_lines(0), None);
    assert_eq!(model.row_meaning(1), PickerRow::ParentDir);
    assert_eq!(
        model.row_auxiliary_lines(1),
        Some(("PARENT FOLDER".to_owned(), r"Go to Z:\".to_owned()))
    );
    assert!(
        matches!(model.row_meaning(2), PickerRow::File(_)),
        "entry base must still derive from the load-source layout"
    );
    assert_eq!(
        model.row_auxiliary_lines(2),
        None,
        "file rows use the stats fields for character info, not navigation copy"
    );
}

/// Save-destination browse adds `[ new ]` below the always-first drive row and above the parent;
/// the auxiliary lines follow those derived meanings rather than hard-coded row offsets. Overflow
/// leaves the row slots for real entries; the movie scrollbar owns the visual affordance.
#[test]
fn save_destination_auxiliary_lines_follow_the_shifted_layout() {
    let model = with_drives(
        destination(r"Z:\saves", PICKER_ROW_COUNT + 4),
        &[r"C:\", r"Z:\"],
    );
    assert_visible_labels_fit_profile_summary_budget(&model);

    assert_eq!(model.row_meaning(0), PickerRow::DriveCycle);
    assert_eq!(label_of(&model, 0), "DRIVES");
    assert_eq!(model.row_auxiliary_lines(0), None);
    assert_eq!(
        model.row_auxiliary_lines(1),
        Some(("NEW SAVE FILE".to_owned(), "Create ER0000.sl2".to_owned()))
    );
    assert_eq!(model.row_meaning(2), PickerRow::ParentDir);
    assert_eq!(
        model.row_auxiliary_lines(2),
        Some(("PARENT FOLDER".to_owned(), r"Go to Z:\".to_owned()))
    );
    assert_eq!(model.next_page_row(), None);
    assert_eq!(model.scroll_up_row(), None);
    assert_eq!(model.scroll_down_row(), None);
    assert_eq!(model.visible_row_count(), PICKER_ROW_COUNT);
    assert_eq!(
        model.row_meaning(9),
        PickerRow::File(PathBuf::from(r"Z:\saves").join("save6.sl2"))
    );
    assert_eq!(model.row_auxiliary_lines(9), None);
}

/// Auxiliary copy is keyed to the same row meanings activation uses. A directory line means
/// activation opens that directory; a `[ new ]` line means activation picks that exact target.
#[test]
fn auxiliary_lines_describe_the_same_rows_activation_uses() {
    let mut load = model_with(PickerIntent::LoadSource, r"Z:\saves", 0);
    let child = PathBuf::from(r"Z:\saves").join("sub");
    load.entries.push(PickerEntry::Dir {
        name: "sub".to_owned(),
        path: child.clone(),
    });
    let dir_row = load.entry_row_base();
    assert_eq!(load.row_meaning(dir_row), PickerRow::Dir(child.clone()));
    assert_eq!(
        load.row_auxiliary_lines(dir_row),
        Some(("FOLDER".to_owned(), "Open sub/".to_owned()))
    );
    assert_eq!(load.activate(dir_row), PickerActivation::Repopulate);
    assert_eq!(load.current_dir(), child.as_path());

    let mut dest = destination(r"Z:\saves", 0);
    let target = PathBuf::from(r"Z:\saves").join("ER0000.sl2");
    assert_eq!(dest.row_meaning(0), PickerRow::NewFile(target.clone()));
    assert_eq!(
        dest.row_auxiliary_lines(0),
        Some(("NEW SAVE FILE".to_owned(), "Create ER0000.sl2".to_owned()))
    );
    assert_eq!(dest.activate(0), PickerActivation::PickedNewFile(target));
}

/// Status/rejection text also lives in the auxiliary text, so the runtime hook can render it
/// through the same `ErStats` field while leaving row names short.
#[test]
fn status_message_auxiliary_lines_override_row_zero_only() {
    let mut model = with_drives(
        model_with(PickerIntent::LoadSource, r"Z:\saves", 2),
        &[r"C:\", r"Z:\"],
    );
    assert_eq!(model.row_meaning(0), PickerRow::DriveCycle);
    model.set_status_message(PickerStatusMessage::new(
        "UNREADABLE SAVE",
        "Pick another file.",
    ));
    assert_visible_labels_fit_profile_summary_budget(&model);
    assert_eq!(
        model.row_auxiliary_lines(0),
        Some((
            "UNREADABLE SAVE".to_owned(),
            "Pick another file.".to_owned()
        ))
    );
    assert_eq!(
        model.row_auxiliary_lines(model.entry_row_base()),
        None,
        "status text must not replace a file row's character stats unless that row is row 0"
    );
}

/// Only a save-FILE row is backed by a file, so only a File row has a last-saved time to show.
/// Enumerated per kind so the decision is pinned independently of any layout.
#[test]
fn only_file_rows_have_a_last_saved_time() {
    let path = PathBuf::from("Z:\\saves\\save0.sl2");
    assert!(picker_row_has_last_saved_time(&PickerRow::File(
        path.clone()
    )));
    for row in [
        PickerRow::ParentDir,
        PickerRow::DriveCycle,
        PickerRow::AtRoot,
        PickerRow::Dir(PathBuf::from("Z:\\saves\\sub")),
        PickerRow::NewFile(path),
        PickerRow::NextPage,
        PickerRow::Empty,
    ] {
        assert!(
            !picker_row_has_last_saved_time(&row),
            "{row:?} is backed by no file, so it has no last-saved time"
        );
    }
}

/// On a real layout the timestamp decision must agree with the row's own label and character
/// info, across both intents and with the drive row present: exactly the file rows carry one,
/// and each carries ITS OWN file's stamp rather than a neighbour's.
#[test]
fn last_saved_time_agrees_with_the_rows_own_file() {
    for model in [
        model_with(PickerIntent::LoadSource, "Z:\\saves", 3),
        destination("Z:\\saves", 3),
        with_drives(
            model_with(PickerIntent::LoadSource, "Z:\\saves", 3),
            &["C:\\", "Z:\\"],
        ),
        with_drives(destination("Z:\\", 3), &["C:\\", "Z:\\"]),
        // Long enough to need the page cycler, which carries no timestamp either.
        model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT + 4),
    ] {
        let mut dated = 0;
        for row in 0..PICKER_ROW_COUNT {
            let is_file = row_label_file(&model, row).is_some();
            assert_eq!(
                picker_row_has_last_saved_time(&model.row_meaning(row)),
                is_file,
                "row {row} ({:?}) disagrees with its label about being a save file",
                model.row_meaning(row)
            );
            assert_eq!(
                is_file,
                model.row_file_characters(row).is_some(),
                "row {row} ({:?}) disagrees with its character info",
                model.row_meaning(row)
            );
            // `model_with` stamps file `idx` at epoch + idx minutes, so the stamp a row renders
            // proves WHICH file it read -- the same own-file property the character info has.
            match (row_label_file(&model, row), model.row_last_saved(row)) {
                (Some(file), Some(stamp)) => {
                    let idx: u64 = file
                        .trim_start_matches("save")
                        .trim_end_matches(".sl2")
                        .parse()
                        .expect("test file names carry their index");
                    assert_eq!(
                        stamp,
                        SystemTime::UNIX_EPOCH + Duration::from_secs(idx * 60),
                        "row {row} labelled {file} rendered another file's timestamp"
                    );
                    dated += 1;
                }
                (Some(file), None) => panic!("file row {row} ({file}) lost its timestamp"),
                (None, Some(_)) => panic!("non-file row {row} produced a timestamp"),
                (None, None) => {}
            }
        }
        assert!(dated > 0, "at least one file row must carry a timestamp");
    }
}

/// A file whose metadata the listing build could not read renders NOTHING -- never a fabricated
/// or epoch-zero date.
#[test]
fn a_file_without_metadata_has_no_last_saved_time() {
    let mut model = model_with(PickerIntent::LoadSource, "Z:\\saves", 2);
    for entry in &mut model.entries {
        if let PickerEntry::File { modified, .. } = entry {
            *modified = None;
        }
    }
    let base = model.entry_row_base();
    assert!(matches!(model.row_meaning(base), PickerRow::File(_)));
    assert_eq!(model.row_last_saved(base), None);
}

/// Known epochs, including the leap-year cases an off-by-one in the era algorithm breaks first.
#[test]
fn civil_time_matches_known_epochs() {
    // `(unix seconds, (year, month, day, hour, minute))` for one civil-time conversion.
    type CivilCase = (i64, (i64, u32, u32, u32, u32));
    let cases: [CivilCase; 8] = [
        (0, (1970, 1, 1, 0, 0)),
        (86_399, (1970, 1, 1, 23, 59)),
        (86_400, (1970, 1, 2, 0, 0)),
        // 2000-02-29 12:00: leap year by the 400-rule.
        (951_825_600, (2000, 2, 29, 12, 0)),
        // 2100-02-28 then 2100-03-01, one day apart: 2100 is NOT a leap year (100-rule), so a
        // date the 4-rule alone would place on 2100-02-29 must not exist.
        (4_107_456_000, (2100, 2, 28, 0, 0)),
        (4_107_542_400, (2100, 3, 1, 0, 0)),
        (1_785_312_180, (2026, 7, 29, 8, 3)),
        // Just under the signed-32-bit epoch limit, which this i64 arithmetic must not care
        // about.
        (2_147_483_640, (2038, 1, 19, 3, 14)),
    ];
    for (secs, (year, month, day, hour, minute)) in cases {
        assert_eq!(
            civil_from_unix_seconds(secs),
            Some(CivilDateTime {
                year,
                month,
                day,
                hour,
                minute
            }),
            "epoch {secs}"
        );
    }
    assert_eq!(civil_from_unix_seconds(-1), None, "pre-epoch is not a date");
}

/// The rendered text, and the DST boundary the offset has to carry: US Pacific springs forward
/// at 2026-03-08 10:00 UTC, so one minute of real time crosses from -08:00 to -07:00 and the
/// local clock jumps 01:59 -> 03:00. Passing the two offsets that boundary switches between is
/// exactly how the OS-supplied offset behaves, so this pins the arithmetic without a machine
/// timezone.
#[test]
fn last_saved_renders_local_time_across_a_dst_boundary() {
    const PST: i64 = -8 * 3_600;
    const PDT: i64 = -7 * 3_600;
    // 2026-03-08 09:59:00 UTC, still PST.
    assert_eq!(
        format_last_saved(1_772_963_940, PST).as_deref(),
        Some("2026-03-08 01:59")
    );
    // 2026-03-08 10:00:00 UTC, one minute later, now PDT: the wall clock skips 02:00.
    assert_eq!(
        format_last_saved(1_772_964_000, PDT).as_deref(),
        Some("2026-03-08 03:00")
    );
    // The same instant in UTC and east of Greenwich, to pin the sign of the offset.
    assert_eq!(
        format_last_saved(1_772_964_000, 0).as_deref(),
        Some("2026-03-08 10:00")
    );
    assert_eq!(
        format_last_saved(1_772_964_000, 2 * 3_600).as_deref(),
        Some("2026-03-08 12:00")
    );
    // An offset that would push the instant before the epoch renders nothing.
    assert_eq!(format_last_saved(60, -3_600), None);
}

#[test]
fn drive_row_renders_every_available_drive_as_its_own_cell() {
    let model = with_drives(
        model_with(PickerIntent::LoadSource, "Z:\\saves", 2),
        &["A:\\", "B:\\", "C:\\", "D:\\"],
    );
    let row = model.drive_row().expect("drive row exists");
    assert_eq!(label_of(&model, row), "DRIVES");
    assert_eq!(model.row_auxiliary_lines(row), None);
    assert_eq!(model.drive_strip_cell_count(), 4);
    assert_eq!(model.drive_row_cell_label(row, 0).as_deref(), Some("[A:]"));
    assert_eq!(model.drive_row_cell_label(row, 1).as_deref(), Some("[B:]"));
    assert_eq!(model.drive_row_cell_label(row, 2).as_deref(), Some("[C:]"));
    assert_eq!(model.drive_row_cell_label(row, 3).as_deref(), Some("[D:]"));
}

#[test]
fn drive_strip_active_cell_tracks_the_current_drive() {
    let mut model = with_drives(
        model_with(PickerIntent::LoadSource, "A:\\saves", 2),
        &["A:\\", "B:\\", "C:\\", "D:\\"],
    );
    assert_eq!(model.drive_strip_active_cell(), Some(0));
    assert!(model.activate_drive_strip_cell(2));
    assert_eq!(model.drive_strip_active_cell(), Some(2));
    model.cursor = 1;
    assert_eq!(
        model.drive_strip_active_cell(),
        Some(2),
        "the active drive is model state; row focus separately decides whether its cursor chrome is visible"
    );
}

#[test]
fn drive_strip_pages_to_keep_every_available_drive_directly_selectable() {
    let mut model = with_drives(
        model_with(PickerIntent::LoadSource, "A:\\saves", 2),
        &[
            "A:\\", "B:\\", "C:\\", "D:\\", "E:\\", "F:\\", "G:\\", "H:\\",
        ],
    );
    let row = model.drive_row().expect("drive row exists");
    assert_eq!(label_of(&model, row), "DRIVES");
    assert_eq!(model.row_auxiliary_lines(row), None);
    assert_eq!(model.drive_strip_cell_count(), DRIVE_STRIP_MAX_CELLS);
    assert_eq!(model.drive_row_cell_label(row, 0).as_deref(), Some(">A:<"));
    assert_eq!(model.drive_row_cell_label(row, 6).as_deref(), Some("[>]"));
    assert!(model.activate_drive_strip_cell(6));
    assert!(model.activate_drive_strip_cell(6));
    assert_eq!(model.drive_row_cell_label(row, 6).as_deref(), Some("[H:]"));
    assert!(model.activate_drive_strip_cell(6));
    assert_eq!(model.current_drive_root(), PathBuf::from("H:\\"));
    let row = model.drive_row().expect("drive row still exists");
    assert_eq!(
        model.cursor, row,
        "cell activation keeps keyboard/mouse focus on the drive row"
    );
    assert_eq!(model.drive_row_cell_label(row, 6).as_deref(), Some(">H:<"));
    assert!(model.cycle_drive_from_drive_strip(true));
    let row = model.drive_row().expect("drive row still exists");
    assert_eq!(
        model.cursor, row,
        "left/right drive cycling keeps focus on the drive row"
    );
}

#[test]
fn right_from_the_rightmost_drive_focuses_current_path_without_wrapping() {
    let mut model = with_drives(
        model_with(PickerIntent::LoadSource, "D:\\saves", 0),
        &["A:\\", "B:\\", "C:\\", "D:\\"],
    );
    assert_eq!(model.drive_strip_focus(), Some(DriveStripFocus::Cell(3)));

    assert!(model.cycle_drive_from_drive_strip(true));

    assert_eq!(model.current_drive_root(), PathBuf::from("D:\\"));
    assert_eq!(
        model.drive_strip_focus(),
        Some(DriveStripFocus::CurrentPath)
    );

    assert!(model.cycle_drive_from_drive_strip(false));
    assert_eq!(model.current_drive_root(), PathBuf::from("D:\\"));
    assert_eq!(model.drive_strip_focus(), Some(DriveStripFocus::Cell(3)));

    assert!(model.focus_current_path_from_drive_strip());
    assert!(model.activate_drive_strip_cell(1));
    assert_eq!(model.current_drive_root(), PathBuf::from("B:\\"));
    assert_eq!(model.drive_strip_focus(), Some(DriveStripFocus::Cell(1)));
}

/// The drive cycler must be excluded from entry indexing in BOTH intents: it shifts the entry
/// base by exactly one and never resolves to an entry itself.
#[test]
fn drive_row_is_excluded_from_entry_indexing_in_both_intents() {
    let load = with_drives(
        model_with(PickerIntent::LoadSource, "Z:\\saves", 2),
        &["C:\\", "Z:\\"],
    );
    assert_eq!(load.drive_row(), Some(0));
    assert_eq!(load.row_meaning(0), PickerRow::DriveCycle);
    assert_eq!(load.row_meaning(1), PickerRow::ParentDir);
    assert_eq!(load.row_file_characters(0), None);
    assert_eq!(
        load.row_meaning(2),
        PickerRow::File(PathBuf::from("Z:\\saves").join("save0.sl2")),
        "the first entry must sit directly under the drive row"
    );

    // Destination layout: the drive strip is always row 0; `[ new ]` stays above the parent
    // row at 1, parent is 2, and entries still start at 3.
    let dest = with_drives(destination("Z:\\saves", 2), &["C:\\", "Z:\\"]);
    assert_eq!(dest.drive_row(), Some(0));
    assert_eq!(dest.new_file_row(), Some(1));
    assert_eq!(dest.parent_row(), Some(2));
    assert_eq!(dest.row_meaning(0), PickerRow::DriveCycle);
    assert_eq!(
        dest.row_meaning(1),
        PickerRow::NewFile(PathBuf::from("Z:\\saves").join("ER0000.sl2"))
    );
    assert_eq!(dest.row_meaning(2), PickerRow::ParentDir);
    assert_eq!(dest.row_file_characters(0), None);
    assert_eq!(dest.row_file_characters(1), None);
    assert_eq!(
        dest.row_meaning(3),
        PickerRow::File(PathBuf::from("Z:\\saves").join("save0.sl2"))
    );
    // Adding the drive row costs exactly one entry row per page, in each intent, once the
    // listing is long enough for the per-page capacity to bind.
    let long = PICKER_ROW_COUNT * 2;
    assert_eq!(
        model_with(PickerIntent::LoadSource, "Z:\\saves", long).entries_per_page(),
        9
    );
    assert_eq!(
        with_drives(
            model_with(PickerIntent::LoadSource, "Z:\\saves", long),
            &["C:\\", "Z:\\"]
        )
        .entries_per_page(),
        8
    );
    assert_eq!(destination("Z:\\saves", long).entries_per_page(), 8);
    assert_eq!(
        with_drives(destination("Z:\\saves", long), &["C:\\", "Z:\\"]).entries_per_page(),
        7
    );
}

/// Activating the drive row background is inert; explicit cells select drives directly.
#[test]
fn drive_row_background_does_not_cycle_drives() {
    let roots = ["C:\\", "S:\\", "Z:\\"];
    let mut model = with_drives(model_with(PickerIntent::LoadSource, "C:\\", 0), &roots);
    let drive_row = model
        .drive_row()
        .expect("three drives must add a drive row");
    assert_eq!(drive_row, 0, "a drive root has no up row above the strip");
    assert_eq!(model.row_meaning(drive_row), PickerRow::DriveCycle);
    assert_eq!(model.activate(drive_row), PickerActivation::Ignored);
    assert_eq!(model.current_dir(), Path::new("C:\\"));
    assert!(model.activate_drive_strip_cell(2));
    assert_eq!(model.current_dir(), Path::new("Z:\\"));
}

/// The drive row's compact name field stays stable; selectable cells live in the native label field.
#[test]
fn drive_row_label_names_the_strip_and_fits_the_name_budget() {
    let model = with_drives(
        model_with(PickerIntent::LoadSource, "C:\\users", 0),
        &["C:\\", "S:\\", "Z:\\"],
    );
    let row = model.drive_row().expect("drive row");
    let label = label_of(&model, row);
    assert_eq!(label, "DRIVES");
    assert_eq!(model.drive_row_cell_label(row, 0).as_deref(), Some(">C:<"));
    assert_eq!(model.drive_row_cell_label(row, 1).as_deref(), Some("[S:]"));
    assert_eq!(model.drive_row_cell_label(row, 2).as_deref(), Some("[Z:]"));
    assert!(model.row_label_utf16(row).len() <= PICKER_ROW_NAME_UTF16_MAX);
    assert!(!label.contains(','), "row labels must be comma-safe");
}

/// Cycling drives must RESUME the folder last browsed on the drive being returned to, instead
/// of dumping the user at the drive root every time -- that resume is what makes the row useful
/// for moving a save between two directories on different drives.
#[test]
fn cycling_drives_resumes_each_drives_remembered_folder() {
    let (real_dir, real_root) = real_dir_and_root("resume");
    // Second drive is a letter that cannot be mounted here, so "never visited" is guaranteed.
    let other_root = PathBuf::from("Q:\\");
    let mut model = with_drives(
        model_with(PickerIntent::LoadSource, "unused", 0),
        &[
            real_root.to_string_lossy().as_ref(),
            other_root.to_string_lossy().as_ref(),
        ],
    );
    model.current_dir = real_dir.clone();

    // Leaving the real drive records where we were; the unvisited drive opens at its root.
    model.cycle_drive(true);
    assert_eq!(model.current_dir(), other_root.as_path());
    assert_eq!(
        model.last_dir_per_drive.get(&real_root),
        Some(&real_dir),
        "the folder being left must be remembered against its own drive"
    );

    // Coming back RESUMES that folder instead of the drive root -- the whole point.
    model.cycle_drive(true);
    assert_eq!(model.current_dir(), real_dir.as_path());
}

/// A remembered folder that has since vanished must fall back to the drive root rather than
/// browsing a dead path.
#[test]
fn cycling_drives_falls_back_to_the_root_when_the_remembered_folder_is_gone() {
    let (real_dir, real_root) = real_dir_and_root("vanished");
    let other_root = PathBuf::from("Q:\\");
    let mut model = with_drives(
        model_with(PickerIntent::LoadSource, "unused", 0),
        &[
            real_root.to_string_lossy().as_ref(),
            other_root.to_string_lossy().as_ref(),
        ],
    );
    model.current_dir = real_dir.clone();
    model.cycle_drive(true);
    assert_eq!(model.current_dir(), other_root.as_path());

    // The folder is remembered, but it is gone by the time we cycle back.
    std::fs::remove_dir_all(&real_dir).expect("temp dir must be removable");
    model.cycle_drive(true);
    assert_eq!(
        model.current_dir(),
        real_root.as_path(),
        "a remembered folder that no longer exists must fall back to the drive root"
    );
    // The memory is still recorded, so the fallback is about resolvability, not forgetting.
    assert_eq!(model.last_dir_per_drive.get(&real_root), Some(&real_dir));
}

/// A single drive still gets the location/path row, while cycling remains inert.
#[test]
fn a_single_drive_keeps_the_complete_path_row_without_fake_cycling() {
    let mut model = with_drives(
        model_with(PickerIntent::LoadSource, "Z:\\home\\banon", 0),
        &["Z:\\"],
    );
    assert_eq!(model.drive_row(), Some(0));
    assert_eq!(model.drive_count(), 1);
    model.cycle_drive(true);
    assert_eq!(model.current_dir(), Path::new("Z:\\home\\banon"));
    // The location row and up row are fixed; the remaining rows stay available to entries.
    assert_eq!(model.entries_per_page(), PICKER_ROW_COUNT - 2);
}

/// The `[..]` row names the folder it goes TO, not just the direction, and truncates rather
/// than overflowing the record's name field.
#[test]
fn up_row_label_names_the_parent_folder() {
    let model = model_with(
        PickerIntent::LoadSource,
        "Z:\\home\\banon\\Roaming\\deep",
        0,
    );
    let up = model.parent_row().expect("a nested folder has an up row");
    assert_eq!(up, 0, "a load browse pins nothing above the up row");
    assert_eq!(label_of(&model, up), "[..] Roaming");
    let long = model_with(
        PickerIntent::LoadSource,
        "Z:\\a-very-long-folder-name-indeed\\child",
        0,
    );
    let long_up = long.parent_row().expect("a nested folder has an up row");
    assert!(long.row_label_utf16(long_up).len() <= PICKER_ROW_NAME_UTF16_MAX);
    // Same row, one index lower, in a destination browse: the label is derived from
    // `parent_row()` rather than a constant, so inserting `[ new ]` above it moves nothing else.
    let dest = destination("Z:\\home\\banon\\Roaming\\deep", 0);
    assert_eq!(dest.parent_row(), Some(1));
    assert_eq!(label_of(&dest, 1), "[..] Roaming");
}

#[test]
fn long_listing_uses_scroll_window_instead_of_page_row() {
    let model = model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT + 20);
    assert_eq!(model.page_count(), 1);
    assert_eq!(model.next_page_row(), None);
    assert_eq!(model.scroll_offset(), 0);
    assert_eq!(model.scroll_max(), 21);
    assert_eq!(model.visible_row_count(), PICKER_ROW_COUNT);
    assert_eq!(model.scroll_up_row(), None);
    assert_eq!(model.scroll_down_row(), None);
    assert_eq!(
        model.row_meaning(PICKER_ROW_COUNT - 1),
        PickerRow::File(PathBuf::from("Z:\\saves").join("save8.sl2"))
    );
}

/// One press at an edge row moves the window exactly one row, and only at an edge. The window used
/// to slide from a pointer DWELL on the edge row, which moved the list under a player who was only
/// resting there; a press is now the sole trigger, so nothing moves without an explicit input.
#[test]
fn an_edge_press_scrolls_exactly_one_row_and_only_from_the_edge() {
    let mut model = model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT + 20);
    let last = PICKER_ROW_COUNT - 1;

    // Repeated presses at the bottom edge advance one row each -- no dwell, no acceleration -- and
    // each reports the edge row the caller must pin the native cursor to. Without that pin the
    // selection leaves the edge and the next press is not an edge press at all.
    for expected in 1..=3 {
        assert_eq!(
            model.scroll_window_from_edge_press(last, true),
            Some(EdgePressOutcome::Scrolled { pin_row: last })
        );
        assert_eq!(model.scroll_offset(), expected);
    }

    // A press away from either edge must not move the window at all.
    assert_eq!(model.scroll_window_from_edge_press(last / 2, true), None);
    assert_eq!(model.scroll_window_from_edge_press(last / 2, false), None);
    assert_eq!(model.scroll_offset(), 3);

    // Pressing DOWN at the top edge (and UP at the bottom) is not an edge press for that direction.
    assert_eq!(model.scroll_window_from_edge_press(0, true), None);
    assert_eq!(model.scroll_offset(), 3);

    // Up at the top edge walks back one row per press, and stops at the top rather than wrapping.
    // The pinned row is the first CONTENT row, which is 1 here rather than 0: this listing has a
    // parent ("up one directory") row above the entries, so row 0 is not an entry. Pinning to the
    // literal top of the window would park the selection on a non-entry row.
    let top_content_row = 1;
    for expected in (0..3).rev() {
        assert_eq!(
            model.scroll_window_from_edge_press(0, false),
            Some(EdgePressOutcome::Scrolled {
                pin_row: top_content_row
            })
        );
        assert_eq!(model.scroll_offset(), expected);
    }
    // At the top of the listing the press holds row 0 rather than reporting nothing: reporting
    // nothing leaves the native list free to wrap the selection to the last row.
    assert_eq!(
        model.scroll_window_from_edge_press(0, false),
        Some(EdgePressOutcome::HeldAtLimit { pin_row: 0 })
    );
    assert_eq!(model.scroll_offset(), 0);
}

/// A DOWN press on the last row of the LAST window holds that row instead of letting the native
/// list wrap the selection back to the top. Reported from a live run (2026-08-12): stepping down
/// through a long listing and pressing DOWN once more at the bottom jumped the selection to the
/// drives row, which reads as the list losing the player's place.
#[test]
fn a_down_press_at_the_end_of_the_listing_holds_instead_of_wrapping_to_the_top() {
    let mut model = model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT + 2);
    let last = PICKER_ROW_COUNT - 1;

    // Walk the window to its last position: from there the scrollbar shows nothing further below,
    // which is the state where the native list wraps instead of scrolling.
    let scrollable_rows = model.scroll_max();
    assert!(scrollable_rows > 0, "fixture must overflow the window");
    for _ in 0..scrollable_rows {
        assert_eq!(
            model.scroll_window_from_edge_press(last, true),
            Some(EdgePressOutcome::Scrolled { pin_row: last })
        );
    }
    assert_eq!(model.scroll_offset(), model.scroll_max());

    for _ in 0..3 {
        assert_eq!(
            model.scroll_window_from_edge_press(last, true),
            Some(EdgePressOutcome::HeldAtLimit { pin_row: last })
        );
        assert_eq!(model.scroll_offset(), model.scroll_max());
    }
}

/// A listing that fits entirely in the window has no scroll at all, and DOWN on its final row must
/// still hold rather than wrap. The window-scrolling path never runs here, so this is the case a
/// scroll-only rule would miss.
#[test]
fn a_down_press_at_the_bottom_of_a_short_listing_holds_without_any_scroll() {
    let mut model = model_with(PickerIntent::LoadSource, "Z:\\saves", 3);
    let last = model.visible_row_count() - 1;

    assert_eq!(model.scroll_max(), 0);
    assert_eq!(
        model.scroll_window_from_edge_press(last, true),
        Some(EdgePressOutcome::HeldAtLimit { pin_row: last })
    );
    assert_eq!(model.scroll_offset(), 0);
}

#[test]
fn cycle_page_compatibility_moves_scroll_window_without_page_row() {
    let mut model = model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT + 20);
    model.cycle_page(true);
    assert_eq!(model.next_page_row(), None);
    assert_eq!(model.scroll_offset(), 9);
    assert_eq!(model.scroll_up_row(), None);
    assert_eq!(model.scroll_down_row(), None);
    assert_eq!(
        model.row_meaning(1),
        PickerRow::File(PathBuf::from("Z:\\saves").join("save9.sl2"))
    );
    model.cycle_page(false);
    assert_eq!(model.scroll_offset(), 0);
    assert!(matches!(model.activate(1), PickerActivation::PickedFile(_)));
    assert_eq!(model.scroll_offset(), 0);
}

/// Rows beyond the listing must be reported as NOT visible, so the staging layer marks their
/// native slots unoccupied and the builder omits them -- that is what stops a short listing
/// rendering placeholder rows with a name, `Level 0` and `0:00:00`.
#[test]
fn rows_beyond_the_listing_are_outside_the_visible_count() {
    // Load source, two files, up row, no drive row: up + 2 entries = 3 visible rows.
    let load = model_with(PickerIntent::LoadSource, "Z:\\saves", 2);
    assert_eq!(load.visible_row_count(), 3);
    for row in load.visible_row_count()..PICKER_ROW_COUNT {
        assert_eq!(load.row_meaning(row), PickerRow::Empty);
        assert!(load.row_label_utf16(row).is_empty());
    }
    // Destination, drive row, one file: [ new ] + up + drive + 1 entry = 4 visible rows.
    let dest = with_drives(destination("Z:\\saves", 1), &["C:\\", "Z:\\"]);
    assert_eq!(dest.visible_row_count(), 4);
    for row in dest.visible_row_count()..PICKER_ROW_COUNT {
        assert_eq!(dest.row_meaning(row), PickerRow::Empty);
        assert!(dest.row_label_utf16(row).is_empty());
    }
    // Overflow is handled by scroll state, not a visible page-cycler row.
    let paged = model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT + 4);
    assert_eq!(paged.next_page_row(), None);
    assert!(paged.scroll_max() > 0);
    assert_eq!(paged.visible_row_count(), PICKER_ROW_COUNT);
}

/// Every visible row must carry a non-empty label: the native staging marks visible slots
/// occupied, and an occupied slot with an empty name would fail the empty-slot activation
/// guard. Checked across the layouts that move the row boundaries.
#[test]
fn every_visible_row_has_a_non_empty_label() {
    for model in [
        model_with(PickerIntent::LoadSource, "Z:\\saves", 0),
        model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT + 4),
        with_drives(destination("Z:\\", 0), &["C:\\", "Z:\\"]),
        with_drives(destination("Z:\\saves", 9), &["C:\\", "S:\\", "Z:\\"]),
        model_with(PickerIntent::LoadSource, "Z:\\", 0),
    ] {
        for row in 0..model.visible_row_count() {
            assert!(
                !model.row_label_utf16(row).is_empty(),
                "visible row {row} has an empty label in {:?} (visible={})",
                model.current_dir(),
                model.visible_row_count()
            );
            assert!(model.row_label_utf16(row).len() <= PICKER_ROW_NAME_UTF16_MAX);
        }
    }
}

/// A listing that overflows keeps every row for content; paging rows stay gone.
#[test]
fn overflowing_listing_uses_scroll_window_without_page_cycler() {
    let fits = model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT - 1);
    assert_eq!(fits.page_count(), 1);
    assert_eq!(fits.next_page_row(), None);
    assert_eq!(fits.entries_per_page(), PICKER_ROW_COUNT - 1);

    let mut overflows = model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT);
    assert_eq!(overflows.entries_per_page(), PICKER_ROW_COUNT - 1);
    assert_eq!(overflows.page_count(), 1);
    assert_eq!(overflows.next_page_row(), None);
    assert_eq!(overflows.scroll_up_row(), None);
    assert_eq!(overflows.scroll_down_row(), None);
    assert_eq!(overflows.visible_row_count(), PICKER_ROW_COUNT);
    overflows.cycle_page(true);
    assert_eq!(overflows.scroll_offset(), 1);
    assert_eq!(
        overflows.row_meaning(1),
        PickerRow::File(PathBuf::from("Z:\\saves").join("save1.sl2"))
    );
    assert_eq!(overflows.next_page_row(), None);
}

#[test]
fn load_source_layout_is_unaffected_by_the_destination_intent() {
    let model = model_with(PickerIntent::LoadSource, "Z:\\saves", 8);
    assert_eq!(model.new_file_row(), None);
    assert_eq!(model.page_count(), 1);
    assert_eq!(
        model.row_meaning(1),
        PickerRow::File(PathBuf::from("Z:\\saves").join("save0.sl2"))
    );
    assert_eq!(
        model.row_meaning(8),
        PickerRow::File(PathBuf::from("Z:\\saves").join("save7.sl2"))
    );
    assert_eq!(model.row_meaning(9), PickerRow::Empty);
}

/// THE DESTINATION CURSOR STARTS ON `[ new ]`, IN EVERY LAYOUT. Since the Save Game row press
/// opens this browser with no question in front of it, the row the cursor rests on is the
/// answer a user gets for pressing confirm twice without reading -- so it must be the row that
/// creates rather than the row that replaces. Checked with entries present and absent, with and
/// without a drive row, and at a drive root where there is no up row at all.
#[test]
fn a_destination_browse_always_starts_the_cursor_on_new_file() {
    for model in [
        destination("Z:\\saves", 0),
        destination("Z:\\saves", 3),
        with_drives(destination("Z:\\saves", 0), &["C:\\", "Z:\\"]),
        with_drives(destination("Z:\\saves", 5), &["C:\\", "S:\\", "Z:\\"]),
        with_drives(destination("Z:\\", 2), &["C:\\", "Z:\\"]),
        destination("Z:\\", 0),
    ] {
        let mut model = model;
        model.cursor = model.first_selectable_row();
        let new_row = model
            .new_file_row()
            .expect("destination browsing always has `[ new ]`");
        if model.drive_row().is_some() {
            assert_eq!(model.drive_row(), Some(0));
            assert_eq!(new_row, 1, "`[ new ]` follows the drive row");
        } else {
            assert_eq!(new_row, 0, "without drives `[ new ]` remains first");
        }
        assert_eq!(
            model.cursor,
            new_row,
            "the destination cursor must start on `[ new ]` in {:?}",
            model.current_dir()
        );
        let expected = model.current_dir().join("ER0000.sl2");
        assert_eq!(
            model.activate_cursor(),
            PickerActivation::PickedNewFile(expected)
        );
    }
}

#[test]
fn direct_cursor_set_accepts_only_selectable_rows() {
    let mut model = model_with(PickerIntent::LoadSource, "Z:\\saves", 2);
    let first_entry = model.entry_row_base();
    model.set_cursor(first_entry + 1);
    assert_eq!(model.cursor(), first_entry + 1);
    model.set_cursor(PICKER_ROW_COUNT - 1);
    assert_eq!(
        model.cursor(),
        first_entry + 1,
        "setting the cursor to an Empty row must leave the current highlight alone"
    );
}

/// On an EMPTY drive the initial cursor must still land on a real, selectable row.
#[test]
fn first_selectable_row_is_sane_on_an_empty_drive() {
    // Empty drive root WITH somewhere else to go: the cycler is the only row, and the cursor
    // must land on it so the user is not stranded.
    let multi = with_drives(
        model_with(PickerIntent::LoadSource, "Z:\\", 0),
        &["C:\\", "Z:\\"],
    );
    assert_eq!(multi.visible_row_count(), 1);
    assert_eq!(multi.first_selectable_row(), 0);
    assert_eq!(multi.row_meaning(0), PickerRow::DriveCycle);

    // Empty single-drive root: the location/path row remains actionable even though drive
    // cycling is inert, so the user can type a complete directory instead of being stranded.
    let alone = with_drives(model_with(PickerIntent::LoadSource, "Z:\\", 0), &["Z:\\"]);
    assert_eq!(alone.visible_row_count(), 1);
    assert_eq!(alone.first_selectable_row(), 0);
    assert_eq!(alone.row_meaning(0), PickerRow::DriveCycle);
    assert_eq!(label_of(&alone, 0), "DRIVES");

    // Empty SUBdirectory: nothing actionable below the nav rows, so fallback preserves the
    // always-first drive control instead of jumping past it to the parent row.
    let empty_dir = with_drives(
        model_with(PickerIntent::LoadSource, "Z:\\saves", 0),
        &["C:\\", "Z:\\"],
    );
    assert_eq!(empty_dir.visible_row_count(), 2);
    assert_eq!(empty_dir.first_selectable_row(), 0);
    assert_eq!(empty_dir.row_meaning(0), PickerRow::DriveCycle);
    assert_eq!(empty_dir.row_meaning(1), PickerRow::ParentDir);
}

#[test]
fn new_file_row_label_fits_the_profile_summary_name_budget() {
    let model = destination("Z:\\saves", 0);
    let row = model
        .new_file_row()
        .expect("destination pins a [ new ] row");
    let label = model.row_label_utf16(row);
    assert!(!label.is_empty() && label.len() <= PICKER_ROW_NAME_UTF16_MAX);
    assert_eq!(String::from_utf16(&label).unwrap(), PICKER_NEW_FILE_LABEL);
}

/// EXACTLY ONE ROW IS `[CURRENT]`, and it is the row whose file the user is playing. With the
/// up-front "Overwrite your loaded save?" box gone, finding that row IS the overwrite-my-own-
/// save flow, so a marker on the wrong row (or on none) sends the user to the wrong file.
#[test]
fn only_the_loaded_saves_row_is_marked_current() {
    let dir = "Z:\\saves";
    let model = destination_loading(dir, 4, "Z:\\saves\\save2.sl2");
    let marked: Vec<usize> = (0..PICKER_ROW_COUNT)
        .filter(|&row| model.row_is_loaded_save(row))
        .collect();
    let base = model.entry_row_base();
    assert_eq!(
        marked,
        vec![base + 2],
        "only save2.sl2's row may be marked (entries start at row {base})"
    );
    assert_eq!(
        row_label_file(&model, base + 2).as_deref(),
        Some("save2.sl2"),
        "the marked row must be the one whose LABEL names the loaded file"
    );
}

/// The marker is a Windows path compare, so it must survive case differences -- `ER0000.SL2`
/// and `er0000.sl2` are one file there, and a case-sensitive compare would leave a user's own
/// save unmarked in the list they are being asked to find it in.
#[test]
fn the_current_marker_ignores_path_case() {
    let model = destination_loading("Z:\\saves", 2, "z:\\SAVES\\SAVE1.SL2");
    let row = model.entry_row_base() + 1;
    assert_eq!(row_label_file(&model, row).as_deref(), Some("save1.sl2"));
    assert!(model.row_is_loaded_save(row));
}

/// NOTHING is marked when the loaded save is not in the browsed folder, and NOTHING is ever
/// marked in a load browse -- there is no "current" there, and a marker would be a claim the
/// model cannot support.
#[test]
fn no_row_is_marked_current_without_a_matching_loaded_save() {
    for model in [
        destination("Z:\\saves", 4),
        model_with(PickerIntent::LoadSource, "Z:\\saves", 4),
    ] {
        for row in 0..PICKER_ROW_COUNT {
            assert!(
                !model.row_is_loaded_save(row),
                "row {row} was marked current in {:?}",
                model.intent
            );
        }
    }
}

/// The marker never lands on a non-file row: `[ new ]` resolves to the loaded save's own leaf,
/// and in the loaded save's own folder that path IS the loaded save -- but `[ new ]` is an
/// ACTION row, not the file's row, and marking it would put `[CURRENT]` on two rows at once.
#[test]
fn the_new_file_row_is_never_marked_current_even_when_it_targets_the_loaded_save() {
    let model = destination_loading("Z:\\saves", 2, "Z:\\saves\\ER0000.sl2");
    let new_row = model.new_file_row().expect("destination pins [ new ]");
    assert_eq!(
        model.row_meaning(new_row),
        PickerRow::NewFile(PathBuf::from("Z:\\saves").join("ER0000.sl2")),
        "the row targets exactly the loaded save's path"
    );
    assert!(
        !model.row_is_loaded_save(new_row),
        "`[ new ]` is an action row; only a File row may be marked"
    );
}

#[test]
fn rejection_reasons_have_distinct_visible_copy() {
    assert_eq!(
        PickRejection::NotBnd4.status_message("SL2").headline(),
        "NOT AN ELDEN RING SAVE"
    );
    assert_eq!(
        PickRejection::NoLoadableCharacter
            .status_message("SL2")
            .headline(),
        "NO LOADABLE CHARACTER"
    );
    let wrong_type = PickRejection::WrongExtension.status_message("CO2/.SL2");
    assert_eq!(wrong_type.headline(), "WRONG FILE TYPE");
    assert!(
        wrong_type.detail().contains(".CO2/.SL2"),
        "the visible reason must name the accepted extension set"
    );
}

#[test]
fn picker_status_survives_rejection_and_clears_on_navigation() {
    let mut model = model_with(PickerIntent::LoadSource, "Z:\\saves", 2);
    model.set_status_message(PickerStatusMessage::new(
        "WRONG SAVE SIZE",
        "Expected a full container.",
    ));
    assert_eq!(
        model.status_message().map(PickerStatusMessage::headline),
        Some("WRONG SAVE SIZE")
    );

    assert_eq!(model.activate(0), PickerActivation::Repopulate);
    assert!(
        model.status_message().is_none(),
        "moving to another folder must not carry a stale rejection"
    );
}

#[test]
fn complete_path_syntax_accepts_posix_and_both_windows_separator_forms() {
    assert!(complete_directory_text_is_absolute(
        "/home/banon/Mixed Case",
        Path::new("/home/banon/Mixed Case")
    ));
    assert!(complete_directory_text_is_absolute(
        "Z:\\home\\banon\\Mixed Case",
        Path::new("Z:\\home\\banon\\Mixed Case")
    ));
    assert!(complete_directory_text_is_absolute(
        "Z:/home/banon/Mixed Case",
        Path::new("Z:/home/banon/Mixed Case")
    ));
    assert!(!complete_directory_text_is_absolute(
        "Z:relative",
        Path::new("Z:relative")
    ));
    assert!(!complete_directory_text_is_absolute(
        "relative/path",
        Path::new("relative/path")
    ));
}

#[test]
fn complete_path_accept_preserves_case_and_spaces() {
    let (root_dir, _) = real_dir_and_root("complete-path");
    let target = root_dir.join("Mixed Case Folder");
    std::fs::create_dir_all(&target).expect("mixed-case target dir must be creatable");
    let mut model = model_with(
        PickerIntent::LoadSource,
        root_dir.to_string_lossy().as_ref(),
        0,
    );
    let exact = target
        .to_str()
        .expect("test temp path must be Unicode")
        .to_owned();
    model.set_status_message(PickerStatusMessage::new(
        "FOLDER NOT FOUND",
        "stale rejection",
    ));

    assert_eq!(model.set_current_dir_from_text(&exact), Ok(true));
    assert_eq!(model.current_dir(), Path::new(&exact));
    assert!(model.status_message().is_none());
}

#[test]
fn entering_path_edit_mode_clears_the_previous_rejection() {
    let mut model = model_with(PickerIntent::LoadSource, "Z:\\saves", 2);
    model.set_status_message(PickerStatusMessage::new(
        "FOLDER NOT FOUND",
        "stale rejection",
    ));

    assert!(model.begin_path_edit());
    assert!(model.status_message().is_none());
    assert!(!model.begin_path_edit());
}

/// A refused entry stays on the CurrentPath control so it can be corrected in place, and a
/// later good entry must take the marking off again -- including the case where the corrected
/// path is the folder already open, which commits `Ok(false)` and never refreshes the listing.
#[test]
fn a_rejected_path_entry_is_kept_for_correction_and_cleared_by_the_next_good_one() {
    let (root_dir, _) = real_dir_and_root("rejected-path-text");
    let target = root_dir.join("Real Folder");
    std::fs::create_dir_all(&target).expect("target dir must be creatable");
    let mut model = model_with(
        PickerIntent::LoadSource,
        root_dir.to_string_lossy().as_ref(),
        0,
    );
    assert_eq!(model.rejected_path_text(), None);

    assert_eq!(
        model.set_current_dir_from_text("relative folder"),
        Err(DirectoryChangeError::NotAbsolute)
    );
    model.set_rejected_path_text("relative folder");
    assert_eq!(
        model.rejected_path_text(),
        Some("relative folder"),
        "a refused entry must survive so the user can fix it rather than retype it"
    );

    let exact = target
        .to_str()
        .expect("test temp path must be Unicode")
        .to_owned();
    assert_eq!(model.set_current_dir_from_text(&exact), Ok(true));
    assert_eq!(
        model.rejected_path_text(),
        None,
        "a directory change must drop the marking"
    );

    model.set_rejected_path_text("nonsense again");
    assert_eq!(
        model.set_current_dir_from_text(&exact),
        Ok(false),
        "re-entering the open folder is valid but changes nothing"
    );
    assert_eq!(
        model.rejected_path_text(),
        None,
        "a valid entry must clear the marking even when the directory does not change"
    );
}

#[test]
fn invalid_complete_path_accept_does_not_mutate_the_model() {
    let (root_dir, _) = real_dir_and_root("invalid-complete-path");
    let mut model = model_with(
        PickerIntent::LoadSource,
        root_dir.to_string_lossy().as_ref(),
        2,
    );
    model.set_status_message(PickerStatusMessage::new("KEEP", "unchanged"));
    let before_dir = model.current_dir().to_path_buf();
    let before_entries = model.entry_count();

    assert_eq!(
        model.set_current_dir_from_text("relative folder"),
        Err(DirectoryChangeError::NotAbsolute)
    );
    assert_eq!(
        model.set_current_dir_from_text(""),
        Err(DirectoryChangeError::Empty)
    );
    assert_eq!(model.current_dir(), before_dir);
    assert_eq!(model.entry_count(), before_entries);
    assert_eq!(
        model.status_message().map(PickerStatusMessage::headline),
        Some("KEEP")
    );
}

#[test]
fn picker_status_clears_on_direct_page_drive_and_up_navigation() {
    let mut paged = model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT);
    paged.set_status_message(PickerStatusMessage::new(
        "NO LOADABLE CHARACTER",
        "Pick another save.",
    ));
    paged.cycle_page(true);
    assert!(
        paged.status_message().is_none(),
        "direct page cycling must not carry a stale rejection"
    );

    let (real_dir, real_root) = real_dir_and_root("status-clear-drive");
    let other_root = PathBuf::from("Q:\\");
    let mut drive = with_drives(
        model_with(PickerIntent::LoadSource, "unused", 0),
        &[
            real_root.to_string_lossy().as_ref(),
            other_root.to_string_lossy().as_ref(),
        ],
    );
    drive.current_dir = real_dir.clone();
    drive.set_status_message(PickerStatusMessage::new(
        "WRONG FILE TYPE",
        "Pick another file.",
    ));
    drive.cycle_drive(true);
    assert!(
        drive.status_message().is_none(),
        "direct drive cycling must not carry a stale rejection"
    );

    let child = real_dir.join("child");
    std::fs::create_dir_all(&child).expect("temp child dir must be creatable");
    let mut up = model_with(
        PickerIntent::LoadSource,
        child.to_string_lossy().as_ref(),
        0,
    );
    up.set_status_message(PickerStatusMessage::new(
        "UNREADABLE SAVE",
        "Pick another file.",
    ));
    up.go_up();
    assert!(
        up.status_message().is_none(),
        "direct up navigation must not carry a stale rejection"
    );
}
