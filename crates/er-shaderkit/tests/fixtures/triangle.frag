#version 450
// Provenance for triangle.frag.spv (committed alongside). Compiled with the
// system glslangValidator to Vulkan SPIR-V:
//   glslangValidator -V triangle.frag -o triangle.frag.spv
// Used to prove er-shaderkit (naga spv-in + validator) ingests *foreign* SPIR-V
// produced by a non-naga toolchain — the same shape dxil-spirv emits.

layout(location = 0) out vec4 out_color;
layout(set = 0, binding = 0) uniform Tint { vec4 tint; } u;

void main() {
    out_color = u.tint;
}
