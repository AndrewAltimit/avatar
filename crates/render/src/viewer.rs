//! Interactive windowed viewer (feature `viewer`). Opens a winit window and draws a [`Scene`] with
//! the same geometry/shader pipeline as the offscreen path (see [`crate::gpu`]), but to a swapchain
//! surface that re-renders every frame from a live camera: drag to orbit, wheel to zoom, WASD (plus
//! Space/Shift) to walk the focus point through the scene, `R` to reset framing.
//!
//! GPU-only, like the rest of this crate — it knows nothing of FBX/Unity; the caller assembles the
//! [`Scene`] (avatar dropped into a world, etc.) and hands it over.

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use glam::Vec3;
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::gpu::{Batch, Globals, SAMPLES, SHADER, build_geometry, upload_texture};
use crate::{Camera, Scene};

/// Open a window showing `scene` and run the interactive event loop until the window is closed.
/// `title` labels the window. Returns once the user exits. Errors if no display/GPU is available.
pub fn view(scene: Scene, title: &str) -> Result<()> {
    let event_loop = EventLoop::new()
        .context("creating the window event loop (no display? this needs a desktop session)")?;
    let mut app = App {
        scene,
        title: title.to_string(),
        state: None,
        init_err: None,
    };
    event_loop.run_app(&mut app).context("running the viewer")?;
    if let Some(e) = app.init_err.take() {
        return Err(e);
    }
    Ok(())
}

/// An orbit camera: a focus `target` the eye circles at `dist`, looking inward. `yaw`/`pitch` are
/// radians; the eye is reconstructed each frame so the camera and movement stay numerically stable.
#[derive(Clone, Copy)]
struct Orbit {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    dist: f32,
}

impl Orbit {
    /// Recover orbit parameters from a framed [`Camera`] so the window opens at the same view the
    /// offscreen render would produce.
    fn from_camera(c: &Camera) -> Orbit {
        let off = c.eye - c.target;
        let dist = off.length().max(1e-3);
        Orbit {
            target: c.target,
            yaw: off.z.atan2(off.x),
            pitch: (off.y / dist).asin().clamp(-1.5, 1.5),
            dist,
        }
    }

    fn direction(&self) -> Vec3 {
        Vec3::new(
            self.yaw.cos() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.sin() * self.pitch.cos(),
        )
    }

    /// A look-at [`Camera`] for the current orbit, with near/far planes scaled to `dist`.
    fn camera(&self) -> Camera {
        let dir = self.direction();
        Camera {
            eye: self.target + dir * self.dist,
            target: self.target,
            up: Vec3::Y,
            fov_y_deg: 45.0,
            znear: (self.dist * 0.02).max(0.01),
            zfar: self.dist * 20.0 + 100.0,
        }
    }
}

struct App {
    scene: Scene,
    title: String,
    state: Option<State>,
    /// Deferred GPU/surface init error, surfaced after the loop exits.
    init_err: Option<anyhow::Error>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return; // already initialised (resumed can fire more than once)
        }
        let attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.init_err = Some(anyhow!("creating window: {e}"));
                event_loop.exit();
                return;
            }
        };
        match pollster::block_on(State::new(window, &self.scene)) {
            Ok(s) => self.state = Some(s),
            Err(e) => {
                self.init_err = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.resize(size.width, size.height),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if code == KeyCode::Escape && event.state == ElementState::Pressed {
                        event_loop.exit();
                    }
                    state.set_key(code, event.state == ElementState::Pressed);
                }
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button: MouseButton::Left,
                ..
            } => {
                state.dragging = btn_state == ElementState::Pressed;
                if !state.dragging {
                    state.last_cursor = None;
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = (position.x, position.y);
                if state.dragging
                    && let Some((px, py)) = state.last_cursor
                {
                    let (dx, dy) = ((x - px) as f32, (y - py) as f32);
                    state.orbit.yaw += dx * 0.008;
                    state.orbit.pitch = (state.orbit.pitch - dy * 0.008).clamp(-1.5, 1.5);
                }
                state.last_cursor = Some((x, y));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 * 0.02,
                };
                // Multiplicative zoom keeps the feel uniform at any distance.
                state.orbit.dist = (state.orbit.dist * (1.0 - scroll * 0.1)).clamp(0.05, 1.0e5);
            }
            WindowEvent::RedrawRequested => {
                state.update();
                if let Err(e) = state.render() {
                    self.init_err = Some(e);
                    event_loop.exit();
                }
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}

