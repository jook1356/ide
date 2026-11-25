//! GPU side: CEF shared-texture import into iced's own wgpu device, and the shader-widget
//! pipeline/primitive that draws a view's newest frame (GPU-imported or CPU-uploaded).
//!
//! Presentation is pixel-exact: the widget's bounds are snapped to whole physical pixels, CEF is
//! sized so its frame covers exactly that rectangle, and the fragment shader copies texels 1:1
//! with `textureLoad` (no sampler, no scaling). A frame that does not match the widget yet (while
//! a resize is in flight) is drawn at its own size anchored top-left; the uncovered part is
//! filled with the view's background colour, the overflow is clipped.

use iced::Rectangle;
use iced::wgpu;
use iced::widget::shader::{self, Viewport};
use std::collections::HashMap;
use std::sync::atomic::Ordering::*;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use crate::events::ViewId;
use crate::view::{FrameDump, ViewShared};

/// iced's wgpu device/queue, captured in [`WebPipeline::new`] (first draw of any web view).
static DEVICE: OnceLock<(wgpu::Device, wgpu::Queue)> = OnceLock::new();

/// Imports one CEF accelerated frame into an owned texture on iced's device.
///
/// `Ok(None)`: iced has not created the pipeline yet (no device to import into).
pub(crate) fn import(info: &cef::AcceleratedPaintInfo) -> Result<Option<wgpu::Texture>, String> {
    use cef::osr_texture_import::shared_texture_handle::SharedTextureHandle;
    let Some((device, queue)) = DEVICE.get() else {
        return Ok(None);
    };
    // Test hook: simulate an import failure to exercise the CPU fallback.
    static FAIL: OnceLock<bool> = OnceLock::new();
    if *FAIL.get_or_init(|| std::env::var_os("MENINX_WEB_TEST_FAIL_IMPORT").is_some()) {
        return Err("simulated import failure (MENINX_WEB_TEST_FAIL_IMPORT)".into());
    }
    let handle = SharedTextureHandle::new(info);
    // SAFETY: called inside `on_accelerated_paint` while CEF keeps the shared texture valid;
    // `queue` belongs to `device`, which sits on the default adapter as CEF's GPU process does.
    // The importer copies into an owned texture and waits for the copy before returning.
    let texture = unsafe { handle.import_texture(device, queue) }.map_err(|e| format!("{e:?}"))?;
    Ok(Some(texture))
}

/// `MENINX_WEB_LEGACY_PRESENT=1`: diagnostic switch that reproduces the pre-fix presentation
/// (view sized to the rounded logical bounds, frame stretched bilinearly over the fractional
/// physical viewport) for before/after comparisons in one binary. Never set it in production.
pub(crate) fn legacy_present() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        std::env::var("MENINX_WEB_LEGACY_PRESENT").is_ok_and(|v| !v.is_empty() && v != "0")
    })
}

/// The widget's rectangle on the physical pixel grid, as the rasterizer covers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PhysRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PhysRect {
    /// The pixels whose centres lie inside `bounds * scale` (exactly the fragments a quad
    /// filling iced's viewport `bounds * scale` produces).
    pub fn of(bounds: &Rectangle, scale: f32) -> Self {
        let edge = |v: f32| (v * scale - 0.5).ceil() as i32;
        let (x0, y0) = (edge(bounds.x), edge(bounds.y));
        let (x1, y1) = (
            edge(bounds.x + bounds.width),
            edge(bounds.y + bounds.height),
        );
        Self {
            x: x0,
            y: y0,
            width: (x1 - x0).max(1) as u32,
            height: (y1 - y0).max(1) as u32,
        }
    }
}

/// Size in DIP to give CEF so its frame (`ceil(dip * scale)` pixels, Chromium's
/// `ScaleToCeiledSize`) covers `px` physical pixels with the least overflow (< 1 DIP).
pub(crate) fn dip_for(px: u32, scale: f32) -> u32 {
    ((px as f32 / scale) - 1e-3).ceil().max(1.0) as u32
}

/// Frame size (physical px) CEF produces for a view of `dip` DIP.
pub(crate) fn px_for(dip: u32, scale: f32) -> u32 {
    ((dip as f32 * scale) - 1e-3).ceil().max(1.0) as u32
}

/// The draw request for one view (created every frame by the widget).
#[derive(Debug)]
pub(crate) struct WebPrimitive {
    pub view: Arc<ViewShared>,
    /// Multiplier for the frame's (gamma-encoded) colour: 1.0 = normal, < 1 dims it.
    pub dim: f32,
}

