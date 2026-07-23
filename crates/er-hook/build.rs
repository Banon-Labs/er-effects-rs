use std::{
    env,
    path::{Path, PathBuf},
};

// Single cc-compile of the vendored MinHook C source, replacing the three near-identical build
// scripts that previously lived in each game cdylib (er-effects-rs, er-reload-trace-dll,
// er-input-harness-dll). Windows-target gated -- the C uses Win32 APIs, so a host `cargo check`
// (which still runs build scripts) must not try to compile it.
fn main() {
    let root_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let target = env::var("TARGET").unwrap();
    if !target.contains("windows") {
        println!("cargo:rerun-if-changed=build.rs");
        return;
    }

    let arch = target.split('-').next().unwrap_or_default();

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

    println!("cargo:rerun-if-changed={}", mh_src_dir.display());
}

fn resolve_minhook_src_dir(manifest_dir: &Path) -> PathBuf {
    if let Ok(dir) = env::var("ER_EFFECTS_MINHOOK_SRC_DIR") {
        let dir = PathBuf::from(dir);
        if dir.join("buffer.c").exists() {
            return dir;
        }
        panic!(
            "ER_EFFECTS_MINHOOK_SRC_DIR={} does not contain buffer.c",
            dir.display()
        );
    }

    let repo_local = manifest_dir.join("../../vendor/minhook/src");
    if repo_local.join("buffer.c").exists() {
        return repo_local;
    }

    for ancestor in manifest_dir.ancestors() {
        let candidate = ancestor.join("vendor/minhook/src");
        if candidate.join("buffer.c").exists() {
            return candidate;
        }
    }

    panic!(
        "unable to find vendor/minhook/src (set ER_EFFECTS_MINHOOK_SRC_DIR to the MinHook src directory)"
    );
}
