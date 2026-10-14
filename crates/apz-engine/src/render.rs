//! The Floor's wgpu renderer: pipelines, instancing and the frame's passes. A host (the desktop's iced shader widget, or the browser canvas) calls `Pipeline::new`, `Primitive::prepare` and `Primitive::draw`.
//!
//! Per frame: upload the globals (camera, sun, hemisphere, fog, point lights) and one instance
//! row per drawn thing; draw the sun's shadow map; draw the scene into a 4x MSAA target (opaque
//! by render order, then transparent back to front, as three.js does); resolve; copy into the
//! target rectangle of the host's frame. Meshes and textures stay on the GPU while they are used.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

use super::fleet::{FleetInstances, PacketInstance, RingInstance, WRAP_PERIOD};
use super::scene::{merge, world_sphere, Draw, DrawKind, Fade, Frame, Frustum, Geo, Lod, Shading, Side, Texture};
use super::hardware::{over_area, CUTAWAY_OPACITY};

/// A rectangle in pixels: the host's widget or target rectangle.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect { pub x: f32, pub y: f32, pub width: f32, pub height: f32 }

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

/// `APZ_PERF`: the renderer's numbers, logged once a second: frames, CPU time per frame (the
/// `prepare` work), draw calls, triangles and instances. The GPU time needs timestamp queries,
/// which the host's device does not enable, so it is reported as not available.
#[derive(Default)]
struct PerfStats {
    since: Option<web_time::Instant>,
    frames: u32,
    cpu: std::time::Duration,
    draws: u64,
    static_draws: u64,
    tris: u64,
    instances: u64,
    culled: u64,
}

static PERF_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
static PERF: std::sync::Mutex<PerfStats> = std::sync::Mutex::new(PerfStats {
    since: None,
    frames: 0,
    cpu: std::time::Duration::ZERO,
    draws: 0,
    static_draws: 0,
    tris: 0,
    instances: 0,
    culled: 0,
});

/// Whether `APZ_PERF` is set.
fn perf_on() -> bool {
    *PERF_ON.get_or_init(|| crate::env_var("APZ_PERF").is_some())
}

/// Adds one frame to the `APZ_PERF` counters and logs them once a second.
fn perf_frame(cpu: std::time::Duration, draws: usize, static_draws: usize, tris: u64, instances: usize, culled: usize) {
    if !perf_on() {
        return;
    }
    let now = web_time::Instant::now();
    let mut s = PERF.lock().unwrap_or_else(|p| p.into_inner());
    let since = *s.since.get_or_insert(now);
    s.frames += 1;
    s.cpu += cpu;
    s.draws += draws as u64;
    s.static_draws += static_draws as u64;
    s.tris += tris;
    s.instances += instances as u64;
    s.culled += culled as u64;
    let span = now.saturating_duration_since(since).as_secs_f64();
    if span >= 1.0 {
        let n = f64::from(s.frames.max(1));
        eprintln!(
            "agentplaza-desktop: render {:.1} fps, CPU {:.2} ms per frame, GPU n/a, {:.0} draw calls ({:.0} static, the rest dynamic), {:.0} dynamic draws culled, {:.0} triangles, {:.0} instances",
            f64::from(s.frames) / span,
            s.cpu.as_secs_f64() * 1000.0 / n,
            s.draws as f64 / n,
            s.static_draws as f64 / n,
            s.culled as f64 / n,
            s.tris as f64 / n,
            s.instances as f64 / n,
        );
        *s = PerfStats { since: Some(now), ..PerfStats::default() };
    }
}
/// The draw passes the counters split a frame's draw calls into, in table order.
pub const DRAW_PASSES: [&str; 5] = ["shadow", "scene mesh", "scene quad", "overlay", "blit"];
const PASS_SHADOW: usize = 0;
const PASS_MESH: usize = 1;
const PASS_QUAD: usize = 2;
const PASS_OVERLAY: usize = 3;
const PASS_BLIT: usize = 4;

/// The programs a draw can select, counted per draw in `Pipeline::programs` (the index of each: [`program_index`]).
const PROGRAM_COUNT: usize = 7;

fn program_index(prog: Prog) -> usize {
    match prog {
        Prog::Toon => 0,
        Prog::Grass => 1,
        Prog::Basic => 2,
        Prog::ShadowCatcher => 3,
        Prog::Outline => 4,
        Prog::Sky => 5,
        Prog::Points => 6,
    }
}

/// The bytes of mesh geometry uploaded so far, over every renderer (a cache that keeps missing shows here).
pub static MESH_UPLOAD_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The draw calls of a frame by category, for the performance pass. `shadow` is the shadow map's draws; `main` the
/// scene's mesh draws that do not outline; `outline` the outline program's; `sprite_label` the point sprites and the
/// labels; `other` the host's blit. `programs` counts the mesh and sprite draws by the program that selects their
/// pipeline (the closest thing to a mesh kind the draw calls carry).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct DrawStats {
    pub total: u32,
    pub shadow: u32,
    pub main: u32,
    pub outline: u32,
    pub sprite_label: u32,
    pub other: u32,
    pub programs: ProgramStats,
}

/// The mesh and sprite draws of a frame by program.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct ProgramStats {
    pub toon: u32,
    pub grass: u32,
    pub basic: u32,
    pub shadow_catcher: u32,
    pub outline: u32,
    pub sky: u32,
    pub points: u32,
}

impl DrawStats {
    /// Builds the stats from the pass counts (in [`DRAW_PASSES`] order) and the program counts (in [`program_index`]
    /// order). The outline program's draws are a part of the mesh pass, so `main` is the rest of it.
    fn from_counts(passes: [u32; 5], programs: [u32; PROGRAM_COUNT]) -> Self {
        let [shadow, mesh, quad, overlay, blit] = passes;
        let outline = programs[program_index(Prog::Outline)];
        DrawStats {
            total: passes.iter().sum(),
            shadow,
            main: mesh.saturating_sub(outline),
            outline,
            sprite_label: quad + overlay,
            other: blit,
            programs: ProgramStats {
                toon: programs[program_index(Prog::Toon)],
                grass: programs[program_index(Prog::Grass)],
                basic: programs[program_index(Prog::Basic)],
                shadow_catcher: programs[program_index(Prog::ShadowCatcher)],
                outline,
                sky: programs[program_index(Prog::Sky)],
                points: programs[program_index(Prog::Points)],
            },
        }
    }

