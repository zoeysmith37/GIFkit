//! Frames rendered on the graphics card (wgpu). The CPU path in `stack.rs`
//! stays the reference and the fallback: a stack only comes here when every
//! layer in it can be drawn here (see `Stack::gpu_frame`), otherwise the
//! whole frame renders on the CPU, so frames never bounce between the two.

use crate::geo::Affine;
use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;
use image::RgbaImage;
use std::collections::HashMap;

/// The frame geometry in one pass: see `Geo` in the shader.
#[derive(Clone, Copy, Debug)]
pub struct GpuGeo {
    /// output pixels -> source frame pixels
    pub inv: Affine,
    /// the part of the source frame that's shown, x0 y0 x1 y1
    pub crop: [f32; 4],
    /// output pixels -> the canvas before a free turn, and its size
    pub inv_canvas: Affine,
    pub canvas: (f32, f32),
    /// inside the padding, x0 y0 x1 y1
    pub inner: [f32; 4],
    pub pad_bg: [u8; 4],
    pub rot_bg: [u8; 4],
    pub nearest: bool,
    /// a size change, done first as the CPU does it (see `GpuResize`)
    pub resize: Option<GpuResize>,
    /// a gap on the GIF track: the frame shows nothing
    pub blank: bool,
}

/// Resizing the shown part of the frame (`crop`) to `size`, with one of the
/// CPU's filters: 0 nearest, 1 triangle, 2 Catmull-Rom, 3 Lanczos.
#[derive(Clone, Copy, Debug)]
pub struct GpuResize {
    pub size: (u32, u32),
    pub kernel: u32,
}

/// What a layer draws: a picture, or a solid colour over a rectangle.
#[derive(Clone, Copy)]
pub enum GpuSrc<'a> {
    #[allow(dead_code)]
    Img(&'a RgbaImage),
    Solid([u8; 4], (f32, f32)),
}

/// One thing drawn over the frame: `m` maps its pixels to output pixels.
#[derive(Clone, Copy)]
pub struct GpuDraw<'a> {
    pub src: GpuSrc<'a>,
    pub m: Affine,
    pub opacity: f32,
}

/// Where a pass reads a picture from.
#[derive(Clone, Copy)]
pub enum GpuIn<'a> {
    /// the frame as it is so far (straight alpha)
    Cur,
    /// a scratch picture an earlier pass made
    Tmp(u8),
    #[allow(dead_code)]
    Img(&'a RgbaImage),
    /// one of `GpuFrame::owned`
    Owned(usize),
    /// per-pixel jitter the CPU makes with `sin`, for this pass's size:
    /// 0 fuzzy frame, 1 colour pencil
    Noise(u8),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PassKind {
    /// `fs_fx` in gpu_fx.wgsl, `u[0]` = op, variant, output size
    Fx,
    /// the CPU's resize, one direction at a time (see `Rs` in the shader)
    ResizeV,
    ResizeH,
    /// Pop Art's brightness cut points, into the shared buffer
    Hist,
}

/// One full-frame pass over straight-alpha pixels.
pub struct GpuPass<'a> {
    pub kind: PassKind,
    pub size: (u32, u32),
    /// None: the result becomes the frame
    pub out: Option<u8>,
    /// keep unrounded floats (between the halves of a blur or resize)
    pub float: bool,
    pub ins: [Option<GpuIn<'a>>; 3],
    pub u: Vec<[f32; 4]>,
}

/// In layer order: things drawn over the frame, and whole-frame work
/// (an effects layer, a censor, the shield) mixed back by `mix`.
pub enum GpuOp<'a> {
    Draw(GpuDraw<'a>),
    Effect { passes: Vec<GpuPass<'a>>, mix: f32 },
    /// a clipping base's coverage: these draws alone, kept as mask `slot`
    Mask { draws: Vec<GpuDraw<'a>>, slot: usize },
    /// a clipped layer starts: the frame as it is now is kept
    ClipBegin,
    /// before a clipping base draws: the frame under it, kept as `slot`
    Under { slot: usize },
    /// an animation clipped to base `slot`: the base is lifted off what's
    /// under it, animated alone and put back
    AnimClip { slot: usize, passes: Vec<GpuPass<'a>>, mix: f32 },
    /// and ends: its result shows only where mask `slot` is
    ClipEnd { slot: usize },
}

/// Everything needed to make one output frame.
pub struct GpuFrame<'a> {
    /// after the frame edits, before any layer
    pub size: (u32, u32),
    pub geo: GpuGeo,
    pub ops: Vec<GpuOp<'a>>,
    /// pictures made for this frame only (counter and REC text)
    pub owned: Vec<RgbaImage>,
}

impl GpuFrame<'_> {
    /// Can a card whose pictures go up to `max` pixels a side make this
    /// frame from `src`? Anything bigger (a huge text sprite, say) would be
    /// refused by the card, so those frames are made on the CPU instead.
    pub fn fits(&self, src: &RgbaImage, max: u32) -> bool {
        let ok = |w: u32, h: u32| w.max(h) <= max;
        let img_ok = |i: &RgbaImage| ok(i.width(), i.height());
        if !ok(self.size.0, self.size.1) || !img_ok(src) || self.geo.resize.is_some_and(|r| !ok(r.size.0, r.size.1)) || !self.owned.iter().all(img_ok) {
            return false;
        }
        let draw_ok = |d: &GpuDraw| match d.src {
            GpuSrc::Img(i) => img_ok(i),
            GpuSrc::Solid(..) => true,
        };
        self.ops.iter().all(|op| match op {
            GpuOp::Draw(d) => draw_ok(d),
            GpuOp::Mask { draws, .. } => draws.iter().all(draw_ok),
            GpuOp::ClipBegin | GpuOp::ClipEnd { .. } | GpuOp::Under { .. } => true,
            GpuOp::AnimClip { passes, .. } => passes.iter().all(|p| ok(p.size.0, p.size.1) && p.ins.iter().all(|i| !matches!(i, Some(GpuIn::Img(i)) if !img_ok(i)))),
            GpuOp::Effect { passes, .. } => passes.iter().all(|p| ok(p.size.0, p.size.1) && p.ins.iter().all(|i| !matches!(i, Some(GpuIn::Img(i)) if !img_ok(i)))),
        })
    }
}

/// The CPU's `sin`-hash jitter (effects::fuzzy, booth's pencil), exactly.
fn noise_values(kind: u8, w: u32, h: u32) -> Vec<f32> {
    let f = (w.min(h) as f32 * 0.08).max(2.0);
    let mut v = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            v.push(if kind == 0 {
                let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
                (((x * 12.9898 + y * 78.233).sin() * 43758.547).fract() - 0.5) * f * 0.6
            } else {
                let (x, y) = (x as f32, y as f32);
                ((x * 12.9898 + y * 78.233).sin() * 43758.545).fract() * 0.8
            });
        }
    }
    v
}

