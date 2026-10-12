//! The Floor's wgpu renderer, inside iced's shader widget: a [`Frame`] in, the picture out.
//!
//! Per frame: upload the globals (camera, sun, hemisphere, fog, point lights) and one instance
//! row per drawn thing; draw the sun's shadow map; draw the scene into a 4x MSAA target (opaque
//! by render order, then transparent back to front, as three.js does); resolve; copy into the
//! widget's rectangle of iced's frame. Meshes and textures stay on the GPU while they are used.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use iced::wgpu;
use iced::wgpu::util::DeviceExt;
use iced::widget::shader::{self, Viewport};
use iced::Rectangle;

use super::scene::{Draw, DrawKind, Frame, Shading, Side, Texture};

const SAMPLES: u32 = 4;

/// Light rendering for a software (CPU) adapter: no MSAA, no sun shadows, the 3D at half
/// resolution, upscaled. Set from the adapter the app reports (`set_light`).
static LIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Switches the light mode on (a software adapter) or off.
pub fn set_light(on: bool) {
    LIGHT.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the light mode is on.
pub fn light() -> bool {
    LIGHT.load(std::sync::atomic::Ordering::Relaxed)
}
const SHADOW_SIZE: u32 = 2048;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SHADER: &str = include_str!("floor.wgsl");

const BLIT: &str = r"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
struct Out { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var o: Out;
    o.clip = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}
@fragment fn fs(o: Out) -> @location(0) vec4<f32> { return textureSample(tex, smp, o.uv); }
";

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

/// One drawn thing (or one instance, or one point): its matrix and material values.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Row {
    model: [[f32; 4]; 4],
    color: [f32; 4],
    emissive: [f32; 4],
    uv: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    light_view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    hemi_sky: [f32; 4],
    hemi_ground: [f32; 4],
    fog_color: [f32; 4],
    params: [f32; 4],
    ink: [f32; 4],
    sky_top: [f32; 4],
    sky_horizon: [f32; 4],
    sky_ground: [f32; 4],
    point_pos: [[f32; 4]; 4],
    point_color: [[f32; 4]; 4],
    counts: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Prog {
    Toon,
    Grass,
    Basic,
    ShadowCatcher,
    Outline,
    Sky,
    Points,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Blend {
    Opaque,
    Alpha,
    Additive,
}

/// What selects a render pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    prog: Prog,
    /// 0 none, 1 cull back faces, 2 cull front faces.
    cull: u8,
    blend: Blend,
    depth_write: bool,
    depth_test: bool,
    samples: u32,
}

/// One draw call of the main pass.
#[derive(Debug, Clone)]
struct Cmd {
    key: Key,
    mesh: Option<u64>,
    tex: u64,
    rows: std::ops::Range<u32>,
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

struct GpuTex {
    group: wgpu::BindGroup,
}

/// The offscreen targets, sized to the widget in physical pixels.
struct Targets {
    size: (u32, u32),
    samples: u32,
    /// The multisampled colour target (None at one sample: draw straight into `resolve`).
    msaa: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
    resolve: wgpu::TextureView,
    blit_group: wgpu::BindGroup,
}

/// The renderer's GPU state, shared by every floor primitive (iced keeps one).
pub struct Pipeline {
    format: wgpu::TextureFormat,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    pipelines: HashMap<Key, wgpu::RenderPipeline>,
    shadow_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    tex_layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    globals_group: wgpu::BindGroup,
    shadow_globals_group: wgpu::BindGroup,
    shadow_view: wgpu::TextureView,
    samplers: HashMap<(bool, bool), wgpu::Sampler>,
    linear_clamp: wgpu::Sampler,
    meshes: HashMap<u64, GpuMesh>,
    textures: HashMap<u64, GpuTex>,
    rows: Option<(wgpu::Buffer, usize)>,
    targets: Option<Targets>,
    // This frame's work, made in `prepare`, drawn in `render`.
    cmds: Vec<Cmd>,
    shadow_cmds: Vec<(u64, std::ops::Range<u32>)>,
    physical: Rectangle<f32>,
    background: wgpu::Color,
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("floor"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("floor globals"),
            entries: &[
                uniform(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let shadow_globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("floor shadow globals"),
            entries: &[uniform(0)],
        });
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("floor map"),
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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("floor"),
            bind_group_layouts: &[&globals_layout, &tex_layout],
            push_constant_ranges: &[],
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("floor shadow"),
            bind_group_layouts: &[&shadow_globals_layout],
            push_constant_ranges: &[],
        });

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("floor globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("floor shadow map"),
            size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_view = shadow.create_view(&wgpu::TextureViewDescriptor::default());
        let compare = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("floor shadow compare"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let globals_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("floor globals"),
            layout: &globals_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&shadow_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&compare) },
            ],
        });
        let shadow_globals_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("floor shadow globals"),
            layout: &shadow_globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });

        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("floor shadow"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &[mesh_layout(), row_layout()],
            },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: 2, slope_scale: 1.5, clamp: 0.0 },
            }),
            multisample: Default::default(),
            fragment: None,
            multiview: None,
            cache: None,
        });

        let blit_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("floor blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT.into()),
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("floor blit"),
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
        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("floor blit"),
            bind_group_layouts: &[&blit_layout],
            push_constant_ranges: &[],
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("floor blit"),
            layout: Some(&blit_pipeline_layout),
            vertex: wgpu::VertexState { module: &blit_module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &blit_module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview: None,
            cache: None,
        });
        let linear_clamp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("floor blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let mut pipeline = Pipeline {
            format,
            module,
            layout,
            pipelines: HashMap::new(),
            shadow_pipeline,
            blit_pipeline,
            blit_layout,
            tex_layout,
            globals,
            globals_group,
            shadow_globals_group,
            shadow_view,
            samplers: HashMap::new(),
            linear_clamp,
            meshes: HashMap::new(),
            textures: HashMap::new(),
            rows: None,
            targets: None,
            cmds: Vec::new(),
            shadow_cmds: Vec::new(),
            physical: Rectangle::default(),
            background: wgpu::Color::BLACK,
        };
        // The white 1x1 map that unmapped materials sample (texture id 0).
        let white = Texture { id: 0, ..Texture::new(1, 1, vec![255; 4]) };
        pipeline.upload_texture(device, queue, &white);
        pipeline
    }
}