    /// The stats as a JSON object.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

impl Pipeline {
    /// Counts one draw call of pass `pass` (an index of [`DRAW_PASSES`]).
    fn count(&self, pass: usize) {
        self.draws[pass].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// The draw calls of the frame so far, by pass (the order of [`DRAW_PASSES`]).
    pub fn draw_breakdown(&self) -> [u32; 5] {
        std::array::from_fn(|i| self.draws[i].load(std::sync::atomic::Ordering::Relaxed))
    }

    /// The draw calls of the frame so far, all passes.
    pub fn draw_calls(&self) -> u32 {
        self.draw_breakdown().iter().sum()
    }

    /// Counts one mesh or sprite draw by the program that selects its pipeline.
    fn count_program(&self, prog: Prog) {
        self.programs[program_index(prog)].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// The draw calls of the frame so far by category (see [`DrawStats`]).
    pub fn draw_stats(&self) -> DrawStats {
        let programs = std::array::from_fn(|i| self.programs[i].load(std::sync::atomic::Ordering::Relaxed));
        DrawStats::from_counts(self.draw_breakdown(), programs)
    }
}

/// The per-pass draw counts summed over the frames of the current `APZ_PERF` window.
struct PassLog {
    since: Option<web_time::Instant>,
    frames: u32,
    draws: [u64; 5],
}

static PASS_LOG: std::sync::Mutex<PassLog> = std::sync::Mutex::new(PassLog { since: None, frames: 0, draws: [0; 5] });

/// Adds one frame's draw counts to the `APZ_PERF` window and logs the per-pass table once a second: the draws per
/// frame of each pass, which is where the Machines view's draw budget goes.
fn perf_passes(counts: [u32; 5]) {
    if !perf_on() {
        return;
    }
    let now = web_time::Instant::now();
    let mut s = PASS_LOG.lock().unwrap_or_else(|p| p.into_inner());
    let since = *s.since.get_or_insert(now);
    s.frames += 1;
    for (total, n) in s.draws.iter_mut().zip(counts) {
        *total += u64::from(n);
    }
    let span = now.saturating_duration_since(since).as_secs_f64();
    if span >= 1.0 {
        let n = f64::from(s.frames.max(1));
        let parts: Vec<String> = DRAW_PASSES.iter().zip(s.draws).map(|(name, d)| format!("{name} {:.0}", d as f64 / n)).collect();
        eprintln!("agentplaza-desktop: draws per frame by pass: {}", parts.join(", "));
        *s = PassLog { since: Some(now), frames: 0, draws: [0; 5] };
    }
}

const SHADOW_SIZE: u32 = 2048;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SHADER: &str = include_str!("floor.wgsl");

/// Overlays: a textured rectangle per instance, in the widget's NDC.
const OVERLAY: &str = r"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
struct Out { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32, @location(0) rect: vec4<f32>) -> Out {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let c = corners[i];
    var o: Out;
    o.clip = vec4<f32>(mix(rect.x, rect.z, c.x), mix(rect.y, rect.w, c.y), 0.0, 1.0);
    o.uv = c;
    return o;
}
@fragment fn fs(o: Out) -> @location(0) vec4<f32> { return textureSample(tex, smp, o.uv); }
";

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
    color: [f32; 3],
    /// The robot slot (0 unchanged, 1 shell, 2 shell2, 3 panel, 4 glass); see `RobotHues`.
    slot: u32,
}

/// One drawn thing (or one instance, or one point): its matrix and material values.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Row {
    model: [[f32; 4]; 4],
    color: [f32; 4],
    emissive: [f32; 4],
    uv: [f32; 4],
    /// The robot hues (rgb; shell, shell2, panel), used when the flags carry 8 (robot).
    robot: [[f32; 4]; 3],
}

/// The fleet's fan-angle table: 4 angles per vec4 (`fleet::MACHINE_SLOTS` machines).
const MACHINE_VEC4: usize = crate::fleet::MACHINE_SLOTS / 4;

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
    /// The fans' angles by fleet machine (`fleet::FleetInstances::machine_angles`), read by hw_anim.wgsl only.
    machine_angle: [[f32; 4]; MACHINE_VEC4],
    /// Each fleet machine's state (`fleet::FleetInstances::machine_states`): busy, activity, hover lift, 0. hw_anim.wgsl
    /// reads it; floor.wgsl reads the lift.
    machine_state: [[f32; 4]; crate::fleet::MACHINE_SLOTS],
    /// Each machine's status colour (`fleet::FleetInstances::machine_statuses`): rgb, 0. Read by hw_anim.wgsl only.
    machine_status: [[f32; 4]; crate::fleet::MACHINE_SLOTS],
    /// The activity LEDs' on colour and off colour (rgb, 0). Read by hw_anim.wgsl only.
    leds: [[f32; 4]; 2],
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
    /// A decal: its depth is biased toward the camera, so it wins the depth test on the solid under it.
    decal: bool,
    samples: u32,
}

/// One draw call of the main pass.
#[derive(Debug, Clone)]
struct Cmd {
    key: Key,
    mesh: Option<u64>,
    tex: u64,
    rows: std::ops::Range<u32>,
    /// The draw's render order.
    order: i32,
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

/// A mesh's buffers are destroyed when it is dropped: the scene's geometry ids change on every rebuild, so the
/// renderer re-uploads and drops meshes often. On the web a dropped GPUBuffer is freed only by garbage collection,
/// and the device runs out of memory first (createBuffer throws); destroy frees them at once.
impl Drop for GpuMesh {
    fn drop(&mut self) {
        self.vertices.destroy();
        self.indices.destroy();
    }
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
    /// The resolved picture's texture (the card probe copies its corner from it).
    resolve_tex: wgpu::Texture,
    blit_group: wgpu::BindGroup,
}

/// The Machines view's instanced animation (`fleet_anim.wgsl`): the cable packets (one instance per slot) and the
/// status rings (one per machine), each one draw. The globals (camera and clock) are written every frame; the
/// instances are uploaded only when they change.
struct FleetGpu {
    module: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    group: wgpu::BindGroup,
    /// The base meshes as triangle lists: a unit sphere (a packet) and a unit ring (a status ring).
    packet_mesh: wgpu::Buffer,
    packet_verts: u32,
    ring_mesh: wgpu::Buffer,
    ring_verts: u32,
    /// The instance buffers and their instance counts.
    packet_rows: (wgpu::Buffer, u32),
    ring_rows: (wgpu::Buffer, u32),
    /// The instances last uploaded: a frame with equal instances uploads nothing.
    uploaded: Option<Arc<FleetInstances>>,
    /// Whether this frame draws the animation, and the pipelines with the sample count they were built for.
    shown: bool,
    pipelines: Option<(u32, wgpu::RenderPipeline, wgpu::RenderPipeline)>,
}

/// The uniform block of `fleet_anim.wgsl` (`Globals`): the camera's view-projection and the clock (x: seconds).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct FleetGlobals {
    view_proj: [[f32; 4]; 4],
    time: [f32; 4],
}

impl FleetGpu {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fleet animation"),
            source: wgpu::ShaderSource::Wgsl(crate::fleet::FLEET_ANIM_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fleet animation"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fleet animation globals"),
            size: std::mem::size_of::<FleetGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fleet animation"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let packet = unit_sphere(8, 6);
        let ring = unit_ring(48, 1.1 / 1.3);
        FleetGpu {
            packet_mesh: vertex_buffer(device, queue, "fleet packet mesh", bytemuck::cast_slice(&packet[..])),
            packet_verts: packet.len() as u32,
            ring_mesh: vertex_buffer(device, queue, "fleet ring mesh", bytemuck::cast_slice(&ring[..])),
            ring_verts: ring.len() as u32,
            packet_rows: (vertex_buffer(device, queue, "fleet packets", &[]), 0),
            ring_rows: (vertex_buffer(device, queue, "fleet rings", &[]), 0),
            module,
            layout,
            globals,
            group,
            uploaded: None,
            shown: false,
            pipelines: None,
        }
    }

    /// This frame: the globals (the clock wrapped at `WRAP_PERIOD`), the instances when they changed, and the
    /// pipelines for `samples` (built once per sample count).
    fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, samples: u32, view_proj: Mat4, time: f32, animated: Option<&Arc<FleetInstances>>) {
        let globals = FleetGlobals { view_proj: view_proj.to_cols_array_2d(), time: [time % WRAP_PERIOD, 0.0, 0.0, 0.0] };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        self.shown = animated.is_some();
        let Some(inst) = animated else { return };
        if self.uploaded.as_deref() != Some(&**inst) {
            replace_rows(&mut self.packet_rows, device, queue, "fleet packets", bytemuck::cast_slice(&inst.packets[..]), inst.packets.len() as u32);
            replace_rows(&mut self.ring_rows, device, queue, "fleet rings", bytemuck::cast_slice(&inst.rings[..]), inst.rings.len() as u32);
            self.uploaded = Some(Arc::clone(inst));
        }
        if self.pipelines.as_ref().map(|p| p.0) != Some(samples) {
            let (packet, ring) = self.build(device, format, samples);
            self.pipelines = Some((samples, packet, ring));
        }
    }

    /// The packet pipeline (additive, depth read only) and the ring pipeline (opaque), for `samples` samples.
    fn build(&self, device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32) -> (wgpu::RenderPipeline, wgpu::RenderPipeline) {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fleet animation"),
            bind_group_layouts: &[&self.layout],
            push_constant_ranges: &[],
        });
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        };
        let packet = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fleet packets"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { module: &self.module, entry_point: Some("vs_packet"), compilation_options: Default::default(), buffers: &[fleet_base_layout(), fleet_packet_layout()] },
            primitive: Default::default(),
            depth_stencil: Some(fleet_depth(false)),
            multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &self.module,
                entry_point: Some("fs_color"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(additive), write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview: None,
            cache: None,
        });
        let ring = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fleet rings"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { module: &self.module, entry_point: Some("vs_ring"), compilation_options: Default::default(), buffers: &[fleet_base_layout(), fleet_ring_layout()] },
            primitive: Default::default(),
            depth_stencil: Some(fleet_depth(true)),
            multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &self.module,
                entry_point: Some("fs_color"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview: None,
            cache: None,
        });
        (packet, ring)
    }

    /// Draws the packets and then the rings, one instanced draw each. Returns the draws issued.
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>) -> usize {
        let Some((_, packet, ring)) = &self.pipelines else { return 0 };
        if !self.shown {
            return 0;
        }
        let mut issued = 0;
        if self.packet_rows.1 > 0 {
            pass.set_pipeline(packet);
            pass.set_bind_group(0, &self.group, &[]);
            pass.set_vertex_buffer(0, self.packet_mesh.slice(..));
            pass.set_vertex_buffer(1, self.packet_rows.0.slice(..));
            pass.draw(0..self.packet_verts, 0..self.packet_rows.1);
            issued += 1;
        }
        if self.ring_rows.1 > 0 {
            pass.set_pipeline(ring);
            pass.set_bind_group(0, &self.group, &[]);
            pass.set_vertex_buffer(0, self.ring_mesh.slice(..));
            pass.set_vertex_buffer(1, self.ring_rows.0.slice(..));
            pass.draw(0..self.ring_verts, 0..self.ring_rows.1);
            issued += 1;
        }
        issued
    }
}

