// Measure a FromSoftware executable's ARXAN OBFUSCATION profile, so two game builds can be
// compared by the kinds of protection present rather than by a single stub count.
//
// The question this exists to answer: "did this build introduce a new obfuscation / anti-tamper
// technique?" A raw stub total cannot answer it -- a count moving from 1597 to 1597 says nothing
// about whether one tea region list quietly became something dearxan does not model. So this
// reports the population broken down by kind at every stage of dearxan's pipeline:
//
//   raw candidates    every `test rsp, 15` in an executable section (the scan seed)
//   -> stubs          candidates that analyze as real Arxan stubs (NotAStub is filtered out)
//   -> ok / error     analysis outcome, errors bucketed by normalized message
//   -> region lists   ok stubs that declare encrypted regions, by ArxanDecryptionKind
//   -> applied        region lists that survive apply_relocs_and_resolve_conflicts, by kind
//
// A genuinely new technique shows up as a new shape in that table -- an error bucket that did not
// exist before, a collapse in one kind's population, or a rise in the eliminated fraction -- long
// before it shows up as a count. If a build shipped a fourth cipher, its stubs would either fail to
// analyze (new error bucket) or declare regions whose "plaintext" is noise (eliminated on entropy).
//
// Usage:
//   cargo run --release --example profile --no-default-features --features rayon -- \
//       <image> [--mapped] [--regions <out.tsv>]
//
// `--mapped` treats <image> as an already-mapped flat image (file offset == RVA), which is what
// the deobfuscate example writes. That matters because the on-disk .exe for an older build is
// usually gone once the game updates, while its deobfuscated flat image is still on disk -- and
// stub discovery plus region declaration are readable from the flat image, since Arxan's stubs are
// never themselves encrypted. Only the decrypted bytes differ there (decrypting plaintext yields
// noise, so the resolver eliminates nearly everything). Read the "applied" row of a --mapped run as
// a lower bound, never as a region count; the pre-resolution rows are the comparable ones.
//
// NOTE: the input is the copyrighted game binary -- do not commit it or any dump of its bytes.
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use dearxan::analysis::encryption::ArxanDecryptionKind;
use dearxan::analysis::{StubAnalyzer, analyze_all_stubs_with, encryption};
use dearxan_test_utils::{FsExe, init_log, pe_file_to_view};
use pelite::pe64::{Pe, PeFile, PeObject, PeView};

/// Bucket an error message by blanking hex-ish runs, so instances of one failure mode collapse
/// into one key. Same normalization as the stub-audit example, kept identical on purpose: the two
/// tools' error tables are meant to be comparable line for line.
fn normalize(msg: &str) -> String {
    let mut out = String::new();
    let mut run = 0usize;
    for c in msg.chars() {
        if c.is_ascii_hexdigit() {
            run += 1;
        } else {
            if run >= 3 {
                out.push_str("<hex>");
            } else {
                for _ in 0..run {
                    out.push('0');
                }
            }
            run = 0;
            out.push(c);
        }
    }
    if run >= 3 {
        out.push_str("<hex>");
    }
    out
}

fn kind_name(k: ArxanDecryptionKind) -> &'static str {
    match k {
        ArxanDecryptionKind::Tea => "Tea",
        ArxanDecryptionKind::Rmx => "Rmx",
        ArxanDecryptionKind::Sub => "Sub",
    }
}

/// Per-kind tally of region lists, their regions, and their plaintext bytes.
#[derive(Default, Clone, Copy)]
struct Tally {
    lists: usize,
    regions: usize,
    bytes: usize,
}