impl std::fmt::Debug for ViewShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewShared").field("id", &self.id).finish()
    }
}

struct ViewGpu {
    seq: u64,
    texture: Option<wgpu::Texture>,
    bind: Option<wgpu::BindGroup>,
    cpu_texture: Option<wgpu::Texture>,
    uniforms: wgpu::Buffer,
    last_used: u64,
}

pub(crate) struct WebPipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    views: HashMap<ViewId, ViewGpu>,
    frame: u64,
}

/// Per-view uniforms (3 x vec4).
struct Uniforms {
    /// Physical pixel of the frame's top-left texel (x, y), dim factor, unused.
    origin_dim: [f32; 4],
    /// Fill for pixels the frame does not cover (premultiplied, gamma-encoded).
    background: [f32; 4],
    /// Diagnostic pre-fix presentation (see [`legacy_present`]): the float viewport the frame
    /// is stretched over with bilinear filtering; width 0 = off.
    legacy: [f32; 4],
}

const UNIFORMS_SIZE: u64 = 48;

impl Uniforms {
    fn bytes(&self) -> [u8; UNIFORMS_SIZE as usize] {
        let mut out = [0u8; UNIFORMS_SIZE as usize];
        let all = self
            .origin_dim
            .iter()
            .chain(&self.background)
            .chain(&self.legacy);
        for (i, v) in all.enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        out
    }
}

const WGSL: &str = r#"
struct U { origin_dim: vec4<f32>, background: vec4<f32>, legacy: vec4<f32> };
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var<uniform> u: U;

@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
  var p = array<vec2<f32>,4>(vec2(-1.0,1.0), vec2(1.0,1.0), vec2(-1.0,-1.0), vec2(1.0,-1.0));
  return vec4(p[i], 0.0, 1.0);
}

// The texel under this fragment, 1:1 (no filtering): `pos` is the framebuffer pixel centre.
fn texel(pos: vec4<f32>) -> vec4<f32> {
  let size = vec2<i32>(textureDimensions(t));
  var c = u.background;
  if (u.legacy.z > 0.0) {
    // Diagnostic only: the pre-fix presentation (frame stretched over the float viewport with
    // bilinear, clamp-to-edge filtering), for before/after comparisons.
    let f = (pos.xy - u.legacy.xy) / u.legacy.zw * vec2<f32>(size) - 0.5;
    let i = vec2<i32>(floor(f));
    let w = f - floor(f);
    let m = size - 1;
    let a = textureLoad(t, clamp(i, vec2(0), m), 0);
    let b = textureLoad(t, clamp(i + vec2(1, 0), vec2(0), m), 0);
    let d = textureLoad(t, clamp(i + vec2(0, 1), vec2(0), m), 0);
    let e = textureLoad(t, clamp(i + vec2(1, 1), vec2(0), m), 0);
    c = mix(mix(a, b, w.x), mix(d, e, w.x), w.y);
  } else {
    let p = vec2<i32>(floor(pos.xy - u.origin_dim.xy));
    if (p.x >= 0 && p.y >= 0 && p.x < size.x && p.y < size.y) {
      c = textureLoad(t, p, 0);
    }
  }
  return vec4(c.rgb * u.origin_dim.z, c.a);
}

// Target is a non-sRGB surface: CEF's gamma-encoded values pass through unchanged.
@fragment fn fs_plain(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  return texel(pos);
}

// Target is an sRGB surface (it encodes on write): decode to linear first, so an opaque
// texel round-trips to the same 8-bit value.
fn to_linear(c: vec3<f32>) -> vec3<f32> {
  return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
}
@fragment fn fs_srgb(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  let c = texel(pos);
  if (c.a <= 0.0) { return vec4(0.0); }
  return vec4(to_linear(c.rgb / c.a) * c.a, c.a);
}
"#;

impl shader::Pipeline for WebPipeline {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let _ = DEVICE.set((device.clone(), queue.clone()));
        tracing::info!(?format, "meninx-web pipeline created on iced's wgpu device");
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("meninx-web bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("meninx-web shader"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("meninx-web pl"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("meninx-web pipeline"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(if format.is_srgb() {
                    "fs_srgb"
                } else {
                    "fs_plain"
                }),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        WebPipeline {
            pipeline,
            layout,
            views: HashMap::new(),
            frame: 0,
        }
    }

    fn trim(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        // Drop GPU resources of views not drawn for ~2 s at 60 fps.
        self.views
            .retain(|_, v| frame.saturating_sub(v.last_used) < 120);
    }
}

