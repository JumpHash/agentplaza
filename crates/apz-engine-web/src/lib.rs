//! The browser shell of AgentPlaza's 3D engine: a wasm-bindgen crate that puts `apz-engine` on one canvas.
//!
//! It owns the canvas's wgpu surface (WebGPU first, WebGL2 when WebGPU is missing or asked for), the pointer,
//! wheel and Escape listeners, the resize and visibility observers, and a requestAnimationFrame loop that draws
//! only when the engine asks for a frame. The whole body is compiled for wasm32 only: on the native target this
//! crate is empty, so the workspace's native build compiles it to nothing.
//!
//! The JS entry point is `start(canvas, { backend? })`, which resolves to a `WebEngine`.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use apz_engine::input::{Button, Event};
use apz_engine::world::view::Label;
use apz_engine::{Cmd, Engine, Motion, Pick};
use js_sys::{Array, Float32Array, Function, Object, Reflect, Uint32Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{
    Document, DomRect, Event as DomEvent, HtmlCanvasElement, IntersectionObserver, IntersectionObserverEntry,
    KeyboardEvent, MediaQueryList, PointerEvent, ResizeObserver, Window, WheelEvent,
};
use web_time::{Duration, Instant};

/// A click is a pointerdown and a pointerup at most this far apart (logical px), the engine's own click test.
const CLICK_SLOP: f32 = 4.0;
/// The ambient tier (the throttled frames) draws at most this often: 12 per second.
const AMBIENT_INTERVAL: Duration = Duration::from_micros(1_000_000 / 12);
/// Frames submitted but not yet done on the GPU. A frame waits for an earlier one to finish once this many are in
/// flight: a slow GPU (a software one in the test browser) would otherwise queue frames, and the staging memory of
/// their uploads, until the device runs out (createBuffer throws).
const MAX_FRAMES_IN_FLIGHT: u32 = 2;

/// Which backends `start` may use.
#[derive(Clone, Copy)]
enum Mode {
    Auto,
    WebGl2,
}

/// What the adapter search gives: the device, the queue, the surface and its format and alpha mode.
struct Gpu {
    backend: wgpu::Backend,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    format: wgpu::TextureFormat,
    alpha_mode: wgpu::CompositeAlphaMode,
    /// The instance the surface came from. It must outlive the surface: a dropped instance leaves the canvas with
    /// no live context ("A valid external Instance reference no longer exists") and the frames never reach it.
    instance: wgpu::Instance,
}

/// The state the closures share. The JS callbacks are kept here as plain JS functions and are called only after
/// the borrow of this state ends, so a callback may call back into the engine.
struct Inner {
    window: Window,
    document: Document,
    canvas: HtmlCanvasElement,
    device: wgpu::Device,
    /// Kept alive for the surface (see [`Gpu::instance`]).
    _instance: wgpu::Instance,
    /// None once destroyed (the surface is dropped then).
    surface: Option<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
    backend: wgpu::Backend,
    engine: Engine,
    dpr: f64,
    /// The canvas is on screen (the IntersectionObserver's answer).
    visible: bool,
    reduced_motion: bool,
    /// The motion of the last frame drawn: decides the tier of the next draw.
    last_motion: Motion,
    last_draw: Option<Instant>,
    first_tick: bool,
    stopped: bool,
    /// The loop's JS function, which each tick hands to requestAnimationFrame.
    tick_fn: Option<Function>,
    raf: Option<i32>,
    /// Where the last pointerdown on the canvas was (logical px), for the click test. None after a non-click.
    down: Option<(f32, f32)>,
    /// The labels' text as last sent to the host (their cid, name, role and line).
    text: Vec<Label>,
    /// The flat [cid, x, y, visible] per label of the last frame.
    positions: Vec<f32>,
    on_pick: Option<Function>,
    on_labels: Option<Function>,
    queue: wgpu::Queue,
    /// Frames submitted and not yet done on the GPU (counted down by the queue's completion callbacks).
    in_flight: Arc<AtomicU32>,
}

/// The listeners and observers of one engine, kept so that `destroy` can remove them.
struct Hooks {
    pointerdown: Closure<dyn FnMut(PointerEvent)>,
    pointermove: Closure<dyn FnMut(PointerEvent)>,
    pointerup: Closure<dyn FnMut(PointerEvent)>,
    pointerleave: Closure<dyn FnMut(PointerEvent)>,
    wheel: Closure<dyn FnMut(WheelEvent)>,
    keydown: Closure<dyn FnMut(KeyboardEvent)>,
    /// The reduced-motion query and its change listener, when the browser has the query.
    motion: Option<(MediaQueryList, Closure<dyn FnMut(DomEvent)>)>,
    resize_observer: ResizeObserver,
    resize: Closure<dyn FnMut(Array, ResizeObserver)>,
    visibility_observer: IntersectionObserver,
    visibility: Closure<dyn FnMut(Array, IntersectionObserver)>,
    tick: Closure<dyn FnMut(f64)>,
}

/// One engine on one canvas, held by the page. `destroy` (or dropping it) stops the loop, disconnects the
/// observers, removes every listener and drops the surface.
#[wasm_bindgen]
pub struct WebEngine {
    shared: Rc<RefCell<Inner>>,
    hooks: RefCell<Option<Hooks>>,
}

/// Starts the engine on `canvas`. `opts` is `{ backend?: 'auto' | 'webgl2' }`; a missing backend is 'auto'.
#[wasm_bindgen]
pub async fn start(canvas: HtmlCanvasElement, opts: JsValue) -> Result<WebEngine, JsValue> {
    install_panic_hook();
    let mode = mode_of(&opts)?;
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("there is no window"))?;
    let document = window.document().ok_or_else(|| JsValue::from_str("the window has no document"))?;
    let dpr = window.device_pixel_ratio();
    let gpu = open_gpu(&canvas, &window, mode).await.map_err(|e| JsValue::from_str(&e))?;

    let (width, height) = backing_size(&canvas, dpr);
    canvas.set_width(width);
    canvas.set_height(height);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: gpu.format,
        width,
        height,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: gpu.alpha_mode,
        view_formats: vec![],
    };
    gpu.surface.configure(&gpu.device, &config);
    let engine = Engine::new(&gpu.device, &gpu.queue, gpu.format);

    let shared = Rc::new(RefCell::new(Inner {
        window: window.clone(),
        document,
        canvas: canvas.clone(),
        device: gpu.device,
        _instance: gpu.instance,
        surface: Some(gpu.surface),
        config,
        backend: gpu.backend,
        engine,
        dpr,
        visible: true,
        reduced_motion: false,
        last_motion: Motion::Active,
        last_draw: None,
        first_tick: true,
        stopped: false,
        tick_fn: None,
        raf: None,
        down: None,
        text: Vec::new(),
        positions: Vec::new(),
        on_pick: None,
        on_labels: None,
        queue: gpu.queue,
        in_flight: Arc::new(AtomicU32::new(0)),
    }));

    let hooks = install(&shared, &window, &canvas)?;
    fit(&mut shared.borrow_mut());
    let engine_web = WebEngine { shared, hooks: RefCell::new(Some(hooks)) };
    Ok(engine_web)
}

