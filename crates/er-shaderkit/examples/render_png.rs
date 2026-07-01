//! Render one extracted ER fragment member (DXIL) through the backend to a PNG.
//!   cargo run -p er-shaderkit --features gpu --example render_png -- <member.ppo> <out.png> [size]
//! Uses the reflection-driven generic renderer (er_shaderkit::render::Headless).

use er_shaderkit::render::Headless;

fn main() {
    let mut args = std::env::args().skip(1);
    let inp = args
        .next()
        .expect("usage: render_png <member> <out.png> [size]");
    let out = args
        .next()
        .expect("usage: render_png <member> <out.png> [size]");
    let size: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(256);

    let bytes = std::fs::read(&inp).unwrap_or_else(|e| panic!("read {inp}: {e}"));
    let spirv = er_shaderkit::dxil_to_spirv(&bytes, None).expect("DXIL -> SPIR-V");
    let headless = Headless::new().expect("no GPU");
    let pixels = headless
        .render_fragment_spirv(&spirv, size)
        .unwrap_or_else(|e| panic!("render {inp}: {e}"));

    let mut img = image::RgbaImage::new(size, size);
    for (i, p) in pixels.iter().enumerate() {
        img.put_pixel((i as u32) % size, (i as u32) / size, image::Rgba(*p));
    }
    img.save(&out).unwrap_or_else(|e| panic!("save {out}: {e}"));
    eprintln!("wrote {out} ({size}x{size}) from {inp}");
}
