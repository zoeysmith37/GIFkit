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

/// Everything needed to make one output frame.
pub struct GpuFrame<'a> {
    pub size: (u32, u32),
    pub geo: GpuGeo,
    pub draws: Vec<GpuDraw<'a>>,
}

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
        while self.bytes + size > self.budget && !self.map.is_empty() {
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
    geo: wgpu::RenderPipeline,
    quad: wgpu::RenderPipeline,
    resize_v: wgpu::RenderPipeline,
    resize_h: wgpu::RenderPipeline,
    blank: wgpu::TextureView,
    sources: TexCache,
    sprites: TexCache,
    pub max_side: u32,
}

impl GpuRenderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("gifkit-frames"), source: wgpu::ShaderSource::Wgsl(include_str!("gpu_render.wgsl").into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gifkit-frames"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("gifkit-frames"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let over = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add };
        let pipe = |vs: &str, fs: &str, blend: Option<wgpu::BlendState>, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), buffers: &[], compilation_options: Default::default() },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let u8f = wgpu::TextureFormat::Rgba8Unorm;
        let geo = pipe("vs_full", "fs_geo", None, u8f);
        let quad = pipe("vs_quad", "fs_quad", Some(wgpu::BlendState { color: over, alpha: over }), u8f);
        let resize_v = pipe("vs_full", "fs_resize_v", None, wgpu::TextureFormat::Rgba32Float);
        let resize_h = pipe("vs_full", "fs_resize_h", None, u8f);
        let blank = upload(&device, &queue, &RgbaImage::new(1, 1), "gifkit-blank").create_view(&Default::default());
        let max_side = device.limits().max_texture_dimension_2d;
        // a quarter of what's typical for the card, capped: frames come back
        // from the CPU copy whenever they've gone
        GpuRenderer { device, queue, layout, geo, quad, resize_v, resize_h, blank, sources: TexCache::new(768 << 20), sprites: TexCache::new(256 << 20), max_side }
    }

    /// A device of its own, for tests and for machines where the window
    /// draws with OpenGL.
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
        let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: &floats(uniforms), usage: wgpu::BufferUsages::UNIFORM });
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) }, wgpu::BindGroupEntry { binding: 1, resource: buf.as_entire_binding() }],
        })
    }

    /// Render `f` from `frame` (cached under `frame_key`) into a new texture.
    /// Sprites are cached by their address, so `sprite_gen` must change
    /// whenever the layers that own them do.
    pub fn render(&mut self, frame: &RgbaImage, frame_key: u64, f: &GpuFrame, sprite_gen: u64) -> wgpu::Texture {
        let (w, h) = (f.size.0.max(1), f.size.1.max(1));
        let out = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gifkit-frame"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let out_view = out.create_view(&Default::default());
        let src_view = self.sources.get_or_upload(&self.device, &self.queue, frame_key, frame);
        let mut g = f.geo;
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
            let bv = self.bind(&src_view, &[axis(ch, nh), [y0, ch as f32, x0, 1.0]].concat());
            self.pass(&mut enc, &mid_view, &self.resize_v, &bv);
            let sized = self.target(nw, nh, wgpu::TextureFormat::Rgba8Unorm);
            let sized_view = sized.create_view(&Default::default());
            let bh = self.bind(&mid_view, &[axis(cw, nw), [0.0, cw as f32, 0.0, 0.0]].concat());
            self.pass(&mut enc, &sized_view, &self.resize_h, &bh);
            // the rest of the geometry now starts from the resized picture
            g.inv = g.inv.then(Affine::translate(-x0, -y0)).then(Affine::scale(nw as f32 / cw.max(1) as f32, nh as f32 / ch.max(1) as f32));
            g.crop = [0.0, 0.0, nw as f32, nh as f32];
            src_view = sized_view;
        }
        let g = &g;
        let foot = footprint(&g.inv);
        let geo_bind = self.bind(
            &src_view,
            &[
                [g.inv.a, g.inv.b, g.inv.c, g.inv.d],
                [g.inv.tx, g.inv.ty, if g.nearest { 1.0 } else { 0.0 }, 0.0],
                g.crop,
                [g.inv_canvas.a, g.inv_canvas.b, g.inv_canvas.c, g.inv_canvas.d],
                [g.inv_canvas.tx, g.inv_canvas.ty, g.canvas.0, g.canvas.1],
                g.inner,
                rgba(g.pad_bg),
                rgba(g.rot_bg),
                [foot.0, foot.1, 0.0, 0.0],
            ]
            .concat(),
        );
        let mut quads = Vec::new();
        for d in &f.draws {
            let inv = d.m.inverse();
            let foot = footprint(&inv);
            let (view, size, col, solid) = match d.src {
                GpuSrc::Img(img) => (self.sprites.get_or_upload(&self.device, &self.queue, (img as *const RgbaImage as u64) ^ sprite_gen.rotate_left(40), img), (img.width() as f32, img.height() as f32), [0.0; 4], 0.0),
                GpuSrc::Solid(c, s) => (self.blank.clone(), s, rgba(c), 1.0),
            };
            quads.push(self.bind(
                &view,
                &[[d.m.a, d.m.b, d.m.c, d.m.d], [d.m.tx, d.m.ty, size.0, size.1], [inv.a, inv.b, inv.c, inv.d], [inv.tx, inv.ty, w as f32, h as f32], col, [d.opacity, solid, foot.0, foot.1]].concat(),
            ));
        }
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gifkit-frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &out_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.geo);
            pass.set_bind_group(0, &geo_bind, &[]);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.quad);
            for b in &quads {
                pass.set_bind_group(0, b, &[]);
                pass.draw(0..6, 0..1);
            }
        }
        self.queue.submit([enc.finish()]);
        out
    }

    /// Read a rendered frame back (for export and tests), straight alpha.
    pub fn read(&self, tex: &wgpu::Texture) -> RgbaImage {
        let (w, h) = (tex.width(), tex.height());
        let row = (4 * w).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("gifkit-read"), size: (row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let Ok(data) = slice.get_mapped_range() else { return RgbaImage::new(w, h) };
        let mut img = RgbaImage::new(w, h);
        for y in 0..h as usize {
            let line = &data[y * row as usize..y * row as usize + 4 * w as usize];
            for (x, p) in line.chunks_exact(4).enumerate() {
                let a = p[3];
                let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
                img.put_pixel(x as u32, y as u32, image::Rgba([un(p[0]), un(p[1]), un(p[2]), a]));
            }
        }
        drop(data);
        buf.unmap();
        img
    }
}