/// The GPU-animated batches (`hw_anim.wgsl`): fan spins and LED blinks, each batch an indexed draw in the main
/// pass (lit, and its ink hull in the outline pass), and the spins also in the sun's shadow pass. The batches upload
/// when the statics' version changes; the pipelines are built on first use. The globals are the floor's: the main
/// pass binds them at group 0 and the shadow pass its own, so this holds no uniform.
struct AnimGpu {
    module: wgpu::ShaderModule,
    scene_layout: wgpu::PipelineLayout,
    shadow_layout: wgpu::PipelineLayout,
    /// The statics' version the batches were uploaded for.
    version: Option<u64>,
    batches: Vec<GpuAnimBatch>,
    /// The main pass's pipelines, for a sample count.
    pipelines: Option<(u32, AnimPipelines)>,
    /// The sun's shadow pipeline (position only), built once a batch casts a shadow.
    shadow: Option<wgpu::RenderPipeline>,
}

/// The main pass's pipelines of hw_anim.wgsl: lit spins and blinks, and their ink hulls.
struct AnimPipelines {
    spin: wgpu::RenderPipeline,
    blink: wgpu::RenderPipeline,
    machine: wgpu::RenderPipeline,
    spin_outline: wgpu::RenderPipeline,
    blink_outline: wgpu::RenderPipeline,
}

/// One animated batch on the GPU: its `AnimVertex` stream (19 floats each), its indices, its kind, whether it is an
/// ink hull, and whether it casts a shadow.
struct GpuAnimBatch {
    kind: super::scene::AnimKind,
    outline: bool,
    shadow: bool,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

impl Drop for GpuAnimBatch {
    fn drop(&mut self) {
        self.vertices.destroy();
        self.indices.destroy();
    }
}

/// The fleet's fan-angle table as the Globals block holds it (`MACHINE_VEC4` vec4s, zero past the fleet).
fn machine_angles(fleet: Option<&crate::fleet::FleetInstances>) -> [[f32; 4]; MACHINE_VEC4] {
    let mut out = [[0.0; 4]; MACHINE_VEC4];
    if let Some(f) = fleet {
        for (i, a) in f.machine_angles.iter().enumerate().take(MACHINE_VEC4 * 4) {
            out[i / 4][i % 4] = *a;
        }
    }
    out
}

/// A fleet table of one vec4 per machine (`fleet::MACHINE_SLOTS` rows), zero past the fleet.
fn machine_rows(rows: Option<&[[f32; 4]]>) -> [[f32; 4]; crate::fleet::MACHINE_SLOTS] {
    let mut out = [[0.0; 4]; crate::fleet::MACHINE_SLOTS];
    for (o, r) in out.iter_mut().zip(rows.unwrap_or(&[])) {
        *o = *r;
    }
    out
}

/// The bytes of one animated vertex: position, normal, colour, params, axis and machine, 19 floats.
const ANIM_STRIDE: u64 = 20 * 4;

fn anim_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32, 6 => Float32];
    wgpu::VertexBufferLayout { array_stride: ANIM_STRIDE, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ATTRS }
}

impl AnimGpu {
    /// `globals` is the floor's globals layout (group 0 of the main pass); `shadow_globals` the shadow pass's.
    fn new(device: &wgpu::Device, globals: &wgpu::BindGroupLayout, shadow_globals: &wgpu::BindGroupLayout) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hardware animation"),
            source: wgpu::ShaderSource::Wgsl(crate::hardware::HW_ANIM_WGSL.into()),
        });
        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hardware animation"),
            bind_group_layouts: &[globals],
            push_constant_ranges: &[],
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hardware animation shadow"),
            bind_group_layouts: &[shadow_globals],
            push_constant_ranges: &[],
        });
        AnimGpu { module, scene_layout, shadow_layout, version: None, batches: Vec::new(), pipelines: None, shadow: None }
    }

    /// This frame: the batches when the statics changed, and the pipelines for `samples` and the shadow pass (built
    /// once, and only when there is something to draw).
    fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, samples: u32, version: u64, batches: &[super::scene::AnimBatch]) {
        if self.version != Some(version) {
            self.batches = batches
                .iter()
                .filter(|b| !b.vertices.is_empty() && matches!(b.draw.kind, DrawKind::Mesh | DrawKind::Outline))
                .map(|b| upload_anim(device, queue, b))
                .collect();
            self.version = Some(version);
        }
        if self.batches.is_empty() {
            return;
        }
        if self.pipelines.as_ref().map(|p| p.0) != Some(samples) {
            let (m, layout) = (&self.module, &self.scene_layout);
            let target = Some((format, samples));
            let pipelines = AnimPipelines {
                spin: anim_pipeline(device, m, layout, target, "vs_spin", Some("fs_color"), None),
                blink: anim_pipeline(device, m, layout, target, "vs_blink", Some("fs_emissive"), None),
                machine: anim_pipeline(device, m, layout, target, "vs_machine", Some("fs_emissive"), None),
                spin_outline: anim_pipeline(device, m, layout, target, "vs_spin_outline", Some("fs_outline"), Some(wgpu::Face::Front)),
                blink_outline: anim_pipeline(device, m, layout, target, "vs_blink_outline", Some("fs_outline"), Some(wgpu::Face::Front)),
            };
            self.pipelines = Some((samples, pipelines));
        }
        if self.shadow.is_none() && self.batches.iter().any(|b| b.shadow) {
            self.shadow = Some(anim_pipeline(device, &self.module, &self.shadow_layout, None, "vs_spin_shadow", None, None));
        }
    }

    /// Draws each batch with its pipeline: one indexed draw per batch. Returns the draws issued. The floor's globals
    /// are bound at group 0 by the caller.
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>) -> usize {
        let Some((_, p)) = &self.pipelines else { return 0 };
        for b in &self.batches {
            pass.set_pipeline(match (b.kind, b.outline) {
                (super::scene::AnimKind::Spin, false) => &p.spin,
                (super::scene::AnimKind::Blink, false) => &p.blink,
                (super::scene::AnimKind::Spin, true) => &p.spin_outline,
                (super::scene::AnimKind::Blink, true) => &p.blink_outline,
                (super::scene::AnimKind::Machine, false) => &p.machine,
                (super::scene::AnimKind::Machine, true) => &p.blink_outline,
            });
            pass.set_vertex_buffer(0, b.vertices.slice(..));
            pass.set_index_buffer(b.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..b.count, 0, 0..1);
        }
        self.batches.len()
    }

    /// The sun's shadow of the spinning parts that cast one. Returns the draws issued.
    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>) -> usize {
        let Some(shadow) = &self.shadow else { return 0 };
        pass.set_pipeline(shadow);
        let mut issued = 0;
        for b in self.batches.iter().filter(|b| b.shadow) {
            pass.set_vertex_buffer(0, b.vertices.slice(..));
            pass.set_index_buffer(b.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..b.count, 0, 0..1);
            issued += 1;
        }
        issued
    }
}

/// An animated batch's buffers: its vertices as flat `AnimVertex` floats, and its indices (identity when the
/// geometry has none).
fn upload_anim(device: &wgpu::Device, queue: &wgpu::Queue, b: &super::scene::AnimBatch) -> GpuAnimBatch {
    let mut flat: Vec<f32> = Vec::with_capacity(b.vertices.len() * 20);
    for v in &b.vertices {
        flat.extend(v.pos);
        flat.extend(v.normal);
        flat.extend(v.color);
        flat.extend(v.params);
        flat.extend(v.axis);
        flat.push(v.machine);
        flat.push(v.lift);
    }
    let indices: Vec<u32> = if b.draw.geo.indices.is_empty() { (0..b.vertices.len() as u32).collect() } else { b.draw.geo.indices.clone() };
    let vertex_bytes: &[u8] = bytemuck::cast_slice(&flat);
    let index_bytes: &[u8] = bytemuck::cast_slice(&indices);
    let vertices = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hardware animation vertices"),
        size: vertex_bytes.len() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&vertices, 0, vertex_bytes);
    let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hardware animation indices"),
        size: index_bytes.len() as u64,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&index_buffer, 0, index_bytes);
    let outline = matches!(b.draw.kind, DrawKind::Outline);
    let shadow = !outline && b.draw.cast_shadow && b.kind == super::scene::AnimKind::Spin;
    GpuAnimBatch { kind: b.kind, outline, shadow, vertices, indices: index_buffer, count: indices.len() as u32 }
}