impl WebPipeline {
    fn bind(
        &self,
        device: &wgpu::Device,
        texture: &wgpu::Texture,
        uniforms: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("meninx-web bg"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniforms.as_entire_binding(),
                },
            ],
        })
    }
}

/// Copies `texture` (BGRA/RGBA 8-bit) back to the CPU as BGRA. Test/diagnostic use only:
/// blocks until the GPU finished the copy.
fn read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> Option<FrameDump> {
    let size = texture.size();
    let row = size.width * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = row.div_ceil(align) * align;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("meninx-web frame dump"),
        size: padded as u64 * size.height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("meninx-web frame dump"),
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    let submission = queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |_| {});
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })
        .ok()?;
    let data = buffer.get_mapped_range(..).ok()?;
    let mut bgra = Vec::with_capacity((row * size.height) as usize);
    for y in 0..size.height as usize {
        let start = y * padded as usize;
        bgra.extend_from_slice(&data[start..start + row as usize]);
    }
    if matches!(
        texture.format(),
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb
    ) {
        for px in bgra.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
    }
    Some(FrameDump {
        width: size.width,
        height: size.height,
        bgra,
        origin: (0, 0),
        widget_px: (0, 0),
        scale: 1.0,
    })
}

impl shader::Primitive for WebPrimitive {
    type Pipeline = WebPipeline;

    fn prepare(
        &self,
        p: &mut WebPipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let sh = &self.view;
        let scale = viewport.scale_factor();
        // Size CEF to the widget's physical rectangle (and its scale to iced's) right here, in
        // the frame that first shows the new bounds.
        let rect = PhysRect::of(bounds, scale);
        let legacy = legacy_present();
        crate::widget::sync_geometry(sh, rect, scale, p.frame, legacy.then_some(bounds));

        let frame = p.frame;
        let entry = p.views.entry(sh.id).or_insert_with(|| ViewGpu {
            seq: 0,
            texture: None,
            bind: None,
            cpu_texture: None,
            uniforms: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("meninx-web uniforms"),
                size: UNIFORMS_SIZE,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            last_used: frame,
        });
        entry.last_used = frame;
        let current = entry.seq;
        let bg = sh.background;
        let uniforms = Uniforms {
            origin_dim: [rect.x as f32, rect.y as f32, self.dim, 0.0],
            background: [bg.r * bg.a, bg.g * bg.a, bg.b * bg.a, bg.a],
            legacy: if legacy {
                [
                    bounds.x * scale,
                    bounds.y * scale,
                    bounds.width * scale,
                    bounds.height * scale,
                ]
            } else {
                [0.0; 4]
            },
        };
        queue.write_buffer(&entry.uniforms, 0, &uniforms.bytes());

        let gpu = sh
            .gpu_frame
            .lock()
            .unwrap()
            .as_ref()
            .map(|(t, s)| (t.clone(), *s));
        let gpu_seq = gpu.as_ref().map(|g| g.1).unwrap_or(0);
        let cpu_seq = sh
            .cpu_frame
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, s)| *s)
            .unwrap_or(0);

        let mut new_size = None;
        if gpu_seq > current && gpu_seq >= cpu_seq {
            let (texture, seq) = gpu.unwrap();
            let bind = p.bind(device, &texture, &p.views[&sh.id].uniforms);
            let entry = p.views.get_mut(&sh.id).unwrap();
            entry.bind = Some(bind);
            entry.seq = seq;
            new_size = Some((texture.width(), texture.height()));
            entry.texture = Some(texture);
            sh.counters.frames_shown.fetch_add(1, Relaxed);
        } else if cpu_seq > current {
            let slot = sh.cpu_frame.lock().unwrap();
            let Some((f, seq)) = slot.as_ref() else {
                return;
            };
            let size = wgpu::Extent3d {
                width: f.width,
                height: f.height,
                depth_or_array_layers: 1,
            };
            let entry = p.views.get_mut(&sh.id).unwrap();
            let recreate = entry.cpu_texture.as_ref().is_none_or(|t| t.size() != size);
            if recreate {
                entry.cpu_texture = Some(device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("meninx-web cpu frame"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                }));
            }
            let texture = entry.cpu_texture.clone().unwrap();
            queue.write_texture(
                texture.as_image_copy(),
                &f.bgra,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(f.width * 4),
                    rows_per_image: Some(f.height),
                },
                size,
            );
            let seq = *seq;
            drop(slot);
            let v = &p.views[&sh.id];
            let need_bind = recreate || v.bind.is_none() || v.texture.as_ref() != Some(&texture);
            let bind = need_bind.then(|| p.bind(device, &texture, &v.uniforms));
            let entry = p.views.get_mut(&sh.id).unwrap();
            if let Some(b) = bind {
                entry.bind = Some(b);
            }
            entry.seq = seq;
            new_size = Some((size.width, size.height));
            entry.texture = Some(texture);
            sh.counters.frames_shown.fetch_add(1, Relaxed);
        }
        if let Some(size) = new_size {
            frame_shown(sh, size, frame);
        }
        sh.drawn_seq.store(p.views[&sh.id].seq, Release);

        if sh.dump_requested.swap(false, AcqRel)
            && let Some(texture) = &p.views[&sh.id].texture
        {
            let dump = read_back(device, queue, texture).map(|d| FrameDump {
                origin: (rect.x, rect.y),
                widget_px: (rect.width, rect.height),
                scale,
                ..d
            });
            *sh.dump.lock().unwrap() = dump;
        }
    }

    fn draw(&self, p: &WebPipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        // iced has set viewport = widget bounds and scissor = clip bounds: the full-viewport
        // quad covers exactly the widget's pixels (clipped by scrollables, covered by later
        // layers); the shader maps each pixel to one texel.
        if let Some(bind) = p.views.get(&self.view.id).and_then(|v| v.bind.as_ref()) {
            pass.set_pipeline(&p.pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..4, 0..1);
        }
        true
    }
}

