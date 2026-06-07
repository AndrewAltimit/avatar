//! The wgpu offscreen pipeline. One uniform (camera + light), one merged vertex/index buffer, one
//! draw, 4× MSAA + depth, read back to RGBA8. Targets the wgpu 29 API.

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use wgpu::util::DeviceExt;

use crate::Scene;

const SAMPLES: u32 = 4;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
    nrm: [f32; 3],
    col: [f32; 4],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    // xyz = light direction (normalized), w unused.
    light_dir: [f32; 4],
    // rgb = light colour, a = ambient fraction.
    light_color: [f32; 4],
}

/// One contiguous span of the index buffer that shares a texture (or `None` = the flat-colour /
/// white default), drawn in a single call with that texture's bind group bound.
struct Batch {
    texture: Option<usize>,
    range: std::ops::Range<u32>,
}

/// Flatten the scene into a single world-space vertex buffer (transforms baked on CPU) plus an
/// index buffer grouped into per-texture [`Batch`]es, so each texture is bound once and drawn over
/// its own index range. Untextured meshes share one `None` batch (the white default texture).
fn build_geometry(scene: &Scene) -> (Vec<Vertex>, Vec<u32>, Vec<Batch>) {
    let mut vertices = Vec::new();
    // texture-key (-1 = none) -> the index list drawing into `vertices` for that texture.
    let mut buckets: std::collections::BTreeMap<i64, Vec<u32>> = std::collections::BTreeMap::new();
    for m in &scene.meshes {
        let normals = if m.normals.len() == m.positions.len() {
            m.normals.clone()
        } else {
            crate::compute_normals(&m.positions, &m.indices)
        };
        let has_uv = m.uvs.len() == m.positions.len();
        // Normals transform by the inverse-transpose of the upper 3×3.
        let normal_mat = glam::Mat3::from_mat4(m.transform).inverse().transpose();
        let base = vertices.len() as u32;
        for (i, p) in m.positions.iter().enumerate() {
            let world = m.transform.transform_point3(Vec3::from(*p));
            let n = normal_mat * Vec3::from(normals[i]);
            // Flip V: FBX/glTF UVs are bottom-left origin, wgpu samples top-left.
            let uv = if has_uv {
                [m.uvs[i][0], 1.0 - m.uvs[i][1]]
            } else {
                [0.0, 0.0]
            };
            vertices.push(Vertex {
                pos: world.into(),
                nrm: n.normalize_or_zero().into(),
                col: m.color,
                uv,
            });
        }
        let vcount = m.positions.len() as u32;
        // A textured mesh with no usable UVs degrades to the flat-colour batch.
        let key = m.texture.filter(|_| has_uv).map(|t| t as i64).unwrap_or(-1);
        let bucket = buckets.entry(key).or_default();
        for &idx in &m.indices {
            if idx < vcount {
                bucket.push(base + idx);
            }
        }
    }

    let mut indices = Vec::new();
    let mut batches = Vec::new();
    for (key, idxs) in buckets {
        if idxs.is_empty() {
            continue;
        }
        let start = indices.len() as u32;
        indices.extend_from_slice(&idxs);
        let end = indices.len() as u32;
        batches.push(Batch {
            texture: (key >= 0).then_some(key as usize),
            range: start..end,
        });
    }
    (vertices, indices, batches)
}

/// Upload one RGBA8 texture and return a view. A zero-sized or mismatched buffer falls back to a
/// 1×1 magenta texel so a bad asset is visible rather than a validation error.
fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    t: &crate::Texture,
) -> wgpu::TextureView {
    let (w, h, data): (u32, u32, std::borrow::Cow<[u8]>) = if t.width > 0
        && t.height > 0
        && t.rgba.len() as u64 >= (t.width as u64 * t.height as u64 * 4)
    {
        (t.width, t.height, std::borrow::Cow::Borrowed(&t.rgba))
    } else {
        (1, 1, std::borrow::Cow::Owned(vec![255, 0, 255, 255]))
    };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene-texture"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    tex.create_view(&Default::default())
}

