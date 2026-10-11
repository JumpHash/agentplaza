//! The plaza floor as an iced canvas: it paints exactly what [`floor`]
//! computes — the projected ground grid, the role station pads, one robot per
//! agent as an extruded box with a face, its ground status ring as a projected
//! ellipse, and the line between an adjacent talking pair — in painter's
//! order, and turns a press on a robot into [`Message::PickRobot`].
//!
//! The layout is a pure function of the model (see [`Model::floor`]), and the
//! projection is [`floor::project`], so the hit-test a press uses and the
//! geometry a frame draws cannot disagree: both come from the same call.
//!
//! [`Model::floor`]: crate::model::Model::floor

use iced::mouse;
use iced::widget::canvas::{self, Action, Canvas, Event, Frame, Geometry, Path, Stroke, Text};
use iced::widget::text::Alignment;
use iced::{Color, Font, Length, Point, Rectangle, Renderer, Size, Theme};

use crate::floor::{self, Camera, FloorLayout, Vec3};
use crate::model::Message;
use crate::theme;

/// The plaza's ground, a hair off the sheet so the bounded plane reads as a
/// plane without becoming a filled block.
pub const GROUND: Color = Color::from_rgb8(0xdf, 0xe5, 0xda);
/// A ground gridline.
pub const GRID: Color = Color::from_rgb8(0xc3, 0xcb, 0xbe);
/// A role's station pad on the ground.
pub const PAD: Color = Color::from_rgb8(0xd2, 0xda, 0xca);
/// A talking pair's link line on the ground.
pub const LINK: Color = Color::from_rgb8(0xe8, 0x59, 0x0c);
/// How far from a projected robot a press still picks it, in pixels.
const PICK_SLACK: f32 = 26.0;

/// The floor: the layout to draw, the selected robot, and the camera to draw
/// it through.
#[derive(Debug)]
pub struct Floor<'a> {
    /// The plaza as [`crate::model::Model::floor`] derived it.
    pub layout: &'a FloorLayout,
    /// The robot the arrows last picked, drawn with the register's one hot
    /// signal.
    pub selected: Option<u64>,
    /// The camera the scene is projected through.
    pub camera: Camera,
}

impl<'a> Floor<'a> {
    /// The floor for `layout`, with the register's camera.
    pub fn new(layout: &'a FloorLayout, selected: Option<u64>) -> Self {
        Self {
            layout,
            selected,
            camera: Camera::rts(),
        }
    }
}