fn mesh_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
    wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Vertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ATTRS }
}

fn row_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
        3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4, 9 => Float32x4
    ];
    wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Row>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &ATTRS }
}

impl Pipeline {
    fn sampler(&mut self, device: &wgpu::Device, wrap: bool, nearest: bool) -> &wgpu::Sampler {
        self.samplers.entry((wrap, nearest)).or_insert_with(|| {
            let mode = if wrap { wgpu::AddressMode::Repeat } else { wgpu::AddressMode::ClampToEdge };
            let filter = if nearest { wgpu::FilterMode::Nearest } else { wgpu::FilterMode::Linear };
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("floor map"),
                address_mode_u: mode,
                address_mode_v: mode,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: if nearest { wgpu::FilterMode::Nearest } else { wgpu::FilterMode::Linear },
                anisotropy_clamp: if nearest { 1 } else { 4 },
                ..Default::default()
            })
        })
    }

    fn upload_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, tex: &Texture) {
        let levels = if tex.nearest { vec![(tex.width, tex.height, tex.rgba.to_vec())] } else { mip_chain(tex.width, tex.height, &tex.rgba) };
        let format = if tex.srgb { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("floor map"),
            size: wgpu::Extent3d { width: tex.width.max(1), height: tex.height.max(1), depth_or_array_layers: 1 },
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, (w, h, data)) in levels.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(*h) },
                wgpu::Extent3d { width: *w, height: *h, depth_or_array_layers: 1 },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self.sampler(device, tex.wrap, tex.nearest).clone();
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("floor map"),
            layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        self.textures.insert(tex.id, GpuTex { group });
    }

    fn upload_mesh(&mut self, device: &wgpu::Device, geo: &super::scene::Geometry) {
        let mut vertices: Vec<Vertex> = (0..geo.positions.len())
            .map(|i| Vertex { position: geo.positions[i], normal: geo.normals[i], uv: geo.uvs[i] })
            .collect();
        let mut indices: Vec<u32> = if geo.indices.is_empty() { (0..geo.positions.len() as u32).collect() } else { geo.indices.clone() };
        let count = indices.len() as u32;
        // wgpu refuses empty buffers: an empty mesh gets one unused vertex and index.
        if vertices.is_empty() {
            vertices.push(Vertex::zeroed());
        }
        if indices.is_empty() {
            indices.push(0);
        }
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("floor vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("floor indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.meshes.insert(geo.id, GpuMesh { vertices, indices, count });
    }

    fn ensure_pipeline(&mut self, device: &wgpu::Device, key: Key) {
        if self.pipelines.contains_key(&key) {
            return;
        }
        let (vs, fs, buffers): (&str, &str, Vec<wgpu::VertexBufferLayout<'static>>) = match key.prog {
            Prog::Toon => ("vs_mesh", "fs_toon", vec![mesh_layout(), row_layout()]),
            Prog::Grass => ("vs_grass", "fs_toon", vec![mesh_layout(), row_layout()]),
            Prog::Basic => ("vs_mesh", "fs_basic", vec![mesh_layout(), row_layout()]),
            Prog::ShadowCatcher => ("vs_mesh", "fs_shadow_catcher", vec![mesh_layout(), row_layout()]),
            Prog::Outline => ("vs_outline", "fs_outline", vec![mesh_layout(), row_layout()]),
            Prog::Sky => ("vs_sky", "fs_sky", vec![mesh_layout(), row_layout()]),
            Prog::Points => ("vs_points", "fs_basic", vec![row_layout()]),
        };
        let blend = match key.blend {
            Blend::Opaque => None,
            Blend::Alpha => Some(wgpu::BlendState::ALPHA_BLENDING),
            Blend::Additive => Some(wgpu::BlendState {
                color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
                alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
            }),
        };
        let cull_mode = match key.cull {
            1 => Some(wgpu::Face::Back),
            2 => Some(wgpu::Face::Front),
            _ => None,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("floor"),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState { module: &self.module, entry_point: Some(vs), compilation_options: Default::default(), buffers: &buffers },
            primitive: wgpu::PrimitiveState { cull_mode, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: key.depth_write,
                depth_compare: if key.depth_test { wgpu::CompareFunction::LessEqual } else { wgpu::CompareFunction::Always },
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: key.samples, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &self.module,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: self.format, blend, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview: None,
            cache: None,
        });
        self.pipelines.insert(key, pipeline);
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, size: (u32, u32), samples: u32) {
        if self.targets.as_ref().is_some_and(|t| t.size == size && t.samples == samples) {
            return;
        }
        let make = |label, format, samples, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let msaa = (samples > 1).then(|| make("floor msaa", self.format, samples, wgpu::TextureUsages::RENDER_ATTACHMENT));
        let depth = make("floor depth", DEPTH, samples, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let resolve = make("floor resolve", self.format, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
        let blit_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("floor blit"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&resolve) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.linear_clamp) },
            ],
        });
        self.targets = Some(Targets { size, samples, msaa, depth, resolve, blit_group });
    }
}