#[wasm_bindgen]
impl WebEngine {
    /// The backend the engine draws with: 'webgpu' or 'webgl2'.
    pub fn backend(&self) -> String {
        backend_name(self.shared.borrow().backend).to_string()
    }

    /// Hands the engine a feed: the JSON that `apz_engine::feed_from_json` reads. A parse error is thrown as text.
    pub fn set_feed(&self, json: &str) -> Result<(), JsValue> {
        let feed = apz_engine::feed_from_json(json).map_err(|e| JsValue::from_str(&e))?;
        self.shared.borrow_mut().engine.set_data(feed);
        Ok(())
    }

    /// Runs a host command: 'machines', 'select' (arg: machine id), 'enter' (arg: machine id), 'back', 'reset',
    /// 'popup' (arg: machine id, or none or empty to close). Unknown names, and a missing id, are ignored.
    pub fn command(&self, name: &str, arg: Option<String>) {
        let cmd = match (name, arg) {
            ("machines", _) => Cmd::ShowMachines,
            ("select", Some(id)) => Cmd::SelectMachine(id),
            ("enter", Some(id)) => Cmd::Enter(id),
            ("back", _) => Cmd::Back,
            ("reset", _) => Cmd::ResetView,
            ("popup", arg) => Cmd::OpenPopup(arg.filter(|id| !id.is_empty())),
            ("show_machine", Some(id)) => Cmd::ShowMachine { id },
            ("leave_machine", _) => Cmd::LeaveMachine,
            _ => return,
        };
        self.shared.borrow_mut().engine.command(cmd);
    }

