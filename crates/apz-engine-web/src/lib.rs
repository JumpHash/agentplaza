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
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use apz_engine::input::{Button, Event};
use apz_engine::machine_stage::View;
use apz_engine::style::{Lamp, Style};
use apz_engine::{Cmd, Engine, Motion, Pick};
use js_sys::{Array, Float32Array, Function, Promise, Reflect, Uint8Array, Uint32Array};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{
    Document, DomRect, Event as DomEvent, HtmlCanvasElement, IntersectionObserver,
    IntersectionObserverEntry, KeyboardEvent, MediaQueryList, PointerEvent, ResizeObserver,
    WheelEvent, Window,
};
use web_time::{Duration, Instant};

/// A click is a pointerdown and a pointerup at most this far apart (logical px), the engine's own click test.
const CLICK_SLOP: f32 = 4.0;
/// On a software adapter (SwiftShader, llvmpipe) the ambient tier draws at most this often: 30 per second, since every
/// frame costs the CPU the whole picture. On a GPU every tier draws on every animation frame, at the display's rate
/// (120 Hz and up): a Machines frame costs well under a millisecond (crates/apz-engine/tests/machines_budget.rs).
const AMBIENT_INTERVAL: Duration = Duration::from_micros(1_000_000 / 30);
/// Frames submitted but not yet done on the GPU. A frame waits for an earlier one to finish once this many are in
/// flight: a slow GPU (a software one in the test browser) would otherwise queue frames, and the staging memory of
/// their uploads, until the device runs out (createBuffer throws). Three keep a fast GPU fed above 120 frames a
/// second (two capped WebGL at about 110 on an RTX 3090 with vsync off).
const MAX_FRAMES_IN_FLIGHT: u32 = 3;

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
    /// The adapter the device came from, kept for the same reason: with it dropped, Chrome rejects every buffer map
    /// with that error, so no thumbnail or portrait ever read back (the Machines list and the agents' portraits).
    adapter: wgpu::Adapter,
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
    /// Kept alive for the buffer maps (see [`Gpu::adapter`]).
    _adapter: wgpu::Adapter,
    /// None once destroyed (the surface is dropped then).
    surface: Option<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
    backend: wgpu::Backend,
    engine: Engine,
    dpr: f64,
    /// The canvas is on screen (the IntersectionObserver's answer).
    visible: bool,
    reduced_motion: bool,
    /// The adapter is a software one (a CPU device, SwiftShader or llvmpipe): the ambient frames are throttled.
    software: bool,
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
    on_pick: Option<Function>,
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
    /// The page's visibilitychange listener: starts the loop again when the page shows after it was hidden.
    page_visible: Closure<dyn FnMut(DomEvent)>,
}

/// One engine on one canvas, held by the page. `destroy` (or dropping it) stops the loop, disconnects the
/// observers, removes every listener and drops the surface.
#[wasm_bindgen]
pub struct WebEngine {
    shared: Rc<RefCell<Inner>>,
    hooks: RefCell<Option<Hooks>>,
}

thread_local! {
    /// The kit's files as the page hands them over (`kit_file`), until `kit_ready` gives them to the engine.
    static KIT: RefCell<Vec<(String, Vec<u8>)>> = const { RefCell::new(Vec::new()) };
}

/// Adds one file of the kit: `name` as the kit names it (`hub.glb`, `grille.ktx2`), and its bytes. The page calls it
/// for every file, then `kit_ready`, before the first frame.
#[wasm_bindgen]
pub fn kit_file(name: String, bytes: Vec<u8>) {
    KIT.with(|kit| kit.borrow_mut().push((name, bytes)));
}

/// Gives the engine the files `kit_file` added (`apz_engine::kit::provide`). Throws when the kit was provided already,
/// or when the engine parsed the kit before this call (the page calls it before `start`, `start_lab` or `lab_catalog`).
#[wasm_bindgen]
pub fn kit_ready() -> Result<(), JsValue> {
    let files = KIT.with(|kit| std::mem::take(&mut *kit.borrow_mut()));
    apz_engine::kit::provide(files).map_err(|e| JsValue::from_str(&e))
}

/// The labs' option lists as JSON, from what the engine embeds (`apz_engine::lab_catalog`): the robot species, clips,
/// tools, carried items, props and effects; the hardware models and each model's params. The labs list
/// their controls from it, so the page loads none of the asset files.
#[wasm_bindgen]
pub fn lab_catalog() -> String {
    apz_engine::lab_catalog::catalog_json()
}