/// The box-filtered mip chain of an RGBA8 image, the full size first.
fn mip_chain(width: u32, height: u32, rgba: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let mut levels = vec![(width.max(1), height.max(1), rgba.to_vec())];
    while let Some((w, h, data)) = levels.last() {
        if *w == 1 && *h == 1 {
            break;
        }
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at = |sx: u32, sy: u32| u32::from(data[((sy.min(h - 1) * w + sx.min(w - 1)) * 4 + c) as usize]);
                    let sum = at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1);
                    next[((y * nw + x) * 4 + c) as usize] = (sum / 4) as u8;
                }
            }
        }
        levels.push((nw, nh, next));
    }
    levels
}

/// The floor for one frame: what iced's shader widget hands the renderer.
#[derive(Debug, Clone)]
pub struct Primitive {
    pub frame: Arc<Frame>,
}

fn key_of(draw: &Draw) -> Key {
    let m = &draw.material;
    let prog = match (&draw.kind, m.shading) {
        (DrawKind::Outline, _) => Prog::Outline,
        (DrawKind::Points(_), _) => Prog::Points,
        (_, Shading::Sky { .. }) => Prog::Sky,
        (_, Shading::Grass) => Prog::Grass,
        (_, Shading::Basic) => Prog::Basic,
        (_, Shading::Shadow) => Prog::ShadowCatcher,
        _ => Prog::Toon,
    };
    let cull = match (prog, m.side) {
        (Prog::Outline, _) | (Prog::Sky, _) => 2,
        (Prog::Points, _) => 0,
        (_, Side::Front) => 1,
        (_, Side::Back) => 2,
        (_, Side::Double) => 0,
    };
    let blend = if m.additive {
        Blend::Additive
    } else if m.transparent || matches!(m.shading, Shading::Shadow) {
        Blend::Alpha
    } else {
        Blend::Opaque
    };
    // three.js writes depth for transparent materials too, unless told not to.
    let depth_write = match prog {
        Prog::Outline => true,
        Prog::Sky => false,
        _ => m.depth_write && !m.additive,
    };
    Key { prog, cull, blend, depth_write, depth_test: m.depth_test, samples: SAMPLES }
}

