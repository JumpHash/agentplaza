//! The floor's iced adapter: the shader widget's program. It maps iced's pointer events into the
//! engine's [`Pointer`] input, turns the engine's actions into the console's messages, and draws the
//! engine's [`Frame`] through the engine's [`Renderer`].

use std::sync::Arc;

use apz_engine::input::{Action, Button, Event, Pointer};
use apz_engine::render;
use apz_engine::scene::Frame;
use apz_engine::Renderer;
use iced::widget::shader::{self, Viewport};
use iced::{mouse, wgpu, Point, Rectangle};

use crate::console::screens::floor::Msg;

/// The shader widget's program: draws the frame the screen built, turns pointer input into messages.
pub struct FloorProgram {
    pub primitive: Arc<Frame>,
}

fn button(b: mouse::Button) -> Button {
    match b {
        mouse::Button::Left => Button::Left,
        _ => Button::Other,
    }
}

impl shader::Program<Msg> for FloorProgram {
    type State = Pointer;
    type Primitive = FloorPrimitive;

    fn update(
        &self,
        st: &mut Pointer,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<shader::Action<Msg>> {
        let at = cursor.position_in(bounds).map(|p| (p.x, p.y));
        let ev = match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(b)) => Event::Pressed { button: button(*b), at },
            // The window-relative position, moved into the widget's space: the drag math only uses
            // differences, but the press and the moves must share one origin.
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                Event::Moved { at, to: (position.x - bounds.x, position.y - bounds.y) }
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(b)) => Event::Released { button: button(*b), at },
            // Wheel down (negative y) zooms out; the engine maps the lines to a zoom.
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) if at.is_some() => Event::Wheel {
                lines: match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / 40.0,
                },
            },
            iced::Event::Mouse(mouse::Event::CursorLeft) => Event::Left,
            // Any other event, or a wheel outside the widget: a zero wheel is a no-op for the engine,
            // but it still reports a size change first, as every event does.
            _ => Event::Wheel { lines: 0.0 },
        };
        let (action, captured) = st.update((bounds.width, bounds.height), ev)?;
        let msg = match action {
            Action::Resized { width, height } => Msg::Resized(width, height),
            Action::Rotate { dx, dy, height } => Msg::Rotate { dx, dy, height },
            Action::Pan { dx, dy, height } => Msg::Pan { dx, dy, height },
            Action::Hover(p) => Msg::Hover(p.map(|(x, y)| Point::new(x, y))),
            Action::Select((x, y)) => Msg::Select(Some(Point::new(x, y))),
            Action::Dolly(steps) => Msg::Dolly(steps),
        };
        let published = shader::Action::publish(msg);
        Some(if captured { published.and_capture() } else { published })
    }

    fn draw(&self, _state: &Pointer, _cursor: mouse::Cursor, _bounds: Rectangle) -> FloorPrimitive {
        FloorPrimitive(Arc::clone(&self.primitive))
    }

    fn mouse_interaction(&self, state: &Pointer, _bounds: Rectangle, _cursor: mouse::Cursor) -> mouse::Interaction {
        if state.dragging() {
            mouse::Interaction::Grabbing
        } else {
            mouse::Interaction::Grab
        }
    }
}

/// The floor's wgpu pipeline, held by iced for the shader widget.
pub struct FloorPipeline(Renderer);

impl shader::Pipeline for FloorPipeline {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        FloorPipeline(Renderer::new(device, queue, format))
    }
}

/// One frame to draw: the engine's frame, drawn with the pipeline above.
#[derive(Debug, Clone)]
pub struct FloorPrimitive(pub Arc<Frame>);

impl shader::Primitive for FloorPrimitive {
    type Pipeline = FloorPipeline;

    fn prepare(
        &self,
        pipeline: &mut FloorPipeline,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let rect = render::Rect { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height };
        pipeline.0.prepare(Arc::clone(&self.0), rect, viewport.scale_factor())
    }

    fn draw(&self, pipeline: &FloorPipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        pipeline.0.draw(pass)
    }
}