fn profile(pe: PeView<'_>, regions_tsv: Option<PathBuf>) {
    let image_base = pe.optional_header().ImageBase;
    let image = pe.image();
    println!("image_base           = {image_base:#x}");
    println!("mapped_size          = {:#x} ({} bytes)", image.len(), image.len());

    // The scan seed. dearxan looks for `test rsp, 15` in every executable section; reproducing the
    // count here separates "the build has fewer stubs" from "the build hides its stubs better".
    const TEST_RSP_15: &[u8] = b"\x48\xf7\xc4\x0f\x00\x00\x00";
    let mut raw_total = 0usize;
    println!("\n== EXECUTABLE SECTIONS ==");
    for sec in pe.section_headers() {
        if sec.Characteristics & 0x2000_0000 == 0 {
            continue;
        }
        let start = sec.VirtualAddress as usize;
        let end = (start + sec.VirtualSize as usize).min(image.len());
        let slice = image.get(start..end).unwrap_or(&[]);
        let n = memchr_count(slice, TEST_RSP_15);
        raw_total += n;
        println!(
            "{:<10} va={:#012x} vsz={:#010x} test_rsp_candidates={n}",
            sec.name().unwrap_or("?"),
            image_base + sec.VirtualAddress as u64,
            sec.VirtualSize,
        );
    }
    println!("raw_test_rsp_candidates = {raw_total}");

    let infos = analyze_all_stubs_with(pe, StubAnalyzer::new());

    let mut ok = 0usize;
    let mut err = 0usize;
    let mut with_regions = 0usize;
    let mut without_regions = 0usize;
    let mut err_kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut declared: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut no_return_gadget = 0usize;

    for r in &infos {
        match r {
            Ok(si) => {
                ok += 1;
                if si.return_gadget.is_none() {
                    no_return_gadget += 1;
                }
                match &si.encrypted_regions {
                    Some(rl) => {
                        with_regions += 1;
                        let t = declared.entry(kind_name(rl.kind)).or_default();
                        t.lists += 1;
                        t.regions += rl.len();
                        t.bytes += rl.decrypted_stream.len();
                    }
                    None => without_regions += 1,
                }
            }
            Err(e) => {
                err += 1;
                *err_kinds.entry(normalize(&format!("{e}"))).or_default() += 1;
            }
        }
    }

    println!("\n== STUB POPULATION ==");
    println!("stubs_analyzed        = {}", infos.len());
    println!("analyzed_ok           = {ok}");
    println!("analyze_error         = {err}");
    println!("ok_without_return_gad = {no_return_gadget}");
    println!("ok_with_regions       = {with_regions}");
    println!("ok_inert (no regions) = {without_regions}");

    println!("\n== ERROR MODES (normalized -> count) ==");
    if err_kinds.is_empty() {
        println!("(none)");
    }
    let mut kinds: Vec<_> = err_kinds.into_iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(&a.1));
    for (k, v) in &kinds {
        println!("{v:5}  {k}");
    }

    let final_patches = encryption::apply_relocs_and_resolve_conflicts(
        infos.iter().filter_map(|si| si.as_ref().ok()).filter_map(|si| si.encrypted_regions.as_ref()),
        pe,
        None,
    )
    .expect("apply_relocs_and_resolve_conflicts failed");

    let mut applied: BTreeMap<&'static str, Tally> = BTreeMap::new();
    for rlist in &final_patches {
        let t = applied.entry(kind_name(rlist.kind)).or_default();
        t.lists += 1;
        t.regions += rlist.len();
        t.bytes += rlist.decrypted_stream.len();
    }

    println!("\n== ENCRYPTED REGION LISTS BY KIND ==");
    println!(
        "{:<6} {:>8} {:>8} {:>10}  {:>8} {:>8} {:>10}  {:>10}",
        "kind", "decl_l", "decl_r", "decl_bytes", "appl_l", "appl_r", "appl_bytes", "elim_lists"
    );
    let mut all_kinds: Vec<&'static str> = declared.keys().copied().collect();
    for k in applied.keys() {
        if !all_kinds.contains(k) {
            all_kinds.push(k);
        }
    }
    all_kinds.sort_unstable();
    let (mut dl, mut dr, mut db, mut al, mut ar, mut ab) = (0, 0, 0, 0, 0, 0);
    for k in &all_kinds {
        let d = declared.get(k).copied().unwrap_or_default();
        let a = applied.get(k).copied().unwrap_or_default();
        println!(
            "{k:<6} {:>8} {:>8} {:>10}  {:>8} {:>8} {:>10}  {:>10}",
            d.lists,
            d.regions,
            d.bytes,
            a.lists,
            a.regions,
            a.bytes,
            d.lists as i64 - a.lists as i64
        );
        dl += d.lists;
        dr += d.regions;
        db += d.bytes;
        al += a.lists;
        ar += a.regions;
        ab += a.bytes;
    }
    println!(
        "{:<6} {dl:>8} {dr:>8} {db:>10}  {al:>8} {ar:>8} {ab:>10}  {:>10}",
        "TOTAL",
        dl as i64 - al as i64
    );

    // A declared region list is present in the image when the bytes now sitting at its RVAs are
    // already its own plaintext. On a ciphertext image that is false for everything; on a
    // deobfuscated image it is true for exactly the lists the deobfuscator applied. That is the
    // only way to recover the applied/eliminated split of an old build once its .exe is gone --
    // the resolver itself cannot, because on an already-decrypted image every list loses the
    // entropy comparison and is eliminated. Relocation does not disturb the comparison: with
    // preferred_base = None the reloc delta is zero, so plaintext bytes are written unmodified.
    let mut present_lists = 0usize;
    let mut present: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut list_rows: Vec<String> = Vec::new();
    for si in infos.iter().filter_map(|r| r.as_ref().ok()) {
        let Some(rl) = si.encrypted_regions.as_ref() else { continue };
        let mut matched_bytes = 0usize;
        let mut total_bytes = 0usize;
        for r in &rl.regions {
            let Some(plain) = r.decrypted_slice(rl) else { continue };
            total_bytes += plain.len();
            let start = r.rva as usize;
            if let Some(have) = image.get(start..start + plain.len()) {
                matched_bytes += have.iter().zip(plain).filter(|(a, b)| a == b).count();
            }
        }
        let all_match = total_bytes > 0 && matched_bytes == total_bytes;
        if all_match {
            present_lists += 1;
            let t = present.entry(kind_name(rl.kind)).or_default();
            t.lists += 1;
            t.regions += rl.len();
            t.bytes += total_bytes;
        }
        list_rows.push(format!(
            "{:x}\t{}\t{}\t{}\t{}\t{}",
            si.test_rsp_va,
            kind_name(rl.kind),
            rl.len(),
            total_bytes,
            matched_bytes,
            all_match
        ));
    }
    println!("\n== DECLARED LISTS WHOSE PLAINTEXT IS ALREADY IN THE IMAGE ==");
    println!("present_lists = {present_lists}");
    for (k, t) in &present {
        println!("{k:<6} lists={:<6} regions={:<8} bytes={}", t.lists, t.regions, t.bytes);
    }

    if let Some(path) = regions_tsv {
        let mut out = std::fs::File::create(&path).expect("create regions tsv");
        writeln!(out, "stage\tkind\trva\tsize").unwrap();
        for si in infos.iter().filter_map(|r| r.as_ref().ok()) {
            let Some(rl) = si.encrypted_regions.as_ref() else { continue };
            for r in &rl.regions {
                writeln!(out, "declared\t{}\t{:x}\t{}", kind_name(rl.kind), r.rva, r.size).unwrap();
            }
        }
        for rl in &final_patches {
            for r in &rl.regions {
                writeln!(out, "applied\t{}\t{:x}\t{}", kind_name(rl.kind), r.rva, r.size).unwrap();
            }
        }
        let lp = path.with_extension("lists.tsv");
        let mut lout = std::fs::File::create(&lp).expect("create lists tsv");
        writeln!(lout, "stub_va\tkind\tregions\tbytes\tmatching_bytes\tall_match").unwrap();
        for row in &list_rows {
            writeln!(lout, "{row}").unwrap();
        }
        println!("\nwrote region TSV -> {}", path.display());
        println!("wrote list TSV   -> {}", lp.display());
    }
}