/// Starts the engine on `canvas`. `opts` is `{ backend?: 'auto' | 'webgl2' }`; a missing backend is 'auto'. The labs call
/// this too, with their own env overrides already set.
#[wasm_bindgen]
pub async fn start(canvas: HtmlCanvasElement, opts: JsValue) -> Result<WebEngine, JsValue> {
    install_panic_hook();
    let mode = mode_of(&opts)?;
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("there is no window"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("the window has no document"))?;
    let dpr = window.device_pixel_ratio();
    let gpu = open_gpu(&canvas, &window, mode)
        .await
        .map_err(|e| JsValue::from_str(&e))?;

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
        software: is_software(&gpu.adapter.get_info()),
        _adapter: gpu.adapter,
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
        on_pick: None,
        queue: gpu.queue,
        in_flight: Arc::new(AtomicU32::new(0)),
    }));

    let hooks = install(&shared, &window, &canvas)?;
    fit(&mut shared.borrow_mut());
    let engine_web = WebEngine {
        shared,
        hooks: RefCell::new(Some(hooks)),
    };
    Ok(engine_web)
}

/// The robot lab's options and the `APZ_ROBOT_*` name each one sets (read by `robots::lab` when the stage builds).
const LAB_OPTIONS: [(&str, &str); 12] = [
    ("species", "APZ_ROBOT_LAB"),
    ("clip", "APZ_ROBOT_CLIP"),
    ("tool", "APZ_ROBOT_TOOL"),
    ("carry", "APZ_ROBOT_CARRY"),
    ("state", "APZ_ROBOT_STATE"),
    ("hue", "APZ_ROBOT_HUE"),
    ("orch", "APZ_ROBOT_ORCH"),
    ("cam", "APZ_ROBOT_CAM"),
    ("t", "APZ_ROBOT_T"),
    ("bg", "APZ_ROBOT_BG"),
    ("prop", "APZ_ROBOT_PROP"),
    ("fx", "APZ_ROBOT_FX"),
];

/// The env name each of a lab's route options sets (`names`): a string as it is, a boolean as 1 or 0, a number as text.
/// An option that is absent or of another type is left out.
fn lab_env(opts: &JsValue, names: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    for &(key, env) in names {
        let v = Reflect::get(opts, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED);
        let value = if let Some(s) = v.as_string() {
            s
        } else if let Some(b) = v.as_bool() {
            (if b { "1" } else { "0" }).to_string()
        } else if let Some(n) = v.as_f64() {
            n.to_string()
        } else {
            continue;
        };
        vars.insert(env.to_string(), value);
    }
    vars
}

/// Starts the engine as the robot lab: `opts` is `{ backend?, species?, clip?, tool?, carry?, state?, hue?, orch?, cam?, t?, bg? }`
/// (a missing species is `all`; a boolean `orch` is 1 or 0). The options are set as env names only while the stage builds.
#[wasm_bindgen]
pub async fn start_lab(canvas: HtmlCanvasElement, opts: JsValue) -> Result<WebEngine, JsValue> {
    let mut vars = lab_env(&opts, &LAB_OPTIONS);
    vars.entry("APZ_ROBOT_LAB".to_string())
        .or_insert_with(|| "all".to_string());
    // The lab is built when the first fleet arrives (Stage::apply_fleet), after `start` resolves, so the options stay
    // set for the lab engine's life; `destroy` clears them. One engine per page, so nothing else reads them meanwhile.
    apz_engine::set_env_overrides(vars);
    let started = start(canvas, opts).await;
    if started.is_err() {
        apz_engine::set_env_overrides(BTreeMap::new());
    }
    started
}

/// The hardware lab's options, as the route names them; each maps to its APZ_HW_* name in the engine.
const HARDWARE_LAB_OPTIONS: [(&str, &str); 7] = [
    ("model", "APZ_HW_LAB"),
    ("lod", "APZ_HW_LOD"),
    ("style", "APZ_HW_STYLE"),
    ("theme", "APZ_HW_THEME"),
    ("status", "APZ_HW_STATUS"),
    ("cam", "APZ_HW_CAM"),
    ("ortho", "APZ_HW_ORTHO"),
];