const HIST_BYTES: u64 = (4 + 255_001) * 4;

/// Textures by key, kept within a memory budget (oldest use goes first).
struct TexCache {
    map: HashMap<u64, (wgpu::Texture, wgpu::TextureView, u64, usize)>,
    bytes: usize,
    budget: usize,
    tick: u64,
}

impl TexCache {
    fn new(budget: usize) -> Self {
        TexCache { map: HashMap::new(), bytes: 0, budget, tick: 0 }
    }

    fn get_or_upload(&mut self, dev: &wgpu::Device, queue: &wgpu::Queue, key: u64, img: &RgbaImage) -> wgpu::TextureView {
        self.tick += 1;
        if let Some(e) = self.map.get_mut(&key) {
            e.2 = self.tick;
            return e.1.clone();
        }
        let size = img.width() as usize * img.height() as usize * 4;
        let budget = crate::pixels::scaled(self.budget);
        while self.bytes + size > budget && !self.map.is_empty() {
            let oldest = *self.map.iter().min_by_key(|(_, e)| e.2).map(|(k, _)| k).unwrap();
            if let Some(e) = self.map.remove(&oldest) {
                self.bytes -= e.3;
                e.0.destroy();
            }
        }
        let tex = upload(dev, queue, img, "gifkit-src");
        let view = tex.create_view(&Default::default());
        self.map.insert(key, (tex, view.clone(), self.tick, size));
        self.bytes += size;
        view
    }

    fn clear(&mut self) {
        for (_, e) in self.map.drain() {
            e.0.destroy();
        }
        self.bytes = 0;
    }
}

fn upload(dev: &wgpu::Device, queue: &wgpu::Queue, img: &RgbaImage, label: &str) -> wgpu::Texture {
    let size = wgpu::Extent3d { width: img.width().max(1), height: img.height().max(1), depth_or_array_layers: 1 };
    let tex = dev.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        img.as_raw(),
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * img.width()), rows_per_image: Some(img.height()) },
        size,
    );
    tex
}

fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn rgba(c: [u8; 4]) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
}

/// Source pixels under one output pixel, per axis, for a map out -> src.
fn footprint(inv: &Affine) -> (f32, f32) {
    ((inv.a * inv.a + inv.c * inv.c).sqrt(), (inv.b * inv.b + inv.d * inv.d).sqrt())
}

pub struct GpuRenderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    hist_layout: wgpu::BindGroupLayout,
    geo: wgpu::RenderPipeline,
    quad: wgpu::RenderPipeline,
    resize_v: wgpu::RenderPipeline,
    resize_h: wgpu::RenderPipeline,
    fx: wgpu::RenderPipeline,
    fx_f32: wgpu::RenderPipeline,
    hist_count: wgpu::ComputePipeline,
    hist_cuts: wgpu::ComputePipeline,
    hist: wgpu::Buffer,
    blank: wgpu::TextureView,
    sources: TexCache,
    sprites: TexCache,
    noise: HashMap<(u8, u32, u32), wgpu::TextureView>,
    pub max_side: u32,
    /// frames sent to the card and not finished yet
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// GIF colour mapping, made the first time it's needed
    quant: Option<wgpu::ComputePipeline>,
}

/// A GIF palette as the card searches it (see gpu_quant.wgsl).
pub struct QuantTable {
    /// 0 NeuQuant's search, 1 nearest by RGB
    pub mode: u32,
    /// RGBA entries, as NeuQuant keeps them (sorted by green in mode 0)
    pub entries: Vec<[i32; 4]>,
    pub netindex: [u32; 256],
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None,
    }
}

fn uniform_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 1,
        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

/// Uniforms for a pass: always the 24 vectors the effect shader declares.
fn uniform_block(u: &[[f32; 4]]) -> Vec<f32> {
    let mut v: Vec<f32> = u.iter().take(24).flatten().copied().collect();
    v.resize(24 * 4, 0.0);
    v
}