/// One depth-tested pipeline of hw_anim.wgsl: the vertex entry `vs`, the fragment entry `fs` when the pass has
/// colour (`scene`: the target format and sample count), and the face culled. The shadow pass has no colour target.
fn anim_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    scene: Option<(wgpu::TextureFormat, u32)>,
    vs: &str,
    fs: Option<&str>,
    cull: Option<wgpu::Face>,
) -> wgpu::RenderPipeline {
    let buffers = [anim_layout()];
    let format = scene.map_or(wgpu::TextureFormat::Rgba8Unorm, |s| s.0);
    let targets = [Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })];
    let fragment = fs.map(|fs| wgpu::FragmentState { module, entry_point: Some(fs), compilation_options: Default::default(), targets: &targets });
    let shadow = scene.is_none();
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("hardware animation"),
        layout: Some(layout),
        vertex: wgpu::VertexState { module, entry_point: Some(vs), compilation_options: Default::default(), buffers: &buffers },
        primitive: wgpu::PrimitiveState { cull_mode: cull, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: Default::default(),
            // The shadow pass takes the floor's shadow bias.
            bias: if shadow { wgpu::DepthBiasState { constant: 2, slope_scale: 1.5, clamp: 0.0 } } else { Default::default() },
        }),
        multisample: wgpu::MultisampleState { count: scene.map_or(1, |s| s.1), mask: !0, alpha_to_coverage_enabled: false },
        fragment,
        multiview: None,
        cache: None,
    })
}

fn fleet_depth(write: bool) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: write, depth_compare: wgpu::CompareFunction::LessEqual, stencil: Default::default(), bias: Default::default() }
}

/// The base vertex buffer of both animation draws: one position per vertex (`pos` at location 0).
fn fleet_base_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];
    wgpu::VertexBufferLayout { array_stride: 12, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ATTRS }
}

/// `PacketInstance` per instance: from, to, params, color (`PacketIn` locations 1 to 4).
fn fleet_packet_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![1 => Float32x3, 2 => Float32x3, 3 => Float32x4, 4 => Float32x4];
    wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<PacketInstance>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &ATTRS }
}

/// `RingInstance` per instance: centre, pulse, color (`RingIn` locations 1 to 3).
fn fleet_ring_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4, 3 => Float32x4];
    wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<RingInstance>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &ATTRS }
}

/// A unit sphere as a triangle list, `slices` around and `stacks` down.
fn unit_sphere(slices: u32, stacks: u32) -> Vec<[f32; 3]> {
    use std::f32::consts::{PI, TAU};
    let at = |i: u32, j: u32| {
        let (theta, phi) = (PI * j as f32 / stacks as f32, TAU * i as f32 / slices as f32);
        [theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()]
    };
    let mut out = Vec::new();
    for j in 0..stacks {
        for i in 0..slices {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            out.extend([a, b, c, a, c, d]);
        }
    }
    out
}

/// A unit status ring in the ground plane (outer radius 1, inner `inner`) as a triangle list of `segments` quads.
fn unit_ring(segments: u32, inner: f32) -> Vec<[f32; 3]> {
    use std::f32::consts::TAU;
    let at = |i: u32, r: f32| {
        let a = TAU * i as f32 / segments as f32;
        [r * a.cos(), 0.0, r * a.sin()]
    };
    let mut out = Vec::new();
    for i in 0..segments {
        let (o0, o1, n0, n1) = (at(i, 1.0), at(i + 1, 1.0), at(i, inner), at(i + 1, inner));
        out.extend([o0, n0, o1, o1, n0, n1]);
    }
    out
}

/// A vertex or instance buffer holding `bytes` (an empty one when there are none).
fn vertex_buffer(device: &wgpu::Device, queue: &wgpu::Queue, label: &str, bytes: &[u8]) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (bytes.len() as u64).max(4),
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    if !bytes.is_empty() {
        queue.write_buffer(&buffer, 0, bytes);
    }
    buffer
}

/// Replaces an instance buffer with `bytes` (`count` instances), destroying the old one.
fn replace_rows(slot: &mut (wgpu::Buffer, u32), device: &wgpu::Device, queue: &wgpu::Queue, label: &str, bytes: &[u8], count: u32) {
    let old = std::mem::replace(slot, (vertex_buffer(device, queue, label, bytes), count));
    old.0.destroy();
}

/// The renderer's GPU state, shared by every floor primitive (the host keeps one).
pub struct Pipeline {
    format: wgpu::TextureFormat,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    pipelines: HashMap<Key, wgpu::RenderPipeline>,
    shadow_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    overlay_pipeline: wgpu::RenderPipeline,
    /// Per overlay: its rectangle (NDC: x0, y0, x1, y1).
    overlay_rects: Option<(wgpu::Buffer, usize)>,
    /// This frame's overlays: texture id, in order.
    overlay_cmds: Vec<u64>,
    tex_layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    globals_group: wgpu::BindGroup,
    shadow_globals_group: wgpu::BindGroup,
    shadow_view: wgpu::TextureView,
    /// Whether the shadow map holds casters from an earlier frame: a frame with none clears it once, so receivers do
    /// not read a view's stale shadows (the board's on the Machines view after Back).
    shadow_written: std::sync::atomic::AtomicBool,
    samplers: HashMap<(bool, bool), wgpu::Sampler>,
    linear_clamp: wgpu::Sampler,
    meshes: HashMap<u64, GpuMesh>,
    textures: HashMap<u64, GpuTex>,
    rows: Option<(wgpu::Buffer, usize)>,
    targets: Option<Targets>,
    // This frame's work, made in `prepare`, drawn in `render`.
    cmds: Vec<Cmd>,
    shadow_cmds: Vec<(u64, std::ops::Range<u32>)>,
    /// The statics, as last built: their source, the stamp they were built with, their rows (the
    /// start of the rows buffer) and their commands. Rebuilt only when they change.
    static_src: Option<Arc<Vec<Draw>>>,
    stamp: Option<Stamp>,
    static_rows: Vec<Row>,
    static_opaque: Vec<Cmd>,
    static_transparent: Vec<(Cmd, Vec3)>,
    static_shadow: Vec<(u64, std::ops::Range<u32>)>,
    static_meshes: HashSet<u64>,
    static_textures: HashSet<u64>,
    /// Which detail levels showed last frame (for the hysteresis).
    lod_on: [bool; 2],
    /// Each cutaway's opacity now, and the frame time it was last eased at.
    fade_alpha: HashMap<u32, f32>,
    fade_time: Option<f32>,
    /// The Machines view instanced animation (`fleet_anim.wgsl`).
    fleet: FleetGpu,
    /// The GPU-animated batches of the statics (`hw_anim.wgsl`: fan spins, LED blinks), drawn in the main pass.
    anim: AnimGpu,
    physical: Rect,
    background: wgpu::Color,
    /// The clear's alpha: 1 for the floor; a card pipeline sets 0 so its clear is transparent (no backdrop).
    pub clear_alpha: f32,
    /// Whether this pipeline's corner pixel is read back once (`APZ_CARD_PROBE`); set by the machine card pipeline only.
    pub probe_corner: bool,
    /// The draw calls issued for this frame: the scene passes in `prepare`, then the host pass in `draw`.
    /// Draw calls of the frame so far, by pass (see [`DRAW_PASSES`]). Reset by each [`Primitive::prepare`].
    pub draws: [std::sync::atomic::AtomicU32; 5],
    /// This frame's mesh and sprite draws by program (see [`program_index`]).
    pub programs: [std::sync::atomic::AtomicU32; PROGRAM_COUNT],
}