pub async fn render(scene: &Scene, width: u32, height: u32) -> Result<Vec<u8>> {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
        .context("no GPU adapter available (need Vulkan/GL/Metal/DX); offscreen render needs a graphics device")?;

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("avatar-render"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::Performance,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            trace: wgpu::Trace::Off,
        })
        .await
        .context("requesting GPU device")?;

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

    let aspect = width as f32 / height as f32;
    let ld = scene.light.direction.normalize_or(Vec3::NEG_Y);
    let globals = Globals {
        view_proj: scene.camera.view_proj(aspect).to_cols_array_2d(),
        light_dir: [ld.x, ld.y, ld.z, 0.0],
        light_color: [
            scene.light.color[0],
            scene.light.color[1],
            scene.light.color[2],
            scene.light.ambient,
        ],
    };
    let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("globals"),
        contents: bytemuck::bytes_of(&globals),
        usage: wgpu::BufferUsages::UNIFORM,
    });

    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("globals-bg"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: ubuf.as_entire_binding(),
        }],
    });

    // Texture bind group (group 1): one sampled texture + a shared sampler. Every batch — including
    // the untextured one (a 1×1 white texel) — binds one of these, so the shader path is uniform.
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
    // The flat-colour default (white → texture is a no-op, vertex colour shows through).
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
        bind_group_layouts: &[Some(&bgl), Some(&tex_bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("mesh-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32x2],
            }],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            targets: &[Some(COLOR_FORMAT.into())],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None, // imported meshes vary in winding; draw both faces
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
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

    // Targets: a multisampled colour texture resolved into a single-sample texture we copy from.
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let msaa = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("msaa-color"),
        size,
        mip_level_count: 1,
        sample_count: SAMPLES,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let resolve = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("resolve-color"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size,
        mip_level_count: 1,
        sample_count: SAMPLES,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let msaa_view = msaa.create_view(&Default::default());
    let resolve_view = resolve.create_view(&Default::default());
    let depth_view = depth.create_view(&Default::default());

    let bg = scene.background;
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("main-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &msaa_view,
                resolve_target: Some(&resolve_view),
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
                view: &depth_view,
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
        if !batches.is_empty() {
            rp.set_pipeline(&pipeline);
            rp.set_bind_group(0, &bind_group, &[]);
            rp.set_vertex_buffer(0, vbuf.slice(..));
            rp.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
            for b in &batches {
                let tex_bg = match b.texture.and_then(|i| scene_bgs.get(i)) {
                    Some(bg) => bg,
                    None => &white_bg,
                };
                rp.set_bind_group(1, tex_bg, &[]);
                rp.draw_indexed(b.range.clone(), 0, 0..1);
            }
        }
    }

    // Read back the resolved colour texture (bytes_per_row 256-aligned).
    let bpp = 4u32;
    let unpadded = width * bpp;
    let padded =
        unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (padded * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &resolve,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        size,
    );
    queue.submit(Some(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .context("polling for readback")?;
    rx.recv()
        .context("readback channel closed")?
        .context("mapping readback buffer")?;

    let data = slice.get_mapped_range();
    let mut rgba = Vec::with_capacity((width * height * bpp) as usize);
    for row in 0..height {
        let start = (row * padded) as usize;
        rgba.extend_from_slice(&data[start..start + unpadded as usize]);
    }
    drop(data);
    readback.unmap();
    Ok(rgba)
}

const SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    light_color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs(@location(0) pos: vec3<f32>, @location(1) nrm: vec3<f32>, @location(2) col: vec4<f32>, @location(3) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.clip = g.view_proj * vec4<f32>(pos, 1.0);
    out.normal = nrm;
    out.color = col;
    out.uv = uv;
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    // Light direction points the way light travels; surface→light is its negation.
    let to_light = normalize(-g.light_dir.xyz);
    let diffuse = max(dot(n, to_light), 0.0);
    let ambient = g.light_color.a;
    let shade = ambient + (1.0 - ambient) * diffuse;
    // Base colour = texture × vertex tint (the white default texture makes the untextured path a
    // pure tint).
    let tex_col = textureSample(tex, samp, in.uv);
    let base = tex_col * in.color;
    // Alpha cutout: foliage / hair / decals are transparent PNGs drawn with an alpha-clip shader in
    // Unity. Without this the transparent texels render as opaque (white) cards. 0.5 is the usual
    // cutoff; opaque textures (alpha≈1 everywhere) are unaffected.
    if (base.a < 0.5) {
        discard;
    }
    let lit = base.rgb * g.light_color.rgb * shade;
    return vec4<f32>(lit, 1.0);
}
"#;