    /// Registers `cb(kind, id)`, called on each click in the scene: kind 'machine', 'robot' or 'none', and id the
    /// machine id, the robot's cid as a string, or ''.
    pub fn on_pick(&self, cb: Function) {
        self.shared.borrow_mut().on_pick = Some(cb);
    }

    /// Registers `cb(labels)`, called when the label text changes. `labels` is an array of
    /// `{ cid, name, role, line, hue }`, in the same order as `label_positions`.
    pub fn on_labels(&self, cb: Function) {
        let mut s = self.shared.borrow_mut();
        s.on_labels = Some(cb);
        s.text.clear();
    }

    /// The flat `[cid, x, y, visible]` per label of the last frame, in logical px (visible is 0 or 1). A view onto the
    /// engine's own buffer, not a copy: it is invalid after the next call that writes the labels (the next frame) or
    /// after wasm memory grows, so the host reads it at once.
    pub fn label_positions(&self) -> Float32Array {
        let s = self.shared.borrow();
        // SAFETY: the view borrows `s.positions`, which only `frame_tick` writes, and never while this call runs; the
        // host reads the view before the next engine call, as the doc above says.
        unsafe { Float32Array::view(&s.positions) }
    }

    /// The screen point of a machine's label ('machine', its id) or a robot's label ('robot', its cid as text), as
    /// [x, y] in logical px from the canvas's top-left. Empty when it is not on screen, or the kind or id is unknown.
    pub fn screen_of(&self, kind: &str, id: &str) -> Float32Array {
        let pick = match kind {
            "machine" => Pick::Machine(id.to_string()),
            "robot" => match id.parse::<u64>() {
                Ok(cid) => Pick::Robot(cid),
                Err(_) => return Float32Array::new_with_length(0),
            },
            _ => return Float32Array::new_with_length(0),
        };
        let at = self.shared.borrow().engine.screen_of(&pick);
        match at {
            Some((x, y)) => Float32Array::from(&[x, y][..]),
            None => Float32Array::new_with_length(0),
        }
    }

    /// Whether the engine wants another frame.
    pub fn busy(&self) -> bool {
        self.shared.borrow().engine.busy()
    }

    /// The frames drawn with the GPU so far.
    pub fn frames_drawn(&self) -> f64 {
        self.shared.borrow().engine.frames_drawn() as f64
    }

    /// The draw calls of the last frame.
    pub fn draw_calls(&self) -> u32 {
        self.shared.borrow().engine.draw_calls()
    }

    /// The last drawn frame's draw calls by pass: [shadow, scene mesh, scene quad, overlay, blit] (the order of
    /// `apz_engine::render::DRAW_PASSES`).
    pub fn draw_breakdown(&self) -> Uint32Array {
        Uint32Array::from(&self.shared.borrow().engine.draw_breakdown()[..])
    }

    /// The last drawn frame's draw calls by category, as JSON: `total`, `shadow`, `main`, `outline`, `sprite_label`,
    /// `other`, and `programs` (the mesh and sprite draws by program). See `apz_engine::render::DrawStats`.
    pub fn draw_stats(&self) -> String {
        self.shared.borrow().engine.draw_stats().to_json()
    }

    /// Stops the loop, disconnects the observers, removes every listener and drops the surface.
    pub fn destroy(&self) {
        self.teardown();
    }
}