impl GpuRenderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("gifkit-frames"), source: wgpu::ShaderSource::Wgsl(include_str!("gpu_render.wgsl").into()) });
        let fx_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("gifkit-effects"), source: wgpu::ShaderSource::Wgsl(include_str!("gpu_fx.wgsl").into()) });
        let hist_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("gifkit-hist"), source: wgpu::ShaderSource::Wgsl(include_str!("gpu_hist.wgsl").into()) });
        let storage = |read_only: bool, vis: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: vis,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gifkit-frames"),
            entries: &[tex_entry(0), uniform_entry(), tex_entry(2), tex_entry(3), storage(true, wgpu::ShaderStages::FRAGMENT)],
        });
        let hist_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("gifkit-hist"), entries: &[tex_entry(0), uniform_entry(), storage(false, wgpu::ShaderStages::COMPUTE)] });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("gifkit-frames"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let hist_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("gifkit-hist"), bind_group_layouts: &[Some(&hist_layout)], immediate_size: 0 });
        let over = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add };
        let pipe = |module: &wgpu::ShaderModule, vs: &str, fs: &str, blend: Option<wgpu::BlendState>, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module, entry_point: Some(vs), buffers: &[], compilation_options: Default::default() },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState { module, entry_point: Some(fs), targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
                multiview_mask: None,
                cache: None,
            })
        };
        let compute = |entry: &str| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(entry), layout: Some(&hist_pl), module: &hist_module, entry_point: Some(entry), compilation_options: Default::default(), cache: None });
        let u8f = wgpu::TextureFormat::Rgba8Unorm;
        let f32f = wgpu::TextureFormat::Rgba32Float;
        let geo = pipe(&module, "vs_full", "fs_geo", None, u8f);
        let quad = pipe(&module, "vs_quad", "fs_quad", Some(wgpu::BlendState { color: over, alpha: over }), u8f);
        let resize_v = pipe(&module, "vs_full", "fs_resize_v", None, f32f);
        let resize_h = pipe(&module, "vs_full", "fs_resize_h", None, u8f);
        let fx = pipe(&fx_module, "vs_full", "fs_fx", None, u8f);
        let fx_f32 = pipe(&fx_module, "vs_full", "fs_fx", None, f32f);
        let hist_count = compute("cs_count");
        let hist_cuts = compute("cs_cuts");
        let hist = device.create_buffer(&wgpu::BufferDescriptor { label: Some("gifkit-hist"), size: HIST_BYTES, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let blank = upload(&device, &queue, &RgbaImage::new(1, 1), "gifkit-blank").create_view(&Default::default());
        let max_side = device.limits().max_texture_dimension_2d;
        // a quarter of what's typical for the card, capped: frames come back
        // from the CPU copy whenever they've gone
        GpuRenderer {
            device,
            queue,
            layout,
            hist_layout,
            geo,
            quad,
            resize_v,
            resize_h,
            fx,
            fx_f32,
            hist_count,
            hist_cuts,
            hist,
            blank,
            sources: TexCache::new(64 << 20),
            sprites: TexCache::new(256 << 20),
            noise: HashMap::new(),
            max_side,
            pending: Default::default(),
            quant: None,
        }
    }

    /// A device of its own, for tests and for machines where the window
    /// draws with OpenGL.
    pub fn headless_named() -> Option<(Self, String)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() })).ok()?;
        let info = adapter.get_info();
        if !worth_using(&info) {
            return None;
        }
        let name = info.name;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        Some((Self::new(device, queue), name))
    }

    #[allow(dead_code)]
    pub fn headless() -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        Some(Self::new(device, queue))
    }

    /// Forget cached sprites (the layers changed).
    pub fn clear_sprites(&mut self) {
        self.sprites.clear();
    }

    /// Forget cached source frames (the frames changed).
    pub fn clear_sources(&mut self) {
        self.sources.clear();
    }

    fn target(&self, w: u32, h: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gifkit-pass"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    /// One full-screen pass of `pipe` into `out`.
    fn pass(&self, enc: &mut wgpu::CommandEncoder, out: &wgpu::TextureView, pipe: &wgpu::RenderPipeline, bind: &wgpu::BindGroup) {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gifkit-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: out, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }

    fn bind(&self, view: &wgpu::TextureView, uniforms: &[f32]) -> wgpu::BindGroup {
        self.bind3([view, &self.blank, &self.blank], uniforms)
    }

    fn bind3(&self, views: [&wgpu::TextureView; 3], uniforms: &[f32]) -> wgpu::BindGroup {
        let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: &floats(uniforms), usage: wgpu::BufferUsages::UNIFORM });
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(views[0]) },
                wgpu::BindGroupEntry { binding: 1, resource: buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(views[1]) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(views[2]) },
                wgpu::BindGroupEntry { binding: 4, resource: self.hist.as_entire_binding() },
            ],
        })
    }

    /// The jitter picture for `kind` at a size, made once.
    fn noise_view(&mut self, kind: u8, w: u32, h: u32) -> wgpu::TextureView {
        if let Some(v) = self.noise.get(&(kind, w, h)) {
            return v.clone();
        }
        if self.noise.len() > 8 {
            self.noise.clear();
        }
        let size = wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 };
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gifkit-noise"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let data = floats(&noise_values(kind, w.max(1), h.max(1)));
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w.max(1)), rows_per_image: Some(h.max(1)) },
            size,
        );
        let v = tex.create_view(&Default::default());
        self.noise.insert((kind, w, h), v.clone());
        v
    }

    fn sprite_view(&mut self, img: &RgbaImage, sprite_gen: u64) -> wgpu::TextureView {
        self.sprites.get_or_upload(&self.device, &self.queue, (img as *const RgbaImage as u64) ^ sprite_gen.rotate_left(40), img)
    }

    /// Draw `quads` over `target` (kept, not cleared).
    /// Palette indices for the `w`×`h` region at (x0, y0) of `img`, the way
    /// the GIF encoder's CPU path picks them; `ordered` adds its Bayer dither.
    #[allow(clippy::too_many_arguments)]
    pub fn map_palette(&mut self, img: &RgbaImage, x0: u32, y0: u32, w: u32, h: u32, t: &QuantTable, ordered: bool) -> Option<Vec<u8>> {
        if w == 0 || h == 0 || img.width().max(img.height()) > self.max_side || t.entries.is_empty() {
            return None;
        }
        if self.quant.is_none() {
            let m = self.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("gifkit-quant"), source: wgpu::ShaderSource::Wgsl(include_str!("gpu_quant.wgsl").into()) });
            self.quant = Some(self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("gifkit-quant"), layout: None, module: &m, entry_point: Some("cs_map"), compilation_options: Default::default(), cache: None }));
        }
        let pipe = self.quant.as_ref()?;
        let src = upload(&self.device, &self.queue, img, "gifkit-quant-src").create_view(&Default::default());
        let total = w as u64 * h as u64;
        let words = total.div_ceil(4);
        let groups = words.div_ceil(64);
        let gx = groups.min(65535) as u32;
        let gy = groups.div_ceil(gx as u64) as u32;
        if gy > 65535 {
            return None;
        }
        let stride = gx * 64;
        let mut uni: Vec<u8> = Vec::new();
        for v in [w, h, x0, y0, t.mode, t.entries.len().min(256) as u32, u32::from(ordered), stride] {
            uni.extend_from_slice(&v.to_le_bytes());
        }
        uni.extend_from_slice(&32.0f32.to_le_bytes());
        uni.resize(48, 0);
        let mut pal: Vec<u8> = Vec::with_capacity(256 * 16);
        for k in 0..256 {
            let e = t.entries.get(k).copied().unwrap_or([0; 4]);
            for c in e {
                pal.extend_from_slice(&c.to_le_bytes());
            }
        }
        let net: Vec<u8> = t.netindex.iter().flat_map(|v| v.to_le_bytes()).collect();
        use wgpu::util::DeviceExt;
        let ub = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: &uni, usage: wgpu::BufferUsages::UNIFORM });
        let pb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: &pal, usage: wgpu::BufferUsages::STORAGE });
        let nb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: &net, usage: wgpu::BufferUsages::STORAGE });
        let size = words * 4;
        let ob = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("gifkit-quant-out"), size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let rb = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("gifkit-quant-read"), size, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipe.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src) },
                wgpu::BindGroupEntry { binding: 1, resource: ub.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: pb.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: nb.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: ob.as_entire_binding() },
            ],
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("gifkit-quant"), timestamp_writes: None });
            cp.set_pipeline(pipe);
            cp.set_bind_group(0, &bg, &[]);
            cp.dispatch_workgroups(gx, gy, 1);
        }
        enc.copy_buffer_to_buffer(&ob, 0, &rb, 0, size);
        self.queue.submit([enc.finish()]);
        rb.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).ok()?;
        let data = rb.slice(..).get_mapped_range().ok()?;
        let out = data[..total as usize].to_vec();
        drop(data);
        MAPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        rb.unmap();
        Some(out)
    }

    /// Shrink pictures to `w`×`h` with the CPU's triangle filter
    /// (imageops::resize), several at a time.
    pub fn resize_triangle(&mut self, imgs: &[&RgbaImage], w: u32, h: u32) -> Option<Vec<RgbaImage>> {
        if w == 0 || h == 0 || imgs.iter().any(|i| i.width().max(i.height()) > self.max_side || i.width() == 0 || i.height() == 0) {
            return None;
        }
        let axis = |a: u32, b: u32| {
            let ratio = a as f32 / b as f32;
            let sratio = ratio.max(1.0);
            [ratio, sratio, sratio, 1.0]
        };
        let mut enc = self.device.create_command_encoder(&Default::default());
        let mut outs = Vec::with_capacity(imgs.len());
        for img in imgs {
            let (fw, fh) = img.dimensions();
            let src = upload(&self.device, &self.queue, img, "gifkit-shrink").create_view(&Default::default());
            let mid = self.target(fw, h, wgpu::TextureFormat::Rgba32Float);
            let mv = mid.create_view(&Default::default());
            let b = self.bind3([&src, &self.blank, &self.blank], &uniform_block(&[axis(fh, h), [0.0, fh as f32, 0.0, 1.0]]));
            self.pass(&mut enc, &mv, &self.resize_v, &b);
            let out = self.target(w, h, wgpu::TextureFormat::Rgba8Unorm);
            let ov = out.create_view(&Default::default());
            let b = self.bind3([&mv, &self.blank, &self.blank], &uniform_block(&[axis(fw, w), [0.0, fw as f32, 0.0, 0.0]]));
            self.pass(&mut enc, &ov, &self.resize_h, &b);
            outs.push(out);
        }
        self.queue.submit([enc.finish()]);
        Some(self.read_raw(&outs))
    }

    /// An effect's passes, from the straight picture `start`; the result, straight.
    fn run_passes(&mut self, enc: &mut wgpu::CommandEncoder, start: (wgpu::Texture, wgpu::TextureView, (u32, u32)), passes: &[GpuPass], f: &GpuFrame, sprite_gen: u64) -> (wgpu::Texture, wgpu::TextureView, (u32, u32)) {
        let (mut s_tex, mut s_view, mut s_size) = start;
        let u8f = wgpu::TextureFormat::Rgba8Unorm;
        let mut enc = enc;
        let mut tmps: HashMap<u8, wgpu::TextureView> = HashMap::new();
        for p in passes {
            let mut views: Vec<wgpu::TextureView> = Vec::with_capacity(3);
            for i in p.ins {
                views.push(match i {
                    None => self.blank.clone(),
                    Some(GpuIn::Cur) => s_view.clone(),
                    Some(GpuIn::Tmp(k)) => tmps.get(&k).cloned().unwrap_or_else(|| self.blank.clone()),
                    Some(GpuIn::Img(img)) => self.sprite_view(img, sprite_gen),
                    Some(GpuIn::Owned(k)) => match f.owned.get(k) {
                        Some(img) => upload(&self.device, &self.queue, img, "gifkit-owned").create_view(&Default::default()),
                        None => self.blank.clone(),
                    },
                    Some(GpuIn::Noise(k)) => self.noise_view(k, p.size.0, p.size.1),
                });
            }
            let vs = [&views[0], &views[1], &views[2]];
            if p.kind == PassKind::Hist {
                enc.clear_buffer(&self.hist, 0, None);
                let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: &floats(&uniform_block(&p.u)), usage: wgpu::BufferUsages::UNIFORM });
                let b = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.hist_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(vs[0]) },
                        wgpu::BindGroupEntry { binding: 1, resource: buf.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 4, resource: self.hist.as_entire_binding() },
                    ],
                });
                let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("gifkit-hist"), timestamp_writes: None });
                cp.set_bind_group(0, &b, &[]);
                cp.set_pipeline(&self.hist_count);
                cp.dispatch_workgroups(p.size.0.div_ceil(16), p.size.1.div_ceil(16), 1);
                cp.set_pipeline(&self.hist_cuts);
                cp.dispatch_workgroups(1, 1, 1);
                continue;
            }
            let format = if p.float { wgpu::TextureFormat::Rgba32Float } else { u8f };
            let t = self.target(p.size.0, p.size.1, format);
            let v = t.create_view(&Default::default());
            let pipe = match (p.kind, p.float) {
                (PassKind::ResizeV, _) => &self.resize_v,
                (PassKind::ResizeH, _) => &self.resize_h,
                (_, true) => &self.fx_f32,
                _ => &self.fx,
            };
            let b = self.bind3(vs, &uniform_block(&p.u));
            self.pass(&mut enc, &v, pipe, &b);
            match p.out {
                None => {
                    s_tex = t;
                    s_view = v;
                    s_size = p.size;
                }
                Some(k) => {
                    tmps.insert(k, v);
                }
            }
        }
        (s_tex, s_view, s_size)
    }

    /// The bind group that draws `d` onto a frame of `size`.
    fn quad(&mut self, d: &GpuDraw, size: (u32, u32), sprite_gen: u64) -> wgpu::BindGroup {
        let inv = d.m.inverse();
        let foot = footprint(&inv);
        let (view, sz, col, solid) = match d.src {
            GpuSrc::Img(img) => (self.sprite_view(img, sprite_gen), (img.width() as f32, img.height() as f32), [0.0; 4], 0.0),
            GpuSrc::Solid(c, s) => (self.blank.clone(), s, rgba(c), 1.0),
        };
        let (ow, oh) = (size.0 as f32, size.1 as f32);
        self.bind(&view, &uniform_block(&[[d.m.a, d.m.b, d.m.c, d.m.d], [d.m.tx, d.m.ty, sz.0, sz.1], [inv.a, inv.b, inv.c, inv.d], [inv.tx, inv.ty, ow, oh], col, [d.opacity, solid, foot.0, foot.1]]))
    }

    /// One effects-shader pass of `op` over `views` into a new picture.
    fn fx_raw(&self, enc: &mut wgpu::CommandEncoder, op: f32, views: [&wgpu::TextureView; 3], size: (u32, u32), more: &[[f32; 4]]) -> (wgpu::Texture, wgpu::TextureView) {
        let t = self.target(size.0, size.1, wgpu::TextureFormat::Rgba8Unorm);
        let v = t.create_view(&Default::default());
        let mut u = vec![[op, 0.0, size.0 as f32, size.1 as f32]];
        u.extend_from_slice(more);
        let b = self.bind3(views, &uniform_block(&u));
        self.pass(enc, &v, &self.fx, &b);
        (t, v)
    }

    fn draw_quads(&self, enc: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, quads: &[wgpu::BindGroup]) {
        if quads.is_empty() {
            return;
        }
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gifkit-layers"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.quad);
        for b in quads {
            pass.set_bind_group(0, b, &[]);
            pass.draw(0..6, 0..1);
        }
    }

    /// Render `f` from `frame` (cached under `frame_key`) into a new texture.
    /// Sprites are cached by their address, so `sprite_gen` must change
    /// whenever the layers that own them do.
    pub fn render(&mut self, frame: &RgbaImage, frame_key: u64, f: &GpuFrame, sprite_gen: u64) -> wgpu::Texture {
        self.render_as(frame, frame_key, f, sprite_gen, false)
    }

    /// As `render`, but straight alpha (for files): no rounding through
    /// premultiplied pixels when the last layer is an effect.
    pub fn render_straight(&mut self, frame: &RgbaImage, frame_key: u64, f: &GpuFrame, sprite_gen: u64) -> wgpu::Texture {
        self.render_as(frame, frame_key, f, sprite_gen, true)
    }

    fn render_as(&mut self, frame: &RgbaImage, frame_key: u64, f: &GpuFrame, sprite_gen: u64, want_straight: bool) -> wgpu::Texture {
        let (w, h) = (f.size.0.max(1), f.size.1.max(1));
        let out = self.target(w, h, wgpu::TextureFormat::Rgba8Unorm);
        let out_view = out.create_view(&Default::default());
        let mut g = f.geo;
        // a gap: a see-through pixel stands in for the picture
        let src_view = if g.blank {
            g.resize = None;
            self.blank.clone()
        } else {
            self.sources.get_or_upload(&self.device, &self.queue, frame_key, frame)
        };
        let mut enc = self.device.create_command_encoder(&Default::default());
        // a size change first, in two passes like the CPU (down, then across)
        let mut src_view = src_view;
        if let Some(r) = g.resize {
            let [x0, y0, x1, y1] = g.crop;
            let (cw, ch) = ((x1 - x0).round() as u32, (y1 - y0).round() as u32);
            let (nw, nh) = r.size;
            let support = [0.0, 1.0, 2.0, 3.0][r.kernel.min(3) as usize];
            let axis = |from: u32, to: u32| {
                let ratio = from as f32 / to as f32;
                let sratio = ratio.max(1.0);
                [ratio, sratio, support * sratio, r.kernel as f32]
            };
            let mid = self.target(cw, nh, wgpu::TextureFormat::Rgba32Float);
            let mid_view = mid.create_view(&Default::default());
            let bv = self.bind(&src_view, &uniform_block(&[axis(ch, nh), [y0, ch as f32, x0, 1.0]]));
            self.pass(&mut enc, &mid_view, &self.resize_v, &bv);
            let sized = self.target(nw, nh, wgpu::TextureFormat::Rgba8Unorm);
            let sized_view = sized.create_view(&Default::default());
            let bh = self.bind(&mid_view, &uniform_block(&[axis(cw, nw), [0.0, cw as f32, 0.0, 0.0]]));
            self.pass(&mut enc, &sized_view, &self.resize_h, &bh);
            // the rest of the geometry now starts from the resized picture
            g.inv = g.inv.then(Affine::translate(-x0, -y0)).then(Affine::scale(nw as f32 / cw.max(1) as f32, nh as f32 / ch.max(1) as f32));
            g.crop = [0.0, 0.0, nw as f32, nh as f32];
            src_view = sized_view;
        }
        let g = &g;
        let foot = footprint(&g.inv);
        // an effect straight after the frame edits takes straight pixels
        let straight_first = matches!(f.ops.first(), Some(GpuOp::Effect { .. })) || (want_straight && f.ops.is_empty());
        let geo_bind = self.bind(
            &src_view,
            &uniform_block(&[
                [g.inv.a, g.inv.b, g.inv.c, g.inv.d],
                [g.inv.tx, g.inv.ty, if g.nearest { 1.0 } else { 0.0 }, if straight_first { 1.0 } else { 0.0 }],
                g.crop,
                [g.inv_canvas.a, g.inv_canvas.b, g.inv_canvas.c, g.inv_canvas.d],
                [g.inv_canvas.tx, g.inv_canvas.ty, g.canvas.0, g.canvas.1],
                g.inner,
                rgba(g.pad_bg),
                rgba(g.rot_bg),
                [foot.0, foot.1, 0.0, 0.0],
            ]),
        );
        self.pass(&mut enc, &out_view, &self.geo, &geo_bind);
        // the frame so far (premultiplied, for the screen) and its size
        let (mut cur_tex, mut cur_view, mut cur_size) = (out, out_view, (w, h));
        let mut quads: Vec<wgpu::BindGroup> = Vec::new();
        let mut straight = straight_first;
        // the result is already straight (an effect came last)
        let mut out_straight = straight_first && f.ops.is_empty();
        let u8f = wgpu::TextureFormat::Rgba8Unorm;
        // clipping: bases' coverage, and the frame as it was before each clipped layer
        let mut masks: HashMap<usize, (wgpu::TextureView, (u32, u32))> = HashMap::new();
        let mut kept: Vec<(wgpu::TextureView, (u32, u32))> = Vec::new();
        let mut unders: HashMap<usize, (wgpu::TextureView, (u32, u32))> = HashMap::new();
        for op in &f.ops {
            match op {
                GpuOp::Draw(d) => {
                    let q = self.quad(d, cur_size, sprite_gen);
                    quads.push(q);
                }
                GpuOp::Mask { draws, slot } => {
                    let t = self.target(cur_size.0, cur_size.1, u8f);
                    let v = t.create_view(&Default::default());
                    // a clear pass, then the base's draws alone
                    let b = self.bind3([&self.blank, &self.blank, &self.blank], &uniform_block(&[[53.0, 0.0, cur_size.0 as f32, cur_size.1 as f32]]));
                    self.pass(&mut enc, &v, &self.fx, &b);
                    let qs: Vec<wgpu::BindGroup> = draws.iter().map(|d| self.quad(d, cur_size, sprite_gen)).collect();
                    self.draw_quads(&mut enc, &v, &qs);
                    masks.insert(*slot, (v, cur_size));
                }
                GpuOp::Under { slot } => {
                    self.draw_quads(&mut enc, &cur_view, &quads);
                    quads.clear();
                    let (_, v) = self.fx_raw(&mut enc, 0.0, [&cur_view, &self.blank, &self.blank], cur_size, &[]);
                    unders.insert(*slot, (v, cur_size));
                }
                GpuOp::AnimClip { slot, passes, mix } => {
                    self.draw_quads(&mut enc, &cur_view, &quads);
                    quads.clear();
                    let (Some((uv, us)), Some((mv, ms))) = (unders.get(slot).cloned(), masks.get(slot).cloned()) else { continue };
                    if us != cur_size || ms != cur_size {
                        continue;
                    }
                    // the base on its own, straight
                    let (gt, gv) = self.fx_raw(&mut enc, 54.0, [&cur_view, &uv, &mv], cur_size, &[]);
                    let before = (*mix < 0.999).then(|| gv.clone());
                    let (mut at, mut av, asz) = self.run_passes(&mut enc, (gt, gv, cur_size), passes, f, sprite_gen);
                    if asz != cur_size {
                        continue;
                    }
                    if let Some(bv) = before {
                        (at, av) = self.fx_raw(&mut enc, 50.0, [&av, &bv, &self.blank], asz, &[[*mix, asz.0 as f32, asz.1 as f32, 0.0]]);
                    }
                    let _ = at;
                    (cur_tex, cur_view) = self.fx_raw(&mut enc, 55.0, [&av, &uv, &self.blank], cur_size, &[]);
                    // later clipped layers follow it
                    masks.insert(*slot, (av, cur_size));
                }
                GpuOp::ClipBegin => {
                    self.draw_quads(&mut enc, &cur_view, &quads);
                    quads.clear();
                    let (_, v) = self.fx_raw(&mut enc, 0.0, [&cur_view, &self.blank, &self.blank], cur_size, &[]);
                    kept.push((v, cur_size));
                }
                GpuOp::ClipEnd { slot } => {
                    self.draw_quads(&mut enc, &cur_view, &quads);
                    quads.clear();
                    let before = kept.pop();
                    if let (Some((bv, bs)), Some((mv, ms))) = (before, masks.get(slot)) {
                        // as the CPU: only when nothing changed the frame's size
                        if bs == cur_size && *ms == cur_size {
                            (cur_tex, cur_view) = self.fx_raw(&mut enc, 52.0, [&cur_view, &bv, mv], cur_size, &[]);
                        }
                    }
                }
                GpuOp::Effect { passes, mix } => {
                    self.draw_quads(&mut enc, &cur_view, &quads);
                    quads.clear();
                    // straight pixels, as the CPU has them
                    let fx_pass = |me: &Self, enc: &mut wgpu::CommandEncoder, op: f32, views: [&wgpu::TextureView; 3], size: (u32, u32), more: &[[f32; 4]]| {
                        let t = me.target(size.0, size.1, u8f);
                        let v = t.create_view(&Default::default());
                        let mut u = vec![[op, 0.0, size.0 as f32, size.1 as f32]];
                        u.extend_from_slice(more);
                        let b = me.bind3(views, &uniform_block(&u));
                        me.pass(enc, &v, &me.fx, &b);
                        (t, v)
                    };
                    let (mut s_tex, mut s_view) = if straight { (cur_tex.clone(), cur_view.clone()) } else { fx_pass(self, &mut enc, 1.0, [&cur_view, &self.blank, &self.blank], cur_size, &[]) };
                    straight = false;
                    let mut s_size = cur_size;
                    let before = (*mix < 0.999).then(|| (s_view.clone(), s_size));
                    (s_tex, s_view, s_size) = self.run_passes(&mut enc, (s_tex, s_view, s_size), passes, f, sprite_gen);
                    // a keyed fade mixes the result with what was under it
                    if let Some((bv, bs)) = before {
                        (s_tex, s_view) = fx_pass(self, &mut enc, 50.0, [&s_view, &bv, &self.blank], s_size, &[[*mix, bs.0 as f32, bs.1 as f32, 0.0]]);
                    }
                    cur_size = s_size;
                    if want_straight && std::ptr::eq(op, f.ops.last().unwrap()) {
                        (cur_tex, cur_view) = (s_tex, s_view);
                        out_straight = true;
                    } else {
                        (cur_tex, cur_view) = fx_pass(self, &mut enc, 2.0, [&s_view, &self.blank, &self.blank], s_size, &[]);
                    }
                }
            }
        }
        self.draw_quads(&mut enc, &cur_view, &quads);
        if want_straight && !out_straight {
            let t = self.target(cur_size.0, cur_size.1, u8f);
            let v = t.create_view(&Default::default());
            let b = self.bind3([&cur_view, &self.blank, &self.blank], &uniform_block(&[[1.0, 0.0, cur_size.0 as f32, cur_size.1 as f32]]));
            self.pass(&mut enc, &v, &self.fx, &b);
            cur_tex = t;
        }
        self.queue.submit([enc.finish()]);
        let pending = self.pending.clone();
        pending.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.queue.on_submitted_work_done(move || {
            pending.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        });
        cur_tex
    }

    /// Is the card still working on a frame sent earlier? Sending more while
    /// it is only queues work up: a heavy effect dragged about would pile up
    /// seconds of it and stall the screen.
    pub fn busy(&self) -> bool {
        let _ = self.device.poll(wgpu::PollType::Poll);
        self.pending.load(std::sync::atomic::Ordering::Relaxed) > 0
    }

    /// Read rendered frames back as they are, all in one wait.
    pub fn read_raw(&self, texs: &[wgpu::Texture]) -> Vec<RgbaImage> {
        let mut enc = self.device.create_command_encoder(&Default::default());
        let bufs: Vec<(wgpu::Buffer, u32, u32, u32)> = texs
            .iter()
            .map(|tex| {
                let (w, h) = (tex.width(), tex.height());
                let row = (4 * w).div_ceil(256) * 256;
                let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("gifkit-read"), size: (row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
                enc.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                );
                (buf, w, h, row)
            })
            .collect();
        self.queue.submit([enc.finish()]);
        for (buf, ..) in &bufs {
            buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        }
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        bufs.iter()
            .map(|(buf, w, h, row)| {
                let (w, h, row) = (*w as usize, *h as usize, *row as usize);
                let mut px = vec![0u8; w * h * 4];
                if let Ok(data) = buf.slice(..).get_mapped_range() {
                    for y in 0..h {
                        px[y * w * 4..(y + 1) * w * 4].copy_from_slice(&data[y * row..y * row + w * 4]);
                    }
                }
                buf.unmap();
                RgbaImage::from_raw(w as u32, h as u32, px).unwrap_or_else(|| RgbaImage::new(w as u32, h as u32))
            })
            .collect()
    }

    /// Read a frame `render` made back, straight alpha.
    #[allow(dead_code)]
    pub fn read(&self, tex: &wgpu::Texture) -> RgbaImage {
        let mut img = self.read_raw(std::slice::from_ref(tex)).pop().unwrap_or_default();
        for p in img.pixels_mut() {
            let a = p[3] as u32;
            for c in 0..3 {
                p[c] = if a == 0 { 0 } else { ((p[c] as u32 * 255 + a / 2) / a).min(255) as u8 };
            }
        }
        img
    }

}