/// Starts the engine as the hardware lab: `opts` is `{ backend?, model?, lod?, style?, theme?, status?, cam?, ortho? }`
/// (a missing model is `all`; a boolean `ortho` is 1 or unset: false leaves APZ_HW_ORTHO out). The options are set as env
/// names only while the stage builds, as `start_lab` does.
#[wasm_bindgen]
pub async fn start_hardware_lab(
    canvas: HtmlCanvasElement,
    opts: JsValue,
) -> Result<WebEngine, JsValue> {
    let mut vars = lab_env(&opts, &HARDWARE_LAB_OPTIONS);
    vars.retain(|name, value| name != "APZ_HW_ORTHO" || value == "1");
    vars.entry("APZ_HW_LAB".to_string())
        .or_insert_with(|| "all".to_string());
    apz_engine::set_env_overrides(vars);
    let started = start(canvas, opts).await;
    if started.is_err() {
        apz_engine::set_env_overrides(BTreeMap::new());
    }
    started
}

#[wasm_bindgen]
impl WebEngine {
    /// One machine's stage pose rendered offscreen at `w` by `h` (each at most 512), as RGBA8 bytes. A Promise, since the
    /// readback is async: it resolves once the GPU has copied the pixels back, and rejects for an unknown id.
    pub fn thumbnail(&self, id: String, w: u32, h: u32) -> Promise {
        self.view(id, "iso".to_string(), w, h)
    }

    /// One machine drawn in `view` ('iso', or the flat 'front', 'side', 'back' and 'top': orthographic, square to that
    /// face), offscreen at `w` by `h` (each at most 512), as RGBA8 bytes. Rejects for an unknown id or view.
    pub fn view(&self, id: String, view: String, w: u32, h: u32) -> Promise {
        let started = View::parse(&view)
            .ok_or_else(|| format!("no view '{view}' (iso, front, side, back or top)"))
            .and_then(|view| {
                self.shared
                    .borrow_mut()
                    .engine
                    .thumbnail_begin(&id, view, w, h)
            });
        wasm_bindgen_futures::future_to_promise(async move {
            let thumb = started.map_err(|e| JsValue::from_str(&e))?;
            let pixels = thumb.pixels().await.map_err(|e| JsValue::from_str(&e))?;
            Ok(Uint8Array::from(pixels.as_slice()).into())
        })
    }

    /// An agent's robot portrait, w by h: `json_spec` is the web's portrait spec (`cid`, `roleHue`, `bodyHue`, `orchestrator`,
    /// `offline`, `species`). Resolves to RGBA8 bytes; a bad spec rejects with the reason as text.
    pub fn portrait(&self, json_spec: String, w: u32, h: u32) -> Promise {
        let started = apz_engine::portrait::PortraitSpec::from_json(&json_spec)
            .and_then(|spec| self.shared.borrow_mut().engine.portrait_begin(&spec, w, h));
        wasm_bindgen_futures::future_to_promise(async move {
            let thumb = started.map_err(|e| JsValue::from_str(&e))?;
            let pixels = thumb.pixels().await.map_err(|e| JsValue::from_str(&e))?;
            Ok(Uint8Array::from(pixels.as_slice()).into())
        })
    }

    /// The kit model one machine draws as and its size, as JSON: `{"model": "server", "size_cm": [w, h, d]}` (one kit
    /// unit is 10 cm), or `null` when the feed has no such machine.
    pub fn machine_info(&self, id: String) -> String {
        match self.shared.borrow().engine.stage.machine_model(&id) {
            Some((model, [w, h, d])) => format!(
                "{{\"model\":\"{model}\",\"size_cm\":[{:.0},{:.0},{:.0}]}}",
                w * 10.0,
                h * 10.0,
                d * 10.0
            ),
            None => "null".to_string(),
        }
    }

    /// Each GPU of one machine with the reference card it matched, as a JSON array of `{gpu, vendor, memory_mib, card}`
    /// (`card`: `{id, name, segment, form, slots, size_mm, cooling, fans, generic}`, or null when no card matched), or
    /// `null` when the feed has no such machine.
    pub fn gpu_cards(&self, id: String) -> String {
        match self.shared.borrow().engine.stage.machine_cards(&id) {
            Some(cards) => serde_json::Value::from(cards).to_string(),
            None => "null".to_string(),
        }
    }

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