impl WebEngine {
    fn teardown(&self) {
        let Some(hooks) = self.hooks.borrow_mut().take() else { return };
        let (window, canvas) = {
            let mut guard = self.shared.borrow_mut();
            let s = &mut *guard;
            s.stopped = true;
            if let Some(id) = s.raf.take() {
                let _ = s.window.cancel_animation_frame(id);
            }
            s.surface = None;
            s.on_pick = None;
            s.on_labels = None;
            s.tick_fn = None;
            (s.window.clone(), s.canvas.clone())
        };
        let Hooks {
            pointerdown,
            pointermove,
            pointerup,
            pointerleave,
            wheel,
            keydown,
            motion,
            resize_observer,
            resize: _resize,
            visibility_observer,
            visibility: _visibility,
            tick: _tick,
        } = hooks;
        let _ = canvas.remove_event_listener_with_callback("pointerdown", pointerdown.as_ref().unchecked_ref());
        let _ = canvas.remove_event_listener_with_callback("pointermove", pointermove.as_ref().unchecked_ref());
        let _ = canvas.remove_event_listener_with_callback("pointerup", pointerup.as_ref().unchecked_ref());
        let _ = canvas.remove_event_listener_with_callback("pointerleave", pointerleave.as_ref().unchecked_ref());
        let _ = canvas.remove_event_listener_with_callback("wheel", wheel.as_ref().unchecked_ref());
        let _ = window.remove_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref());
        if let Some((query, change)) = motion {
            let _ = query.remove_event_listener_with_callback("change", change.as_ref().unchecked_ref());
        }
        resize_observer.disconnect();
        visibility_observer.disconnect();
    }
}

impl Drop for WebEngine {
    fn drop(&mut self) {
        self.teardown();
    }
}

/// Sends a Rust panic's message to the console (the default hook on wasm says nothing, and the page sees only "unreachable").
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| web_sys::console::error_1(&JsValue::from_str(&info.to_string()))));
}

/// Reads `opts.backend`: 'auto' (or missing) or 'webgl2'.
fn mode_of(opts: &JsValue) -> Result<Mode, JsValue> {
    if !opts.is_object() {
        return Ok(Mode::Auto);
    }
    let value = Reflect::get(opts, &JsValue::from_str("backend"))?;
    match value.as_string().as_deref() {
        None | Some("auto") => Ok(Mode::Auto),
        Some("webgl2") => Ok(Mode::WebGl2),
        Some(other) => Err(JsValue::from_str(&format!("unknown backend \"{other}\": use 'auto' or 'webgl2'"))),
    }
}

fn backend_name(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::BrowserWebGpu => "webgpu",
        wgpu::Backend::Gl => "webgl2",
        _ => "unknown",
    }
}

/// The GPU for the canvas: WebGPU first when the page has `navigator.gpu`, then WebGL2.
async fn open_gpu(canvas: &HtmlCanvasElement, window: &Window, mode: Mode) -> Result<Gpu, String> {
    match mode {
        Mode::WebGl2 => open_backend(canvas, wgpu::Backends::GL)
            .await?
            .ok_or_else(|| "no WebGL2 adapter is available".to_string()),
        Mode::Auto => {
            let has_webgpu = Reflect::get(&window.navigator(), &JsValue::from_str("gpu"))
                .map(|gpu| !gpu.is_undefined())
                .unwrap_or(false);
            // A canvas bound to a WebGPU context cannot later take a WebGL2 one, so ask for an adapter before any
            // surface exists: no adapter leaves the canvas free for WebGL2.
            if has_webgpu && webgpu_adapter_exists().await {
                if let Some(gpu) = open_backend(canvas, wgpu::Backends::BROWSER_WEBGPU).await? {
                    return Ok(gpu);
                }
            }
            open_backend(canvas, wgpu::Backends::GL)
                .await?
                .ok_or_else(|| "no WebGPU or WebGL2 adapter is available in this browser".to_string())
        }
    }
}

/// Whether the browser gives a WebGPU adapter. Asked without a surface, so a no answer touches no canvas.
async fn webgpu_adapter_exists() -> bool {
    let instance =
        wgpu::Instance::new(&wgpu::InstanceDescriptor { backends: wgpu::Backends::BROWSER_WEBGPU, ..Default::default() });
    instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.is_ok()
}