/// The floor as a widget that fills its slot.
pub fn widget(floor: Floor<'_>) -> Canvas<Floor<'_>, Message> {
    Canvas::new(floor)
        .width(Length::Fill)
        .height(Length::Fill)
}

/// One thing to draw, with the depth that orders it: the painter's algorithm
/// draws the largest depth (farthest) first.
enum Shape {
    /// A filled and/or stroked polygon.
    Polygon {
        /// The projected corners.
        points: Vec<Point>,
        /// The fill, when it has one.
        fill: Option<Color>,
        /// The outline and its width, when it has one.
        stroke: Option<(Color, f32)>,
    },
    /// A ground line: a talking pair's link.
    Line {
        /// The line's projected ends.
        from: Point,
        /// The line's projected ends.
        to: Point,
        /// The line's ink and width.
        color: Color,
        /// The line's width.
        width: f32,
    },
    /// A canvas label.
    Label {
        /// Where it is anchored.
        at: Point,
        /// What it says.
        text: String,
        /// The label's size.
        size: f32,
        /// The label's ink.
        color: Color,
        /// The anchor's horizontal alignment.
        align: Alignment,
    },
}

impl Floor<'_> {
    /// Projects one world point, or `None` when it is behind the camera.
    fn at(&self, x: f32, y: f32, z: f32, size: Size) -> Option<floor::Projected> {
        floor::project(Vec3 { x, y, z }, &self.camera, size.width, size.height)
    }

    /// A projected polygon at `depth` (the mean of its corners), or `None`
    /// when any corner is behind the camera.
    fn polygon(
        &self,
        corners: &[Vec3],
        size: Size,
        fill: Option<Color>,
        stroke: Option<(Color, f32)>,
    ) -> Option<(f32, Shape)> {
        let mut points = Vec::with_capacity(corners.len());
        let mut depth = 0.0;
        for corner in corners {
            let projected = self.at(corner.x, corner.y, corner.z, size)?;
            depth += projected.depth;
            points.push(Point::new(projected.x, projected.y));
        }
        let depth = depth / corners.len() as f32;
        Some((
            depth,
            Shape::Polygon {
                points,
                fill,
                stroke,
            },
        ))
    }

    /// A ground quad of half-extent `half` centred at `(x, z)`.
    fn ground(x: f32, z: f32, half: f32) -> [Vec3; 4] {
        [
            Vec3 { x: x - half, y: 0.0, z: z - half },
            Vec3 { x: x + half, y: 0.0, z: z - half },
            Vec3 { x: x + half, y: 0.0, z: z + half },
            Vec3 { x: x - half, y: 0.0, z: z + half },
        ]
    }

    /// Draws the floor: the ground, the grid, the pads and the pair links as
    /// the base plane, then the robots' rings and bodies painter-ordered, then
    /// the labels on top.
    ///
    /// The ground is a *base* layer rather than a depth-sorted polygon: a
    /// ground quad's mean depth is nearer than the robots standing on its far
    /// side, so sorting it with them would paint the plaza over its own robots.
    /// The plane is under everything, so it is drawn first.
    fn paint(&self, frame: &mut Frame, size: Size) {
        let mut base: Vec<Shape> = Vec::new();
        let mut shapes: Vec<(f32, Shape)> = Vec::new();
        let mut labels: Vec<Shape> = Vec::new();

        // The ground plane.
        if let Some((_, shape)) = self.polygon(
            &Self::ground(0.0, 0.0, floor::GRID_HALF),
            size,
            Some(GROUND),
            Some((theme::RULE, 1.0)),
        ) {
            base.push(shape);
        }

        // The grid: a line across the ground each way.
        let mut step = -floor::GRID_HALF;
        while step <= floor::GRID_HALF + f32::EPSILON {
            for (from, to) in [
                (
                    Vec3 { x: step, y: 0.0, z: -floor::GRID_HALF },
                    Vec3 { x: step, y: 0.0, z: floor::GRID_HALF },
                ),
                (
                    Vec3 { x: -floor::GRID_HALF, y: 0.0, z: step },
                    Vec3 { x: floor::GRID_HALF, y: 0.0, z: step },
                ),
            ] {
                let (Some(a), Some(b)) = (self.at(from.x, from.y, from.z, size), self.at(to.x, to.y, to.z, size))
                else {
                    continue;
                };
                base.push(Shape::Line {
                    from: Point::new(a.x, a.y),
                    to: Point::new(b.x, b.y),
                    color: GRID,
                    width: 1.0,
                });
            }
            step += floor::GRID_STEP;
        }

        // The station pads, each labelled with its role.
        for pad in &self.layout.pads {
            if let Some((_, shape)) = self.polygon(
                &Self::ground(pad.x, pad.z, floor::PAD_HALF),
                size,
                Some(PAD),
                Some((theme::INK_3, 1.0)),
            ) {
                base.push(shape);
            }
            if let Some(projected) = self.at(pad.x, 0.02, pad.z - floor::PAD_HALF - 0.06, size) {
                labels.push(Shape::Label {
                    at: Point::new(projected.x, projected.y),
                    text: pad.role.clone(),
                    size: 9.5,
                    color: theme::INK_3,
                    align: Alignment::Center,
                });
            }
        }

        // A talking pair's link on the ground, under the robots.
        for link in &self.layout.links {
            let (Some(a), Some(b)) = (
                self.at(link.a.0, 0.02, link.a.1, size),
                self.at(link.b.0, 0.02, link.b.1, size),
            ) else {
                continue;
            };
            base.push(Shape::Line {
                from: Point::new(a.x, a.y),
                to: Point::new(b.x, b.y),
                color: LINK,
                width: 1.5,
            });
        }

        // The robots, over the plane.
        for robot in &self.layout.robots {
            let ink = floor::role_ink(&robot.role);
            self.push_ring(&mut shapes, robot, size);
            self.push_body(&mut shapes, ink, robot.x, robot.z, size);
            if let Some(projected) = self.at(robot.x, floor::ROBOT_H + 0.34, robot.z, size) {
                labels.push(Shape::Label {
                    at: Point::new(projected.x, projected.y),
                    text: robot.role.clone(),
                    size: 10.0,
                    color: theme::INK,
                    align: Alignment::Center,
                });
            }
            if let Some(projected) = self.at(robot.x, floor::ROBOT_H + 0.18, robot.z, size) {
                labels.push(Shape::Label {
                    at: Point::new(projected.x, projected.y),
                    text: format!("cid {}", robot.cid),
                    size: 9.0,
                    color: theme::INK_2,
                    align: Alignment::Center,
                });
            }
        }

        // The plane, then the robots painting the farthest first so a nearer
        // one covers the one behind it, then the labels.
        for shape in &base {
            paint_shape(frame, shape);
        }
        shapes.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, shape) in shapes {
            paint_shape(frame, &shape);
        }
        for shape in labels {
            paint_shape(frame, &shape);
        }
    }

    /// Pushes a robot's ground status ring — a circle in the ground plane,
    /// projected to an ellipse — and, when it is the picked robot, the
    /// register's one hot signal as an outer ring around it.
    fn push_ring(&self, shapes: &mut Vec<(f32, Shape)>, robot: &floor::Robot, size: Size) {
        self.push_ground_ring(shapes, robot.x, robot.z, floor::RING_R, rgb(robot.status.ink()), 2.0, size);
        if self.selected == Some(robot.cid) {
            self.push_ground_ring(
                shapes,
                robot.x,
                robot.z,
                floor::RING_R * 1.3,
                theme::SIGNAL,
                2.5,
                size,
            );
        }
    }

    /// Pushes one circle in the ground plane, projected as an ellipse.
    fn push_ground_ring(
        &self,
        shapes: &mut Vec<(f32, Shape)>,
        x: f32,
        z: f32,
        radius: f32,
        ink: Color,
        width: f32,
        size: Size,
    ) {
        let mut corners = Vec::with_capacity(floor::RING_SEGS);
        let mut depth = 0.0;
        for segment in 0..floor::RING_SEGS {
            let angle = std::f32::consts::TAU * segment as f32 / floor::RING_SEGS as f32;
            let Some(projected) = self.at(
                x + radius * angle.cos(),
                0.015,
                z + radius * angle.sin(),
                size,
            ) else {
                return;
            };
            depth += projected.depth;
            corners.push(Point::new(projected.x, projected.y));
        }
        shapes.push((
            depth / corners.len() as f32,
            Shape::Polygon {
                points: corners,
                fill: None,
                stroke: Some((ink, width)),
            },
        ));
    }

    /// Pushes a robot's body: an extruded box, its top and sides shaded, with
    /// an eye on the near face. A near-side quad is drawn after its far side
    /// by the painter's order of the caller.
    fn push_body(
        &self,
        shapes: &mut Vec<(f32, Shape)>,
        ink: [u8; 3],
        x: f32,
        z: f32,
        size: Size,
    ) {
        let h = floor::ROBOT_HALF;
        let top = floor::ROBOT_H;
        let corners = |y: f32| {
            [
                Vec3 { x: x - h, y, z: z - h },
                Vec3 { x: x + h, y, z: z - h },
                Vec3 { x: x + h, y, z: z + h },
                Vec3 { x: x - h, y, z: z + h },
            ]
        };
        let (back, front) = (corners(0.0), corners(top));
        // The roof, in the role's own ink: the one face a frame can be read by.
        if let Some(shape) = self.polygon(&front, size, Some(rgb(ink)), Some((theme::INK, 1.0))) {
            shapes.push(shape);
        }
        // The four walls, each shaded a little differently so the box reads as
        // a box.
        for (index, factor) in [(0usize, 0.82f32), (1, 0.72), (2, 0.90), (3, 0.78)] {
            let next = (index + 1) % 4;
            let wall = [back[index], back[next], front[next], front[index]];
            if let Some(shape) = self.polygon(&wall, size, Some(shade(ink, factor)), Some((theme::INK, 1.0))) {
                shapes.push(shape);
            }
        }
        // The eye: a small dark rectangle on the near (+z) face.
        let eye = [
            Vec3 { x: x - 0.08, y: top * 0.45, z: z + h + 0.001 },
            Vec3 { x: x + 0.08, y: top * 0.45, z: z + h + 0.001 },
            Vec3 { x: x + 0.08, y: top * 0.75, z: z + h + 0.001 },
            Vec3 { x: x - 0.08, y: top * 0.75, z: z + h + 0.001 },
        ];
        if let Some(shape) = self.polygon(&eye, size, Some(theme::INK), None) {
            shapes.push(shape);
        }
    }

    /// The robot nearest a press within [`PICK_SLACK`], the floor's picking.
    fn pick(&self, point: Point, size: Size) -> Option<u64> {
        let mut best: Option<(f32, u64)> = None;
        for robot in &self.layout.robots {
            let Some(projected) = self.at(robot.x, floor::ROBOT_H / 2.0, robot.z, size) else {
                continue;
            };
            let distance = ((projected.x - point.x).powi(2) + (projected.y - point.y).powi(2)).sqrt();
            if distance <= PICK_SLACK && best.is_none_or(|(held, _)| distance < held) {
                best = Some((distance, robot.cid));
            }
        }
        best.map(|(_, cid)| cid)
    }
}