    /// Runs a host command: 'reset', 'popup' (arg: machine id, or none or empty to close), 'show_machine' (arg: machine
    /// id), 'leave_machine'. Unknown names, and a missing id, are ignored.
    pub fn command(&self, name: &str, arg: Option<String>) {
        let cmd = match (name, arg) {
            ("reset", _) => Cmd::ResetView,
            ("popup", arg) => Cmd::OpenPopup(arg.filter(|id| !id.is_empty())),
            ("show_machine", Some(id)) => Cmd::ShowMachine { id },
            ("leave_machine", _) => Cmd::LeaveMachine,
            _ => return,
        };
        self.shared.borrow_mut().engine.command(cmd);
    }

    /// Draws the Machines view in the style `style` ('ink', 'blueprint' or 'night', any case) for a dark or light
    /// console theme, and asks for a frame. An unknown style name keeps the current one.
    pub fn set_style(&self, style: String, dark: bool) {
        let mut s = self.shared.borrow_mut();
        let style = Style::parse(&style).unwrap_or(s.engine.stage.style);
        s.engine.set_style(style, dark);
    }

    /// The four status lamps' colours in the stage's palette, as a JSON array of CSS hex colours ("#rrggbb"): Working,
    /// Idle, Strained, Offline.
    pub fn lamps(&self) -> String {
        let palette = self.shared.borrow().engine.stage.palette();
        let hex: Vec<String> = [Lamp::Working, Lamp::Idle, Lamp::Strained, Lamp::Offline]
            .into_iter()
            .map(|lamp| {
                let [r, g, b] = palette.lamp(lamp).to_srgb_u8();
                format!("\"#{r:02x}{g:02x}{b:02x}\"")
            })
            .collect();
        format!("[{}]", hex.join(","))
    }

    /// Aims the camera at (x, y, z) from `dist` away, at polar angle `polar` from +Y and `azimuth` (radians, as the
    /// web's `__apzLookAt`: an eye at target + spherical(dist, polar, azimuth), three.js convention).
    pub fn look_at(&self, x: f32, y: f32, z: f32, dist: f32, polar: f32, azimuth: f32) {
        let (sp, cp) = polar.sin_cos();
        let (sa, ca) = azimuth.sin_cos();
        let eye = [x + dist * sp * sa, y + dist * cp, z + dist * sp * ca];
        self.shared
            .borrow_mut()
            .engine
            .look_at([x, y, z], Some(eye));
    }

    /// Registers `cb(kind, id)`, called on each click in the scene: kind 'machine' or 'none', and id the machine id,
    /// or ''.
    pub fn on_pick(&self, cb: Function) {
        self.shared.borrow_mut().on_pick = Some(cb);
    }

    /// The screen point of a machine ('machine', its id), as [x, y] in logical px from the canvas's top-left. Empty
    /// when it is not on screen, or the kind or id is unknown.
    pub fn screen_of(&self, kind: &str, id: &str) -> Float32Array {
        if kind != "machine" {
            return Float32Array::new_with_length(0);
        }
        let at = self
            .shared
            .borrow()
            .engine
            .screen_of(&Pick::Machine(id.to_string()));
        match at {
            Some((x, y)) => Float32Array::from(&[x, y][..]),
            None => Float32Array::new_with_length(0),
        }
    }

    /// Whether the engine wants another frame.
    pub fn busy(&self) -> bool {
        self.shared.borrow().engine.busy()
    }

    /// The shown view's scene graph, JSON (`__apzScene`: three's Scene in the retired build, see `apz_engine::debug`).
    pub fn scene_json(&self) -> String {
        self.shared.borrow_mut().engine.stage.scene_json()
    }

    /// The shown view's name, as `__apzPerf` reports it.
    pub fn view_name(&self) -> String {
        self.shared.borrow().engine.stage.view_name()
    }