/// One backend: an instance, the canvas surface, an adapter compatible with it, and a device. None when the
/// backend has no adapter (the caller tries the next one).
async fn open_backend(canvas: &HtmlCanvasElement, backends: wgpu::Backends) -> Result<Option<Gpu>, String> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor { backends, ..Default::default() });
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
        .map_err(|e| format!("cannot create the canvas surface: {e}"))?;
    let options = wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        ..Default::default()
    };
    let Ok(adapter) = instance.request_adapter(&options).await else {
        return Ok(None);
    };
    let backend = adapter.get_info().backend;
    let base = if backend == wgpu::Backend::Gl {
        wgpu::Limits::downlevel_webgl2_defaults()
    } else {
        wgpu::Limits::default()
    };
    let descriptor = wgpu::DeviceDescriptor {
        label: Some("apz-engine-web"),
        required_limits: base.using_resolution(adapter.limits()),
        ..Default::default()
    };
    let (device, queue) = adapter
        .request_device(&descriptor)
        .await
        .map_err(|e| format!("cannot open the GPU device: {e}"))?;
    let caps = surface.get_capabilities(&adapter);
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| f.is_srgb())
        .or_else(|| caps.formats.first().copied())
        .ok_or_else(|| "the canvas surface has no texture formats".to_string())?;
    // The engine paints its own background (the Floor's sky and ground), so the canvas is opaque: a premultiplied
    // canvas shows the colour of any pixel left at zero alpha, which the WebGPU canvas did as white blocks.
    let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
        wgpu::CompositeAlphaMode::Opaque
    } else {
        caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto)
    };
    Ok(Some(Gpu { backend, device, queue, surface, format, alpha_mode, instance }))
}

/// The canvas's backing size: its CSS size times the device pixel ratio, rounded, at least 1 by 1.
fn backing_size(canvas: &HtmlCanvasElement, dpr: f64) -> (u32, u32) {
    let rect: DomRect = canvas.get_bounding_client_rect();
    let width = (rect.width() * dpr).round().max(1.0) as u32;
    let height = (rect.height() * dpr).round().max(1.0) as u32;
    (width, height)
}

/// Fits the canvas to its CSS size at the current device pixel ratio: resizes the backing store and the surface
/// when the size changed, and gives the engine the ratio.
fn fit(s: &mut Inner) {
    let dpr = s.window.device_pixel_ratio();
    let (width, height) = backing_size(&s.canvas, dpr);
    if (width, height) != (s.config.width, s.config.height) {
        s.canvas.set_width(width);
        s.canvas.set_height(height);
        s.config.width = width;
        s.config.height = height;
        if let Some(surface) = &s.surface {
            surface.configure(&s.device, &s.config);
        }
        s.engine.touch();
    }
    s.dpr = dpr;
    s.engine.set_scale(dpr as f32);
}

/// A client point in logical px relative to the canvas.
fn local_point(rect: &DomRect, client_x: f64, client_y: f64) -> (f32, f32) {
    ((client_x - rect.left()) as f32, (client_y - rect.top()) as f32)
}

/// The point when it is inside the canvas, else None.
fn inside(rect: &DomRect, p: (f32, f32)) -> Option<(f32, f32)> {
    let inside_x = (0.0..=rect.width() as f32).contains(&p.0);
    let inside_y = (0.0..=rect.height() as f32).contains(&p.1);
    (inside_x && inside_y).then_some(p)
}

fn button_of(ev: &PointerEvent) -> Button {
    if ev.button() == 0 { Button::Left } else { Button::Other }
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn pick_pair(pick: Option<Pick>) -> (&'static str, String) {
    match pick {
        Some(Pick::Machine(id)) => ("machine", id),
        Some(Pick::Robot(cid)) => ("robot", cid.to_string()),
        None => ("none", String::new()),
    }
}

fn emit_pick(cb: &Function, kind: &str, id: &str) {
    if let Err(e) = cb.call2(&JsValue::NULL, &JsValue::from_str(kind), &JsValue::from_str(id)) {
        web_sys::console::error_1(&e);
    }
}

fn same_text(a: &[Label], b: &[Label]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.cid == y.cid && x.name == y.name && x.role == y.role && x.line == y.line)
}

fn set(object: &Object, key: &str, value: JsValue) {
    let _ = Reflect::set(object, &JsValue::from_str(key), &value);
}

fn label_array(labels: &[Label]) -> Array {
    let array = Array::new();
    for label in labels {
        let object = Object::new();
        set(&object, "cid", JsValue::from_f64(label.cid as f64));
        set(&object, "name", JsValue::from_str(&label.name));
        set(&object, "role", JsValue::from_str(&label.role));
        set(&object, "line", JsValue::from_str(&label.line));
        set(&object, "hue", JsValue::from_f64(label.hue as f64));
        array.push(&object);
    }
    array
}

