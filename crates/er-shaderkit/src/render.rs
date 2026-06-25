//! Headless wgpu render+readback harness (`--features gpu`).
//!
//! End-to-end pixel proof: build a render pipeline from a WGSL fragment shader,
//! draw a fullscreen triangle into an offscreen RGBA8 texture, and read the
//! pixels back to the CPU. Used by GPU-gated tests to assert a known output
//! colour. Construction returns an error (rather than panicking) when no adapter
//! is available, so callers can skip cleanly off-GPU hosts.

use pollster::FutureExt as _;

/// A minimal headless GPU context.
pub struct Headless {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("no suitable GPU adapter available: {0}")]
    NoAdapter(String),
    #[error("device request failed: {0}")]
    Device(String),
    #[error("readback failed: {0}")]
    Readback(String),
}

/// One RGBA8 pixel.
pub type Rgba = [u8; 4];

impl Headless {
    /// Initialise a headless device on any available backend. Errors (does not
    /// panic) when no adapter can be acquired.
    pub fn new() -> Result<Self, RenderError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: Default::default(),
            display: None,
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .block_on()
            .map_err(|e| RenderError::NoAdapter(e.to_string()))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("er-shaderkit headless"),
                ..Default::default()
            })
            .block_on()
            .map_err(|e| RenderError::Device(e.to_string()))?;
        Ok(Self { device, queue })
    }

    /// Render `fragment_wgsl` (must expose `vs_main`/`fs_main` like the test
    /// fixtures) into a `size x size` RGBA8 target and return the pixels row-major.
    pub fn render_wgsl(&self, wgsl: &str, size: u32) -> Result<Vec<Rgba>, RenderError> {
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("er-shaderkit shader"),
                source: wgpu::ShaderSource::Wgsl(wgsl.into()),
            });

        let format = wgpu::TextureFormat::Rgba8Unorm;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("er-shaderkit target"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("er-shaderkit layout"),
                bind_group_layouts: &[],
                immediate_size: 0,
            });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("er-shaderkit pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        // Tightly packed readback buffer needs a 256-byte row alignment.
        let bytes_per_pixel = 4u32;
        let unpadded = size * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = unpadded.div_ceil(align) * align;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("er-shaderkit readback"),
            size: (padded * size) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("er-shaderkit pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(size),
                },
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .ok();
        rx.recv()
            .map_err(|e| RenderError::Readback(e.to_string()))?
            .map_err(|e| RenderError::Readback(e.to_string()))?;

        let data = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity((size * size) as usize);
        for row in 0..size {
            let start = (row * padded) as usize;
            for col in 0..size {
                let p = start + (col * bytes_per_pixel) as usize;
                pixels.push([data[p], data[p + 1], data[p + 2], data[p + 3]]);
            }
        }
        Ok(pixels)
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
