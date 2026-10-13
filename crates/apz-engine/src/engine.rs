//! The Floor engine as a host drives it: one object that owns the stage and the pointer state. The host feeds it
//! data ([`Feed`]), pointer events ([`Engine::input`]), actions ([`Engine::apply`]) and commands ([`Cmd`]), then
//! asks for a frame. A host with its own render pass (the desktop's iced shader widget) advances the engine with
//! [`Engine::advance`] and draws with a [`Renderer`]; a host with a plain target (the browser's canvas) calls
//! [`Engine::frame`], which does both. Neither creates a window or a surface here.

use std::sync::Arc;

use serde_json::Value;
use web_time::{Duration, Instant};

use crate::fleet::FleetResponse;
use crate::input::{self, Action, Pointer};
use crate::render;
use crate::scene::{Camera, Frame};
use crate::stage::{machine_at, Motion, Stage, View};
use crate::world::view::Label;
use crate::world::AgentInfo;

/// What the host has fetched since the last call. `None` means "no change" for that source.
#[derive(Debug, Default, Clone)]
pub struct Feed {
    /// `GET /v1/fleet`.
    pub fleet: Option<FleetResponse>,
    /// `GET /v1/agents`.
    pub agents: Option<Vec<AgentInfo>>,
    /// The last fleet request failed with this message.
    pub fleet_error: Option<String>,
}

/// A command from the host's controls: what the Floor screen does on a button or a click outside the scene.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    /// Show the fleet's Machines view.
    ShowMachines,
    /// Show one machine's view by its id.
    SelectMachine(String),
    /// ENTER: fly into the machine with this id.
    Enter(String),
    /// Back out of the machine view (the reverse of ENTER).
    Back,
    /// Reset the camera to the view's default.
    ResetView,
    /// Open the popup of a machine (Machines view), or close it with None.
    OpenPopup(Option<String>),
    /// Show one machine of the feed on the studio stage (its orbit view), by its id.
    ShowMachine { id: String },
    /// Leave the machine stage for the view underneath.
    LeaveMachine,
}

/// What is under a point: a machine in the Machines view, or a robot in a machine's view.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// The id of the machine under the point (the Machines view).
    Machine(String),
    /// The cid of the robot under the point (a machine's view).
    Robot(u64),
}

/// What one frame produced, for the host to draw around the picture and to act on.
#[derive(Debug)]
pub struct FrameOut {
    /// How the host should schedule the next frame: [`Motion::Active`] at the full display rate, [`Motion::Ambient`]
    /// throttled (12 fps), [`Motion::Rest`] not at all. Active covers input in the last 250 ms and any change since
    /// the last frame.
    pub motion: Motion,
    /// The name labels of the shown view, in logical px (the host draws them, or the overlays already do).
    pub labels: Vec<Label>,
    /// The shown view's camera, for the host's own overlays.
    pub camera: Camera,
}

/// The GPU side of the engine: the device and queue it was given, the render pipeline, and the primitive of the
/// last [`Renderer::prepare`]. A host with its own render pass keeps one and calls `prepare` then `draw` in it.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    pipeline: render::Pipeline,
    primitive: Option<render::Primitive>,
    /// The offscreen target of the last thumbnail, reused while its size holds.
    thumb: Option<(u32, u32, wgpu::Texture)>,
    /// The pipeline the thumbnails draw with: the main format in its sRGB form, and a card's transparent clear (built on first use).
    thumb_pipeline: Option<render::Pipeline>,
}