impl Pipeline {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
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
                // Premultiplied: the picture's colour is premultiplied by its alpha (a card's clear is 0), so a transparent
                // texel leaves the page showing through it. The floor's alpha is 1 everywhere: its picture is unchanged.
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview: None,
            cache: None,
        });
        let overlay_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("floor overlay"),
            source: wgpu::ShaderSource::Wgsl(OVERLAY.into()),
        });
        let overlay_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("floor overlay"),
            bind_group_layouts: &[&tex_layout],
            push_constant_ranges: &[],
        });
        const RECT: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x4];
        let overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("floor overlay"),
            layout: Some(&overlay_layout),
            vertex: wgpu::VertexState {
                module: &overlay_module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout { array_stride: 16, step_mode: wgpu::VertexStepMode::Instance, attributes: &RECT }],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &overlay_module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
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
            overlay_pipeline,
            overlay_rects: None,
            overlay_cmds: Vec::new(),
            tex_layout,
            globals,
            globals_group,
            shadow_globals_group,
            shadow_view,
            shadow_written: std::sync::atomic::AtomicBool::new(false),
            samplers: HashMap::new(),
            linear_clamp,
            meshes: HashMap::new(),
            textures: HashMap::new(),
            rows: None,
            targets: None,
            cmds: Vec::new(),
            shadow_cmds: Vec::new(),
            static_src: None,
            stamp: None,
            static_rows: Vec::new(),
            static_opaque: Vec::new(),
            static_transparent: Vec::new(),
            static_shadow: Vec::new(),
            static_meshes: HashSet::new(),
            static_textures: HashSet::new(),
            lod_on: [false; 2],
            fade_alpha: HashMap::new(),
            fade_time: None,
            fleet: FleetGpu::new(device, queue),
            anim: AnimGpu::new(device, &globals_layout, &shadow_globals_layout),
            physical: Rect::default(),
            background: wgpu::Color::BLACK,
            clear_alpha: 1.0,
            draws: Default::default(),
            programs: Default::default(),
            probe_corner: false,
        };
        // The white 1x1 map that unmapped materials sample (texture id 0).
        let white = Texture { id: 0, ..Texture::new(1, 1, vec![255; 4]) };
        pipeline.upload_texture(device, queue, &white);
        pipeline
    }
}

fn mesh_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 10 => Float32x3, 11 => Uint32];
    wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Vertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ATTRS }
}

fn row_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 10] = wgpu::vertex_attr_array![
        3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4, 9 => Float32x4,
        12 => Float32x4, 13 => Float32x4, 14 => Float32x4
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

    fn upload_mesh(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, geo: &super::scene::Geometry) {
        let mut vertices: Vec<Vertex> = (0..geo.positions.len())
            .map(|i| Vertex {
                position: geo.positions[i],
                normal: geo.normals[i],
                uv: geo.uvs[i],
                color: geo.colors.get(i).copied().unwrap_or([1.0; 3]),
                // The robot slot in the low byte, the lift slot (scene::Node::lift) above it.
                slot: u32::from(geo.slots.get(i).copied().unwrap_or(0)) | (u32::from(geo.lifts.get(i).copied().unwrap_or(0)) << 8),
            })
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
        // Unmapped buffers filled with write_buffer: a buffer mapped at creation takes the device's staging memory,
        // which runs out when a busy scene re-uploads many meshes at once (WebGPU's createBuffer throws then).
        let vertex_bytes: &[u8] = bytemuck::cast_slice(&vertices);
        let index_bytes: &[u8] = bytemuck::cast_slice(&indices);
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("floor vertices"),
            size: vertex_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertices, 0, vertex_bytes);
        let indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("floor indices"),
            size: index_bytes.len() as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&indices, 0, index_bytes);
        MESH_UPLOAD_BYTES.fetch_add((vertex_bytes.len() + index_bytes.len()) as u64, std::sync::atomic::Ordering::Relaxed);
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
                bias: if key.decal { wgpu::DepthBiasState { constant: -2, slope_scale: -1.0, clamp: 0.0 } } else { Default::default() },
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
        let resolve_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("floor resolve"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let resolve = resolve_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let blit_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("floor blit"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&resolve) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.linear_clamp) },
            ],
        });
        self.targets = Some(Targets { size, samples, msaa, depth, resolve, resolve_tex, blit_group });
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

/// The floor for one frame: what the host hands the renderer.
#[derive(Debug, Clone)]
pub struct Primitive {
    pub frame: Arc<Frame>,
}

/// Where a level of detail switches on or off: this far (as a fraction) inside or past its threshold.
const LOD_HYSTERESIS: f32 = 0.05;
/// An instanced part casts a shadow only when its geometry is at least this big (units): a resistor
/// in a copper array does not.
const SHADOW_MIN_INSTANCE: f32 = 1.0;

/// What the cached statics were built for: a change of any of these rebuilds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    shadows: bool,
    ink: [u32; 3],
    samples: u32,
    /// The detail levels shown (see `Pipeline::detail_mask`).
    mask: u8,
}

/// Whether a draw shows at a detail mask: level 0 always, level 1 and 2 by their bits (2 and 4).
fn visible_at(d: &Draw, mask: u8) -> bool {
    d.lod == 0 || (mask >> d.lod) & 1 == 1
}

/// Whether a dynamic draw's bounds touch a view volume: a mesh's (or hull's) sphere under its matrix,
/// or, for an instanced draw, one sphere around all its instances. Points are always kept.
fn bounds_visible(d: &Draw, volume: &Frustum) -> bool {
    match &d.kind {
        DrawKind::Points(_) => true,
        DrawKind::Mesh | DrawKind::Outline => {
            let (c, r) = world_sphere(&d.geo, d.model);
            volume.sphere_visible(c, r)
        }
        DrawKind::Instanced(instances, count) | DrawKind::OutlineInstanced(instances, count) => {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for inst in instances.iter().take(*count) {
                let (c, r) = world_sphere(&d.geo, d.model * inst.matrix);
                lo = lo.min(c - Vec3::splat(r));
                hi = hi.max(c + Vec3::splat(r));
            }
            if lo.x > hi.x {
                return false;
            }
            volume.sphere_visible(0.5 * (lo + hi), 0.5 * (hi - lo).length())
        }
    }
}

/// Whether a draw casts a shadow: meshes, and instanced parts big enough to matter.
fn casts_shadow(d: &Draw) -> bool {
    match &d.kind {
        DrawKind::Mesh => true,
        DrawKind::Instanced(..) => d.geo.radius >= SHADOW_MIN_INSTANCE,
        _ => false,
    }
}

/// Whether a draw is drawn after the opaque ones, back to front: transparent, additive, a shadow
/// catcher, or faded.
fn is_transparent(d: &Draw, alpha: f32) -> bool {
    d.material.transparent || d.material.additive || matches!(d.material.shading, Shading::Shadow) || alpha < 1.0
}

/// The opacity a cutaway eases toward: `CUTAWAY_OPACITY` while the segment from the eye to the focus
/// passes through its box, or the plaza area is known and the box stands over it; else 100 %.
fn fade_target(f: &Fade, eye: Vec3, focus: Vec3, plaza: Option<(Vec3, Vec3)>) -> f32 {
    let hit = segment_hits(f.min, f.max, eye, focus) || plaza.is_some_and(|(lo, hi)| over_area(f.min, f.max, lo, hi));
    if hit {
        CUTAWAY_OPACITY
    } else {
        1.0
    }
}

