//! er-objecttrace: trace an Elden Ring shader/material to the objects that use it.
//!
//!   er-objecttrace <shader-or-material>      e.g. "C[Fur]"  or  "C[DetailBlend].spx"
//!   er-objecttrace --list [substr]           list known shader names (optional filter)
//!   er-objecttrace --extract <shader>        (re)extract the matbin archive first
//!   er-objecttrace --matbin-dir <dir> ...    use an explicit extracted-matbin dir
//!
//! The matbin corpus is extracted once (via the er-soulsformats wine shaderbridge)
//! to <repo>/target/er-objectkit/matbin and reused.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use er_objectkit::TraceIndex;

const MATBIN_ARCHIVE: &str = "material/allmaterial.matbinbnd.dcx";

fn default_matbin_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/er-objectkit/matbin")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut matbin_dir = default_matbin_dir();
    let mut force_extract = false;
    let mut list = false;
    let mut positional: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--matbin-dir" => match it.next() {
                Some(d) => matbin_dir = PathBuf::from(d),
                None => return usage("--matbin-dir needs a path"),
            },
            "--extract" => force_extract = true,
            "--list" => list = true,
            "-h" | "--help" => return usage(""),
            other => positional.push(other.to_owned()),
        }
    }

    if force_extract || !dir_has_matbins(&matbin_dir) {
        if let Err(e) = extract_corpus(&matbin_dir) {
            eprintln!("extraction failed: {e}");
            eprintln!(
                "(need Elden Ring + Smithbox; or point --matbin-dir at an existing extraction)"
            );
            return ExitCode::FAILURE;
        }
    }

    let (idx, skipped) = match TraceIndex::from_matbin_dir(&matbin_dir) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("reading {}: {e}", matbin_dir.display());
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "indexed {} materials ({} distinct shaders, {skipped} skipped) from {}",
        idx.entries.len(),
        idx.shaders().len(),
        matbin_dir.display()
    );

    if list {
        let filter = positional.first().map(|s| s.as_str()).unwrap_or("");
        for s in idx.shaders().into_iter().filter(|s| s.contains(filter)) {
            println!("{s}");
        }
        return ExitCode::SUCCESS;
    }

    let Some(query) = positional.first() else {
        return usage("missing <shader-or-material>");
    };

    let entries = idx.trace_shader(query);
    if entries.is_empty() {
        println!("no materials use shader {query:?}");
        println!("(try --list {query} to find the right name)");
        return ExitCode::SUCCESS;
    }
    let objects = idx.objects_for_shader(query);
    println!(
        "shader {query:?}: {} materials across {} objects",
        entries.len(),
        objects.len()
    );
    for o in &objects {
        let flver = o
            .flver_container()
            .map(|c| format!("  ->  {c}"))
            .unwrap_or_default();
        println!("  [{}] {}{}", o.category.as_str(), o.model, flver);
    }
    ExitCode::SUCCESS
}

fn dir_has_matbins(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut rd| {
            rd.any(|e| {
                e.ok()
                    .map(|e| e.path().extension().and_then(|x| x.to_str()) == Some("matbin"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn extract_corpus(dir: &Path) -> Result<(), String> {
    use er_soulsformats::shaders::{self, ShaderConfig};
    eprintln!("extracting {MATBIN_ARCHIVE} -> {} ...", dir.display());
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let config = ShaderConfig::discover().map_err(|e| e.to_string())?;
    let manifest = shaders::extract(&config, MATBIN_ARCHIVE, dir).map_err(|e| e.to_string())?;
    eprintln!("extracted {} members", manifest.members.len());
    Ok(())
}

fn usage(msg: &str) -> ExitCode {
    if !msg.is_empty() {
        eprintln!("error: {msg}\n");
    }
    eprintln!(
        "usage:\n  er-objecttrace <shader-or-material>\n  er-objecttrace --list [substr]\n  er-objecttrace [--extract] [--matbin-dir <dir>] <shader>"
    );
    if msg.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
