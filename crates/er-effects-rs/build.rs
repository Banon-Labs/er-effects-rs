use std::{
    env, fs,
    path::{Path, PathBuf},
};

const BANNED_HARD_LINK_SYMBOLS: &[&str] = &[
    "CreateDXGIFactory2",
    "D3D12CreateDevice",
    "D3D12SerializeRootSignature",
    "D3DCompile",
];

const ALLOWED_SOURCE_LINE_SNIPPETS: &[&str] = &[
    "Raw = unsafe extern",
    "s!(\"",
    "w!(\"",
    "GetProcAddress",
    "failed",
    "format_args!",
    "append_autoload_debug",
    "type ",
];

fn main() {
    let root_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let target = env::var("TARGET").unwrap();
    let arch = target.split('-').next().unwrap_or_default();

    enforce_windows_proof_source_gate(Path::new(&root_dir).join("src").as_path());

    let hde = match arch {
        "i686" => "hde/hde32.c",
        "x86_64" => "hde/hde64.c",
        _ => panic!("Architecture '{arch}' not supported by bundled MinHook"),
    };

    let mh_src_dir = resolve_minhook_src_dir(Path::new(&root_dir));

    cc::Build::new()
        .file(mh_src_dir.join("buffer.c"))
        .file(mh_src_dir.join("hook.c"))
        .file(mh_src_dir.join("trampoline.c"))
        .file(mh_src_dir.join(hde))
        .compile("minhook");

    println!("cargo:rerun-if-env-changed=ER_MINHOOK_SRC_DIR");
    println!("cargo:rerun-if-env-changed=ER_EFFECTS_MINHOOK_SRC_DIR");
    println!("cargo:rerun-if-changed={}", mh_src_dir.display());
}

fn resolve_minhook_src_dir(manifest_dir: &Path) -> PathBuf {
    for env_name in ["ER_MINHOOK_SRC_DIR", "ER_EFFECTS_MINHOOK_SRC_DIR"] {
        if let Ok(dir) = env::var(env_name) {
            let dir = PathBuf::from(dir);
            if dir.join("buffer.c").is_file() {
                return dir;
            }
            panic!("{env_name}={} does not point at MinHook src", dir.display());
        }
    }

    let repo_local = manifest_dir.join("../../vendor/minhook/src");
    if repo_local.join("buffer.c").is_file() {
        return repo_local;
    }

    for ancestor in manifest_dir.ancestors() {
        let candidate = ancestor.join("vendor/minhook/src");
        if candidate.join("buffer.c").is_file() {
            return candidate;
        }
    }

    panic!(
        "unable to find vendor/minhook/src (checked {} and ancestor vendor dirs; set ER_MINHOOK_SRC_DIR or ER_EFFECTS_MINHOOK_SRC_DIR to the MinHook src directory)",
        repo_local.display()
    );
}

fn enforce_windows_proof_source_gate(src_root: &Path) {
    let mut failures = Vec::new();
    visit_rust_files(src_root, &mut |path| {
        println!("cargo:rerun-if-changed={}", path.display());
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        for (idx, line) in text.lines().enumerate() {
            if !line_mentions_banned_symbol(line) || source_line_allowed(line) {
                continue;
            }
            failures.push(format!(
                "{}:{}: Windows-proof render must not hard-link D3D12/DXGI/compiler entrypoints: {}",
                path.display(),
                idx + 1,
                line.trim()
            ));
        }
    });
    if !failures.is_empty() {
        panic!(
            "Windows-proof source gate failed; dynamic-load these entrypoints or move them out of the product DLL:\n{}",
            failures.join("\n")
        );
    }
}

fn visit_rust_files(dir: &Path, f: &mut dyn FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit_rust_files(&path, f);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            f(&path);
        }
    }
}

fn line_mentions_banned_symbol(line: &str) -> bool {
    BANNED_HARD_LINK_SYMBOLS
        .iter()
        .any(|symbol| contains_word(line, symbol))
}

fn contains_word(line: &str, symbol: &str) -> bool {
    let mut start = 0;
    while let Some(offset) = line[start..].find(symbol) {
        let pos = start + offset;
        let before = line[..pos].chars().next_back();
        let after = line[pos + symbol.len()..].chars().next();
        if !is_ident_char(before) && !is_ident_char(after) {
            return true;
        }
        start = pos + symbol.len();
    }
    false
}

fn is_ident_char(ch: Option<char>) -> bool {
    matches!(ch, Some(c) if c == '_' || c.is_ascii_alphanumeric())
}

fn source_line_allowed(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with("///") || trimmed.starts_with("//!") {
        return true;
    }
    if line.contains('"') {
        return true;
    }
    if ALLOWED_SOURCE_LINE_SNIPPETS
        .iter()
        .any(|snippet| line.contains(snippet))
    {
        return true;
    }
    trimmed.starts_with("let ") || trimmed.starts_with("const ")
}