/// Wires the canvas, window and observers to the shared state.
fn install(shared: &Rc<RefCell<Inner>>, window: &Window, canvas: &HtmlCanvasElement) -> Result<Hooks, JsValue> {
    let pointerdown = {
        let shared = shared.clone();
        Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            let mut guard = shared.borrow_mut();
            let s = &mut *guard;
            let rect = s.canvas.get_bounding_client_rect();
            let p = local_point(&rect, ev.client_x() as f64, ev.client_y() as f64);
            let _ = s.canvas.set_pointer_capture(ev.pointer_id());
            s.down = (ev.button() == 0).then_some(p);
            s.engine.input(Event::Pressed { button: button_of(&ev), at: inside(&rect, p) });
        })
    };
    let pointermove = {
        let shared = shared.clone();
        Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            let mut guard = shared.borrow_mut();
            let s = &mut *guard;
            let rect = s.canvas.get_bounding_client_rect();
            let p = local_point(&rect, ev.client_x() as f64, ev.client_y() as f64);
            s.engine.input(Event::Moved { at: inside(&rect, p), to: p });
        })
    };
    let pointerup = {
        let shared = shared.clone();
        Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            let mut guard = shared.borrow_mut();
            let s = &mut *guard;
            let rect = s.canvas.get_bounding_client_rect();
            let p = local_point(&rect, ev.client_x() as f64, ev.client_y() as f64);
            s.engine.input(Event::Released { button: button_of(&ev), at: inside(&rect, p) });
            let click = match s.down.take() {
                Some(down) => ev.button() == 0 && distance(down, p) <= CLICK_SLOP,
                None => false,
            };
            let call = if click {
                let (kind, id) = pick_pair(s.engine.pick(p));
                s.on_pick.clone().map(|cb| (cb, kind, id))
            } else {
                None
            };
            drop(guard);
            if let Some((cb, kind, id)) = call {
                emit_pick(&cb, kind, &id);
            }
        })
    };
    let pointerleave = {
        let shared = shared.clone();
        Closure::<dyn FnMut(PointerEvent)>::new(move |_ev: PointerEvent| {
            let mut guard = shared.borrow_mut();
            let s = &mut *guard;
            s.down = None;
            s.engine.input(Event::Left);
        })
    };
    let wheel = {
        let shared = shared.clone();
        Closure::<dyn FnMut(WheelEvent)>::new(move |ev: WheelEvent| {
            ev.prevent_default();
            // Web deltaY is positive down; the engine's lines are positive up. Pixel deltas come in about 40 per line.
            let dy = ev.delta_y() as f32;
            let lines = if ev.delta_mode() == 0 { -dy / 40.0 } else { -dy };
            shared.borrow_mut().engine.input(Event::Wheel { lines });
        })
    };
    let keydown = {
        let shared = shared.clone();
        Closure::<dyn FnMut(KeyboardEvent)>::new(move |ev: KeyboardEvent| {
            if ev.key() != "Escape" {
                return;
            }
            let mut guard = shared.borrow_mut();
            let s = &mut *guard;
            let cmd = if s.engine.stage.popup.is_some() { Cmd::OpenPopup(None) } else { Cmd::Back };
            s.engine.command(cmd);
        })
    };
    canvas.add_event_listener_with_callback("pointerdown", pointerdown.as_ref().unchecked_ref())?;
    canvas.add_event_listener_with_callback("pointermove", pointermove.as_ref().unchecked_ref())?;
    canvas.add_event_listener_with_callback("pointerup", pointerup.as_ref().unchecked_ref())?;
    canvas.add_event_listener_with_callback("pointerleave", pointerleave.as_ref().unchecked_ref())?;
    canvas.add_event_listener_with_callback("wheel", wheel.as_ref().unchecked_ref())?;
    window.add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref())?;

    let motion = match window.match_media("(prefers-reduced-motion: reduce)")? {
        Some(query) => {
            shared.borrow_mut().reduced_motion = query.matches();
            let change = {
                let shared = shared.clone();
                let query = query.clone();
                Closure::<dyn FnMut(DomEvent)>::new(move |_ev: DomEvent| {
                    let reduced = query.matches();
                    let mut guard = shared.borrow_mut();
                    let s = &mut *guard;
                    s.reduced_motion = reduced;
                    s.engine.set_reduced_motion(reduced);
                })
            };
            query.add_event_listener_with_callback("change", change.as_ref().unchecked_ref())?;
            Some((query, change))
        }
        None => None,
    };
    if motion.is_some() {
        let reduced = shared.borrow().reduced_motion;
        shared.borrow_mut().engine.set_reduced_motion(reduced);
    }

    let visibility = {
        let shared = shared.clone();
        Closure::<dyn FnMut(Array, IntersectionObserver)>::new(move |entries: Array, _observer: IntersectionObserver| {
            let last = entries
                .iter()
                .filter_map(|entry| entry.dyn_into::<IntersectionObserverEntry>().ok())
                .last();
            if let Some(entry) = last {
                shared.borrow_mut().visible = entry.is_intersecting();
            }
        })
    };
    let visibility_observer = IntersectionObserver::new(visibility.as_ref().unchecked_ref())?;
    visibility_observer.observe(canvas);

    let resize = {
        let shared = shared.clone();
        Closure::<dyn FnMut(Array, ResizeObserver)>::new(move |_entries: Array, _observer: ResizeObserver| {
            fit(&mut shared.borrow_mut());
        })
    };
    let resize_observer = ResizeObserver::new(resize.as_ref().unchecked_ref())?;
    resize_observer.observe(canvas);

    let tick = {
        let shared = shared.clone();
        Closure::<dyn FnMut(f64)>::new(move |_time: f64| frame_tick(&shared))
    };
    let tick_fn: Function = tick.as_ref().unchecked_ref::<Function>().clone();
    {
        let mut s = shared.borrow_mut();
        s.tick_fn = Some(tick_fn.clone());
        s.raf = window.request_animation_frame(&tick_fn).ok();
    }

    Ok(Hooks {
        pointerdown,
        pointermove,
        pointerup,
        pointerleave,
        wheel,
        keydown,
        motion,
        resize_observer,
        resize,
        visibility_observer,
        visibility,
        tick,
    })
}

