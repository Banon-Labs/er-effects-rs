use std::{env, path::Path};

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

    let mh_src_dir = Path::new(&root_dir).join("../../vendor/minhook/src");

    cc::Build::new()
        .file(mh_src_dir.join("buffer.c"))
        .file(mh_src_dir.join("hook.c"))
        .file(mh_src_dir.join("trampoline.c"))
        .file(mh_src_dir.join(hde))
        .compile("minhook");

    println!("cargo:rerun-if-changed=../../vendor/minhook/src");
}