impl Renderer {
    /// Builds the renderer on the host's device. The assets are embedded in the crate.
    /// `wgpu::Device` and `wgpu::Queue` are cheap handles, so the renderer keeps its own clones.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Renderer {
            device: device.clone(),
            queue: queue.clone(),
            format,
            pipeline: render::Pipeline::new(device, queue, format),
            primitive: None,
            thumb: None,
            thumb_pipeline: None,
        }
    }

    /// Builds this frame's work from the stage's frame, for `bounds` (logical px) at `scale` device px per logical px.
    pub fn prepare(&mut self, frame: Arc<Frame>, bounds: render::Rect, scale: f32) {
        let primitive = render::Primitive { frame };
        primitive.prepare(&mut self.pipeline, &self.device, &self.queue, &bounds, scale);
        self.primitive = Some(primitive);
    }

    /// Draws the prepared frame into a pass the host has set up (viewport and scissor on the target rectangle).
    /// Returns false when nothing was prepared yet.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) -> bool {
        match &self.primitive {
            Some(primitive) => primitive.draw(&self.pipeline, pass),
            None => false,
        }
    }

    /// The draw calls of the prepared frame: its scene passes and its host pass.
    pub fn draw_calls(&self) -> u32 {
        self.pipeline.draw_calls()
    }

    /// The draw calls of the prepared frame by pass (see [`render::DRAW_PASSES`]).
    pub fn draw_breakdown(&self) -> [u32; 5] {
        self.pipeline.draw_breakdown()
    }

    /// The draw calls of the prepared frame by category (see [`render::DrawStats`]).
    pub fn draw_stats(&self) -> render::DrawStats {
        self.pipeline.draw_stats()
    }

}

/// The largest side a thumbnail renders at, in device px.
pub const THUMB_MAX: u32 = 512;

/// A thumbnail's target format: the main surface's format in its sRGB form. The shaders write linear colour, and only an
/// sRGB target encodes it, so a linear target reads darker than the live canvas.
fn thumb_format(format: wgpu::TextureFormat) -> wgpu::TextureFormat {
    format.add_srgb_suffix()
}

/// The readback slot a thumbnail's buffer reports into when it is mapped.
#[derive(Default)]
struct Mapped {
    result: Option<Result<(), wgpu::BufferAsyncError>>,
    waker: Option<std::task::Waker>,
}

/// A thumbnail on its way back from the GPU, its readback buffer being mapped. [`Thumbnail::pixels`] resolves it.
pub struct Thumbnail {
    buffer: wgpu::Buffer,
    slot: Arc<std::sync::Mutex<Mapped>>,
    width: u32,
    height: u32,
    /// Bytes per buffer row, padded to wgpu's copy alignment.
    row: u32,
    /// The target is blue-red swapped (a BGRA format), so its pixels are swapped back to RGBA.
    swap_rb: bool,
}

impl Thumbnail {
    /// The RGBA8 pixels, row by row from the top, once the GPU has mapped them. On the browser this await is the only
    /// wait; natively the device must be polled first ([`Renderer::thumbnail`] does both).
    pub async fn pixels(self) -> Result<Vec<u8>, String> {
        let slot = self.slot.clone();
        let result = std::future::poll_fn(|cx| {
            let mut mapped = slot.lock().unwrap_or_else(|e| e.into_inner());
            match mapped.result.take() {
                Some(result) => std::task::Poll::Ready(result),
                None => {
                    mapped.waker = Some(cx.waker().clone());
                    std::task::Poll::Pending
                }
            }
        })
        .await;
        result.map_err(|e| format!("the thumbnail readback failed: {e:?}"))?;
        let (line, rows, row) = (self.width as usize * 4, self.height as usize, self.row as usize);
        let mut out = Vec::with_capacity(line * rows);
        {
            let view = self.buffer.slice(..).get_mapped_range();
            for bytes in view.chunks(row).take(rows) {
                out.extend_from_slice(&bytes[..line]);
            }
        }
        self.buffer.unmap();
        if self.swap_rb {
            for px in out.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        Ok(out)
    }
}

impl Renderer {
    /// Renders one machine's stage pose offscreen at `w` by `h` (each capped at [`THUMB_MAX`]) and starts the readback.
    /// The pixels come from [`Thumbnail::pixels`].
    pub fn thumbnail_begin(&mut self, machine: &crate::machine_stage::MachineStage, w: u32, h: u32) -> Result<Thumbnail, String> {
        let (w, h) = (w.clamp(1, THUMB_MAX), h.clamp(1, THUMB_MAX));
        self.draw_offscreen(machine.thumbnail_frame(w as f32 / h as f32), w, h)
    }