/// All GPU + window state for the live render loop.
struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    globals_bg: wgpu::BindGroup,
    ubuf: wgpu::Buffer,
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    batches: Vec<Batch>,
    scene_bgs: Vec<wgpu::BindGroup>,
    white_bg: wgpu::BindGroup,
    msaa_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    background: [f32; 4],
    light_dir: [f32; 4],
    light_color: [f32; 4],

    orbit: Orbit,
    home: Orbit,
    move_speed: f32,
    pressed: [bool; 6], // W, S, A, D, Space, Shift
    dragging: bool,
    last_cursor: Option<(f64, f64)>,
    last_frame: Instant,
}

impl State {
    async fn new(window: Arc<Window>, scene: &Scene) -> Result<State> {
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .context("creating window surface")?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("no GPU adapter compatible with the window surface")?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("avatar-viewer"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: wgpu::MemoryHints::Performance,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace: wgpu::Trace::Off,
            })
            .await
            .context("requesting GPU device")?;

        let caps = surface.get_capabilities(&adapter);
        // Prefer an sRGB target so linear vertex colours/textures display correctly, matching the
        // offscreen path's Rgba8UnormSrgb.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: w,
            height: h,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Geometry + textures (built once; the scene is static, only the camera moves).
        let (vertices, indices, batches) = build_geometry(scene);
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals-bg"),
            layout: &globals_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: ubuf.as_entire_binding(),
            }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("texture-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
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
        let make_tex_bg = |view: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("texture-bg"),
                layout: &tex_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            })
        };
        let white_view = upload_texture(
            &device,
            &queue,
            &crate::Texture {
                width: 1,
                height: 1,
                rgba: vec![255, 255, 255, 255],
            },
        );
        let white_bg = make_tex_bg(&white_view);
        let scene_views: Vec<wgpu::TextureView> = scene
            .textures
            .iter()
            .map(|t| upload_texture(&device, &queue, t))
            .collect();
        let scene_bgs: Vec<wgpu::BindGroup> = scene_views.iter().map(make_tex_bg).collect();

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh-shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(SHADER)),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pl"),
            bind_group_layouts: &[Some(&globals_bgl), Some(&tex_bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 12 * 4,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32x2],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(format.into())],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::gpu::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });

        let (msaa_view, depth_view) = make_targets(&device, format, w, h);
        let home = Orbit::from_camera(&scene.camera);
        let ld = scene.light.direction.normalize_or(Vec3::NEG_Y);

        Ok(State {
            window,
            surface,
            device,
            queue,
            config,
            format,
            pipeline,
            globals_bg,
            ubuf,
            vbuf,
            ibuf,
            batches,
            scene_bgs,
            white_bg,
            msaa_view,
            depth_view,
            background: scene.background,
            light_dir: [ld.x, ld.y, ld.z, 0.0],
            light_color: [
                scene.light.color[0],
                scene.light.color[1],
                scene.light.color[2],
                scene.light.ambient,
            ],
            orbit: home,
            home,
            // Walk speed scaled to the framed size so it feels right at any world scale.
            move_speed: home.dist,
            pressed: [false; 6],
            dragging: false,
            last_cursor: None,
            last_frame: Instant::now(),
        })
    }

    fn set_key(&mut self, code: KeyCode, down: bool) {
        let slot = match code {
            KeyCode::KeyW | KeyCode::ArrowUp => 0,
            KeyCode::KeyS | KeyCode::ArrowDown => 1,
            KeyCode::KeyA | KeyCode::ArrowLeft => 2,
            KeyCode::KeyD | KeyCode::ArrowRight => 3,
            KeyCode::Space => 4,
            KeyCode::ShiftLeft | KeyCode::ShiftRight => 5,
            KeyCode::KeyR if down => {
                self.orbit = self.home; // reset framing
                return;
            }
            _ => return,
        };
        self.pressed[slot] = down;
    }

    fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        let (m, d) = make_targets(&self.device, self.format, w, h);
        self.msaa_view = m;
        self.depth_view = d;
    }

    fn reconfigure(&mut self) {
        self.resize(self.config.width, self.config.height);
    }

    /// Integrate WASD/Space/Shift movement into the orbit target, scaled by frame time.
    fn update(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        let dir = self.orbit.direction();
        // Ground-plane forward (toward the target) and right vectors for WASD.
        let forward = Vec3::new(-dir.x, 0.0, -dir.z).normalize_or(Vec3::NEG_Z);
        let right = forward.cross(Vec3::Y).normalize_or(Vec3::X);
        let mut motion = Vec3::ZERO;
        if self.pressed[0] {
            motion += forward;
        }
        if self.pressed[1] {
            motion -= forward;
        }
        if self.pressed[3] {
            motion += right;
        }
        if self.pressed[2] {
            motion -= right;
        }
        if self.pressed[4] {
            motion += Vec3::Y;
        }
        if self.pressed[5] {
            motion -= Vec3::Y;
        }
        if motion != Vec3::ZERO {
            self.orbit.target += motion.normalize() * self.move_speed * dt;
        }
    }

    fn render(&mut self) -> Result<()> {
        // wgpu 29: acquisition returns a status enum, not a Result.
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            // Surface went stale (resize/minimise/DPI change) — reconfigure and skip this frame.
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.reconfigure();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(anyhow!("surface texture acquisition failed validation"));
            }
        };
        let view = frame.texture.create_view(&Default::default());

        let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
        let globals = Globals {
            view_proj: self.orbit.camera().view_proj(aspect).to_cols_array_2d(),
            light_dir: self.light_dir,
            light_color: self.light_color,
        };
        self.queue
            .write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&globals));

        let bg = self.background;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.msaa_view,
                    resolve_target: Some(&view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg[0] as f64,
                            g: bg[1] as f64,
                            b: bg[2] as f64,
                            a: bg[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !self.batches.is_empty() {
                rp.set_pipeline(&self.pipeline);
                rp.set_bind_group(0, &self.globals_bg, &[]);
                rp.set_vertex_buffer(0, self.vbuf.slice(..));
                rp.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                for b in &self.batches {
                    let tex_bg = match b.texture.and_then(|i| self.scene_bgs.get(i)) {
                        Some(bg) => bg,
                        None => &self.white_bg,
                    };
                    rp.set_bind_group(1, tex_bg, &[]);
                    rp.draw_indexed(b.range.clone(), 0, 0..1);
                }
            }
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

/// (Re)create the MSAA colour + depth render targets for a given size.
fn make_targets(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
) -> (wgpu::TextureView, wgpu::TextureView) {
    let size = wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: 1,
    };
    let msaa = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("msaa-color"),
        size,
        mip_level_count: 1,
        sample_count: SAMPLES,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size,
        mip_level_count: 1,
        sample_count: SAMPLES,
        dimension: wgpu::TextureDimension::D2,
        format: crate::gpu::DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    (
        msaa.create_view(&Default::default()),
        depth.create_view(&Default::default()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_round_trips_through_a_camera() {
        // A framed camera → orbit params → camera should reproduce the same eye/target.
        let cam = Camera {
            eye: Vec3::new(3.0, 2.0, -4.0),
            target: Vec3::new(1.0, 0.5, 2.0),
            up: Vec3::Y,
            fov_y_deg: 45.0,
            znear: 0.1,
            zfar: 100.0,
        };
        let back = Orbit::from_camera(&cam).camera();
        assert!((back.eye - cam.eye).length() < 1e-3, "eye {:?}", back.eye);
        assert!((back.target - cam.target).length() < 1e-5);
    }
}
