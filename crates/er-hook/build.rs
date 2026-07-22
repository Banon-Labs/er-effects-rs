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

    let mh_src_dir = resolve_minhook_src_dir(&root_dir);

    cc::Build::new()
        .file(mh_src_dir.join("buffer.c"))
        .file(mh_src_dir.join("hook.c"))
        .file(mh_src_dir.join("trampoline.c"))
        .file(mh_src_dir.join(hde))
        .compile("minhook");

    println!("cargo:rerun-if-env-changed=ER_MINHOOK_SRC_DIR");
    println!("cargo:rerun-if-changed={}", mh_src_dir.display());
}

fn resolve_minhook_src_dir(root_dir: &str) -> PathBuf {
    if let Ok(override_dir) = env::var("ER_MINHOOK_SRC_DIR") {
        let path = PathBuf::from(override_dir);
        if path.join("buffer.c").is_file() {
            return path;
        }
        panic!(
            "ER_MINHOOK_SRC_DIR does not point at MinHook src: {}",
            path.display()
        );
    }

    let manifest_dir = Path::new(root_dir);
    let default = manifest_dir.join("../../vendor/minhook/src");
    if default.join("buffer.c").is_file() {
        return default;
    }

    for ancestor in manifest_dir.ancestors() {
        let candidate = ancestor.join("vendor/minhook/src");
        if candidate.join("buffer.c").is_file() {
            return candidate;
        }
    }

    panic!(
        "could not find vendor/minhook/src (checked {} and ancestor vendor dirs); set ER_MINHOOK_SRC_DIR to the MinHook src directory",
        default.display()
    );
}
