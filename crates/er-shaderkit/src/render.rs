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
    passthrough: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("no suitable GPU adapter available: {0}")]
    NoAdapter(String),
    #[error("device request failed: {0}")]
    Device(String),
    #[error("readback failed: {0}")]
    Readback(String),
    #[error("pipeline/shader rejected: {0}")]
    Pipeline(String),
}

/// One RGBA8 pixel.
pub type Rgba = [u8; 4];

/// Resource kind for an object-pipeline binding (from passthrough reflection).
#[derive(Clone, Copy, Debug)]
pub enum ObjBind {
    Texture,
    Sampler,
    Uniform,
    Storage,
}

/// A reconstructed object render pipeline: passthrough vertex+pixel SPIR-V plus the
/// vertex-input + bind-group layout recovered from reflection (er-objectkit supplies
/// this). Creating it validates the layout against the real shader interface — the
/// Vulkan driver checks compatibility at pipeline creation.
pub struct ObjPipeline<'a> {
    pub vertex_spirv: &'a [u8],
    pub pixel_spirv: &'a [u8],
    /// Entry-point names (passthrough modules carry no reflection, so wgpu needs
    /// these explicitly). dxil-spirv typically emits `main`.
    pub vertex_entry: &'a str,
    pub pixel_entry: &'a str,
    /// Vertex input attribute locations (each fed `Float32x4`).
    pub vertex_locations: &'a [u32],
    /// `(set, binding, kind)` for every resource the shaders use.
    pub bindings: &'a [(u32, u32, ObjBind)],
    /// Number of colour render targets (fragment output locations).
    pub color_targets: usize,
}

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
        // ER shaders use SPIR-V capabilities naga's frontend rejects (e.g.
        // DrawParameters on nearly every FLVER vertex shader). The escape hatch is
        // SPIR-V passthrough: hand validated SPIR-V straight to the driver. Enable
        // it when the adapter supports it so the viewer can run real ER shaders.
        let passthrough = adapter
            .features()
            .contains(wgpu::Features::PASSTHROUGH_SHADERS);
        let required_features = if passthrough {
            wgpu::Features::PASSTHROUGH_SHADERS
        } else {
            wgpu::Features::empty()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("er-shaderkit headless"),
                required_features,
                // ER shaders bind far more than the downlevel defaults (e.g. 18
                // sampled textures vs the default 16); ask for the adapter's full
                // limits so reconstructed pipelines aren't rejected on limits.
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .block_on()
            .map_err(|e| RenderError::Device(e.to_string()))?;
        Ok(Self {
            device,
            queue,
            passthrough,
        })
    }

    /// Whether this adapter exposes SPIR-V passthrough (the path that runs ER
    /// shaders naga can't validate).
    pub fn supports_passthrough(&self) -> bool {
        self.passthrough
    }

    /// Create a shader module from raw SPIR-V via passthrough (no naga
    /// validation), capturing any driver/validation error. This is the runtime
    /// path the viewer uses for ER shaders that fail naga. Returns `Ok` if the
    /// driver accepts the module.
    pub fn create_spirv_passthrough(&self, spirv: &[u8]) -> Result<(), RenderError> {
        if !self.passthrough {
            return Err(RenderError::NoAdapter(
                "SPIRV passthrough unsupported".into(),
            ));
        }
        let words = wgpu::util::make_spirv_raw(spirv);
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _module = unsafe {
            self.device
                .create_shader_module_passthrough(wgpu::ShaderModuleDescriptorPassthrough {
                    label: Some("er-shaderkit passthrough"),
                    num_workgroups: (0, 0, 0),
                    spirv: Some(words),
                    dxil: None,
                    hlsl: None,
                    metallib: None,
                    msl: None,
                    glsl: None,
                    wgsl: None,
                })
        };
        match scope.pop().block_on() {
            Some(err) => Err(RenderError::Device(err.to_string())),
            None => Ok(()),
        }
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

    /// Render an ER **fragment** shader from its SPIR-V, reflection-driven: a
    /// matching fullscreen vertex stage is generated from the fragment's location
    /// inputs, and every resource binding (uniform/texture/sampler) is stubbed
    /// with zeros. Returns the first colour target as RGBA pixels. Errors (rather
    /// than panicking) when the shader isn't naga-ingestible or the pipeline is
    /// rejected, so a viewer can mark it instead of crashing.
    pub fn render_fragment_spirv(&self, spirv: &[u8], size: u32) -> Result<Vec<Rgba>, RenderError> {
        use naga::{Binding, TypeInner};

        let module = naga::front::spv::parse_u8_slice(spirv, &naga::front::spv::Options::default())
            .map_err(|e| RenderError::Pipeline(format!("spv parse: {e}")))?;
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .map_err(|e| RenderError::Pipeline(format!("naga validate: {e}")))?;
        let frag_wgsl =
            naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
                .map_err(|e| RenderError::Pipeline(format!("wgsl emit: {e}")))?;

        let entry = module
            .entry_points
            .iter()
            .find(|e| e.stage == naga::ShaderStage::Fragment)
            .ok_or_else(|| RenderError::Pipeline("no fragment entry point".into()))?;
        let entry_name = entry.name.clone();

        // Collect location inputs (skip builtins; wgpu provides them).
        let mut inputs: Vec<(u32, &naga::Type, bool)> = Vec::new();
        for arg in &entry.function.arguments {
            match &arg.binding {
                Some(Binding::Location { location, .. }) => {
                    let ty = &module.types[arg.ty];
                    inputs.push((*location, ty, is_int_type(ty)));
                }
                None => {
                    if let TypeInner::Struct { members, .. } = &module.types[arg.ty].inner {
                        for m in members {
                            if let Some(Binding::Location { location, .. }) = m.binding {
                                let ty = &module.types[m.ty];
                                inputs.push((location, ty, is_int_type(ty)));
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Count colour outputs (one render target per location output).
        let n_targets = match &entry.function.result {
            Some(res) => match res.binding {
                Some(Binding::Location { .. }) => 1,
                _ => match &module.types[res.ty].inner {
                    TypeInner::Struct { members, .. } => members
                        .iter()
                        .filter(|m| matches!(m.binding, Some(Binding::Location { .. })))
                        .count(),
                    _ => 1,
                },
            },
            None => 0,
        };
        if n_targets == 0 {
            return Err(RenderError::Pipeline("shader has no colour output".into()));
        }

        // Generate the vertex stage: fullscreen triangle + each input filled from uv.
        let mut vout = String::from("struct VOut {\n  @builtin(position) pos: vec4<f32>,\n");
        let mut body = String::new();
        for (loc, ty, is_int) in &inputs {
            let interp = if *is_int { "@interpolate(flat) " } else { "" };
            vout += &format!("  {interp}@location({loc}) v{loc}: {},\n", wgsl_type(ty));
            body += &format!("  o.v{loc} = {};\n", fill_expr(ty));
        }
        vout += "}\n";
        let vert_wgsl = format!(
            "{vout}\n@vertex\nfn vs_main(@builtin(vertex_index) vi: u32) -> VOut {{\n  \
             var ps = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));\n  \
             let xy = ps[vi];\n  let uv = xy * vec2<f32>(0.5, 0.5) + vec2<f32>(0.5, 0.5);\n  \
             var o: VOut;\n  o.pos = vec4<f32>(xy, 0.0, 1.0);\n{body}  return o;\n}}\n"
        );

        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);

        let vmod = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("er-frag vertex"),
                source: wgpu::ShaderSource::Wgsl(vert_wgsl.into()),
            });
        let fmod = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("er-frag fragment"),
                source: wgpu::ShaderSource::Wgsl(frag_wgsl.into()),
            });

        // Stub every binding (group 0 assumed; ER shaders bind there).
        let mut layout_entries: Vec<wgpu::BindGroupLayoutEntry> = Vec::new();
        let mut buffers: Vec<wgpu::Buffer> = Vec::new();
        let mut views: Vec<wgpu::TextureView> = Vec::new();
        let mut samplers: Vec<wgpu::Sampler> = Vec::new();
        // Record (binding, kind, index-into-vec) to build bind group entries after.
        enum Res {
            Buf(usize),
            Tex(usize),
            Smp(usize),
        }
        let mut res: Vec<(u32, Res)> = Vec::new();
        for (_h, gv) in module.global_variables.iter() {
            let Some(rb) = &gv.binding else { continue };
            match &module.types[gv.ty].inner {
                TypeInner::Image { .. } => {
                    // A 64x64 gradient (not flat) so texture-driven shaders show detail.
                    let dim = 64u32;
                    let mut texels = Vec::with_capacity((dim * dim * 4) as usize);
                    for y in 0..dim {
                        for x in 0..dim {
                            texels.extend_from_slice(&[
                                (x * 255 / dim) as u8,
                                (y * 255 / dim) as u8,
                                128,
                                255,
                            ]);
                        }
                    }
                    let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                        label: None,
                        size: wgpu::Extent3d {
                            width: dim,
                            height: dim,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    });
                    self.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &tex,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &texels,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(dim * 4),
                            rows_per_image: Some(dim),
                        },
                        wgpu::Extent3d {
                            width: dim,
                            height: dim,
                            depth_or_array_layers: 1,
                        },
                    );
                    layout_entries.push(wgpu::BindGroupLayoutEntry {
                        binding: rb.binding,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    });
                    views.push(tex.create_view(&wgpu::TextureViewDescriptor::default()));
                    res.push((rb.binding, Res::Tex(views.len() - 1)));
                }
                TypeInner::Sampler { .. } => {
                    layout_entries.push(wgpu::BindGroupLayoutEntry {
                        binding: rb.binding,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    });
                    samplers.push(
                        self.device
                            .create_sampler(&wgpu::SamplerDescriptor::default()),
                    );
                    res.push((rb.binding, Res::Smp(samplers.len() - 1)));
                }
                _ => {
                    // Uniform buffer: filled with 0.5-floats (not zero) so colour
                    // params drawn from cbuffers produce visible mid-tones. Generously
                    // sized so any in-shader index stays in-bounds.
                    let sz = 65536u64;
                    let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: None,
                        size: sz,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    let half = 0.5f32.to_le_bytes();
                    let fill: Vec<u8> = half.iter().copied().cycle().take(sz as usize).collect();
                    self.queue.write_buffer(&buf, 0, &fill);
                    layout_entries.push(wgpu::BindGroupLayoutEntry {
                        binding: rb.binding,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    });
                    buffers.push(buf);
                    res.push((rb.binding, Res::Buf(buffers.len() - 1)));
                }
            }
        }

        let bgl = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("er-frag bgl"),
                entries: &layout_entries,
            });
        let bg_entries: Vec<wgpu::BindGroupEntry> = res
            .iter()
            .map(|(binding, r)| wgpu::BindGroupEntry {
                binding: *binding,
                resource: match r {
                    Res::Buf(i) => buffers[*i].as_entire_binding(),
                    Res::Tex(i) => wgpu::BindingResource::TextureView(&views[*i]),
                    Res::Smp(i) => wgpu::BindingResource::Sampler(&samplers[*i]),
                },
            })
            .collect();
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("er-frag bg"),
            layout: &bgl,
            entries: &bg_entries,
        });
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("er-frag layout"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });

        let format = wgpu::TextureFormat::Rgba8Unorm;
        let targets: Vec<Option<wgpu::ColorTargetState>> = (0..n_targets)
            .map(|_| {
                Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })
            })
            .collect();
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("er-frag pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &vmod,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &fmod,
                    entry_point: Some(&entry_name),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        // One texture per target; target 0 is also COPY_SRC for readback.
        let textures: Vec<wgpu::Texture> = (0..n_targets)
            .map(|i| {
                let usage = if i == 0 {
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC
                } else {
                    wgpu::TextureUsages::RENDER_ATTACHMENT
                };
                self.device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: size,
                        height: size,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            })
            .collect();
        let tviews: Vec<wgpu::TextureView> = textures
            .iter()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();
        let attachments: Vec<Option<wgpu::RenderPassColorAttachment>> = tviews
            .iter()
            .map(|v| {
                Some(wgpu::RenderPassColorAttachment {
                    view: v,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })
            })
            .collect();

        let bytes_per_pixel = 4u32;
        let padded = (size * bytes_per_pixel).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (padded * size) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("er-frag pass"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &textures[0],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
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

        if let Some(err) = scope.pop().block_on() {
            return Err(RenderError::Pipeline(err.to_string()));
        }

        let slice = readback.slice(..);
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

    /// Build an object render pipeline from passthrough vertex+pixel SPIR-V and a
    /// reconstructed vertex/bind layout, returning `Ok` if the driver accepts it (the
    /// real check that the reconstructed interface matches the shaders). Does not draw;
    /// pipeline creation alone validates the layout/shader compatibility.
    pub fn create_object_pipeline_passthrough(&self, p: &ObjPipeline) -> Result<(), RenderError> {
        if !self.passthrough {
            return Err(RenderError::NoAdapter(
                "SPIRV passthrough unsupported".into(),
            ));
        }
        let make = |spirv: &[u8], label: &'static str| unsafe {
            self.device
                .create_shader_module_passthrough(wgpu::ShaderModuleDescriptorPassthrough {
                    label: Some(label),
                    num_workgroups: (0, 0, 0),
                    spirv: Some(wgpu::util::make_spirv_raw(spirv)),
                    dxil: None,
                    hlsl: None,
                    metallib: None,
                    msl: None,
                    glsl: None,
                    wgsl: None,
                })
        };

        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let vs = make(p.vertex_spirv, "obj-vs");
        let fs = make(p.pixel_spirv, "obj-fs");

        // Bind-group layouts grouped by descriptor set.
        use std::collections::BTreeMap;
        let mut by_set: BTreeMap<u32, Vec<(u32, ObjBind)>> = BTreeMap::new();
        for &(set, binding, kind) in p.bindings {
            by_set.entry(set).or_default().push((binding, kind));
        }
        let vis = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let bgls: Vec<wgpu::BindGroupLayout> = by_set
            .values()
            .map(|binds| {
                let entries: Vec<wgpu::BindGroupLayoutEntry> = binds
                    .iter()
                    .map(|&(binding, kind)| wgpu::BindGroupLayoutEntry {
                        binding,
                        visibility: vis,
                        ty: match kind {
                            ObjBind::Texture => wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            ObjBind::Sampler => {
                                wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
                            }
                            ObjBind::Uniform => wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            ObjBind::Storage => wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                        },
                        count: None,
                    })
                    .collect();
                self.device
                    .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some("obj-bgl"),
                        entries: &entries,
                    })
            })
            .collect();
        let bgl_refs: Vec<Option<&wgpu::BindGroupLayout>> = bgls.iter().map(Some).collect();
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("obj-pl"),
                bind_group_layouts: &bgl_refs,
                immediate_size: 0,
            });

        // One interleaved vertex buffer, Float32x4 per input location.
        let attrs: Vec<wgpu::VertexAttribute> = p
            .vertex_locations
            .iter()
            .enumerate()
            .map(|(i, &loc)| wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: (i as u64) * 16,
                shader_location: loc,
            })
            .collect();
        let vbl = wgpu::VertexBufferLayout {
            array_stride: ((p.vertex_locations.len() as u64) * 16).max(16),
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &attrs,
        };

        let targets: Vec<Option<wgpu::ColorTargetState>> = (0..p.color_targets.max(1))
            .map(|_| {
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })
            })
            .collect();

        let _pipe = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("obj-pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &vs,
                    entry_point: Some(p.vertex_entry),
                    compilation_options: Default::default(),
                    buffers: &[vbl],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &fs,
                    entry_point: Some(p.pixel_entry),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        match scope.pop().block_on() {
            Some(err) => Err(RenderError::Pipeline(err.to_string())),
            None => Ok(()),
        }
    }
}

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