/// Whether the segment `a`..`b` passes through the box `min`..`max` (the slab test).
fn segment_hits(min: Vec3, max: Vec3, a: Vec3, b: Vec3) -> bool {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for i in 0..3 {
        if d[i].abs() < 1e-6 {
            if a[i] < min[i] || a[i] > max[i] {
                return false;
            }
        } else {
            let inv = 1.0 / d[i];
            let (mut ta, mut tb) = ((min[i] - a[i]) * inv, (max[i] - a[i]) * inv);
            if ta > tb {
                std::mem::swap(&mut ta, &mut tb);
            }
            t0 = t0.max(ta);
            t1 = t1.min(tb);
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

impl Pipeline {
    /// Which detail levels show from the eye: bit 1 for detail 1 (the eye within 160 units of the
    /// board's bounds), bit 2 for detail 2 (within 90), as the web's DETAIL_RANGE: the board's default
    /// view shows both, a far view drops them, and zooming in brings them back. A level switches on
    /// inside its threshold less 5 % and off past it plus 5 %. Without a board, every level shows.
    fn detail_mask(&mut self, lod: Option<Lod>, eye: Vec3) -> u8 {
        let Some(lod) = lod else { return 0b110 };
        let distance = (eye.distance(lod.center) - lod.radius).max(0.0);
        let mut mask = 0;
        for (i, limit) in [160.0f32, 90.0].into_iter().enumerate() {
            let on = &mut self.lod_on[i];
            *on = if *on { distance < limit * (1.0 + LOD_HYSTERESIS) } else { distance < limit * (1.0 - LOD_HYSTERESIS) };
            if *on {
                mask |= 2 << i;
            }
        }
        mask
    }

    /// Eases each cutaway's opacity toward `CUTAWAY_OPACITY` while the segment from the eye to the focus
    /// passes through its box or the box is over the plaza area, and back to 100 % once it is not
    /// (settled in about 0.3 s).
    fn fade_step(&mut self, fades: &[Fade], eye: Vec3, focus: Vec3, plaza: Option<(Vec3, Vec3)>, time: f32) {
        let dt = self.fade_time.map_or(0.0, |t| (time - t).clamp(0.0, 0.1));
        self.fade_time = Some(time);
        let k = 1.0 - (-dt * 12.0).exp();
        for f in fades {
            let target = fade_target(f, eye, focus, plaza);
            let a = self.fade_alpha.entry(f.id).or_insert(1.0);
            *a += (target - *a) * k;
        }
    }

    /// Whether the cached statics were built from this Frame's statics with this stamp.
    fn statics_are(&self, src: &Arc<Vec<Draw>>, stamp: Stamp) -> bool {
        self.static_src.as_ref().is_some_and(|s| Arc::ptr_eq(s, src)) && self.stamp == Some(stamp)
    }

    /// Builds the statics' rows, draws and shadows: one row per copy, the uploads of their meshes
    /// and textures, and the commands in their order. Rebuilt when the statics or the stamp change;
    /// their rows are then written to the start of the rows buffer.
    fn build_statics(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, src: &Arc<Vec<Draw>>, stamp: Stamp, ink: [f32; 3], samples: u32) {
        self.static_rows.clear();
        self.static_opaque.clear();
        self.static_transparent.clear();
        self.static_shadow.clear();
        self.static_meshes.clear();
        self.static_textures.clear();
        // The meshes that cast shadows are merged into one geometry for the shadow pass.
        let mut casters: Vec<(Geo, Mat4)> = Vec::new();
        for d in src.iter().filter(|d| visible_at(d, stamp.mask)) {
            let a = d.material.opacity;
            if stamp.shadows && d.cast_shadow && matches!(d.kind, DrawKind::Mesh) && !is_transparent(d, a) {
                casters.push((Arc::clone(&d.geo), d.model));
            }
            let rows = rows_of(d, ink, stamp.shadows, a);
            if rows.is_empty() {
                continue;
            }
            let start = self.static_rows.len() as u32;
            self.static_rows.extend(rows);
            let range = start..self.static_rows.len() as u32;
            let key = Key { samples, ..key_of(d, a) };
            self.ensure_pipeline(device, key);
            let mesh = match d.kind {
                DrawKind::Points(_) => None,
                _ => {
                    if !self.meshes.contains_key(&d.geo.id) {
                        self.upload_mesh(device, queue, &d.geo);
                    }
                    self.static_meshes.insert(d.geo.id);
                    Some(d.geo.id)
                }
            };
            let tex = match &d.material.map {
                Some(t) => {
                    if !self.textures.contains_key(&t.id) {
                        self.upload_texture(device, queue, t);
                    }
                    self.static_textures.insert(t.id);
                    t.id
                }
                None => 0,
            };
            if d.cast_shadow && stamp.shadows && casts_shadow(d) && !matches!(d.kind, DrawKind::Mesh) {
                if let Some(id) = mesh {
                    self.static_shadow.push((id, range.clone()));
                }
            }
            let cmd = Cmd { key, mesh, tex, rows: range, order: d.render_order };
            if is_transparent(d, a) {
                self.static_transparent.push((cmd, d.model.w_axis.truncate()));
            } else {
                self.static_opaque.push(cmd);
            }
        }
        if !casters.is_empty() {
            let merged = merge(&casters);
            if !self.meshes.contains_key(&merged.id) {
                self.upload_mesh(device, queue, &merged);
            }
            self.static_meshes.insert(merged.id);
            let start = self.static_rows.len() as u32;
            self.static_rows.push(Row {
                model: Mat4::IDENTITY.to_cols_array_2d(),
                color: [1.0, 1.0, 1.0, 1.0],
                emissive: [0.0; 4],
                uv: [1.0, 1.0, 0.0, 0.0],
                robot: [[0.0; 4]; 3],
            });
            self.static_shadow.push((merged.id, start..start + 1));
        }
        self.static_opaque.sort_by_key(|c| c.order);
        self.static_src = Some(Arc::clone(src));
        self.stamp = Some(stamp);
    }
}

fn key_of(draw: &Draw, alpha: f32) -> Key {
    let m = &draw.material;
    let prog = match (&draw.kind, m.shading) {
        (DrawKind::Outline | DrawKind::OutlineInstanced(..), _) => Prog::Outline,
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
    } else if m.transparent || matches!(m.shading, Shading::Shadow) || alpha < 1.0 {
        Blend::Alpha
    } else {
        Blend::Opaque
    };
    // three.js writes depth for transparent materials too, unless told not to. A faded cutaway writes it as well:
    // its parts are merged by material (the PCB, the copper, the shroud of a card in a few draws), so without depth
    // every inner layer would blend over the outer shell and tint it (a GPU card read brown); with it the nearest
    // surface of the cutaway shows, as three.js's per-mesh sort did.
    let depth_write = match prog {
        Prog::Outline => true,
        Prog::Sky => false,
        _ => m.depth_write && !m.additive,
    };
    Key { prog, cull, blend, depth_write, depth_test: m.depth_test, decal: m.decal, samples: SAMPLES }
}

fn rows_of(draw: &Draw, world_ink: [f32; 3], shadows: bool, alpha: f32) -> Vec<Row> {
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
    // 8: a robot material (its vertex slots 1 to 3 take the hues below).
    let hues = m.robot.map_or([[0.0; 4]; 3], |h| [[h.shell.r, h.shell.g, h.shell.b, 0.0], [h.shell2.r, h.shell2.g, h.shell2.b, 0.0], [h.panel.r, h.panel.g, h.panel.b, 0.0]]);
    if m.robot.is_some() {
        flags += 8.0;
    }
    // 16: a billboard (floor.wgsl vs_mesh lays the quad out on the camera's axes).
    if m.billboard {
        flags += 16.0;
    }
    let emissive = m.emissive.scale(m.emissive_intensity);
    // uv' = uv * repeat + offset; a flipY texture (a canvas's) reads v as 1 - v first, as three uploads it.
    let uv = m.map.as_ref().map_or([1.0, 1.0, 0.0, 0.0], |t| {
        if t.flip_y {
            [t.repeat[0], -t.repeat[1], t.offset[0], t.offset[1] + t.repeat[1]]
        } else {
            [t.repeat[0], t.repeat[1], t.offset[0], t.offset[1]]
        }
    });
    let row = |model: Mat4, color: [f32; 3], uv: [f32; 4]| Row {
        model: model.to_cols_array_2d(),
        color: [color[0], color[1], color[2], alpha],
        emissive: [emissive.r, emissive.g, emissive.b, flags],
        uv,
        robot: hues,
    };
    match &draw.kind {
        DrawKind::Mesh => vec![row(draw.model, m.color.to_array(), uv)],
        DrawKind::Outline => vec![row(draw.model, world_ink, uv)],
        DrawKind::Instanced(instances, count) => instances
            .iter()
            .take(*count)
            .map(|inst| row(draw.model * inst.matrix, m.color.mul(inst.color).to_array(), uv))
            .collect(),
        DrawKind::OutlineInstanced(instances, count) => {
            instances.iter().take(*count).map(|inst| row(draw.model * inst.matrix, world_ink, uv)).collect()
        }
        DrawKind::Points(points) => {
            let size = if let Shading::Points { size } = m.shading { size } else { 0.1 };
            points.iter().map(|p| row(draw.model * Mat4::from_translation(Vec3::from(*p)), m.color.to_array(), [size, 0.0, 0.0, 0.0])).collect()
        }
    }
}

impl Primitive {
    pub fn prepare(&self, p: &mut Pipeline, device: &wgpu::Device, queue: &wgpu::Queue, bounds: &Rect, scale: f32) {
        for count in &p.draws {
            count.store(0, std::sync::atomic::Ordering::Relaxed);
        }
        for count in &p.programs {
            count.store(0, std::sync::atomic::Ordering::Relaxed);
        }
        let started = web_time::Instant::now();
        let full = (((bounds.width * scale).round() as u32).max(1), ((bounds.height * scale).round() as u32).max(1));
        let light = light();
        let (samples, shadows) = if light { (1, false) } else { (SAMPLES, true) };
        // Light mode draws the 3D at half resolution; the blit stretches it over the widget.
        let size = if light { ((full.0 / 2).max(1), (full.1 / 2).max(1)) } else { full };
        p.physical = Rect { x: bounds.x * scale, y: bounds.y * scale, width: full.0 as f32, height: full.1 as f32 };
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
        let sky = f
            .statics
            .iter()
            .chain(&f.draws)
            .find_map(|d| if let Shading::Sky { top, horizon, ground } = d.material.shading { Some((top, horizon, ground)) } else { None });
        let flat = (w.background, w.background, w.background);
        let (sky_top, sky_horizon, sky_ground) = if w.flat_background { flat } else { sky.unwrap_or(flat) };
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
            counts: [f.lights.len().min(4) as f32, 1.0 / SHADOW_SIZE as f32, f.time % WRAP_PERIOD, 0.0],
            machine_angle: machine_angles(f.animated.as_deref()),
            machine_state: machine_rows(f.animated.as_deref().map(|x| x.machine_states.as_slice())),
            machine_status: machine_rows(f.animated.as_deref().map(|x| x.machine_statuses.as_slice())),
            leds: crate::hardware::led_colours(),
        };
        queue.write_buffer(&p.globals, 0, bytemuck::bytes_of(&globals));
        p.fleet.prepare(device, queue, p.format, samples, proj * view, f.time, f.animated.as_ref());
        p.anim.prepare(device, queue, p.format, samples, f.statics_version, f.anim_batches.as_slice());
        let bg = w.background;
        // Premultiplied: the clear's alpha scales its colour, so a card's clear of 0 is fully transparent.
        let a = f64::from(p.clear_alpha);
        p.background = if p.format.is_srgb() {
            wgpu::Color { r: f64::from(bg.r) * a, g: f64::from(bg.g) * a, b: f64::from(bg.b) * a, a }
        } else {
            let [r, g, b] = bg.to_srgb();
            wgpu::Color { r: f64::from(r) * a, g: f64::from(g) * a, b: f64::from(b) * a, a }
        };

        let ink = w.ink.to_array();
        // Detail levels (by the eye's distance, with hysteresis) and the cutaways' fades.
        let eye = camera.eye;
        let mask = p.detail_mask(f.lod, eye);
        p.fade_step(&f.fades, eye, f.focus.unwrap_or(camera.target), f.plaza, f.time);
        let fades_now = p.fade_alpha.clone();
        let alpha = |d: &Draw| -> f32 {
            let fade = if d.fade != 0 { fades_now.get(&d.fade).copied().unwrap_or(1.0) } else { 1.0 };
            d.material.opacity * fade
        };
        let stamp = Stamp { shadows, ink: ink.map(f32::to_bits), samples, mask };
        let statics_changed = !p.statics_are(&f.statics, stamp);
        if statics_changed {
            p.build_statics(device, queue, &f.statics, stamp, ink, samples);
        }

        // The dynamic draws: whatever moves this frame, one draw per mesh and material.
        let row_size = std::mem::size_of::<Row>();
        let base = p.static_rows.len() as u32;
        let mut dyn_rows: Vec<Row> = Vec::new();
        let mut dyn_opaque: Vec<Cmd> = Vec::new();
        let mut dyn_transparent: Vec<(Cmd, f32)> = Vec::new();
        let mut dyn_shadow: Vec<(u64, std::ops::Range<u32>)> = Vec::new();
        let mut used_meshes = HashSet::new();
        let mut used_textures = HashSet::from([0u64]);
        // Culling: a dynamic draw outside the camera's view is left out of the main pass, and one outside
        // the sun's shadow volume out of the shadow pass. A caster off screen stays while it is inside the
        // volume, since it can shadow what is on screen.
        let main_frustum = Frustum::from_view_proj(proj * view);
        let shadow_frustum = Frustum::from_view_proj(light_proj * light_view);
        let mut culled = 0usize;
        for d in f.draws.iter().filter(|d| visible_at(d, mask)) {
            let a = alpha(d);
            let in_main = bounds_visible(d, &main_frustum);
            let in_shadow = shadows && d.cast_shadow && casts_shadow(d) && bounds_visible(d, &shadow_frustum);
            if !in_main {
                culled += 1;
            }
            if !in_main && !in_shadow {
                continue;
            }
            let rows = rows_of(d, ink, shadows, a);
            if rows.is_empty() {
                continue;
            }
            let start = base + dyn_rows.len() as u32;
            dyn_rows.extend(rows);
            let range = start..base + dyn_rows.len() as u32;
            let key = Key { samples, ..key_of(d, a) };
            p.ensure_pipeline(device, key);
            let mesh = match d.kind {
                DrawKind::Points(_) => None,
                _ => {
                    if !p.meshes.contains_key(&d.geo.id) {
                        p.upload_mesh(device, queue, &d.geo);
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
            if in_shadow {
                if let Some(id) = mesh {
                    dyn_shadow.push((id, range.clone()));
                }
            }
            if !in_main {
                continue;
            }
            let cmd = Cmd { key, mesh, tex, rows: range, order: d.render_order };
            if is_transparent(d, a) {
                dyn_transparent.push((cmd, d.model.w_axis.truncate().distance(eye)));
            } else {
                dyn_opaque.push(cmd);
            }
        }

        // Order: opaque by render order (the statics first on a tie); then transparent by render
        // order, farthest first.
        let mut cmds = std::mem::take(&mut p.cmds);
        cmds.clear();
        cmds.extend(p.static_opaque.iter().cloned());
        cmds.extend(dyn_opaque);
        cmds.sort_by_key(|c| c.order);
        let mut transparent: Vec<(Cmd, f32)> = p.static_transparent.iter().map(|(c, at)| (c.clone(), at.distance(eye))).collect();
        transparent.extend(dyn_transparent);
        transparent.sort_by(|a, b| a.0.order.cmp(&b.0.order).then(b.1.total_cmp(&a.1)));
        cmds.extend(transparent.into_iter().map(|(c, _)| c));
        p.cmds = cmds;
        let mut shadow_cmds = std::mem::take(&mut p.shadow_cmds);
        shadow_cmds.clear();
        shadow_cmds.extend(p.static_shadow.iter().cloned());
        shadow_cmds.extend(dyn_shadow);
        p.shadow_cmds = shadow_cmds;
        // Overlays: their rectangles in the widget's NDC, their textures uploaded.
        let mut rects: Vec<[f32; 4]> = Vec::with_capacity(f.overlays.len());
        p.overlay_cmds.clear();
        for o in &f.overlays {
            if !p.textures.contains_key(&o.tex.id) {
                p.upload_texture(device, queue, &o.tex);
            }
            used_textures.insert(o.tex.id);
            let (w, h) = (bounds.width.max(1.0), bounds.height.max(1.0));
            rects.push([o.x / w * 2.0 - 1.0, 1.0 - o.y / h * 2.0, (o.x + o.w) / w * 2.0 - 1.0, 1.0 - (o.y + o.h) / h * 2.0]);
            p.overlay_cmds.push(o.tex.id);
        }
        if !rects.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&rects);
            if p.overlay_rects.as_ref().is_none_or(|(_, cap)| *cap < bytes.len()) {
                let cap = bytes.len().next_power_of_two();
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("floor overlays"),
                    size: cap as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                p.overlay_rects = Some((buffer, cap));
            }
            if let Some((buffer, _)) = &p.overlay_rects {
                queue.write_buffer(buffer, 0, bytes);
            }
        }
        p.meshes.retain(|id, _| used_meshes.contains(id) || p.static_meshes.contains(id));
        p.textures.retain(|id, _| used_textures.contains(id) || p.static_textures.contains(id));

        // The rows: the statics' (uploaded when they change), then this frame's dynamic ones.
        let static_bytes = p.static_rows.len() * row_size;
        let need = (static_bytes + dyn_rows.len() * row_size).max(row_size);
        let mut fresh = false;
        if p.rows.as_ref().is_none_or(|(_, cap)| *cap < need) {
            let cap = need.next_power_of_two();
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("floor rows"),
                size: cap as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            p.rows = Some((buffer, cap));
            fresh = true;
        }
        if let Some((buffer, _)) = &p.rows {
            if (fresh || statics_changed) && static_bytes > 0 {
                queue.write_buffer(buffer, 0, bytemuck::cast_slice(&p.static_rows));
            }
            if !dyn_rows.is_empty() {
                queue.write_buffer(buffer, static_bytes as u64, bytemuck::cast_slice(&dyn_rows));
            }
        }
        record(p, device, queue);
        probe_card_corner(p, device, queue);
        if perf_on() {
            let tris: u64 = p
                .cmds
                .iter()
                .filter_map(|c| c.mesh.and_then(|id| p.meshes.get(&id)).map(|m| u64::from(m.count / 3) * u64::from(c.rows.end - c.rows.start)))
                .sum();
            let instances = p.static_rows.len() + dyn_rows.len();
            let static_draws = p.static_opaque.len() + p.static_transparent.len() + p.static_shadow.len();
            perf_frame(started.elapsed(), p.cmds.len() + p.shadow_cmds.len(), static_draws, tris, instances, culled);
        }
    }

    pub fn draw(&self, p: &Pipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        // The host has set the viewport and scissor to the target rectangle: copy the finished picture in.
        if let Some(targets) = &p.targets {
            pass.set_pipeline(&p.blit_pipeline);
            pass.set_bind_group(0, &targets.blit_group, &[]);
            p.count(PASS_BLIT);
            pass.draw(0..3, 0..1);
        }
        // The overlays (labels), at full resolution over the picture, in order.
        if let (Some((rects, _)), false) = (&p.overlay_rects, p.overlay_cmds.is_empty()) {
            pass.set_pipeline(&p.overlay_pipeline);
            pass.set_vertex_buffer(0, rects.slice(..));
            for (i, id) in p.overlay_cmds.iter().enumerate() {
                if let Some(tex) = p.textures.get(id) {
                    pass.set_bind_group(0, &tex.group, &[]);
                    p.count(PASS_OVERLAY);
                    pass.draw(0..6, i as u32..i as u32 + 1);
                }
            }
        }
        if perf_on() {
            perf_passes(p.draw_breakdown());
        }
        true
    }
}

/// Set once the card probe has logged (`APZ_CARD_PROBE`).
static CARD_PROBED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `APZ_CARD_PROBE`: once per process, the corner pixel of a machine card's offscreen target is read back from the GPU
/// after its frame is drawn and before the blit copies it into the page, and logged. A card's corner is empty, so it
/// shows the clear: (0, 0, 0, 0) when the target is transparent.
fn probe_card_corner(p: &Pipeline, device: &wgpu::Device, queue: &wgpu::Queue) {
    if !p.probe_corner || p.clear_alpha >= 1.0 || crate::env_var("APZ_CARD_PROBE").is_none() {
        return;
    }
    let (Some(targets), Some(_)) = (&p.targets, &p.rows) else { return };
    if CARD_PROBED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("card probe"),
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("card probe") });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &targets.resolve_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: None } },
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    if matches!(rx.recv(), Ok(Ok(()))) {
        let px = slice.get_mapped_range();
        eprintln!(
            "agentplaza-desktop: card corner (0,0) of the {}x{} offscreen target before the blit: rgba({}, {}, {}, {}), format {:?}",
            targets.size.0, targets.size.1, px[0], px[1], px[2], px[3], p.format
        );
        drop(px);
        buffer.unmap();
    }
}