    /// The shown view's camera as JSON (`__apzCamera`), for developers and the parity checks.
    pub fn camera_json(&self) -> String {
        self.shared.borrow_mut().engine.stage.camera_json()
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

    /// The lab's readout as JSON (`Stage::lab_info`), or `{}` when no lab runs. Read by the page after the engine starts.
    pub fn lab_info(&self) -> String {
        self.shared.borrow().engine.stage.lab_info()
    }

    /// The last drawn frame's draw calls by category, as JSON: `total`, `shadow`, `main`, `outline`, `sprite_label`,
    /// `other`, and `programs` (the mesh and sprite draws by program). See `apz_engine::render::DrawStats`.
    pub fn draw_stats(&self) -> String {
        self.shared.borrow().engine.draw_stats().to_json()
    }

    /// The robot lab's draw counts of the last drawn frame, as JSON (`apz_engine::robots::lab::DrawCounts`): `props`,
    /// `robots` (each with its `species` and its main-pass `calls`), and `calls` (the main pass, the whole frame). `{}`
    /// when no robot lab runs.
    pub fn lab_draws(&self) -> String {
        match self.shared.borrow().engine.lab_draws() {
            Some(counts) => serde_json::to_string(&counts).unwrap_or_else(|_| "{}".to_string()),
            None => "{}".to_string(),
        }
    }

    /// Stops the loop, disconnects the observers, removes every listener and drops the surface.
    pub fn destroy(&self) {
        self.teardown();
        // A lab engine's options (start_lab) end with it.
        apz_engine::set_env_overrides(BTreeMap::new());
    }
}

impl WebEngine {
    fn teardown(&self) {
        let Some(hooks) = self.hooks.borrow_mut().take() else {
            return;
        };
        let (window, canvas) = {
            let mut guard = self.shared.borrow_mut();
            let s = &mut *guard;
            s.stopped = true;
            if let Some(id) = s.raf.take() {
                let _ = s.window.cancel_animation_frame(id);
            }
            s.surface = None;
            s.on_pick = None;
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
            page_visible,
            tick: _tick,
        } = hooks;
        let _ = canvas.remove_event_listener_with_callback(
            "pointerdown",
            pointerdown.as_ref().unchecked_ref(),
        );
        let _ = canvas.remove_event_listener_with_callback(
            "pointermove",
            pointermove.as_ref().unchecked_ref(),
        );
        let _ = canvas
            .remove_event_listener_with_callback("pointerup", pointerup.as_ref().unchecked_ref());
        let _ = canvas.remove_event_listener_with_callback(
            "pointerleave",
            pointerleave.as_ref().unchecked_ref(),
        );
        let _ = canvas.remove_event_listener_with_callback("wheel", wheel.as_ref().unchecked_ref());
        let _ =
            window.remove_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref());
        if let Some((query, change)) = motion {
            let _ = query
                .remove_event_listener_with_callback("change", change.as_ref().unchecked_ref());
        }
        resize_observer.disconnect();
        visibility_observer.disconnect();
        if let Some(document) = window.document() {
            let _ = document.remove_event_listener_with_callback(
                "visibilitychange",
                page_visible.as_ref().unchecked_ref(),
            );
        }
    }
}

impl Drop for WebEngine {
    fn drop(&mut self) {
        self.teardown();
    }
}

/// Sends a Rust panic's message to the console (the default hook on wasm says nothing, and the page sees only "unreachable").
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&JsValue::from_str(&info.to_string()))
    }));
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
        Some(other) => Err(JsValue::from_str(&format!(
            "unknown backend \"{other}\": use 'auto' or 'webgl2'"
        ))),
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
            if has_webgpu
                && webgpu_adapter_exists().await
                && let Some(gpu) = open_backend(canvas, wgpu::Backends::BROWSER_WEBGPU).await?
            {
                return Ok(gpu);
            }
            open_backend(canvas, wgpu::Backends::GL)
                .await?
                .ok_or_else(|| {
                    "no WebGPU or WebGL2 adapter is available in this browser".to_string()
                })
        }
    }
}

/// Whether the browser gives a WebGPU adapter. Asked without a surface, so a no answer touches no canvas; and asked of
/// `navigator.gpu` itself, not through a second wgpu instance: dropping one left Chrome's WebGPU without "a valid
/// external Instance reference", and every buffer map after it failed (no thumbnail or portrait read back).
async fn webgpu_adapter_exists() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let Ok(gpu) = Reflect::get(&window.navigator(), &JsValue::from_str("gpu")) else {
        return false;
    };
    let Ok(request) = Reflect::get(&gpu, &JsValue::from_str("requestAdapter"))
        .and_then(|f| f.dyn_into::<Function>())
    else {
        return false;
    };
    let Ok(promise) = request.call0(&gpu).and_then(|p| p.dyn_into::<Promise>()) else {
        return false;
    };
    matches!(wasm_bindgen_futures::JsFuture::from(promise).await, Ok(adapter) if !adapter.is_null() && !adapter.is_undefined())
}