fn memchr_count(haystack: &[u8], needle: &[u8]) -> usize {
    let mut n = 0usize;
    let mut i = 0usize;
    while i + needle.len() <= haystack.len() {
        match haystack[i..].windows(needle.len()).position(|w| w == needle) {
            Some(p) => {
                n += 1;
                i += p + 1;
            }
            None => break,
        }
    }
    n
}

fn main() {
    init_log(log::LevelFilter::Error);

    let mut args = std::env::args().skip(1);
    let in_path = args.next().expect("usage: profile <image> [--mapped] [--regions <out.tsv>]");
    let mut mapped_input = false;
    let mut regions_tsv: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--mapped" => mapped_input = true,
            "--regions" => regions_tsv = Some(PathBuf::from(args.next().expect("--regions needs a path"))),
            other => panic!("unknown argument {other}"),
        }
    }

    println!("== ARXAN PROFILE: {in_path} ==");
    println!("input_form           = {}", if mapped_input { "mapped flat image" } else { "on-disk PE" });

    if mapped_input {
        // A flat mapped image is what PeView expects, so no re-mapping step is involved.
        let bytes = std::fs::read(&in_path).expect("read image");
        let pe = PeView::from_bytes(&bytes).expect("not a mapped PE image");
        profile(pe, regions_tsv);
    } else {
        let path = PathBuf::from(&in_path);
        let game = FsExe {
            game: path.file_stem().unwrap().to_string_lossy().to_string(),
            ver: "0".to_string(),
            path,
        };
        let disk = std::fs::read(&game.path).expect("read exe");
        // Mapped through the same helper the deobfuscate example uses, so the two agree byte for
        // byte on what "the image" means.
        let view = pe_file_to_view(PeFile::from_bytes(&disk).expect("not a PE file"));
        let pe = PeView::from_bytes(&view).expect("mapping produced a bad image");
        profile(pe, regions_tsv);
    }
}
