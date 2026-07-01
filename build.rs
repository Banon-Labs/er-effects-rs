use std::{env, path::Path};

fn main() {
    let root_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let target = env::var("TARGET").unwrap();
    let arch = target.split('-').next().unwrap_or_default();

    let hde = match arch {
        "i686" => "hde/hde32.c",
        "x86_64" => "hde/hde64.c",
        _ => panic!("Architecture '{arch}' not supported by bundled MinHook"),
    };

    let mh_src_dir = Path::new(&root_dir).join("vendor/minhook/src");

    cc::Build::new()
        .file(mh_src_dir.join("buffer.c"))
        .file(mh_src_dir.join("hook.c"))
        .file(mh_src_dir.join("trampoline.c"))
        .file(mh_src_dir.join(hde))
        .compile("minhook");

    println!("cargo:rerun-if-changed=vendor/minhook/src");
}