/// The shadow and scene passes, on the floor's own encoder, submitted at once: they run before the host's
/// frame, which then only copies the finished picture (`Primitive::draw`). No pass of the host's is split.
fn record(p: &Pipeline, device: &wgpu::Device, queue: &wgpu::Queue) {
    let (Some(targets), Some((rows, _))) = (&p.targets, &p.rows) else {
        return;
    };
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("floor") });
    // No shadow casters (light mode, the Machines view): the pass is skipped, after one that clears what an earlier
    // frame's casters left in the map.
    let casters = !p.shadow_cmds.is_empty();
    if casters || p.shadow_written.swap(casters, std::sync::atomic::Ordering::Relaxed) {
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
                    p.count(PASS_SHADOW);
                    pass.draw_indexed(0..mesh.count, 0, range.clone());
                }
            }
            for _ in 0..p.anim.draw_shadow(&mut pass) {
                p.count(PASS_SHADOW);
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
                // A card's clear is transparent: its sky dome would paint an opaque square behind the machine, so only the meshes draw.
                if p.clear_alpha < 1.0 && cmd.key.prog == Prog::Sky {
                    continue;
                }
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
                        p.count(PASS_MESH);
                        p.count_program(cmd.key.prog);
                        pass.draw_indexed(0..mesh.count, 0, cmd.rows.clone());
                    }
                    None if cmd.key.prog == Prog::Points => {
                        pass.set_vertex_buffer(0, rows.slice(..));
                        p.count(PASS_QUAD);
                        p.count_program(cmd.key.prog);
                        pass.draw(0..6, cmd.rows.clone());
                    }
                    None => {}
                }
            }
            for _ in 0..p.fleet.draw(&mut pass) {
                p.count(PASS_MESH);
            }
            // The fleet's instanced draws leave their own group at 0; the animated hardware reads the floor's globals.
            pass.set_bind_group(0, &p.globals_group, &[]);
            for _ in 0..p.anim.draw(&mut pass) {
                p.count(PASS_MESH);
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
    fn draw_stats_split_the_passes_and_keep_outline_apart_from_the_main_pass() {
        // Passes [shadow, mesh, quad, overlay, blit]; programs [toon, grass, basic, shadow catcher, outline, sky, points].
        let stats = DrawStats::from_counts([1, 10, 4, 3, 1], [2, 1, 3, 0, 4, 0, 4]);
        assert_eq!(stats.total, 19);
        assert_eq!(stats.shadow, 1);
        assert_eq!(stats.outline, 4);
        assert_eq!(stats.main, 6);
        assert_eq!(stats.sprite_label, 7);
        assert_eq!(stats.other, 1);
        assert_eq!(stats.programs.toon, 2);
        assert_eq!(stats.programs.points, 4);
        let json: serde_json::Value = serde_json::from_str(&stats.to_json()).unwrap();
        assert_eq!(json["sprite_label"], 7);
        assert_eq!(json["programs"]["shadow_catcher"], 0);
    }

    #[test]
    fn globals_match_the_shaders_layout() {
        // 4 mat4 + 11 vec4 + 2 x 4 vec4 + 1 vec4 + the fan table + the machine state and status tables + 2 LED colours, all 16-byte aligned.
        assert_eq!(std::mem::size_of::<Globals>(), 4 * 64 + 11 * 16 + 8 * 16 + 16 + MACHINE_VEC4 * 16 + 2 * crate::fleet::MACHINE_SLOTS * 16 + 2 * 16);
        // 112 bytes of material and transform, plus the robot hues (shell, shell2, panel) as three vec4s.
        assert_eq!(std::mem::size_of::<Row>(), 112 + 3 * 16);
    }
}