    /// An agent's robot portrait ([`crate::portrait`]) offscreen at `w` by `h` (each capped at [`THUMB_MAX`]). The pixels
    /// come from [`Thumbnail::pixels`].
    pub fn portrait_begin(&mut self, spec: &crate::portrait::PortraitSpec, w: u32, h: u32) -> Result<Thumbnail, String> {
        let (w, h) = (w.clamp(1, THUMB_MAX), h.clamp(1, THUMB_MAX));
        self.draw_offscreen(crate::portrait::frame(spec, w as f32 / h as f32, true), w, h)
    }

    /// One frame drawn offscreen as a card at `w` by `h` (capped at [`THUMB_MAX`]): a transparent clear, the sky left out.
    pub(crate) fn draw_offscreen(&mut self, frame: Frame, w: u32, h: u32) -> Result<Thumbnail, String> {
        let (w, h) = (w.clamp(1, THUMB_MAX), h.clamp(1, THUMB_MAX));
        let format = thumb_format(self.format);
        let frame = Arc::new(frame);
        let bounds = render::Rect { x: 0.0, y: 0.0, width: w as f32, height: h as f32 };
        let primitive = render::Primitive { frame };
        let pipeline = self.thumb_pipeline.get_or_insert_with(|| {
            let mut p = render::Pipeline::new(&self.device, &self.queue, format);
            // A thumbnail is a card: its clear is transparent and its sky is left out, so only the machine and its shadow draw.
            p.clear_alpha = 0.0;
            p
        });
        primitive.prepare(pipeline, &self.device, &self.queue, &bounds, 1.0);

        let texture = match self.thumb.take() {
            Some((tw, th, texture)) if (tw, th) == (w, h) => texture,
            _ => self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("apz-engine thumbnail"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            }),
        };
        self.thumb = Some((w, h, texture.clone()));

        let row = (w * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("apz-engine thumbnail readback"),
            size: u64::from(row) * u64::from(h),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("apz-engine thumbnail") });
        {
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("apz-engine thumbnail pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            primitive.draw(pipeline, &mut pass);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(encoder.finish()));

        let slot = Arc::new(std::sync::Mutex::new(Mapped::default()));
        let sink = slot.clone();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let mut mapped = sink.lock().unwrap_or_else(|e| e.into_inner());
            mapped.result = Some(result);
            if let Some(waker) = mapped.waker.take() {
                waker.wake();
            }
        });
        let swap_rb = matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        Ok(Thumbnail { buffer, slot, width: w, height: h, row, swap_rb })
    }

    /// One machine's thumbnail as RGBA8 bytes, natively: the device is polled until the pixels are mapped.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn thumbnail(&mut self, machine: &crate::machine_stage::MachineStage, w: u32, h: u32) -> Result<Vec<u8>, String> {
        let thumb = self.thumbnail_begin(machine, w, h)?;
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("{e:?}"))?;
        block_ready(thumb.pixels())
    }

    /// An agent's robot portrait as RGBA8 bytes, natively: the device is polled until the pixels are mapped.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn portrait(&mut self, spec: &crate::portrait::PortraitSpec, w: u32, h: u32) -> Result<Vec<u8>, String> {
        let thumb = self.portrait_begin(spec, w, h)?;
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("{e:?}"))?;
        block_ready(thumb.pixels())
    }
}

/// Drives a future that is ready once the GPU has done its work (the native readback, after a device poll).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn block_ready<F: std::future::Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(out) = future.as_mut().poll(&mut cx) {
            return out;
        }
        std::thread::yield_now();
    }
}

/// The engine: the stage (the Floor's state), the pointer state, and the GPU side when there is one.
pub struct Engine {
    /// The Floor's state. The host may read it, and change it then call [`Engine::touch`].
    pub stage: Stage,
    pointer: Pointer,
    gpu: Option<Renderer>,
    /// Something changed since the last advance, so the next frame must be built even if nothing moves.
    dirty: bool,
    /// The last advance asked for another frame.
    busy: bool,
    /// Frames drawn with the GPU.
    frames: u64,
    /// When the last pointer event was applied (the input window keeps the frame rate at Active for a moment).
    last_input: Option<Instant>,
}

/// How long after a pointer event the scene stays at [`Motion::Active`].
const INPUT_WINDOW: Duration = Duration::from_millis(250);

