//! End-to-end shader read-path test against a real Elden Ring install.
//!
//! Heavy and environment-dependent (needs the game, Smithbox, wine, dotnet), so
//! it is `#[ignore]` by default. Run explicitly with:
//!     cargo test -p er-soulsformats --test shaders_e2e -- --ignored --nocapture
//! It guards against Andre/SoulsFormats API drift and proves the chain still
//! reaches readable DXIL.

use er_soulsformats::shaders::{self, DxKind, ShaderConfig};

const FLVER_CONTAINER: &str = "/shader/gxflvershader.shaderbnd.dcx";

#[test]
#[ignore = "needs Elden Ring install + Smithbox + wine + dotnet"]
fn survey_and_extract_reaches_dxil() {
    let config = match ShaderConfig::discover() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping: environment not available: {e}");
            return;
        }
    };

    let containers = shaders::survey(&config).expect("survey");
    assert!(
        containers.iter().any(|c| c.path == FLVER_CONTAINER),
        "survey should find {FLVER_CONTAINER}; got {} containers",
        containers.len()
    );

    let out = config.repo_root.join("target/er-shaderbridge/e2e-out");
    let manifest = shaders::extract(&config, FLVER_CONTAINER, &out).expect("extract");
    assert!(
        !manifest.members.is_empty(),
        "container should have members"
    );

    // Every member should be a DXIL/SM6 container.
    let mut classified = 0;
    for m in &manifest.members {
        let bytes = std::fs::read(out.join(&m.file)).expect("read member");
        assert_eq!(
            shaders::classify(&bytes),
            DxKind::Dxil,
            "member {} should be DXIL",
            m.name
        );
        // The DXIL chunk must carve to LLVM bitcode ('BC...').
        let bc = shaders::carve_dxil(&bytes).expect("carve dxil");
        assert_eq!(&bc[0..2], b"BC", "carved bitcode should start with BC");
        classified += 1;
    }
    assert!(classified > 0);
}