/// One backend: an instance, the canvas surface, an adapter compatible with it, and a device. None when the
/// backend has no adapter (the caller tries the next one).
async fn open_backend(
    canvas: &HtmlCanvasElement,
    backends: wgpu::Backends,
) -> Result<Option<Gpu>, String> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends,
        ..Default::default()
    });
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
        caps.alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto)
    };
    Ok(Some(Gpu {
        backend,
        device,
        queue,
        surface,
        format,
        alpha_mode,
        instance,
        adapter,
    }))
}

/// Whether an adapter draws on the CPU: its device type says so, or its name is a known software rasteriser (WebGL
/// reports no device type).
fn is_software(info: &wgpu::AdapterInfo) -> bool {
    let name = info.name.to_ascii_lowercase();
    info.device_type == wgpu::DeviceType::Cpu
        || ["swiftshader", "llvmpipe", "softpipe", "software"]
            .iter()
            .any(|s| name.contains(s))
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
    (
        (client_x - rect.left()) as f32,
        (client_y - rect.top()) as f32,
    )
}

/// The point when it is inside the canvas, else None.
fn inside(rect: &DomRect, p: (f32, f32)) -> Option<(f32, f32)> {
    let inside_x = (0.0..=rect.width() as f32).contains(&p.0);
    let inside_y = (0.0..=rect.height() as f32).contains(&p.1);
    (inside_x && inside_y).then_some(p)
}

fn button_of(ev: &PointerEvent) -> Button {
    if ev.button() == 0 {
        Button::Left
    } else {
        Button::Other
    }
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn pick_pair(pick: Option<Pick>) -> (&'static str, String) {
    match pick {
        Some(Pick::Machine(id)) => ("machine", id),
        None => ("none", String::new()),
    }
}

fn emit_pick(cb: &Function, kind: &str, id: &str) {
    if let Err(e) = cb.call2(
        &JsValue::NULL,
        &JsValue::from_str(kind),
        &JsValue::from_str(id),
    ) {
        web_sys::console::error_1(&e);
    }
}

/// Wires the canvas, window and observers to the shared state.
fn install(
    shared: &Rc<RefCell<Inner>>,
    window: &Window,
    canvas: &HtmlCanvasElement,
) -> Result<Hooks, JsValue> {
    let pointerdown = {
        let shared = shared.clone();
        Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            let mut guard = shared.borrow_mut();
            let s = &mut *guard;
            let rect = s.canvas.get_bounding_client_rect();
            let p = local_point(&rect, ev.client_x() as f64, ev.client_y() as f64);
            let _ = s.canvas.set_pointer_capture(ev.pointer_id());
            s.down = (ev.button() == 0).then_some(p);
            s.engine.input(Event::Pressed {
                button: button_of(&ev),
                at: inside(&rect, p),
            });
        })
    };
    let pointermove = {
        let shared = shared.clone();
        Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            let mut guard = shared.borrow_mut();
            let s = &mut *guard;
            let rect = s.canvas.get_bounding_client_rect();
            let p = local_point(&rect, ev.client_x() as f64, ev.client_y() as f64);
            s.engine.input(Event::Moved {
                at: inside(&rect, p),
                to: p,
            });
        })
    };
    let pointerup = {
        let shared = shared.clone();
        Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            let mut guard = shared.borrow_mut();
            let s = &mut *guard;
            let rect = s.canvas.get_bounding_client_rect();
            let p = local_point(&rect, ev.client_x() as f64, ev.client_y() as f64);
            s.engine.input(Event::Released {
                button: button_of(&ev),
                at: inside(&rect, p),
            });
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
            let lines = if ev.delta_mode() == 0 {
                -dy / 40.0
            } else {
                -dy
            };
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
            if s.engine.stage.popup.is_some() {
                s.engine.command(Cmd::OpenPopup(None));
            } else if s.engine.stage.machine.is_some() {
                s.engine.command(Cmd::LeaveMachine);
            }
        })
    };
    canvas.add_event_listener_with_callback("pointerdown", pointerdown.as_ref().unchecked_ref())?;
    canvas.add_event_listener_with_callback("pointermove", pointermove.as_ref().unchecked_ref())?;
    canvas.add_event_listener_with_callback("pointerup", pointerup.as_ref().unchecked_ref())?;
    canvas
        .add_event_listener_with_callback("pointerleave", pointerleave.as_ref().unchecked_ref())?;
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
        Closure::<dyn FnMut(Array, IntersectionObserver)>::new(
            move |entries: Array, _observer: IntersectionObserver| {
                let last = entries
                    .iter()
                    .filter_map(|entry| entry.dyn_into::<IntersectionObserverEntry>().ok())
                    .next_back();
                if let Some(entry) = last {
                    shared.borrow_mut().visible = entry.is_intersecting();
                }
            },
        )
    };
    let visibility_observer = IntersectionObserver::new(visibility.as_ref().unchecked_ref())?;
    visibility_observer.observe(canvas);

    let resize = {
        let shared = shared.clone();
        Closure::<dyn FnMut(Array, ResizeObserver)>::new(
            move |_entries: Array, _observer: ResizeObserver| {
                fit(&mut shared.borrow_mut());
            },
        )
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

    // The page shows again: the loop that stopped while it was hidden starts once more (a no-op while a tick is queued).
    let page_visible = {
        let shared = shared.clone();
        Closure::<dyn FnMut(DomEvent)>::new(move |_ev: DomEvent| {
            let mut guard = shared.borrow_mut();
            let s = &mut *guard;
            if !s.stopped && !s.document.hidden() {
                request_tick(s);
            }
        })
    };
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("the window has no document"))?;
    document.add_event_listener_with_callback(
        "visibilitychange",
        page_visible.as_ref().unchecked_ref(),
    )?;

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
        page_visible,
        tick,
    })
}

