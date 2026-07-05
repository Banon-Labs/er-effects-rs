
/// naga vector/scalar type -> WGSL type string (inputs are scalars/vectors).
fn wgsl_type(ty: &naga::Type) -> String {
    use naga::TypeInner;
    match &ty.inner {
        TypeInner::Scalar(s) => wgsl_scalar(*s).to_owned(),
        TypeInner::Vector { size, scalar } => {
            format!("vec{}<{}>", vec_len(*size), wgsl_scalar(*scalar))
        }
        _ => "vec4<f32>".to_owned(),
    }
}

fn is_int_type(ty: &naga::Type) -> bool {
    use naga::{ScalarKind, TypeInner};
    matches!(
        ty.inner,
        TypeInner::Scalar(s) | TypeInner::Vector { scalar: s, .. }
            if matches!(s.kind, ScalarKind::Sint | ScalarKind::Uint)
    )
}

fn wgsl_scalar(s: naga::Scalar) -> &'static str {
    match s.kind {
        naga::ScalarKind::Sint => "i32",
        naga::ScalarKind::Uint => "u32",
        naga::ScalarKind::Bool => "bool",
        _ => "f32",
    }
}

fn vec_len(s: naga::VectorSize) -> u32 {
    match s {
        naga::VectorSize::Bi => 2,
        naga::VectorSize::Tri => 3,
        naga::VectorSize::Quad => 4,
    }
}

/// An expression of the given type, derived from `uv` (a vec2<f32> in scope).
fn fill_expr(ty: &naga::Type) -> String {
    use naga::{ScalarKind, TypeInner};
    match &ty.inner {
        TypeInner::Scalar(s) => match s.kind {
            ScalarKind::Float => "uv.x".to_owned(),
            ScalarKind::Uint => "0u".to_owned(),
            ScalarKind::Sint => "0i".to_owned(),
            _ => "false".to_owned(),
        },
        TypeInner::Vector { size, scalar } => {
            let n = vec_len(*size);
            let t = wgsl_scalar(*scalar);
            if matches!(scalar.kind, ScalarKind::Float) {
                let comps = ["uv.x", "uv.y", "0.0", "1.0"];
                format!("vec{n}<f32>({})", comps[..n as usize].join(", "))
            } else {
                let zero = if matches!(scalar.kind, ScalarKind::Uint) {
                    "0u"
                } else {
                    "0i"
                };
                let comps = vec![zero; n as usize];
                format!("vec{n}<{t}>({})", comps.join(", "))
            }
        }
        _ => "vec4<f32>(uv.x, uv.y, 0.0, 1.0)".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOLID_RED: &str = r#"
        @vertex
        fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
            let x = f32(i32(i) - 1) * 4.0;
            let y = f32(i32(i & 1u) * 2 - 1) * 4.0;
            return vec4<f32>(x, y, 0.0, 1.0);
        }
        @fragment
        fn fs_main() -> @location(0) vec4<f32> {
            return vec4<f32>(1.0, 0.0, 0.0, 1.0);
        }
    "#;

    // The decisive Tier-B proof: a real ER vertex shader that naga REJECTS
    // (DrawParameters capability) is nonetheless accepted by the GPU via SPIR-V
    // passthrough — the path the viewer (er-effects-rs-f9t) uses for real shaders.
    // Gated on GPU + dxil-spirv + a locally extracted member (game bytecode is not
    // committed); skips cleanly otherwise.
    #[test]
    fn real_er_drawparameters_shader_accepted_via_passthrough() {
        let headless = match Headless::new() {
            Ok(h) => h,
            Err(e) => {
                eprintln!("SKIP passthrough proof (no GPU): {e}");
                return;
            }
        };
        if !headless.supports_passthrough() {
            eprintln!("SKIP passthrough proof: adapter lacks SPIRV_SHADER_PASSTHROUGH");
            return;
        }
        if crate::discover_dxil_spirv().is_none() {
            eprintln!("SKIP passthrough proof: dxil-spirv not built");
            return;
        }
        // Find a locally extracted vertex member (DrawParameters-using).
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/er-shaderbridge/disasm-tmp");
        let member = std::fs::read_dir(&dir).ok().and_then(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.extension().and_then(|e| e.to_str()) == Some("vpo"))
        });
        let Some(member) = member else {
            eprintln!(
                "SKIP passthrough proof: no extracted .vpo under {}",
                dir.display()
            );
            return;
        };

        let spirv = crate::translate::dxil_file_to_spirv(&member, None)
            .expect("real ER vertex shader should translate to SPIR-V");

        // Confirm the premise: naga rejects this shader...
        let naga = crate::validate_spirv(&spirv);
        assert!(
            naga.is_err(),
            "expected naga to reject a DrawParameters shader, but it passed: {member:?}"
        );
        // ...yet passthrough accepts it on the real driver.
        headless
            .create_spirv_passthrough(&spirv)
            .expect("GPU should accept the ER shader via SPIR-V passthrough");
    }

    #[test]
    fn solid_red_shader_fills_centre_pixel_red() {
        let headless = match Headless::new() {
            Ok(h) => h,
            // No GPU in this environment: the deterministic naga tests still
            // cover ingestion; skip the pixel proof rather than fail spuriously.
            Err(e) => {
                eprintln!("SKIP solid_red_shader_fills_centre_pixel_red: {e}");
                return;
            }
        };
        let size = 8;
        let pixels = headless.render_wgsl(SOLID_RED, size).expect("render");
        let centre = pixels[(size * size / 2 + size / 2) as usize];
        assert!(
            centre[0] > 200 && centre[1] < 50 && centre[2] < 50,
            "centre pixel should be red, got {centre:?}"
        );
    }
}
