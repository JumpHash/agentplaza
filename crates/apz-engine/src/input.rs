//! The floor's pointer input, platform-agnostic: a host maps its events into [`Event`]s, and
//! [`Pointer`] turns them into the floor's [`Action`]s (orbit, pan, zoom, hover, select).
//! Positions are in the widget's own space, origin at its top-left corner.

/// A mouse button, as the floor sees it: the left one orbits, any other pans.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Button {
    Left,
    Other,
}

/// A pointer event from the host.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// A button went down. `at` is the cursor inside the widget, None when outside.
    Pressed { button: Button, at: Option<(f32, f32)> },
    /// The cursor moved. `at` is its position inside the widget (None when outside), and `to` is
    /// its position always, even outside, so a drag keeps going past the edge.
    Moved { at: Option<(f32, f32)>, to: (f32, f32) },
    /// A button went up. `at` is the cursor inside the widget, None when outside.
    Released { button: Button, at: Option<(f32, f32)> },
    /// A wheel turn, in lines (positive is up, already normalised by the host). Sent only while
    /// the cursor is inside the widget.
    Wheel { lines: f32 },
    /// The cursor left the window.
    Left,
}

/// What the floor should do about an event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// The widget's size changed, in logical px.
    Resized { width: f32, height: f32 },
    /// Orbit the camera by a drag of `dx`, `dy` px in a view `height` tall.
    Rotate { dx: f32, dy: f32, height: f32 },
    /// Pan the camera by a drag of `dx`, `dy` px in a view `height` tall.
    Pan { dx: f32, dy: f32, height: f32 },
    /// The pointer is over the floor at this point, or left it (None).
    Hover(Option<(f32, f32)>),
    /// A click at this point.
    Select((f32, f32)),
    /// Zoom by this many steps (positive zooms out).
    Dolly(f32),
}

/// The pointer state between events: the button held and where it was last, and how far it moved
/// since it went down (a click is under 4 px).
#[derive(Debug, Default)]
pub struct Pointer {
    drag: Option<(Button, (f32, f32))>,
    moved: f32,
    /// The widget's size as of the last event (reported when it changes).
    size: (f32, f32),
}

impl Pointer {
    /// Feeds one event, for a widget `size` px in size. Returns the action, and whether the host
    /// should capture the event (keep it from the widgets beneath). A size change is reported first
    /// and drops the event.
    pub fn update(&mut self, size: (f32, f32), event: Event) -> Option<(Action, bool)> {
        if size != self.size {
            self.size = size;
            return Some((Action::Resized { width: size.0, height: size.1 }, false));
        }
        match event {
            Event::Pressed { button, at } => {
                if let Some(p) = at {
                    self.drag = Some((button, p));
                    self.moved = 0.0;
                }
                None
            }
            Event::Moved { at, to } => {
                if let Some((button, last)) = self.drag {
                    let (dx, dy) = (to.0 - last.0, to.1 - last.1);
                    self.drag = Some((button, to));
                    self.moved += dx.abs() + dy.abs();
                    let height = size.1;
                    let action = match button {
                        Button::Left => Action::Rotate { dx, dy, height },
                        Button::Other => Action::Pan { dx, dy, height },
                    };
                    return Some((action, true));
                }
                at.map(|p| (Action::Hover(Some(p)), false))
            }
            Event::Released { at, .. } => {
                self.drag.take()?;
                match at {
                    Some(p) if self.moved < 4.0 => Some((Action::Select(p), true)),
                    _ => None,
                }
            }
            // Wheel down (negative lines) zooms out, as the web's OrbitControls do.
            Event::Wheel { lines } => (lines != 0.0).then_some((Action::Dolly(-lines), true)),
            Event::Left => Some((Action::Hover(None), false)),
        }
    }

    /// Whether a button is held down over a drag.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (f32, f32) = (800.0, 600.0);

    /// A pointer whose size is already known, so the next event is not swallowed by the resize.
    fn primed() -> Pointer {
        let mut p = Pointer::default();
        p.update(SIZE, Event::Left);
        p
    }

    #[test]
    fn a_new_size_is_reported_first_and_drops_the_event() {
        let mut p = Pointer::default();
        let wheel = Event::Wheel { lines: 1.0 };
        assert_eq!(p.update(SIZE, wheel), Some((Action::Resized { width: 800.0, height: 600.0 }, false)));
        assert_eq!(p.update(SIZE, wheel), Some((Action::Dolly(-1.0), true)));
    }

    #[test]
    fn a_held_button_orbits_with_left_and_pans_with_other() {
        let mut p = primed();
        assert_eq!(p.update(SIZE, Event::Pressed { button: Button::Left, at: Some((10.0, 10.0)) }), None);
        let moved = Event::Moved { at: Some((13.0, 14.0)), to: (13.0, 14.0) };
        assert_eq!(p.update(SIZE, moved), Some((Action::Rotate { dx: 3.0, dy: 4.0, height: 600.0 }, true)));

        let mut p = primed();
        assert_eq!(p.update(SIZE, Event::Pressed { button: Button::Other, at: Some((10.0, 10.0)) }), None);
        assert_eq!(p.update(SIZE, moved), Some((Action::Pan { dx: 3.0, dy: 4.0, height: 600.0 }, true)));
    }

    #[test]
    fn a_short_press_and_release_selects_and_a_release_without_a_drag_does_nothing() {
        let mut p = primed();
        p.update(SIZE, Event::Pressed { button: Button::Left, at: Some((5.0, 5.0)) });
        let release = Event::Released { button: Button::Left, at: Some((5.0, 5.0)) };
        assert_eq!(p.update(SIZE, release), Some((Action::Select((5.0, 5.0)), true)));
        assert_eq!(p.update(SIZE, release), None);
    }
}
