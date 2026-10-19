//! The theme's background under the consoles: one full-window pass of the shared backdrop shader
//! (`apz_engine::backdrop`), drawn beneath the shell so the page shows the theme's ground, pattern and pointer
//! light. A plain background draws nothing and the page keeps its paper. Where the renderer is tiny-skia there is
//! no GPU pass, so one still frame at half resolution stands in, and no frames are ticked.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use apz_engine::backdrop::{Backdrop, MAX_FPS, animates, render_cpu, uniforms};
use apz_theme::Resolved;
use iced::widget::shader::{self, Viewport};
use iced::widget::{image, responsive};
use iced::{ContentFit, Element, Length, Point, Rectangle, Size, Subscription, mouse, wgpu};

/// Whether a backdrop is on screen for the view being built. The page then keeps no ground of its own (see
/// `theme::paper`), so the backdrop shows through.
static DRAWN: AtomicBool = AtomicBool::new(false);

/// Whether the backdrop is drawn beneath the page now.
pub fn drawn() -> bool {
    DRAWN.load(Ordering::Relaxed)
}

/// Clears the backdrop flag. The console calls it at the start of each view, so a view without the backdrop (the
/// chooser, the wizard) gets the page paper back.
pub fn clear() {
    DRAWN.store(false, Ordering::Relaxed);
}

/// The backdrop's state: when its clock started, the pointer over the window, and whether the renderer is the
/// tiny-skia fallback (which cannot run the shader).
pub struct State {
    start: Instant,
    pointer: Option<Point>,
    cpu: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            start: Instant::now(),
            pointer: None,
            cpu: false,
        }
    }
}

/// The backdrop's messages: a frame tick for a moving background, and the pointer over the window (None once it
/// leaves).
#[derive(Debug, Clone)]
pub enum Msg {
    Tick,
    Pointer(Option<Point>),
}

/// Records the renderer the window runs on. The tiny-skia backend is named in the backend string, and it draws
/// the still frame instead of the shader.
pub fn renderer(state: &mut State, backend: &str) {
    state.cpu = backend.to_ascii_lowercase().contains("skia");
}

/// Applies a backdrop message. A tick needs no state: it only makes the view draw the next frame.
pub fn update(state: &mut State, msg: Msg) {
    if let Msg::Pointer(at) = msg {
        state.pointer = at;
    }
}

/// The theme's background when it is drawn: every background but Plain.
fn spec() -> Option<&'static Resolved> {
    let resolved = crate::theme::resolved();
    (resolved.background.id != "plain").then_some(resolved)
}

/// The frame ticks a moving backdrop needs: 30 a second while the background moves or lights under the pointer and
/// the window has focus (`focused` is false while it is unfocused or minimised); none otherwise and none on the
/// still-frame renderer.
pub fn subscription(state: &State, focused: bool) -> Subscription<Msg> {
    match spec() {
        Some(resolved) if focused && !state.cpu && animates(&resolved.background) => {
            iced::time::every(Duration::from_millis((1000.0 / MAX_FPS) as u64)).map(|_| Msg::Tick)
        }
        _ => Subscription::none(),
    }
}

/// The backdrop layer to draw beneath the shell, or None when the background is Plain (the page then keeps its
/// paper). `wrap` maps the backdrop's messages into the host's.
pub fn layer<'a, M: 'a>(state: &'a State, wrap: fn(Msg) -> M) -> Option<Element<'a, M>> {
    let resolved = spec()?;
    DRAWN.store(true, Ordering::Relaxed);
    if state.cpu {
        return Some(still(resolved));
    }
    let time = if animates(&resolved.background) {
        state.start.elapsed().as_secs_f32()
    } else {
        0.0
    };
    let program = BackdropProgram {
        resolved,
        time,
        pointer: state.pointer,
        pointer_on: resolved.background.pointer,
        wrap,
    };
    Some(
        iced::widget::shader(program)
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
    )
}

/// The still frame for the tiny-skia renderer: the backdrop at half the window's size, stretched over the window.
/// It is drawn at the window's layout, so it follows a resize.
fn still<'a, M: 'a>(resolved: &'static Resolved) -> Element<'a, M> {
    responsive(move |size: Size| -> Element<'a, M> {
        let width = ((size.width * 0.5).max(1.0)).round() as u32;
        let height = ((size.height * 0.5).max(1.0)).round() as u32;
        // Half the logical size at half scale: the pattern cells and the pointer keep their css px.
        let u = uniforms(
            &resolved.background,
            &resolved.tokens,
            width as f32,
            height as f32,
            0.5,
            0.0,
            None,
        );
        let pixels = render_cpu(&u, width, height);
        image(image::Handle::from_rgba(width, height, pixels))
            .content_fit(ContentFit::Fill)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    })
    .into()
}

/// The shader program: draws the backdrop of the theme in use, and reports the pointer over the window while the
/// background's pointer light is on.
struct BackdropProgram<M> {
    resolved: &'static Resolved,
    time: f32,
    pointer: Option<Point>,
    pointer_on: bool,
    wrap: fn(Msg) -> M,
}

impl<M> shader::Program<M> for BackdropProgram<M> {
    type State = ();
    type Primitive = BackdropPrimitive;

    fn update(
        &self,
        _state: &mut (),
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<shader::Action<M>> {
        if !self.pointer_on {
            return None;
        }
        let now = match event {
            iced::Event::Mouse(mouse::Event::CursorMoved { .. }) => cursor.position_in(bounds),
            iced::Event::Mouse(mouse::Event::CursorLeft) => None,
            _ => return None,
        };
        // Only a change is reported, so a still pointer does not rebuild the view.
        (now != self.pointer).then(|| shader::Action::publish((self.wrap)(Msg::Pointer(now))))
    }

    fn draw(&self, _state: &(), _cursor: mouse::Cursor, _bounds: Rectangle) -> BackdropPrimitive {
        BackdropPrimitive {
            resolved: self.resolved,
            time: self.time,
            pointer: self.pointer.filter(|_| self.pointer_on).map(|p| (p.x, p.y)),
        }
    }
}

/// The backdrop's wgpu pipeline, held by iced for the shader widget.
pub struct BackdropPipeline(Backdrop);

impl shader::Pipeline for BackdropPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        BackdropPipeline(Backdrop::new(device, format))
    }
}

/// One frame of the backdrop: the theme's spec, and the clock and pointer it is drawn at.
#[derive(Debug, Clone)]
pub struct BackdropPrimitive {
    resolved: &'static Resolved,
    time: f32,
    pointer: Option<(f32, f32)>,
}

impl shader::Primitive for BackdropPrimitive {
    type Pipeline = BackdropPipeline;

    fn prepare(
        &self,
        pipeline: &mut BackdropPipeline,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        // The target is the widget's bounds in device px; the pointer and the layout stay in css px.
        let scale = viewport.scale_factor();
        let u = uniforms(
            &self.resolved.background,
            &self.resolved.tokens,
            bounds.width * scale,
            bounds.height * scale,
            scale,
            self.time,
            self.pointer,
        );
        pipeline.0.prepare(queue, &u);
    }

    fn draw(&self, pipeline: &BackdropPipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        pipeline.0.draw(pass);
        true
    }
}
