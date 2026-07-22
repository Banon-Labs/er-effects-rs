use std::{env, path::PathBuf};

const MINHOOK_SRC_DIR_ENV: &str = "MINHOOK_SRC_DIR";

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

    println!("cargo:rerun-if-env-changed={MINHOOK_SRC_DIR_ENV}");
    let default_mh_src_dir = PathBuf::from(&root_dir).join("../../vendor/minhook/src");
    let mh_src_dir = env::var_os(MINHOOK_SRC_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or(default_mh_src_dir);

    cc::Build::new()
        .file(mh_src_dir.join("buffer.c"))
        .file(mh_src_dir.join("hook.c"))
        .file(mh_src_dir.join("trampoline.c"))
        .file(mh_src_dir.join(hde))
        .compile("minhook");

    println!("cargo:rerun-if-changed={}", mh_src_dir.display());
}