impl Engine {
    /// Builds the engine with a GPU on the host's device (the browser). The assets are embedded in the crate.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Engine { gpu: Some(Renderer::new(device, queue, format)), ..Self::detached() }
    }

    /// Builds the engine with no GPU: the host draws with a [`Renderer`] of its own (the desktop).
    pub fn detached() -> Self {
        Engine {
            stage: Stage::default(),
            pointer: Pointer::default(),
            gpu: None,
            dirty: true,
            busy: false,
            frames: 0,
            last_input: None,
        }
    }

    /// Holds the ambient visuals still (a frozen pose at t=0) and stops their frames: the host sets it from the
    /// platform's reduced-motion preference.
    pub fn set_reduced_motion(&mut self, on: bool) {
        if self.stage.reduced_motion != on {
            self.stage.reduced_motion = on;
            self.dirty = true;
        }
    }

    /// Hands the stage what the host has fetched. Each source applies only when it is `Some`.
    pub fn set_data(&mut self, feed: Feed) {
        if let Some(fleet) = feed.fleet {
            self.stage.apply_fleet(fleet);
        }
        if let Some(agents) = feed.agents {
            self.stage.apply_agents(agents);
        }
        if let Some(error) = feed.fleet_error {
            self.stage.fleet_failed(error);
        }
        self.dirty = true;
    }

    /// Feeds one pointer event. Returns whether the host should capture it (keep it from the widgets beneath).
    pub fn input(&mut self, event: input::Event) -> bool {
        let size = self.stage.size;
        self.last_input = Some(Instant::now());
        match self.pointer.update(size, event) {
            Some((action, captured)) => {
                self.apply(action);
                captured
            }
            None => false,
        }
    }

    /// Applies one action to the stage.
    pub fn apply(&mut self, action: Action) {
        match action {
            Action::Resized { width, height } => self.stage.resize(width, height),
            Action::Rotate { dx, dy, height } => self.stage.rotate(dx, dy, height),
            Action::Pan { dx, dy, height } => self.stage.pan(dx, dy, height),
            Action::Hover(p) => self.stage.hover(p),
            Action::Select(p) => self.stage.select(Some(p)),
            Action::Dolly(steps) => self.stage.dolly(steps),
        }
        self.dirty = true;
    }

    /// Runs a host command on the stage.
    pub fn command(&mut self, cmd: Cmd) {
        if !matches!(cmd, Cmd::ShowMachine { .. }) {
            self.stage.machine = None;
        }
        match cmd {
            Cmd::ShowMachines => self.stage.set_view(View::Machines),
            Cmd::SelectMachine(id) => self.stage.set_view(View::Machine(id)),
            Cmd::Enter(id) => self.stage.enter(id),
            Cmd::Back => self.stage.leave(),
            Cmd::ResetView => self.stage.reset_view(),
            Cmd::OpenPopup(id) => self.stage.open_popup(id),
            Cmd::ShowMachine { id } => {
                self.stage.show_machine(&id);
            }
            Cmd::LeaveMachine => self.stage.leave_machine(),
        }
        self.dirty = true;
    }

    /// Aims the active floor's camera at `target`, from `eye` (or from where it is now, keeping its distance).
    /// The host's debug look: it does not change the scene, only the framing.
    pub fn look_at(&mut self, target: [f32; 3], eye: Option<[f32; 3]>) {
        let target = glam::Vec3::from_array(target);
        let floor = self.stage.active();
        let eye = match eye {
            Some(e) => glam::Vec3::from_array(e),
            None => floor.orbit.camera(1.0).eye,
        };
        floor.orbit.look(eye, target);
        self.touch();
    }

    /// Marks the state as changed, for a host that changed [`Engine::stage`] directly.
    pub fn touch(&mut self) {
        self.dirty = true;
    }

    /// Sets the device pixels per logical pixel the stage's label textures are built at. The web passes the page's
    /// devicePixelRatio; the desktop keeps the stage's default (2.0), which its snapshots were taken at.
    pub fn set_scale(&mut self, scale: f32) {
        if self.stage.scale != scale {
            self.stage.scale = scale;
            self.dirty = true;
        }
    }

    /// Steps the stage to `now` and builds its frame. `size` is the stage's size in logical px. The stage keeps its
    /// own scale (its label textures' device px per logical px), which the desktop has never changed.
    pub fn advance(&mut self, now: Instant, size: (f32, f32)) -> FrameOut {
        if self.stage.size != size {
            self.stage.resize(size.0, size.1);
            self.dirty = true;
        }
        let stage_motion = self.stage.tick(now);
        let recent_input = self.last_input.is_some_and(|t| now.saturating_duration_since(t) < INPUT_WINDOW);
        let motion = if self.dirty || recent_input { stage_motion.max(Motion::Active) } else { stage_motion };
        self.dirty = false;
        self.busy = motion != Motion::Rest;

        if let Some(machine) = self.stage.machine.as_ref() {
            return FrameOut { motion, labels: Vec::new(), camera: machine.camera() };
        }
        let floor = match self.stage.view {
            View::Machines => &self.stage.machines,
            _ => &self.stage.floor,
        };
        let aspect = if size.1 > 0.0 { size.0 / size.1 } else { 1.0 };
        FrameOut { motion, labels: floor.labels(size.0, size.1), camera: floor.orbit.camera(aspect) }
    }

    /// Advances to `now`, then draws the frame into `target` (cleared to transparent) when there is a GPU.
    ///
    /// `size_px` is the target's size in device pixels and `scale` is device pixels per logical pixel (above zero).
    /// The stage works in logical pixels, the same units the pointer reports, so the engine converts once here.
    pub fn frame(&mut self, now: Instant, target: &wgpu::TextureView, size_px: (u32, u32), scale: f32) -> FrameOut {
        let logical = (size_px.0 as f32 / scale, size_px.1 as f32 / scale);
        let out = self.advance(now, logical);
        if let Some(gpu) = self.gpu.as_mut() {
            let bounds = render::Rect { x: 0.0, y: 0.0, width: logical.0, height: logical.1 };
            gpu.prepare(self.stage.frame.clone(), bounds, scale);

            let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("apz-engine frame") });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("apz-engine floor"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                gpu.draw(&mut pass);
            }
            gpu.queue.submit(Some(encoder.finish()));
            self.frames += 1;
        }
        out
    }

    /// Starts the thumbnail of one machine of the feed, rendered offscreen at `w` by `h` ([`Renderer::thumbnail_begin`]).
    /// Err when there is no GPU, the feed has no such machine, or the render fails.
    pub fn thumbnail_begin(&mut self, id: &str, w: u32, h: u32) -> Result<Thumbnail, String> {
        let machine = self.stage.machine_stage_for(id).ok_or_else(|| format!("no machine '{id}' on the feed"))?;
        let gpu = self.gpu.as_mut().ok_or_else(|| "the engine has no GPU".to_string())?;
        gpu.thumbnail_begin(&machine, w, h)
    }

    /// Starts an agent's robot portrait, offscreen at `w` by `h` ([`Renderer::portrait_begin`]). Err when there is no GPU.
    pub fn portrait_begin(&mut self, spec: &crate::portrait::PortraitSpec, w: u32, h: u32) -> Result<Thumbnail, String> {
        let gpu = self.gpu.as_mut().ok_or_else(|| "the engine has no GPU".to_string())?;
        gpu.portrait_begin(spec, w, h)
    }

    /// The frame the stage built at the last advance.
    pub fn scene(&self) -> Arc<Frame> {
        self.stage.frame.clone()
    }

    /// Whether the engine needs another frame: the last advance asked for one, or something changed since.
    pub fn busy(&self) -> bool {
        self.busy || self.dirty
    }

    /// What is under the logical px point `at`: a machine in the Machines view, else a robot of the shown board.
    pub fn pick(&self, at: (f32, f32)) -> Option<Pick> {
        match self.stage.view {
            View::Machines => machine_at(&self.stage, at).map(Pick::Machine),
            _ => {
                let (w, h) = self.stage.size;
                self.stage.floor.pick(at.0, at.1, w, h).map(Pick::Robot)
            }
        }
    }

    /// Where a pick's label sits on the shown view, as (x, y) in logical px from the top-left, or None when it is not on
    /// screen. A machine's label is projected with the Machines' camera, a robot's with the shown board's.
    pub fn screen_of(&self, pick: &Pick) -> Option<(f32, f32)> {
        let (w, h) = self.stage.size;
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        match pick {
            Pick::Machine(id) => {
                let anchor = self.stage.fleet.as_ref()?.label_of(id)?;
                self.stage.machines.orbit.camera(w / h).to_screen(glam::Vec3::from_array(anchor), w, h)
            }
            Pick::Robot(cid) => {
                let floor = &self.stage.floor;
                floor.orbit.camera(w / h).to_screen(floor.anchor_of(*cid)?, w, h)
            }
        }
    }

    /// Frames drawn with the GPU so far.
    pub fn frames_drawn(&self) -> u64 {
        self.frames
    }

    /// The draw calls of the last frame drawn with the GPU (0 without a GPU).
    pub fn draw_calls(&self) -> u32 {
        self.gpu.as_ref().map_or(0, Renderer::draw_calls)
    }

    /// The draw calls of the last frame drawn with the GPU, by pass (see [`render::DRAW_PASSES`]); zeros without a GPU.
    pub fn draw_breakdown(&self) -> [u32; 5] {
        self.gpu.as_ref().map_or([0; 5], Renderer::draw_breakdown)
    }

    /// The draw calls of the last frame drawn with the GPU by category (zeros without a GPU).
    pub fn draw_stats(&self) -> render::DrawStats {
        self.gpu.as_ref().map_or_else(render::DrawStats::default, Renderer::draw_stats)
    }

    /// Seconds since the ENTER sequence began (`Entry::elapsed`), or None when no sequence runs. The debug hook reads it.
    pub fn entry_time(&self) -> Option<f64> {
        self.stage.entry.as_ref().map(|e| e.elapsed())
    }
}