fn rows_of(draw: &Draw, world_ink: [f32; 3], shadows: bool) -> Vec<Row> {
    let m = &draw.material;
    let mut flags = 0.0;
    if draw.receive_shadow && shadows {
        flags += 1.0;
    }
    if m.map.is_some() {
        flags += 2.0;
    }
    if m.side == Side::Double {
        flags += 4.0;
    }
    let emissive = m.emissive.scale(m.emissive_intensity);
    let uv = m.map.as_ref().map_or([1.0, 1.0, 0.0, 0.0], |t| [t.repeat[0], t.repeat[1], t.offset[0], t.offset[1]]);
    let row = |model: Mat4, color: [f32; 3], uv: [f32; 4]| Row {
        model: model.to_cols_array_2d(),
        color: [color[0], color[1], color[2], m.opacity],
        emissive: [emissive.r, emissive.g, emissive.b, flags],
        uv,
    };
    match &draw.kind {
        DrawKind::Mesh => vec![row(draw.model, m.color.to_array(), uv)],
        DrawKind::Outline => vec![row(draw.model, world_ink, uv)],
        DrawKind::Instanced(instances, count) => instances
            .iter()
            .take(*count)
            .map(|inst| row(draw.model * inst.matrix, m.color.mul(inst.color).to_array(), uv))
            .collect(),
        DrawKind::Points(points) => {
            let size = if let Shading::Points { size } = m.shading { size } else { 0.1 };
            points.iter().map(|p| row(draw.model * Mat4::from_translation(Vec3::from(*p)), m.color.to_array(), [size, 0.0, 0.0, 0.0])).collect()
        }
    }
}

impl shader::Primitive for Primitive {
    type Pipeline = Pipeline;

