//! Headless wgpu render+readback harness (`--features gpu`).
//!
//! End-to-end pixel proof: build a render pipeline from a WGSL fragment shader,
//! draw a fullscreen triangle into an offscreen RGBA8 texture, and read the
//! pixels back to the CPU. Used by GPU-gated tests to assert a known output
//! colour. Construction returns an error (rather than panicking) when no adapter
//! is available, so callers can skip cleanly off-GPU hosts.

include!("render/core_pipeline.rs");
include!("render/wgsl_fill.rs");
