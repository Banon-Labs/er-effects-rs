use std::{env, path::Path};

// MinHook C build, copied verbatim from crates/er-reload-trace-dll/build.rs (the model crate). The
// harness DLL owns its OWN MinHook instance for the XInput runtime-DLL hooks (XInputGetState /
// XInputGetCapabilities). Those addresses live in xinput1_*.dll, NOT in the game image, so they never
// collide with the product DLL's game-address hooks -- a private MinHook instance is correct here and
// no cross-DLL union is required (unlike er-reload-trace-dll, which unions its game-address hooks).
fn main() {
    let root_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let target = env::var("TARGET").unwrap();
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
