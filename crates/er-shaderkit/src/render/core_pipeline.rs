use pollster::FutureExt as _;

/// A minimal headless GPU context.
pub struct Headless {
    device: wgpu::Device,
    queue: wgpu::Queue,
    passthrough: bool,
    adapter_name: String,
    is_software: bool,
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

/// One vertex buffer to bind in an object draw: the verbatim interleaved bytes plus
/// the attributes (location, format, offset) the shader reads from it.
pub struct ObjVbo<'a> {
    pub data: &'a [u8],
    pub stride: u64,
    /// `(shader_location, format, byte_offset_within_vertex)`.
    pub attributes: &'a [(u32, wgpu::VertexFormat, u64)],
}

/// Write `bytes` into the stub uniform buffer at `(set, binding)` starting at `offset`
/// — used to place a synthesized matrix (e.g. WorldViewProj) at its reflected cbuffer
/// offset so the native vertex shader projects geometry on-screen.
pub struct UniformWrite<'a> {
    pub set: u32,
    pub binding: u32,
    pub offset: u64,
    pub bytes: &'a [u8],
}

/// A REAL captured texture's pixels for the frame-replay path: replaces a stub at
/// `(set, binding)` with the game's actual texture (IBL cubemap, GI irradiance volume,
/// material map). `data` is tightly-packed UNCOMPRESSED texels (mip 0, all layers) in
/// `format`; the extract step decodes BCn → rgba so the upload stays simple.
pub struct RealTexture<'a> {
    pub set: u32,
    pub binding: u32,
    pub width: u32,
    pub height: u32,
    /// Depth (3D) or array-layer count (cube = 6, 2D = 1).
    pub depth_or_layers: u32,
    pub dim: wgpu::TextureViewDimension,
    pub format: wgpu::TextureFormat,
    pub data: &'a [u8],
}

/// A full object DRAW (not just pipeline creation): real vertex+index buffers bound to
/// the native vertex+pixel passthrough shaders, every resource stubbed, selected cbuffers
/// overwritten with real matrices, drawn into an offscreen target and read back.
pub struct ObjDrawDesc<'a> {
    pub vertex_spirv: &'a [u8],
    pub pixel_spirv: &'a [u8],
    pub vertex_entry: &'a str,
    pub pixel_entry: &'a str,
    pub vertex_buffers: &'a [ObjVbo<'a>],
    pub indices: &'a [u32],
    /// Every resource binding either stage uses: `(set, binding, kind)`.
    pub bindings: &'a [(u32, u32, ObjBind)],
    /// Minimum byte size for a uniform buffer at `(set, binding)`; absent → 64 KiB.
    pub uniform_sizes: &'a [(u32, u32, u64)],
    /// Matrix/constant overrides written into the stub uniform buffers.
    pub uniform_writes: &'a [UniformWrite<'a>],
    /// FRAME-REPLAY: full captured contents for a uniform/storage buffer at `(set, binding)`
    /// — the game's real cbuffers (scene lighting, material params). Sizes the buffer to the
    /// data and uploads it, replacing the zeroed stub. `uniform_writes` still apply on top.
    pub buffer_data: &'a [(u32, u32, &'a [u8])],
    /// FRAME-REPLAY: real captured textures (IBL/GI/material), replacing the gray stubs.
    pub textures: &'a [RealTexture<'a>],
    pub color_targets: usize,
    pub size: u32,
    /// When `Some`, replace the native pixel shader with this WGSL fragment (entry
    /// `fs_main`, one colour target). Isolates the native VERTEX shader's projection
    /// from the native pixel shader's shading — a solid colour appears wherever real
    /// geometry rasterises, regardless of lighting/texture cbuffers.
    pub pixel_wgsl: Option<&'a str>,
}

/// A sampled-texture binding's reflected view dimension + sample type, so the harness can
/// stub each texture to MATCH. A generic 1×1 2D stub bound where the shader samples a cube
/// or 3D texture segfaults llvmpipe's fragment thread (ER pixel shaders sample IBL cubemaps
/// and 3D irradiance/fog volumes).
#[derive(Clone, Copy, Debug)]
pub struct ImageBinding {
    pub set: u32,
    pub binding: u32,
    pub view_dim: wgpu::TextureViewDimension,
    pub sample_type: wgpu::TextureSampleType,
}

