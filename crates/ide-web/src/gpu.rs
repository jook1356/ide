//! GPU side: CEF shared-texture import into iced's own wgpu device, and the shader-widget
//! pipeline/primitive that draws a view's newest frame (GPU-imported or CPU-uploaded).

use iced::Rectangle;
use iced::wgpu;
use iced::widget::shader::{self, Viewport};
use std::collections::HashMap;
use std::sync::atomic::Ordering::*;
use std::sync::{Arc, OnceLock};

use crate::events::ViewId;
use crate::view::ViewShared;

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
    if *FAIL.get_or_init(|| std::env::var_os("IDE_WEB_TEST_FAIL_IMPORT").is_some()) {
        return Err("simulated import failure (IDE_WEB_TEST_FAIL_IMPORT)".into());
    }
    let handle = SharedTextureHandle::new(info);
    // SAFETY: called inside `on_accelerated_paint` while CEF keeps the shared texture valid;
    // `queue` belongs to `device`, which sits on the default adapter as CEF's GPU process does.
    // The importer copies into an owned texture and waits for the copy before returning.
    let texture = unsafe { handle.import_texture(device, queue) }.map_err(|e| format!("{e:?}"))?;
    Ok(Some(texture))
}

/// The draw request for one view (created every frame by the widget).
#[derive(Debug)]
pub(crate) struct WebPrimitive {
    pub view: Arc<ViewShared>,
}

impl std::fmt::Debug for ViewShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewShared").field("id", &self.id).finish()
    }
}

struct ViewGpu {
    seq: u64,
    bind: Option<wgpu::BindGroup>,
    cpu_texture: Option<wgpu::Texture>,
    last_used: u64,
}

pub(crate) struct WebPipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    views: HashMap<ViewId, ViewGpu>,
    frame: u64,
}

const WGSL: &str = r#"
struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> VOut {
  var p = array<vec2<f32>,4>(vec2(-1.0,1.0), vec2(1.0,1.0), vec2(-1.0,-1.0), vec2(1.0,-1.0));
  var u = array<vec2<f32>,4>(vec2(0.0,0.0), vec2(1.0,0.0), vec2(0.0,1.0), vec2(1.0,1.0));
  var o: VOut; o.pos = vec4(p[i], 0.0, 1.0); o.uv = u[i]; return o;
}
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
// Target is a non-sRGB surface: CEF's gamma-encoded values pass through.
@fragment fn fs_plain(v: VOut) -> @location(0) vec4<f32> { return textureSample(t, s, v.uv); }
// Target is an sRGB surface (it will encode on write): decode to linear first.
fn to_linear(c: vec3<f32>) -> vec3<f32> {
  return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
}
@fragment fn fs_srgb(v: VOut) -> @location(0) vec4<f32> {
  let c = textureSample(t, s, v.uv);
  if (c.a <= 0.0) { return vec4(0.0); }
  return vec4(to_linear(c.rgb / c.a) * c.a, c.a);
}
"#;

impl shader::Pipeline for WebPipeline {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let _ = DEVICE.set((device.clone(), queue.clone()));
        tracing::info!(?format, "ide-web pipeline created on iced's wgpu device");
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ide-web bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ide-web shader"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ide-web pl"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ide-web pipeline"),
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ide-web sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        WebPipeline {
            pipeline,
            layout,
            sampler,
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
    fn bind(&self, device: &wgpu::Device, texture: &wgpu::Texture) -> wgpu::BindGroup {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ide-web bg"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }
}

impl shader::Primitive for WebPrimitive {
    type Pipeline = WebPipeline;

    fn prepare(
        &self,
        p: &mut WebPipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let sh = &self.view;
        // Keep CEF's device scale in sync with the window (cheap no-op when unchanged).
        crate::widget::sync_scale(sh, viewport.scale_factor());

        let frame = p.frame;
        let entry = p.views.entry(sh.id).or_insert(ViewGpu {
            seq: 0,
            bind: None,
            cpu_texture: None,
            last_used: frame,
        });
        entry.last_used = frame;
        let current = entry.seq;

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

        if gpu_seq > current && gpu_seq >= cpu_seq {
            let (texture, seq) = gpu.unwrap();
            let bind = p.bind(device, &texture);
            let entry = p.views.get_mut(&sh.id).unwrap();
            entry.bind = Some(bind);
            entry.seq = seq;
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
                    label: Some("ide-web cpu frame"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
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
            let need_bind = recreate || p.views[&sh.id].bind.is_none();
            let bind = if need_bind {
                Some(p.bind(device, &texture))
            } else {
                None
            };
            let entry = p.views.get_mut(&sh.id).unwrap();
            if let Some(b) = bind {
                entry.bind = Some(b);
            }
            entry.seq = seq;
            sh.counters.frames_shown.fetch_add(1, Relaxed);
        }
        sh.drawn_seq.store(p.views[&sh.id].seq, Release);
    }

    fn draw(&self, p: &WebPipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        // iced has set viewport = widget bounds and scissor = clip bounds: a full-viewport quad
        // is clipped by scrollables and covered by later layers (overlays) automatically.
        if let Some(bind) = p.views.get(&self.view.id).and_then(|v| v.bind.as_ref()) {
            pass.set_pipeline(&p.pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..4, 0..1);
        }
        true
    }
}