/// Book-keeping when a new frame is bound: completes a pending resize measurement once a frame
/// of the requested size is on screen.
fn frame_shown(sh: &ViewShared, size: (u32, u32), frame: u64) {
    sh.frame_px
        .store(((size.0 as u64) << 32) | size.1 as u64, Relaxed);
    // Requests are queued (a divider drag issues one per frame, faster than CEF answers): the
    // frame completes the request it was rendered for; older ones were skipped by CEF.
    let mut pending = sh.resize_pending.lock().unwrap();
    if let Some(i) = pending.iter().rposition(|r| r.expect_px == size) {
        let r = pending[i];
        pending.drain(..=i);
        let frames = frame.saturating_sub(r.frame);
        let ns = r.at.elapsed().as_nanos() as u64;
        let c = &sh.counters;
        c.resizes.fetch_add(1, Relaxed);
        c.resize_frames.fetch_add(frames, Relaxed);
        c.resize_frames_max.fetch_max(frames, Relaxed);
        c.resize_ns.fetch_add(ns, Relaxed);
        c.resize_max_ns.fetch_max(ns, Relaxed);
    }
}

/// A resize waiting for CEF's matching frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingResize {
    pub at: Instant,
    /// Pipeline frame counter when the new size was requested.
    pub frame: u64,
    /// Frame size (physical px) CEF will produce for the new view size.
    pub expect_px: (u32, u32),
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::{Point, Size};

    #[test]
    fn phys_rect_snaps_like_the_rasterizer() {
        // x = 18.3 at 1.5 -> 27.45: the first pixel centre inside is 27.5 -> pixel 27.
        let r = PhysRect::of(
            &Rectangle::new(Point::new(18.3, 10.0), Size::new(100.5, 50.0)),
            1.5,
        );
        assert_eq!((r.x, r.y), (27, 15));
        // Right edge 118.8 * 1.5 = 178.2: the last pixel centre inside is 177.5.
        assert_eq!(r.width, 178 - 27);
        assert_eq!(r.height, 75);
    }

    #[test]
    fn dip_size_covers_the_pixels_with_less_than_one_dip_overflow() {
        for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0, 2.25] {
            for px in 1..2000u32 {
                let dip = dip_for(px, scale);
                let out = px_for(dip, scale);
                assert!(out >= px, "scale {scale} px {px}: dip {dip} -> {out}");
                assert!(
                    (out - px) as f32 <= scale.ceil(),
                    "scale {scale} px {px}: overflow {}",
                    out - px
                );
            }
        }
        assert_eq!(dip_for(1200, 1.5), 800);
        assert_eq!(px_for(800, 1.5), 1200);
        assert_eq!(dip_for(1201, 1.5), 801); // 801 DIP -> 1202 px, one px clipped
    }
}
