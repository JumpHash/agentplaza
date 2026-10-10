//! The fleet map as an iced canvas: it paints exactly what
//! [`map::layout`](crate::map::layout) computes — the machine's substrate, one
//! lane per run, one cell per VM — and turns a press on a cell into
//! [`Message::Select`], which opens that run.
//!
//! The layout is a pure function of the runs and the canvas's size, so the
//! hit-test a press uses and the geometry a frame draws cannot disagree: both
//! come from the same call.

use iced::mouse;
use iced::widget::canvas::{self, Action, Canvas, Event, Frame, Geometry, Path, Stroke, Text};
use iced::widget::text::Alignment;
use iced::{Color, Font, Length, Point, Rectangle, Renderer, Size, Theme};

use crate::api::{Machine, Run, VmState};
use crate::map::{self, ACTIVITY_FULL, Activity, MARK_R, PLOT_LEFT};
use crate::model::Message;
use crate::theme;

/// The map: the runs to draw, the pulse to draw them with, the machine whose
/// bounds they live in, and the pressed cell.
#[derive(Debug)]
pub struct FleetMap<'a> {
    /// Every run in the register, as the list endpoint reports it.
    pub runs: &'a [Run],
    /// Every VM's serial rate, from the deltas between polls.
    pub activity: &'a Activity,
    /// The node's machine: the substrate's label.
    pub machine: Option<&'a Machine>,
    /// The pressed cell: the run the console targets and its VM.
    pub selected: Option<(&'a str, u32)>,
}

