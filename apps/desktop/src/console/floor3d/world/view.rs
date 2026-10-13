//! The floor in iced: the shader widget's program (pointer input → orbit, hover, pick), and the
//! overlay — name labels over the robots, the agent card, the legend — in the console's look.

use std::sync::Arc;

use glam::Vec3;
use iced::widget::shader;
use iced::{mouse, Point, Rectangle};

use super::{display_name, Bot, Floor, Phase};
use crate::console::floor3d::render;
use crate::console::floor3d::scene::Frame;
use crate::console::screens::floor::Msg;

/// A name label to draw over the scene, in widget pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub cid: u64,
    pub name: String,
    pub role: String,
    /// The second line: what it is doing.
    pub line: String,
    pub hue: f32,
    pub x: f32,
    pub y: f32,
    pub working: bool,
    pub highlighted: bool,
}

/// The second line of a label and the card (`shown` in floor.ts).
pub fn shown(bot: &Bot) -> String {
    if bot.agent.status == "offline" {
        "offline".to_string()
    } else if bot.social.as_ref().is_some_and(|s| s.phase == Phase::Talk) {
        "· · ·".to_string()
    } else {
        bot.activity.clone()
    }
}

impl Floor {
    /// The labels for a `width` × `height` view (`updateLabels`). Robots behind the camera are left out.
    pub fn labels(&self, width: f32, height: f32) -> Vec<Label> {
        if width <= 0.0 || height <= 0.0 {
            return Vec::new();
        }
        let camera = self.orbit.camera(width / height);
        let mut out = Vec::with_capacity(self.bots.len());
        for bot in &self.bots {
            let Some((x, y)) = camera.to_screen(bot.robot.label_anchor(), width, height) else { continue };
            let cid = bot.agent.cid;
            out.push(Label {
                cid,
                name: display_name(cid, &bot.agent.name),
                role: bot.agent.role.clone(),
                line: shown(bot),
                hue: self.districts.get(bot.district).map_or(0.0, |d| d.hue),
                x,
                y,
                working: bot.agent.status == "working",
                highlighted: self.hovered == Some(cid) || self.selected == Some(cid),
            });
        }
        out
    }

    /// The robot under the pointer (`pick`): the nearest whose bounding sphere the ray hits. The
    /// sphere is centred 0.6 above the robot's position, radius 0.75.
    pub fn pick(&self, x: f32, y: f32, width: f32, height: f32) -> Option<u64> {
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        let camera = self.orbit.camera(width / height);
        let (origin, dir) = camera.ray(x, y, width, height);
        let dir = dir.normalize_or_zero();
        let mut best: Option<(f32, u64)> = None;
        for bot in &self.bots {
            let centre = bot.robot.group.position() + Vec3::Y * 0.6;
            let Some(t) = ray_sphere(origin, dir, centre, 0.75) else { continue };
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, bot.agent.cid));
            }
        }
        best.map(|(_, cid)| cid)
    }
}

/// The distance along a unit ray to a sphere, if the ray reaches it.
fn ray_sphere(origin: Vec3, dir: Vec3, centre: Vec3, radius: f32) -> Option<f32> {
    let oc = origin - centre;
    let b = oc.dot(dir);
    let c = oc.length_squared() - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let s = disc.sqrt();
    // The near side, or the far side when the origin is inside the sphere.
    let t = if -b - s >= 0.0 { -b - s } else { -b + s };
    (t >= 0.0).then_some(t)
}

/// The pointer state between events: the button held and where it was last, and how far it moved
/// since it went down (a click is under 4 px).
#[derive(Debug, Default)]
pub struct FloorState {
    drag: Option<(mouse::Button, Point)>,
    moved: f32,
    /// The widget's size as of the last event (reported to the screen when it changes).
    size: (f32, f32),
}

/// The shader widget's program: draws the frame the screen built, turns pointer input into messages.
pub struct FloorProgram {
    pub primitive: Arc<Frame>,
}

impl shader::Program<Msg> for FloorProgram {
    type State = FloorState;
    type Primitive = render::Primitive;

    fn update(
        &self,
        st: &mut FloorState,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<shader::Action<Msg>> {
        let size = (bounds.width, bounds.height);
        if st.size != size {
            st.size = size;
            return Some(shader::Action::publish(Msg::Resized(size.0, size.1)));
        }
        match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                if let Some(p) = cursor.position().filter(|_| cursor.is_over(bounds)) {
                    st.drag = Some((*button, p));
                    st.moved = 0.0;
                }
                None
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some((button, last)) = st.drag {
                    let (dx, dy) = (position.x - last.x, position.y - last.y);
                    st.drag = Some((button, *position));
                    st.moved += dx.abs() + dy.abs();
                    let msg = if button == mouse::Button::Left {
                        Msg::Rotate { dx, dy, height: bounds.height }
                    } else {
                        Msg::Pan { dx, dy, height: bounds.height }
                    };
                    return Some(shader::Action::publish(msg).and_capture());
                }
                if cursor.is_over(bounds) {
                    return Some(shader::Action::publish(Msg::Hover(cursor.position_in(bounds))));
                }
                None
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(_)) => {
                st.drag.take()?;
                if st.moved < 4.0 && cursor.is_over(bounds) {
                    return Some(shader::Action::publish(Msg::Select(cursor.position_in(bounds))).and_capture());
                }
                None
            }
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if !cursor.is_over(bounds) {
                    return None;
                }
                // Wheel down (negative y) zooms out, as the web's OrbitControls do.
                let y = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / 40.0,
                };
                (y != 0.0).then(|| shader::Action::publish(Msg::Dolly(-y)).and_capture())
            }
            iced::Event::Mouse(mouse::Event::CursorLeft) =>Some(shader::Action::publish(Msg::Hover(None))),
            _ => None,
        }
    }

    fn draw(&self, _state: &FloorState, _cursor: mouse::Cursor, _bounds: Rectangle) -> render::Primitive {
        render::Primitive { frame: Arc::clone(&self.primitive) }
    }

    fn mouse_interaction(&self, state: &FloorState, _bounds: Rectangle, _cursor: mouse::Cursor) -> mouse::Interaction {
        if state.drag.is_some() {
            mouse::Interaction::Grabbing
        } else {
            mouse::Interaction::Grab
        }
    }
}