#[cfg(test)]
mod plaza_fade_tests {
    use super::*;

    #[test]
    fn a_part_over_the_plaza_eases_to_the_cutaway_opacity() {
        // The part's box is clear of the eye-to-focus segment (both at x = 100), but over the plaza area.
        let part = Fade { id: 1, min: Vec3::new(10.0, 0.0, 10.0), max: Vec3::new(12.0, 2.0, 12.0) };
        let (eye, focus) = (Vec3::new(100.0, 50.0, 100.0), Vec3::new(100.0, 0.0, 100.0));
        let plaza = Some((Vec3::new(-20.0, -1.0, -20.0), Vec3::new(20.0, 5.0, 20.0)));
        assert_eq!(fade_target(&part, eye, focus, None), 1.0);
        assert_eq!(fade_target(&part, eye, focus, plaza), CUTAWAY_OPACITY);
        // Eased with the fade's rate over 0.1 s steps, it settles at the cutaway opacity.
        let k = 1.0 - (-0.1f32 * 12.0).exp();
        let mut a = 1.0f32;
        for _ in 0..60 {
            a += (fade_target(&part, eye, focus, plaza) - a) * k;
        }
        assert!((a - 0.15).abs() < 1e-3, "settled at {a}");
    }
}

#[cfg(test)]
mod hw_anim_tests {
    /// The text of WGSL function `name`, from its `fn` through the closing brace at column 0.
    fn wgsl_fn<'a>(src: &'a str, name: &str) -> &'a str {
        let start = src.find(&format!("fn {name}(")).unwrap_or_else(|| panic!("no fn {name}"));
        let close = src[start..].find("\n}\n").unwrap_or_else(|| panic!("fn {name} is not closed"));
        &src[start..start + close + 3]
    }

    #[test]
    fn the_anim_lighting_copies_match_the_floor() {
        // hw_anim.wgsl copies the floor's lighting functions (WGSL has no includes): they must stay identical.
        let floor = include_str!("floor.wgsl");
        let anim = include_str!("shaders/hw_anim.wgsl");
        for name in ["ramp", "sun_shadow", "attenuation", "fogged", "encode"] {
            assert_eq!(wgsl_fn(anim, name), wgsl_fn(floor, name), "hw_anim.wgsl's {name} drifted from floor.wgsl");
        }
    }
}