impl canvas::Program<Message> for Floor<'_> {
    type State = ();

    fn update(
        &self,
        _state: &mut (),
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<Message>> {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let position = cursor.position_in(bounds)?;
                let cid = self.pick(position, bounds.size())?;
                Some(Action::publish(Message::PickRobot(cid)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        self.paint(&mut frame, bounds.size());
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &(),
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        let Some(position) = cursor.position_in(bounds) else {
            return mouse::Interaction::None;
        };
        if self.pick(position, bounds.size()).is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::None
        }
    }
}

/// Paints one shape.
fn paint_shape(frame: &mut Frame, shape: &Shape) {
    match shape {
        Shape::Polygon {
            points,
            fill,
            stroke,
        } => {
            let Some(first) = points.first().copied() else {
                return;
            };
            let path = Path::new(|builder| {
                builder.move_to(first);
                for point in &points[1..] {
                    builder.line_to(*point);
                }
                builder.close();
            });
            if let Some(fill) = fill {
                frame.fill(&path, *fill);
            }
            if let Some((color, width)) = stroke {
                frame.stroke(
                    &path,
                    Stroke::default().with_color(*color).with_width(*width),
                );
            }
        }
        Shape::Line {
            from,
            to,
            color,
            width,
        } => {
            frame.stroke(
                &Path::line(*from, *to),
                Stroke::default().with_color(*color).with_width(*width),
            );
        }
        Shape::Label {
            at,
            text,
            size,
            color,
            align,
        } => {
            frame.fill_text(Text {
                content: text.clone(),
                position: *at,
                color: *color,
                size: (*size).into(),
                font: Font::MONOSPACE,
                align_x: *align,
                ..Text::default()
            });
        }
    }
}

/// A palette ink as an iced colour.
fn rgb(ink: [u8; 3]) -> Color {
    Color::from_rgb8(ink[0], ink[1], ink[2])
}

/// A shade of an ink: the same hue, scaled by `factor` (a wall darker than the
/// roof, the roof a little lighter).
fn shade(ink: [u8; 3], factor: f32) -> Color {
    let channel = |value: u8| -> u8 {
        let scaled = f32::from(value) * factor;
        scaled.clamp(0.0, 255.0) as u8
    };
    Color::from_rgb8(channel(ink[0]), channel(ink[1]), channel(ink[2]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::floor::{FloorLayout, Pad, Robot, Status};

    fn layout() -> FloorLayout {
        FloorLayout {
            pads: vec![Pad {
                role: "builder".to_string(),
                index: 0,
                col: 0,
                row: 0,
                x: 0.0,
                z: -2.25,
            }],
            robots: vec![Robot {
                cid: 100,
                role: "builder".to_string(),
                x: 0.0,
                z: -2.25,
                status: Status::Idle,
                at_station: true,
            }],
            links: Vec::new(),
        }
    }

    #[test]
    fn a_press_on_a_robot_picks_it_and_off_it_picks_nothing() {
        let layout = layout();
        let floor = Floor::new(&layout, None);
        let size = Size::new(1000.0, 600.0);
        let robot = &layout.robots[0];
        let projected = floor
            .at(robot.x, floor::ROBOT_H / 2.0, robot.z, size)
            .expect("the robot is in front of the camera");
        let hit = floor
            .pick(Point::new(projected.x, projected.y), size)
            .expect("a press on the robot picks it");
        assert_eq!(hit, 100);
        assert!(
            floor.pick(Point::new(10.0, 10.0), size).is_none(),
            "a press in the corner picks nothing"
        );
    }

    #[test]
    fn the_nearest_robot_wins_a_press_between_two() {
        let mut layout = layout();
        layout.robots.push(Robot {
            cid: 101,
            role: "builder".to_string(),
            x: 1.4,
            z: -2.25,
            status: Status::Working,
            at_station: true,
        });
        let floor = Floor::new(&layout, None);
        let size = Size::new(1000.0, 600.0);
        let near = floor
            .at(1.4, floor::ROBOT_H / 2.0, -2.25, size)
            .expect("in front");
        assert_eq!(floor.pick(Point::new(near.x, near.y), size), Some(101));
    }
}