/// One requestAnimationFrame tick: asks for the next tick first, then draws when the canvas is on screen, the
/// page is visible and the engine is busy, and the tier of the last frame allows it. The first tick always draws.
fn frame_tick(shared: &Rc<RefCell<Inner>>) {
    let mut guard = shared.borrow_mut();
    let s = &mut *guard;
    if s.stopped {
        return;
    }
    if let Some(tick_fn) = &s.tick_fn {
        s.raf = s.window.request_animation_frame(tick_fn).ok();
    }

    let now = Instant::now();
    let first = std::mem::replace(&mut s.first_tick, false);
    // Ambient frames are throttled to 12 per second; with reduced motion they are treated as Rest (no throttle).
    let ambient_wait = s.last_motion == Motion::Ambient
        && !s.reduced_motion
        && s.last_draw.is_some_and(|t| now.saturating_duration_since(t) < AMBIENT_INTERVAL);
    let drawable = s.visible && !s.document.hidden() && !ambient_wait && s.engine.busy();
    if !(first || drawable) || s.in_flight.load(Ordering::SeqCst) >= MAX_FRAMES_IN_FLIGHT {
        return;
    }

    let Some(surface) = s.surface.as_ref() else { return };
    let texture = match surface.get_current_texture() {
        Ok(texture) => texture,
        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
            surface.configure(&s.device, &s.config);
            return;
        }
        Err(_) => {
            web_sys::console::error_1(&JsValue::from_str("apz-engine-web: no surface texture for this frame"));
            return;
        }
    };
    let view = texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
    let out = s.engine.frame(now, &view, (s.canvas.width(), s.canvas.height()), s.dpr as f32);
    drop(view);
    texture.present();
    let pending = Arc::clone(&s.in_flight);
    pending.fetch_add(1, Ordering::SeqCst);
    s.queue.on_submitted_work_done(move || {
        pending.fetch_sub(1, Ordering::SeqCst);
    });

    s.last_motion = out.motion;
    s.last_draw = Some(now);
    s.positions.clear();
    for label in &out.labels {
        let visible = if label.is_shown() { 1.0 } else { 0.0 };
        s.positions.extend_from_slice(&[label.cid as f32, label.x, label.y, visible]);
    }
    let mut labels_call = None;
    if !same_text(&s.text, &out.labels) {
        s.text = out.labels;
        if let Some(cb) = &s.on_labels {
            labels_call = Some((cb.clone(), label_array(&s.text)));
        }
    }
    drop(guard);
    if let Some((cb, array)) = labels_call {
        if let Err(e) = cb.call1(&JsValue::NULL, &array) {
            web_sys::console::error_1(&e);
        }
    }
}