fn spv_word(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]])
}

/// Reflect every sampled-image (texture) binding's dimension + sample type from SPIR-V,
/// so [`Headless::draw_object_passthrough`] can stub each with a matching 1×1 texture.
pub fn parse_image_bindings(spv: &[u8]) -> Vec<ImageBinding> {
    use std::collections::{HashMap, HashSet};
    if spv.len() < 20 || spv_word(spv, 0) != 0x0723_0203 {
        return Vec::new();
    }
    let total = spv.len() / 4;
    let mut floats: HashSet<u32> = HashSet::new();
    let mut sints: HashSet<u32> = HashSet::new();
    let mut uints: HashSet<u32> = HashSet::new();
    let mut images: HashMap<u32, (u32, u32, u32)> = HashMap::new(); // id -> (sampled_ty, dim, arrayed)
    let mut sampled_images: HashMap<u32, u32> = HashMap::new(); // id -> image id
    let mut uc_ptr: HashMap<u32, u32> = HashMap::new(); // UniformConstant ptr -> pointee
    let mut var_ptr: HashMap<u32, u32> = HashMap::new(); // var -> result-type ptr
    let mut set_of: HashMap<u32, u32> = HashMap::new();
    let mut bind_of: HashMap<u32, u32> = HashMap::new();
    let mut i = 5;
    while i < total {
        let w0 = spv_word(spv, i);
        let wc = (w0 >> 16) as usize;
        let op = (w0 & 0xffff) as u16;
        if wc == 0 || i + wc > total {
            break;
        }
        match op {
            22 if wc >= 2 => {
                floats.insert(spv_word(spv, i + 1));
            }
            21 if wc >= 4 => {
                if spv_word(spv, i + 3) == 1 {
                    sints.insert(spv_word(spv, i + 1));
                } else {
                    uints.insert(spv_word(spv, i + 1));
                }
            }
            // OpTypeImage: result sampled_type dim depth arrayed ms sampled format
            25 if wc >= 9 => {
                images.insert(
                    spv_word(spv, i + 1),
                    (
                        spv_word(spv, i + 2),
                        spv_word(spv, i + 3),
                        spv_word(spv, i + 5),
                    ),
                );
            }
            27 if wc >= 3 => {
                sampled_images.insert(spv_word(spv, i + 1), spv_word(spv, i + 2));
            }
            32 if wc >= 4 => {
                if spv_word(spv, i + 2) == 0 {
                    uc_ptr.insert(spv_word(spv, i + 1), spv_word(spv, i + 3));
                }
            }
            59 if wc >= 4 => {
                if spv_word(spv, i + 3) == 0 {
                    var_ptr.insert(spv_word(spv, i + 2), spv_word(spv, i + 1));
                }
            }
            71 if wc >= 4 => {
                let t = spv_word(spv, i + 1);
                match spv_word(spv, i + 2) {
                    33 => {
                        bind_of.insert(t, spv_word(spv, i + 3));
                    }
                    34 => {
                        set_of.insert(t, spv_word(spv, i + 3));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        i += wc;
    }
    let mut out = Vec::new();
    for (&var, &ptype) in &var_ptr {
        let Some(&pointee) = uc_ptr.get(&ptype) else {
            continue;
        };
        let img = if images.contains_key(&pointee) {
            pointee
        } else if let Some(&im) = sampled_images.get(&pointee) {
            im
        } else {
            continue;
        };
        let Some(&(st, dim, arrayed)) = images.get(&img) else {
            continue;
        };
        let (s, b) = match (set_of.get(&var), bind_of.get(&var)) {
            (Some(&s), Some(&b)) => (s, b),
            _ => continue,
        };
        let view_dim = match (dim, arrayed) {
            (1, 0) => wgpu::TextureViewDimension::D2,
            (1, 1) => wgpu::TextureViewDimension::D2Array,
            (3, 0) => wgpu::TextureViewDimension::Cube,
            (3, 1) => wgpu::TextureViewDimension::CubeArray,
            (2, _) => wgpu::TextureViewDimension::D3,
            (0, _) => wgpu::TextureViewDimension::D1,
            _ => wgpu::TextureViewDimension::D2,
        };
        let sample_type = if uints.contains(&st) {
            wgpu::TextureSampleType::Uint
        } else if sints.contains(&st) {
            wgpu::TextureSampleType::Sint
        } else {
            let _ = &floats;
            wgpu::TextureSampleType::Float { filterable: true }
        };
        out.push(ImageBinding {
            set: s,
            binding: b,
            view_dim,
            sample_type,
        });
    }
    out
}

impl Headless {
    /// Initialise a headless device on any available backend (prefers real hardware).
    pub fn new() -> Result<Self, RenderError> {
        Self::with_options(false)
    }

    /// Initialise on a SOFTWARE adapter (lavapipe / llvmpipe) via
    /// `force_fallback_adapter`. A shader fault here is a CPU process error, NOT a
    /// hardware GPU reset — the safe way to execute a translated ER shader whose
    /// hardware draw deterministically faults the real GPU.
    pub fn new_software() -> Result<Self, RenderError> {
        Self::with_options(true)
    }

    fn with_options(force_software: bool) -> Result<Self, RenderError> {
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
                force_fallback_adapter: force_software,
                compatible_surface: None,
            })
            .block_on()
            .map_err(|e| RenderError::NoAdapter(e.to_string()))?;
        let info = adapter.get_info();
        let is_software = matches!(info.device_type, wgpu::DeviceType::Cpu)
            || info.name.to_lowercase().contains("llvmpipe")
            || info.name.to_lowercase().contains("lavapipe");
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
            adapter_name: info.name,
            is_software,
        })
    }

    /// The adapter's reported name (e.g. `llvmpipe (LLVM ...)` for lavapipe).
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// True when running on a CPU/software adapter (lavapipe/llvmpipe) — a draw cannot
    /// hard-reset the physical GPU.
    pub fn is_software(&self) -> bool {
        self.is_software
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
        // Bounded wait: a problematic passthrough shader must never wedge the process.
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(25)),
        });
        match rx.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(RenderError::Readback(e.to_string())),
            Err(_) => {
                return Err(RenderError::Readback(
                    "draw did not complete within timeout (device wedged on this shader)".into(),
                ));
            }
        }
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

    /// Draw a real object through the native vertex+pixel passthrough shaders: bind the
    /// supplied vertex/index buffers, stub every resource the shaders reference (uniform
    /// buffers zeroed except `uniform_writes`, storage buffers zeroed, textures a 1×1
    /// mid-grey, samplers default), draw indexed into an offscreen RGBA8 target, and read
    /// the pixels back. Returns `Pipeline` errors instead of panicking so callers can mark
    /// a shader. Used to prove a synthesized MVP makes the native vertex shader project
    /// real geometry on-screen (non-blank).
    pub fn draw_object_passthrough(&self, d: &ObjDrawDesc) -> Result<Vec<Rgba>, RenderError> {
        use std::collections::BTreeMap;
        if !self.passthrough {
            return Err(RenderError::NoAdapter(
                "SPIRV passthrough unsupported".into(),
            ));
        }
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);

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
        let vs = make(d.vertex_spirv, "obj-vs");
        // Fragment: native pixel SPIR-V (passthrough) or a WGSL override for VS isolation.
        let (frag_mod, pixel_entry): (wgpu::ShaderModule, &str) = match d.pixel_wgsl {
            Some(wgsl) => (
                self.device
                    .create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("obj-fs-wgsl"),
                        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
                    }),
                "fs_main",
            ),
            None => (make(d.pixel_spirv, "obj-fs"), d.pixel_entry),
        };

        let uniform_size = |set: u32, binding: u32| -> u64 {
            d.uniform_sizes
                .iter()
                .find(|(s, b, _)| *s == set && *b == binding)
                .map(|&(_, _, sz)| sz.max(256))
                .unwrap_or(65536)
        };

        let mut by_set: BTreeMap<u32, Vec<(u32, ObjBind)>> = BTreeMap::new();
        for &(set, binding, kind) in d.bindings {
            by_set.entry(set).or_default().push((binding, kind));
        }
        // Reflect each texture binding's dimension + sample type so stubs MATCH; a 1×1 2D
        // stub bound where the shader samples a cube/3D texture segfaults the rasteriser.
        let mut img_map: std::collections::HashMap<(u32, u32), ImageBinding> =
            std::collections::HashMap::new();
        for ib in parse_image_bindings(d.vertex_spirv)
            .into_iter()
            .chain(parse_image_bindings(d.pixel_spirv))
        {
            img_map.insert((ib.set, ib.binding), ib);
        }
        let max_set = by_set.keys().copied().max().unwrap_or(0);

        // One bind group per set index (0..=max_set); sets with no bindings get an empty
        // group so the layout array is contiguous (wgpu indexes by group number).
        let mut bgls: Vec<wgpu::BindGroupLayout> = Vec::new();
        let mut bind_groups: Vec<wgpu::BindGroup> = Vec::new();
        // Stub resources MUST outlive the draw: keep every buffer/view/sampler alive
        // until after submit, or the descriptors dangle (lavapipe segfaults on a freed
        // backing store).
        let mut keep_buffers: Vec<wgpu::Buffer> = Vec::new();
        let mut keep_views: Vec<wgpu::TextureView> = Vec::new();
        let mut keep_samplers: Vec<wgpu::Sampler> = Vec::new();
        let vis = wgpu::ShaderStages::VERTEX_FRAGMENT;
        for set in 0..=max_set {
            let binds = by_set.get(&set).cloned().unwrap_or_default();
            let mut layout_entries: Vec<wgpu::BindGroupLayoutEntry> = Vec::new();
            let mut local_buffers: Vec<wgpu::Buffer> = Vec::new();
            let mut local_views: Vec<wgpu::TextureView> = Vec::new();
            let mut local_samplers: Vec<wgpu::Sampler> = Vec::new();
            enum R {
                Buf(usize),
                Tex(usize),
                Smp(usize),
            }
            let mut res: Vec<(u32, R)> = Vec::new();

            for (binding, kind) in binds {
                match kind {
                    ObjBind::Uniform | ObjBind::Storage => {
                        let storage = matches!(kind, ObjBind::Storage);
                        // FRAME-REPLAY: seed from the captured buffer contents when provided,
                        // else a zeroed stub. uniform_writes (matrices) apply on top.
                        let captured = d
                            .buffer_data
                            .iter()
                            .find(|(s, b, _)| *s == set && *b == binding)
                            .map(|(_, _, data)| *data);
                        let base = match captured {
                            Some(data) => (data.len() as u64).max(uniform_size(set, binding)),
                            None => uniform_size(set, binding),
                        };
                        let sz = (base + 15) & !15; // 16-byte aligned
                        let mut host = vec![0u8; sz as usize];
                        if let Some(data) = captured {
                            let n = data.len().min(host.len());
                            host[..n].copy_from_slice(&data[..n]);
                        }
                        for w in d.uniform_writes {
                            if w.set == set && w.binding == binding {
                                let o = w.offset as usize;
                                let end = (o + w.bytes.len()).min(host.len());
                                if o < host.len() {
                                    host[o..end].copy_from_slice(&w.bytes[..end - o]);
                                }
                            }
                        }
                        let usage = if storage {
                            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST
                        } else {
                            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST
                        };
                        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                            label: None,
                            size: sz,
                            usage,
                            mapped_at_creation: false,
                        });
                        self.queue.write_buffer(&buf, 0, &host);
                        layout_entries.push(wgpu::BindGroupLayoutEntry {
                            binding,
                            visibility: vis,
                            ty: wgpu::BindingType::Buffer {
                                ty: if storage {
                                    wgpu::BufferBindingType::Storage { read_only: true }
                                } else {
                                    wgpu::BufferBindingType::Uniform
                                },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        });
                        local_buffers.push(buf);
                        res.push((binding, R::Buf(local_buffers.len() - 1)));
                    }
                    ObjBind::Texture => {
                        // Layout must match the shader's declared dimension/sample type.
                        let info = img_map.get(&(set, binding)).copied();
                        let view_dim = info
                            .map(|i| i.view_dim)
                            .unwrap_or(wgpu::TextureViewDimension::D2);
                        let sample_type = info
                            .map(|i| i.sample_type)
                            .unwrap_or(wgpu::TextureSampleType::Float { filterable: true });
                        // FRAME-REPLAY: real captured texture (IBL/GI/material) if provided.
                        let real = d
                            .textures
                            .iter()
                            .find(|t| t.set == set && t.binding == binding);
                        let (width, height, layers, tex_dim, format) = match real {
                            Some(rt) => {
                                let td = match rt.dim {
                                    wgpu::TextureViewDimension::D3 => wgpu::TextureDimension::D3,
                                    wgpu::TextureViewDimension::D1 => wgpu::TextureDimension::D1,
                                    _ => wgpu::TextureDimension::D2,
                                };
                                (
                                    rt.width.max(1),
                                    rt.height.max(1),
                                    rt.depth_or_layers.max(1),
                                    td,
                                    rt.format,
                                )
                            }
                            None => {
                                let (td, layers) = match view_dim {
                                    wgpu::TextureViewDimension::D3 => {
                                        (wgpu::TextureDimension::D3, 1)
                                    }
                                    wgpu::TextureViewDimension::Cube
                                    | wgpu::TextureViewDimension::CubeArray => {
                                        (wgpu::TextureDimension::D2, 6)
                                    }
                                    wgpu::TextureViewDimension::D1 => {
                                        (wgpu::TextureDimension::D1, 1)
                                    }
                                    _ => (wgpu::TextureDimension::D2, 1),
                                };
                                let fmt = match sample_type {
                                    wgpu::TextureSampleType::Uint => wgpu::TextureFormat::Rgba8Uint,
                                    wgpu::TextureSampleType::Sint => wgpu::TextureFormat::Rgba8Sint,
                                    _ => wgpu::TextureFormat::Rgba8Unorm,
                                };
                                (1, 1, layers, td, fmt)
                            }
                        };
                        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                            label: None,
                            size: wgpu::Extent3d {
                                width,
                                height,
                                depth_or_array_layers: layers,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: tex_dim,
                            format,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING
                                | wgpu::TextureUsages::COPY_DST,
                            view_formats: &[],
                        });
                        // Uncompressed bytes-per-texel (extract decodes BCn → rgba).
                        let bpp = format.block_copy_size(None).unwrap_or(4);
                        let expected = (width * height * layers * bpp) as usize;
                        let mut upload: Vec<u8> = match real {
                            Some(rt) => rt.data.to_vec(),
                            None => (0..layers).flat_map(|_| [128u8, 128, 128, 255]).collect(),
                        };
                        if upload.len() < expected {
                            upload.resize(expected, 0);
                        }
                        self.queue.write_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: &tex,
                                mip_level: 0,
                                origin: wgpu::Origin3d::ZERO,
                                aspect: wgpu::TextureAspect::All,
                            },
                            &upload,
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(width * bpp),
                                rows_per_image: Some(height),
                            },
                            wgpu::Extent3d {
                                width,
                                height,
                                depth_or_array_layers: layers,
                            },
                        );
                        layout_entries.push(wgpu::BindGroupLayoutEntry {
                            binding,
                            visibility: vis,
                            ty: wgpu::BindingType::Texture {
                                sample_type,
                                view_dimension: view_dim,
                                multisampled: false,
                            },
                            count: None,
                        });
                        local_views.push(tex.create_view(&wgpu::TextureViewDescriptor {
                            dimension: Some(view_dim),
                            ..Default::default()
                        }));
                        res.push((binding, R::Tex(local_views.len() - 1)));
                    }
                    ObjBind::Sampler => {
                        layout_entries.push(wgpu::BindGroupLayoutEntry {
                            binding,
                            visibility: vis,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        });
                        local_samplers.push(
                            self.device
                                .create_sampler(&wgpu::SamplerDescriptor::default()),
                        );
                        res.push((binding, R::Smp(local_samplers.len() - 1)));
                    }
                }
            }

            let bgl = self
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("obj-draw-bgl"),
                    entries: &layout_entries,
                });
            let entries: Vec<wgpu::BindGroupEntry> = res
                .iter()
                .map(|(binding, r)| wgpu::BindGroupEntry {
                    binding: *binding,
                    resource: match r {
                        R::Buf(i) => local_buffers[*i].as_entire_binding(),
                        R::Tex(i) => wgpu::BindingResource::TextureView(&local_views[*i]),
                        R::Smp(i) => wgpu::BindingResource::Sampler(&local_samplers[*i]),
                    },
                })
                .collect();
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("obj-draw-bg"),
                layout: &bgl,
                entries: &entries,
            });
            bgls.push(bgl);
            bind_groups.push(bg);
            // Move the stub resources into the function-scoped keep-alive vecs.
            keep_buffers.append(&mut local_buffers);
            keep_views.append(&mut local_views);
            keep_samplers.append(&mut local_samplers);
        }
        let bgl_refs: Vec<Option<&wgpu::BindGroupLayout>> = bgls.iter().map(Some).collect();
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("obj-draw-pl"),
                bind_group_layouts: &bgl_refs,
                immediate_size: 0,
            });

        // Vertex buffer layouts (one per FLVER buffer).
        let attr_storage: Vec<Vec<wgpu::VertexAttribute>> = d
            .vertex_buffers
            .iter()
            .map(|vb| {
                vb.attributes
                    .iter()
                    .map(|&(loc, format, offset)| wgpu::VertexAttribute {
                        format,
                        offset,
                        shader_location: loc,
                    })
                    .collect()
            })
            .collect();
        let vbls: Vec<wgpu::VertexBufferLayout> = d
            .vertex_buffers
            .iter()
            .zip(&attr_storage)
            .map(|(vb, attrs)| wgpu::VertexBufferLayout {
                array_stride: vb.stride.max(4),
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: attrs,
            })
            .collect();

        let targets: Vec<Option<wgpu::ColorTargetState>> = (0..d.color_targets.max(1))
            .map(|_| {
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })
            })
            .collect();

        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("obj-draw-pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &vs,
                    entry_point: Some(d.vertex_entry),
                    compilation_options: Default::default(),
                    buffers: &vbls,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &frag_mod,
                    entry_point: Some(pixel_entry),
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
        // Surface any creation/binding validation error NOW (before the draw executes),
        // so a bad descriptor is reported instead of segfaulting the software driver.
        if let Some(err) = scope.pop().block_on() {
            return Err(RenderError::Pipeline(format!("setup: {err}")));
        }
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);

        // Upload geometry.
        let gpu_vbos: Vec<wgpu::Buffer> = d
            .vertex_buffers
            .iter()
            .map(|vb| {
                let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("obj-vbo"),
                    size: vb.data.len().max(4) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.queue.write_buffer(&buf, 0, vb.data);
                buf
            })
            .collect();
        let ibo = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("obj-ibo"),
            size: (d.indices.len().max(1) * 4) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&ibo, 0, bytemuck::cast_slice(d.indices));

        // Offscreen target + readback.
        let size = d.size;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("obj-draw-target"),
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
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        // Extra targets for shaders with multiple outputs (only target 0 is read back).
        let extra: Vec<wgpu::Texture> = (1..d.color_targets.max(1))
            .map(|_| {
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
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
            })
            .collect();
        let extra_views: Vec<wgpu::TextureView> = extra
            .iter()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();
        let mut attachments: Vec<Option<wgpu::RenderPassColorAttachment>> =
            vec![Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })];
        for v in &extra_views {
            attachments.push(Some(wgpu::RenderPassColorAttachment {
                view: v,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            }));
        }

        let bpp = 4u32;
        let padded = (size * bpp).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
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
                label: Some("obj-draw-pass"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline);
            for (i, bg) in bind_groups.iter().enumerate() {
                pass.set_bind_group(i as u32, bg, &[]);
            }
            for (slot, buf) in gpu_vbos.iter().enumerate() {
                pass.set_vertex_buffer(slot as u32, buf.slice(..));
            }
            pass.set_index_buffer(ibo.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..d.indices.len() as u32, 0, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
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
        // Bounded wait: a problematic passthrough shader must never wedge the process.
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(25)),
        });
        match rx.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(RenderError::Readback(e.to_string())),
            Err(_) => {
                return Err(RenderError::Readback(
                    "draw did not complete within timeout (device wedged on this shader)".into(),
                ));
            }
        }
        let data = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity((size * size) as usize);
        for row in 0..size {
            let start = (row * padded) as usize;
            for col in 0..size {
                let p = start + (col * bpp) as usize;
                pixels.push([data[p], data[p + 1], data[p + 2], data[p + 3]]);
            }
        }
        Ok(pixels)
    }
}