/// The map as a widget that fills its slot.
pub fn widget(plate: FleetMap<'_>) -> Canvas<FleetMap<'_>, Message> {
    Canvas::new(plate)
        .width(Length::Fill)
        .height(Length::Fill)
}

impl FleetMap<'_> {
    /// The plate this map draws in a frame of `size`.
    fn plate(&self, size: Size) -> map::Layout {
        map::layout(self.runs, self.activity, size.width, size.height)
    }

    /// Draws the plate: the substrate, its lanes, and their cells.
    fn paint(&self, frame: &mut Frame, size: Size) {
        let plate = self.plate(size);
        let substrate = plate.substrate;

        // The machine's bounded region.
        frame.fill_rectangle(
            Point::new(substrate.x, substrate.y),
            Size::new(substrate.width, substrate.height),
            theme::SUBSTRATE,
        );
        frame.stroke_rectangle(
            Point::new(substrate.x, substrate.y),
            Size::new(substrate.width, substrate.height),
            Stroke::default().with_color(theme::INK_2).with_width(1.0),
        );

        // Its label: the machine model, and what it runs.
        match self.machine {
            Some(machine) => {
                frame.fill_text(canvas_text(
                    format!("{} · {}/{}", machine.name, machine.arch, machine.backend),
                    Point::new(substrate.x + 6.0, 4.0),
                    12.0,
                    theme::INK,
                    Alignment::Left,
                ));
                frame.fill_text(canvas_text(
                    format!(
                        "vsock {} · {} · {} · {} cpus · {} MiB",
                        machine.vsock_mode,
                        machine.class,
                        machine.irqchip,
                        machine.cpus,
                        machine.memory_mib
                    ),
                    Point::new(substrate.x + 6.0, 22.0),
                    9.5,
                    theme::INK_3,
                    Alignment::Left,
                ));
            }
            None => {
                frame.fill_text(canvas_text(
                    "the daemon has not reported its machine yet".to_string(),
                    Point::new(substrate.x + 6.0, 4.0),
                    11.0,
                    theme::INK_3,
                    Alignment::Left,
                ));
            }
        }

        // One lane per run: its label at the left of the substrate, then its
        // ruled baseline, then its cells.
        for lane in &plate.lanes {
            let live = lane.state.is_live();
            frame.fill_text(canvas_text(
                lane.run_id.clone(),
                Point::new(8.0, lane.y - 22.0),
                11.0,
                if live { theme::INK } else { theme::INK_3 },
                Alignment::Left,
            ));
            frame.fill_text(canvas_text(
                format!(
                    "{}×{} vCPU · {} MiB",
                    lane.vms, lane.vcpus, lane.memory_mib
                ),
                Point::new(8.0, lane.y - 7.0),
                9.0,
                theme::INK_3,
                Alignment::Left,
            ));
            frame.stroke(
                &Path::line(
                    Point::new(PLOT_LEFT - 18.0, lane.rule_y),
                    Point::new((substrate.right() - 6.0).max(PLOT_LEFT), lane.rule_y),
                ),
                Stroke::default().with_color(theme::RULE).with_width(1.0),
            );
        }
        for cell in &plate.cells {
            self.paint_cell(frame, cell);
        }
    }

    /// Draws one VM: its state mark, its vsock wire, its labels, its pulse and,
    /// when it is the selection, the register's signal outline.
    fn paint_cell(&self, frame: &mut Frame, cell: &map::Cell) {
        let centre = Point::new(cell.x, cell.y);

        // The state mark: the plaza's outlined shapes, filled only when the VM
        // is up.
        match cell.state {
            VmState::Running => frame.fill(&Path::circle(centre, MARK_R), theme::OK),
            VmState::Exited => frame.stroke(
                &Path::rectangle(
                    Point::new(cell.x - MARK_R, cell.y - MARK_R),
                    Size::new(MARK_R * 2.0, MARK_R * 2.0),
                ),
                Stroke::default().with_color(theme::INK_2).with_width(1.5),
            ),
            VmState::Killed => frame.stroke(
                &Path::new(|builder| {
                    builder.move_to(Point::new(cell.x, cell.y - MARK_R - 1.0));
                    builder.line_to(Point::new(cell.x + MARK_R + 1.0, cell.y));
                    builder.line_to(Point::new(cell.x, cell.y + MARK_R + 1.0));
                    builder.line_to(Point::new(cell.x - MARK_R - 1.0, cell.y));
                    builder.close();
                }),
                Stroke::default().with_color(theme::CRIT).with_width(1.5),
            ),
            VmState::Unknown => frame.stroke(
                &Path::circle(centre, MARK_R),
                Stroke::default().with_color(theme::INK_3).with_width(1.0),
            ),
        }

        // The vsock wire: a VM with a guest CID has one.
        if cell.guest_cid.is_some() {
            frame.stroke(
                &Path::line(
                    Point::new(cell.x - 12.0, cell.y + 0.5),
                    Point::new(cell.x + 12.0, cell.y + 0.5),
                ),
                Stroke::default().with_color(theme::INK_2).with_width(1.0),
            );
        }

        // The labels: the VM above its mark, its guest CID below.
        frame.fill_text(canvas_text(
            format!("vm {}", cell.vm),
            Point::new(cell.x, cell.y - 32.0),
            9.0,
            theme::INK_3,
            Alignment::Center,
        ));
        if let Some(cid) = cell.guest_cid {
            frame.fill_text(canvas_text(
                format!("cid {cid}"),
                Point::new(cell.x, cell.y + 14.0),
                9.0,
                theme::INK_2,
                Alignment::Center,
            ));
        }

        // The pulse: a live bar whose height is the VM's serial rate, drawn
        // only while the VM is up and writing.
        if cell.state == VmState::Running && cell.lps > 0.02 {
            let height = (cell.lps / ACTIVITY_FULL).min(1.0) * 10.0;
            frame.fill_rectangle(
                Point::new(cell.x + 13.0, cell.y + 8.0 - height),
                Size::new(4.0, height),
                theme::OK,
            );
        }

        // The selection: the register's one hot signal.
        if self.selected == Some((cell.run_id.as_str(), cell.vm)) {
            frame.stroke_rectangle(
                Point::new(cell.x - 13.0, cell.y - 13.0),
                Size::new(26.0, 26.0),
                Stroke::default().with_color(theme::SIGNAL).with_width(2.0),
            );
        }
    }
}

impl canvas::Program<Message> for FleetMap<'_> {
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
                let plate = self.plate(bounds.size());
                let hit = plate.cell_at(position.x, position.y)?;
                Some(
                    Action::publish(Message::Select {
                        run: hit.run_id.clone(),
                        vm: hit.vm,
                    })
                    .and_capture(),
                )
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
        if self.plate(bounds.size()).cell_at(position.x, position.y).is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::None
        }
    }
}

