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
    pipeline: render::Pipeline,
    primitive: Option<render::Primitive>,
}

impl Renderer {
    /// Builds the renderer on the host's device. The assets are embedded in the crate.
    /// `wgpu::Device` and `wgpu::Queue` are cheap handles, so the renderer keeps its own clones.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Renderer {
            device: device.clone(),
            queue: queue.clone(),
            pipeline: render::Pipeline::new(device, queue, format),
            primitive: None,
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
        match cmd {
            Cmd::ShowMachines => self.stage.set_view(View::Machines),
            Cmd::SelectMachine(id) => self.stage.set_view(View::Machine(id)),
            Cmd::Enter(id) => self.stage.enter(id),
            Cmd::Back => self.stage.leave(),
            Cmd::ResetView => self.stage.reset_view(),
            Cmd::OpenPopup(id) => self.stage.open_popup(id),
        }
        self.dirty = true;
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
