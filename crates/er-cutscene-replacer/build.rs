use std::{env, fs, path::PathBuf};

const CUTSCENE_OVERLAY_HELPER_EXE_ENV: &str = "ER_CUTSCENE_OVERLAY_HELPER_EXE";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed={CUTSCENE_OVERLAY_HELPER_EXE_ENV}");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let generated = out_dir.join("cutscene_overlay_helper_embed.rs");
    let source = env::var_os(CUTSCENE_OVERLAY_HELPER_EXE_ENV)
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    let body = if let Some(path) = source {
        println!("cargo:rerun-if-changed={}", path.display());
        format!(
            "const EMBEDDED_CUTSCENE_OVERLAY_HELPER: Option<&'static [u8]> = Some(include_bytes!({:?}));\n",
            path.to_string_lossy()
        )
    } else {
        "const EMBEDDED_CUTSCENE_OVERLAY_HELPER: Option<&'static [u8]> = None;\n".to_owned()
    };
    fs::write(generated, body).expect("write generated cutscene overlay helper embed module");
}