/// The agents from the hub's list: a bare array, or `{ "agents": [...] }`.
pub fn agent_list(v: &Value) -> Vec<AgentInfo> {
    let items = v.as_array().or_else(|| v.get("agents").and_then(Value::as_array)).cloned().unwrap_or_default();
    items
        .iter()
        .filter_map(|a| {
            let text = |key: &str| a.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
            let status = text("status");
            Some(AgentInfo {
                cid: a.get("cid")?.as_u64()?,
                name: text("name"),
                role: text("role"),
                status: if status.is_empty() { "idle".to_string() } else { status },
                tier: text("tier"),
                model: text("model"),
                species: a.get("species").and_then(Value::as_str).map(str::to_string),
            })
        })
        .collect()
}

/// Parses one feed from a JSON object: `fleet` (a fleet response), `agents` (a raw array, as [`agent_list`] reads it)
/// and `fleet_error` (a message). A missing or null key is no change for that source.
pub fn feed_from_json(json: &str) -> Result<Feed, String> {
    let value: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let object = value.as_object().ok_or_else(|| "the feed is not a JSON object".to_string())?;
    let present = |key: &str| object.get(key).filter(|v| !v.is_null());

    let fleet = present("fleet")
        .map(|v| serde_json::from_value::<FleetResponse>(v.clone()).map_err(|e| e.to_string()))
        .transpose()?;
    let agents = present("agents").map(agent_list);
    let fleet_error = present("fleet_error")
        .map(|v| v.as_str().map(str::to_string).ok_or_else(|| "fleet_error is not a string".to_string()))
        .transpose()?;
    Ok(Feed { fleet, agents, fleet_error })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_feed_is_no_change() {
        let feed = Feed::default();
        assert!(feed.fleet.is_none() && feed.agents.is_none() && feed.fleet_error.is_none());
    }

    #[test]
    fn a_null_fleet_is_no_change_and_an_empty_agent_array_is_an_empty_list() {
        let feed = feed_from_json(r#"{"fleet":null,"agents":[]}"#).unwrap();
        assert!(feed.fleet.is_none());
        assert_eq!(feed.agents.map(|agents| agents.len()), Some(0));
    }

    #[test]
    fn a_feed_that_is_not_an_object_is_an_error() {
        assert!(feed_from_json("[1, 2]").is_err());
        assert!(feed_from_json("42").is_err());
    }
}