    fn prepare(&self, p: &mut Pipeline, device: &wgpu::Device, queue: &wgpu::Queue, bounds: &Rectangle, viewport: &Viewport) {
        let scale = viewport.scale_factor();
        let full = (((bounds.width * scale).round() as u32).max(1), ((bounds.height * scale).round() as u32).max(1));
        let light = light();
        let (samples, shadows) = if light { (1, false) } else { (SAMPLES, true) };
        // Light mode draws the 3D at half resolution; the blit stretches it over the widget.
        let size = if light { ((full.0 / 2).max(1), (full.1 / 2).max(1)) } else { full };
        p.physical = Rectangle { x: bounds.x * scale, y: bounds.y * scale, width: full.0 as f32, height: full.1 as f32 };
        p.ensure_targets(device, size, samples);

        let f = &self.frame;
        let w = &f.world;
        let mut camera = f.camera;
        camera.aspect = size.0 as f32 / size.1 as f32;
        let (view, proj) = (camera.view(), camera.projection());
        let sun_dir = w.sun_position.normalize_or_zero();
        let light_view = Mat4::look_at_rh(w.sun_position, Vec3::ZERO, Vec3::Y);
        let e = w.shadow_extent;
        let light_proj = Mat4::orthographic_rh(-e, e, -e, e, w.shadow_near, w.shadow_far);
        let (sky_top, sky_horizon, sky_ground) = f
            .statics
            .iter()
            .chain(&f.draws)
            .find_map(|d| if let Shading::Sky { top, horizon, ground } = d.material.shading { Some((top, horizon, ground)) } else { None })
            .unwrap_or((w.background, w.background, w.background));
        let v4 = |c: super::scene::Color, a: f32| [c.r, c.g, c.b, a];
        let mut point_pos = [[0.0; 4]; 4];
        let mut point_color = [[0.0; 4]; 4];
        for (i, l) in f.lights.iter().take(4).enumerate() {
            point_pos[i] = [l.position.x, l.position.y, l.position.z, l.distance];
            point_color[i] = v4(l.color.scale(l.intensity), l.decay);
        }
        let globals = Globals {
            view_proj: (proj * view).to_cols_array_2d(),
            view: view.to_cols_array_2d(),
            proj: proj.to_cols_array_2d(),
            light_view_proj: (light_proj * light_view).to_cols_array_2d(),
            eye: [camera.eye.x, camera.eye.y, camera.eye.z, f.time],
            sun_dir: [sun_dir.x, sun_dir.y, sun_dir.z, w.shadow_bias],
            sun_color: v4(w.sun_color.scale(w.sun_intensity), w.shadow_normal_bias),
            hemi_sky: v4(w.hemi_sky.scale(w.hemi_intensity), 0.0),
            hemi_ground: v4(w.hemi_ground.scale(w.hemi_intensity), 0.0),
            fog_color: v4(w.fog_color, w.fog_near),
            params: [w.fog_far, w.outline_thickness, size.1 as f32, if p.format.is_srgb() { 0.0 } else { 1.0 }],
            ink: v4(w.ink, 1.0),
            sky_top: v4(sky_top, 0.0),
            sky_horizon: v4(sky_horizon, 0.0),
            sky_ground: v4(sky_ground, 0.0),
            point_pos,
            point_color,
            counts: [f.lights.len().min(4) as f32, 1.0 / SHADOW_SIZE as f32, 0.0, 0.0],
        };
        queue.write_buffer(&p.globals, 0, bytemuck::bytes_of(&globals));
        let bg = w.background;
        p.background = if p.format.is_srgb() {
            wgpu::Color { r: f64::from(bg.r), g: f64::from(bg.g), b: f64::from(bg.b), a: 1.0 }
        } else {
            let [r, g, b] = bg.to_srgb();
            wgpu::Color { r: f64::from(r), g: f64::from(g), b: f64::from(b), a: 1.0 }
        };

        // Order: opaque by render order; then transparent by render order, farthest first.
        let mut opaque: Vec<&Draw> = Vec::new();
        let mut transparent: Vec<(&Draw, f32)> = Vec::new();
        for d in f.statics.iter().chain(&f.draws) {
            if d.material.transparent || d.material.additive || matches!(d.material.shading, Shading::Shadow) {
                transparent.push((d, d.model.w_axis.truncate().distance(camera.eye)));
            } else {
                opaque.push(d);
            }
        }
        opaque.sort_by_key(|d| d.render_order);
        transparent.sort_by(|a, b| a.0.render_order.cmp(&b.0.render_order).then(b.1.total_cmp(&a.1)));

        let ink = w.ink.to_array();
        let mut rows: Vec<Row> = Vec::new();
        let mut used_meshes = HashSet::new();
        let mut used_textures = HashSet::from([0u64]);
        p.cmds.clear();
        p.shadow_cmds.clear();
        for d in opaque.into_iter().chain(transparent.into_iter().map(|(d, _)| d)) {
            let new_rows = rows_of(d, ink, shadows);
            if new_rows.is_empty() {
                continue;
            }
            let start = rows.len() as u32;
            rows.extend(new_rows);
            let range = start..rows.len() as u32;
            let key = Key { samples, ..key_of(d) };
            p.ensure_pipeline(device, key);
            let mesh = match d.kind {
                DrawKind::Points(_) => None,
                _ => {
                    if !p.meshes.contains_key(&d.geo.id) {
                        p.upload_mesh(device, &d.geo);
                    }
                    used_meshes.insert(d.geo.id);
                    Some(d.geo.id)
                }
            };
            let tex = match &d.material.map {
                Some(t) => {
                    if !p.textures.contains_key(&t.id) {
                        p.upload_texture(device, queue, t);
                    }
                    used_textures.insert(t.id);
                    t.id
                }
                None => 0,
            };
            if d.cast_shadow && shadows {
                if let Some(id) = mesh {
                    p.shadow_cmds.push((id, range.clone()));
                }
            }
            p.cmds.push(Cmd { key, mesh, tex, rows: range });
        }
        p.meshes.retain(|id, _| used_meshes.contains(id));
        p.textures.retain(|id, _| used_textures.contains(id));

        let bytes: &[u8] = bytemuck::cast_slice(&rows);
        let need = bytes.len().max(std::mem::size_of::<Row>());
        if p.rows.as_ref().is_none_or(|(_, cap)| *cap < need) {
            let cap = need.next_power_of_two();
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("floor rows"),
                size: cap as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            p.rows = Some((buffer, cap));
        }
        if let Some((buffer, _)) = &p.rows {
            if !bytes.is_empty() {
                queue.write_buffer(buffer, 0, bytes);
            }
        }
        record(p, device, queue);
    }

