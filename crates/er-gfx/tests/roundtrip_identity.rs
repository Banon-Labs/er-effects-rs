//! Can we reproduce an ARBITRARY movie byte-for-byte?
//!
//! This is the precondition for ever touching a movie we do not have a baked fingerprint for
//! -- i.e. a `.gfx` a user supplied through ME3 from some other mod. The badge edit itself is
//! already structural (it finds tiles by their named children, mirrors `AttributeIcon`, reads
//! the placeholder's atlas cell, allocates fresh character ids), so it does not care about
//! exact bytes. What DOES care is the writer: if `parse -> write` is not the identity on an
//! unmodified movie, then re-serialising someone else's movie could silently drop or reshape
//! a tag we did not model, and we would hand the game a corrupted HUD.
//!
//! So: sweep the whole corpus and report exactly which movies round-trip. Whatever fraction
//! survives here is the fraction for which a "derive on an unknown movie" path could be made
//! safe; the rest must always be served untouched.
//!
//!   cargo test -p er-gfx --test roundtrip_identity -- --nocapture

mod common;

use er_gfx::Movie;

#[test]
fn corpus_roundtrip_identity() {
    let root = common::corpus_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        eprintln!("SKIP: corpus {} absent", root.display());
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".gfx"))
        .collect();
    names.sort();

    let (mut ok, mut differs, mut unparsed, mut unwritten) = (0usize, 0usize, 0usize, 0usize);
    let mut bad: Vec<String> = Vec::new();
    for name in &names {
        let Ok(bytes) = std::fs::read(root.join(name)) else {
            continue;
        };
        let movie = match Movie::parse(&bytes) {
            Ok(m) => m,
            Err(e) => {
                unparsed += 1;
                bad.push(format!("  UNPARSED {name}: {e}"));
                continue;
            }
        };
        match movie.write() {
            Err(e) => {
                unwritten += 1;
                bad.push(format!("  UNWRITTEN {name}: {e}"));
            }
            Ok(out) if out == bytes => ok += 1,
            Ok(out) => {
                differs += 1;
                let at = out
                    .iter()
                    .zip(bytes.iter())
                    .position(|(a, b)| a != b)
                    .map(|i| format!("first diff @ {i}"))
                    .unwrap_or_else(|| "prefix identical".into());
                bad.push(format!(
                    "  DIFFERS  {name}: in={} out={} ({at})",
                    bytes.len(),
                    out.len()
                ));
            }
        }
    }
    println!(
        "corpus {}: {} movies -- identity {ok}, differs {differs}, unparsed {unparsed}, \
         unwritten {unwritten}",
        root.display(),
        names.len()
    );
    for line in bad.iter().take(40) {
        println!("{line}");
    }
    if bad.len() > 40 {
        println!("  ... and {} more", bad.len() - 40);
    }
    // Not an assertion about the corpus as a whole -- this test EXISTS to measure it. The
    // movies we actually ship an edit for must round-trip, though; that is non-negotiable.
    for target in &er_gfx::arts_badge::TARGETS {
        let path = root.join(target.file_name);
        if !path.exists() {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read");
        let out = Movie::parse(&bytes)
            .expect("target parses")
            .write()
            .expect("target writes");
        assert_eq!(
            out, bytes,
            "{} must round-trip byte-identically",
            target.file_name
        );
    }
}
