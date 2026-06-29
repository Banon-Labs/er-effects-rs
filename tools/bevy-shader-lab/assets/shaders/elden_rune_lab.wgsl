#import bevy_sprite::mesh2d_vertex_output::VertexOutput

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> glow: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> shadow: vec4<f32>;

fn ring(distance_from_center: f32, radius: f32, width: f32) -> f32 {
    return 1.0 - smoothstep(width, width + 0.015, abs(distance_from_center - radius));
}

fn spoke(uv: vec2<f32>, angle: f32, width: f32) -> f32 {
    let direction = vec2<f32>(cos(angle), sin(angle));
    let perpendicular_distance = abs(uv.x * direction.y - uv.y * direction.x);
    let along = dot(uv, direction);
    let line = 1.0 - smoothstep(width, width + 0.01, perpendicular_distance);
    let mask = smoothstep(-0.9, -0.15, along) * (1.0 - smoothstep(0.15, 0.9, along));
    return line * mask;
}

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let uv = mesh.uv * 2.0 - vec2<f32>(1.0, 1.0);
    let d = length(uv);
    let angle = atan2(uv.y, uv.x);

    let outer = ring(d, 0.78, 0.018);
    let inner = ring(d, 0.38 + 0.035 * sin(angle * 6.0), 0.018);
    let halo = 1.0 - smoothstep(0.10, 0.82, d);
    let runes = step(0.72, fract((angle + 3.14159265) * 11.0)) * ring(d, 0.58, 0.025);

    let spokes = max(
        max(spoke(uv, 0.0, 0.012), spoke(uv, 1.04719755, 0.012)),
        spoke(uv, 2.09439510, 0.012),
    );

    let intensity = clamp(outer + inner + runes + spokes + halo * 0.18, 0.0, 1.0);
    let color = mix(shadow, glow, intensity);
    return vec4<f32>(color.rgb, 1.0);
}