    fn draw(&self, p: &Pipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        // iced has set the viewport and scissor to the widget: copy the finished picture in.
        if let Some(targets) = &p.targets {
            pass.set_pipeline(&p.blit_pipeline);
            pass.set_bind_group(0, &targets.blit_group, &[]);
            pass.draw(0..3, 0..1);
        }
        true
    }
}

/// The shadow and scene passes, on the floor's own encoder, submitted at once: they run before iced's
/// frame, which then only copies the finished picture (`Primitive::draw`). No pass of iced's is split.
fn record(p: &Pipeline, device: &wgpu::Device, queue: &wgpu::Queue) {
    let (Some(targets), Some((rows, _))) = (&p.targets, &p.rows) else {
        return;
    };
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("floor") });
    // No shadow casters (light mode): the shadow map is not read, so its pass is skipped.
    if !p.shadow_cmds.is_empty() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("floor shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &p.shadow_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&p.shadow_pipeline);
            pass.set_bind_group(0, &p.shadow_globals_group, &[]);
            pass.set_vertex_buffer(1, rows.slice(..));
            for (id, range) in &p.shadow_cmds {
                if let Some(mesh) = p.meshes.get(id) {
                    pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.count, 0, range.clone());
                }
            }
        }
    {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("floor"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: targets.msaa.as_ref().unwrap_or(&targets.resolve),
                    depth_slice: None,
                    resolve_target: targets.msaa.as_ref().map(|_| &targets.resolve),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(p.background),
                        store: if targets.msaa.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &p.globals_group, &[]);
            let mut current: Option<Key> = None;
            for cmd in &p.cmds {
                let Some(pipeline) = p.pipelines.get(&cmd.key) else { continue };
                if current != Some(cmd.key) {
                    pass.set_pipeline(pipeline);
                    current = Some(cmd.key);
                }
                if let Some(tex) = p.textures.get(&cmd.tex).or_else(|| p.textures.get(&0)) {
                    pass.set_bind_group(1, &tex.group, &[]);
                }
                match cmd.mesh.and_then(|id| p.meshes.get(&id)) {
                    Some(mesh) => {
                        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                        pass.set_vertex_buffer(1, rows.slice(..));
                        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..mesh.count, 0, cmd.rows.clone());
                    }
                    None if cmd.key.prog == Prog::Points => {
                        pass.set_vertex_buffer(0, rows.slice(..));
                        pass.draw(0..6, cmd.rows.clone());
                    }
                    None => {}
                }
            }
        }
    queue.submit([encoder.finish()]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mip_chain_halves_down_to_one_pixel_and_averages() {
        let levels = mip_chain(4, 2, &[0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
            0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255]);
        assert_eq!(levels.iter().map(|l| (l.0, l.1)).collect::<Vec<_>>(), vec![(4, 2), (2, 1), (1, 1)]);
        assert_eq!(levels[1].2[0], 127);
    }

    #[test]
    fn globals_match_the_shaders_layout() {
        // 4 mat4 + 11 vec4 + 2 x 4 vec4 + 1 vec4, all 16-byte aligned.
        assert_eq!(std::mem::size_of::<Globals>(), 4 * 64 + 11 * 16 + 8 * 16 + 16);
        assert_eq!(std::mem::size_of::<Row>(), 112);
    }
}