/// Queues the next requestAnimationFrame tick unless one is queued already. `raf` is the pending flag: Some while a
/// tick is queued and cleared when it runs, so there is never more than one loop.
fn request_tick(s: &mut Inner) {
    if s.raf.is_some() {
        return;
    }
    if let Some(tick_fn) = &s.tick_fn {
        s.raf = s.window.request_animation_frame(tick_fn).ok();
    }
}

/// One requestAnimationFrame tick: asks for the next tick first, then draws when the canvas is on screen, the
/// page is visible and the engine is busy, and the tier of the last frame allows it. The first tick always draws.
/// A hidden page queues nothing: the loop stops here, and the visibilitychange listener starts it again.
fn frame_tick(shared: &Rc<RefCell<Inner>>) {
    let mut guard = shared.borrow_mut();
    let s = &mut *guard;
    // This tick has run, so none is queued now.
    s.raf = None;
    let hidden = s.document.hidden();
    if s.stopped || hidden {
        return;
    }
    request_tick(s);

    let now = Instant::now();
    let first = std::mem::replace(&mut s.first_tick, false);
    // On a software adapter the ambient frames are throttled to 30 per second; with reduced motion they are treated as
    // Rest (no throttle). On a GPU nothing waits: every animation frame draws.
    let ambient_wait = s.software
        && s.last_motion == Motion::Ambient
        && !s.reduced_motion
        && s.last_draw
            .is_some_and(|t| now.saturating_duration_since(t) < AMBIENT_INTERVAL);
    let drawable = s.visible && !hidden && !ambient_wait && s.engine.busy();
    if s.in_flight.load(Ordering::SeqCst) >= MAX_FRAMES_IN_FLIGHT {
        // On WebGL, wgpu runs the work-done callbacks only when it maintains the queue: at a submit or a poll. With the
        // frames held back here nothing would submit again, and the view would freeze for good, so poll for them.
        let _ = s.device.poll(wgpu::PollType::Poll);
    }
    if !(first || drawable) || s.in_flight.load(Ordering::SeqCst) >= MAX_FRAMES_IN_FLIGHT {
        return;
    }

    let Some(surface) = s.surface.as_ref() else {
        return;
    };
    let texture = match surface.get_current_texture() {
        Ok(texture) => texture,
        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
            surface.configure(&s.device, &s.config);
            return;
        }
        Err(_) => {
            web_sys::console::error_1(&JsValue::from_str(
                "apz-engine-web: no surface texture for this frame",
            ));
            return;
        }
    };
    let view = texture
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    // The surface config has the canvas's backing size: start and fit set both together.
    let out = s
        .engine
        .frame(now, &view, (s.config.width, s.config.height), s.dpr as f32);
    drop(view);
    texture.present();
    let pending = Arc::clone(&s.in_flight);
    pending.fetch_add(1, Ordering::SeqCst);
    s.queue.on_submitted_work_done(move || {
        pending.fetch_sub(1, Ordering::SeqCst);
    });

    s.last_motion = out.motion;
    s.last_draw = Some(now);
}