// ---- a renderer shared by background work (export, the compressed preview)

static SHARED: std::sync::Mutex<Option<GpuRenderer>> = std::sync::Mutex::new(None);

/// Hand background work a renderer (on the window's device, or its own).
pub fn set_shared(r: GpuRenderer, name: String) {
    if let Ok(mut s) = SHARED.lock() {
        *s = Some(r);
        if std::env::var_os("GIFKIT_PERF").is_some() {
            eprintln!("background frames on {name}");
        }
    }
}

/// Regions the card has colour mapped (tests check it was used).
pub static MAPPED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Let go of the shared renderer (tests).
#[cfg(test)]
pub fn clear_shared() {
    if let Ok(mut s) = SHARED.lock() {
        *s = None;
    }
}

/// A real graphics card: software ones (llvmpipe, WARP) are slower than
/// the CPU path, which spreads over every core (GIFKIT_SOFT_GPU=1 allows them,
/// for tests).
pub fn worth_using(info: &wgpu::AdapterInfo) -> bool {
    info.device_type != wgpu::DeviceType::Cpu || std::env::var_os("GIFKIT_SOFT_GPU").is_some()
}

/// `resize_triangle` on the shared renderer, in batches; None when there's no
/// card to use (the caller resizes on the CPU).
pub fn shrink_many(imgs: &[&RgbaImage], w: u32, h: u32) -> Option<Vec<RgbaImage>> {
    let mut out = Vec::with_capacity(imgs.len());
    for chunk in imgs.chunks(8) {
        let r = with_shared_keep(|g| g.resize_triangle(chunk, w, h))??;
        out.extend(r);
    }
    Some(out)
}

/// `with_shared`, keeping the renderer's picture caches (for work that
/// doesn't use them).
pub fn with_shared_keep<T>(f: impl FnOnce(&mut GpuRenderer) -> T) -> Option<T> {
    if !enabled() {
        return None;
    }
    let mut g = SHARED.lock().ok()?;
    let r = g.as_mut()?;
    Some(f(r))
}

/// Run `f` with the shared renderer, when there is one and it's turned on.
/// One job at a time: its caches are only good for the job that filled them.
pub fn with_shared<T>(f: impl FnOnce(&mut GpuRenderer) -> T) -> Option<T> {
    if !enabled() {
        return None;
    }
    let mut g = SHARED.lock().ok()?;
    let r = g.as_mut()?;
    r.clear_sources();
    r.clear_sprites();
    Some(f(r))
}

/// Settings ▸ Use Hardware Acceleration, shared with video (`gpu::enabled`).
pub fn enabled() -> bool {
    std::env::var_os("GIFKIT_CPU_PREVIEW").is_none() && crate::gpu::enabled()
}