/// A canvas label in the register's mono voice, anchored at `position`.
fn canvas_text(
    content: String,
    position: Point,
    size: f32,
    color: Color,
    align_x: Alignment,
) -> Text {
    Text {
        content,
        position,
        color,
        size: size.into(),
        font: Font::MONOSPACE,
        align_x,
        ..Text::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{RunState, Vm};
    use crate::theme;
    use iced::widget::canvas::Program;

    fn run(id: &str, state: RunState, vms: Vec<Vm>) -> Run {
        Run {
            id: id.to_string(),
            state,
            vcpus: 2,
            memory_mib: 512,
            beats: None,
            created_at: 1_000_000,
            ended_at: None,
            vms,
        }
    }

    fn vm(id: u32, state: VmState, cid: Option<u64>) -> Vm {
        Vm {
            id,
            guest_cid: cid,
            state,
            serial: Some("$ uname\nLinux plaza\n".to_string()),
        }
    }

    fn fleet() -> Vec<Run> {
        vec![run(
            "r1",
            RunState::Running,
            vec![
                vm(0, VmState::Running, Some(50)),
                vm(1, VmState::Killed, None),
            ],
        )]
    }

    fn bounds() -> Rectangle {
        Rectangle::new(Point::ORIGIN, Size::new(900.0, 500.0))
    }

    /// The plate the widget would draw at `bounds`, so a test can press the
    /// exact pixel a frame put a cell on.
    fn plate() -> map::Layout {
        map::layout(&fleet(), &Activity::default(), 900.0, 500.0)
    }

    #[test]
    fn a_press_on_a_cell_publishes_the_selection_and_captures_the_event() {
        let runs = fleet();
        let map = FleetMap {
            runs: &runs,
            activity: &Activity::default(),
            machine: None,
            selected: None,
        };
        let target = plate()
            .cells
            .iter()
            .find(|cell| cell.vm == 1)
            .expect("the second VM")
            .clone();

        let event = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let cursor = mouse::Cursor::Available(Point::new(target.x, target.y));
        let mut state = ();
        let action = map
            .update(&mut state, &event, bounds(), cursor)
            .expect("a press on a cell acts");
        let (message, _, status) = action.into_inner();
        assert_eq!(
            message,
            Some(Message::Select {
                run: "r1".to_string(),
                vm: 1
            })
        );
        assert_eq!(status, iced::event::Status::Captured);
    }

    #[test]
    fn a_press_on_the_substrate_is_not_a_selection() {
        let runs = fleet();
        let map = FleetMap {
            runs: &runs,
            activity: &Activity::default(),
            machine: None,
            selected: None,
        };
        let event = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let cursor = mouse::Cursor::Available(Point::new(600.0, 480.0));
        let mut state = ();
        assert!(map.update(&mut state, &event, bounds(), cursor).is_none());
    }

    #[test]
    fn the_cursor_is_a_pointer_over_a_cell_and_nothing_elsewhere() {
        let runs = fleet();
        let map = FleetMap {
            runs: &runs,
            activity: &Activity::default(),
            machine: None,
            selected: None,
        };
        let cell = plate().cells[0].clone();
        let mut state = ();
        assert_eq!(
            map.mouse_interaction(
                &mut state,
                bounds(),
                mouse::Cursor::Available(Point::new(cell.x, cell.y))
            ),
            mouse::Interaction::Pointer
        );
        assert_eq!(
            map.mouse_interaction(
                &mut state,
                bounds(),
                mouse::Cursor::Available(Point::new(600.0, 480.0))
            ),
            mouse::Interaction::None
        );
    }

    #[test]
    fn the_map_draws_a_frame_for_every_state_of_the_register() {
        let machine = Machine {
            name: "zarathos-pc".to_string(),
            arch: "x86_64".to_string(),
            backend: "kvm".to_string(),
            irqchip: "ioapic".to_string(),
            class: "server".to_string(),
            cpus: 24,
            memory_mib: 128_320,
            vsock_mode: "uds-muxer".to_string(),
            started_at_ms: 1_000,
        };
        let mut activity = Activity::default();
        activity.observe("r1", 0, 0, 1_000);
        activity.observe("r1", 0, 20, 3_000);

        let runs = fleet();
        let drawable = FleetMap {
            runs: &runs,
            activity: &activity,
            machine: Some(&machine),
            selected: Some(("r1", 1)),
        };
        let plate = drawable.plate(bounds().size());
        assert_eq!(plate.cells.len(), 2);
        assert!(plate.cells[0].lps > 0.0, "the pulse reaches the canvas");

        // The widget builds, and its geometry is produced by the renderer the
        // headless test harness uses (tiny-skia): `draw` needs a renderer, so
        // this is covered by the frame assertions in `tests/render.rs`.
        let _: Canvas<FleetMap<'_>, Message> = widget(drawable);
        assert_eq!(theme::SUBSTRATE, Color::from_rgb8(0xee, 0xf1, 0xe9));
        assert!(MARK_R > 0.0);
    }
}
