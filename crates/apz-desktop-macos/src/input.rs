//! Input actions planned as the events the platform posts.
//!
//! [`plan`] turns one [`InputAction`] into [`Step`]s and tracks the pointer: a press or a release with no position of
//! its own happens where the pointer is. Everything here is pure, so the sequence of events is tested on any host;
//! the macOS backend only turns each step into a `CGEvent`.

use apz_desktop::{DesktopError, Result, parse_combo};
use apz_station::{Button, InputAction};

use crate::keys;

/// The points a drag passes through between its press and its release.
pub const DRAG_STEPS: i32 = 12;

/// The most UTF-16 units one typed chunk carries. macOS takes at most 20 per keyboard event.
pub const TYPE_CHUNK_UNITS: usize = 20;

/// One event to post, in the order given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The pointer moves to `(x, y)` in screen points.
    Move { x: i32, y: i32 },
    /// A button goes down at `(x, y)` as the `count`-th press of a click run (2 is the second press of a double
    /// click).
    Press {
        button: Button,
        x: i32,
        y: i32,
        count: u8,
    },
    /// The button comes up at `(x, y)`, the `count`-th release of a click run.
    Release {
        button: Button,
        x: i32,
        y: i32,
        count: u8,
    },
    /// The pointer moves to `(x, y)` with `button` held.
    Dragged { button: Button, x: i32, y: i32 },
    /// A wheel scroll at `(x, y)`, in the wheel's own signs: `dy` positive scrolls up, `dx` positive scrolls
    /// left. The planner turns the protocol's signs (`dy > 0` down, `dx > 0` right) into these.
    Scroll { x: i32, y: i32, dx: i32, dy: i32 },
    /// A virtual key goes down or up, with the flags the chord holds.
    Key { code: u16, flags: u64, down: bool },
    /// A chunk of text typed as one keyboard event, as UTF-16.
    Text { units: Vec<u16> },
}

/// The events an action posts, and the pointer afterwards. `pointer` is where the pointer is before the action.
pub fn plan(action: &InputAction, pointer: (i32, i32)) -> Result<(Vec<Step>, (i32, i32))> {
    match action {
        InputAction::Move { x, y } => Ok((vec![Step::Move { x: *x, y: *y }], (*x, *y))),
        InputAction::Click {
            x,
            y,
            button,
            count,
        } => {
            if *count == 0 {
                return Err(DesktopError::Invalid(
                    "a click needs at least one press".into(),
                ));
            }
            let mut steps = vec![Step::Move { x: *x, y: *y }];
            for count in 1..=*count {
                steps.push(Step::Press {
                    button: *button,
                    x: *x,
                    y: *y,
                    count,
                });
                steps.push(Step::Release {
                    button: *button,
                    x: *x,
                    y: *y,
                    count,
                });
            }
            Ok((steps, (*x, *y)))
        }
        InputAction::Down { button } => Ok((
            vec![Step::Press {
                button: *button,
                x: pointer.0,
                y: pointer.1,
                count: 1,
            }],
            pointer,
        )),
        InputAction::Up { button } => Ok((
            vec![Step::Release {
                button: *button,
                x: pointer.0,
                y: pointer.1,
                count: 1,
            }],
            pointer,
        )),
        // The protocol's `dy > 0` scrolls content down, which is a negative wheel1 (CoreGraphics counts up as
        // positive); `dx > 0` scrolls right, which is a negative wheel2.
        InputAction::Scroll { x, y, dx, dy } => Ok((
            vec![
                Step::Move { x: *x, y: *y },
                Step::Scroll {
                    x: *x,
                    y: *y,
                    dx: dx.saturating_neg(),
                    dy: dy.saturating_neg(),
                },
            ],
            (*x, *y),
        )),
        InputAction::Type { text } => Ok((
            type_chunks(text)
                .into_iter()
                .map(|units| Step::Text { units })
                .collect(),
            pointer,
        )),
        InputAction::Key { combo } => {
            let (code, flags) = keys::chord(&parse_combo(combo)?)?;
            Ok((
                vec![
                    Step::Key {
                        code,
                        flags,
                        down: true,
                    },
                    Step::Key {
                        code,
                        flags,
                        down: false,
                    },
                ],
                pointer,
            ))
        }
        InputAction::Drag { from, to } => {
            let mut steps = vec![
                Step::Move {
                    x: from.0,
                    y: from.1,
                },
                Step::Press {
                    button: Button::Left,
                    x: from.0,
                    y: from.1,
                    count: 1,
                },
            ];
            for step in 1..=DRAG_STEPS {
                let (x, y) = interpolate(*from, *to, step);
                steps.push(Step::Dragged {
                    button: Button::Left,
                    x,
                    y,
                });
            }
            steps.push(Step::Release {
                button: Button::Left,
                x: to.0,
                y: to.1,
                count: 1,
            });
            Ok((steps, *to))
        }
    }
}

/// The point `step` of [`DRAG_STEPS`] along the way from `from` to `to`. The last step is `to` exactly.
fn interpolate(from: (i32, i32), to: (i32, i32), step: i32) -> (i32, i32) {
    let along = |a: i32, b: i32| {
        let delta = i64::from(b) - i64::from(a);
        let moved = delta * i64::from(step) / i64::from(DRAG_STEPS);
        // The span is a difference of two i32 values, so the result fits back into an i32.
        i32::try_from(i64::from(a) + moved).unwrap_or(b)
    };
    (along(from.0, to.0), along(from.1, to.1))
}

/// The CoreGraphics button number a button event carries.
pub fn button_number(button: Button) -> i64 {
    match button {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => 2,
    }
}

/// The text cut into chunks of at most [`TYPE_CHUNK_UNITS`] UTF-16 units, never splitting a character.
pub fn type_chunks(text: &str) -> Vec<Vec<u16>> {
    let mut chunks = Vec::new();
    let mut current: Vec<u16> = Vec::new();
    for c in text.chars() {
        let mut buffer = [0u16; 2];
        let units = c.encode_utf16(&mut buffer);
        if !current.is_empty() && current.len() + units.len() > TYPE_CHUNK_UNITS {
            chunks.push(std::mem::take(&mut current));
        }
        current.extend_from_slice(units);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(x: i32, y: i32, count: u8) -> InputAction {
        InputAction::Click {
            x,
            y,
            button: Button::Left,
            count,
        }
    }

    #[test]
    fn plan_turns_each_input_action_into_steps() {
        // Was a_move_sets_the_pointer: a move plans one move step and sets the pointer.
        {
            let (steps, pointer) = plan(&InputAction::Move { x: 4, y: 9 }, (0, 0)).expect("plans");
            assert_eq!(steps, vec![Step::Move { x: 4, y: 9 }]);
            assert_eq!(pointer, (4, 9));
        }

        // Was a_click_moves_then_presses_and_releases: a click moves, presses and releases at its point.
        {
            let (steps, pointer) = plan(&click(10, 20, 1), (0, 0)).expect("plans");
            assert_eq!(
                steps,
                vec![
                    Step::Move { x: 10, y: 20 },
                    Step::Press {
                        button: Button::Left,
                        x: 10,
                        y: 20,
                        count: 1,
                    },
                    Step::Release {
                        button: Button::Left,
                        x: 10,
                        y: 20,
                        count: 1,
                    },
                ]
            );
            assert_eq!(pointer, (10, 20));
        }

        // Was a_double_click_numbers_its_presses: a double click numbers its presses 1, 1, 2, 2.
        {
            let (steps, _) = plan(&click(1, 1, 2), (0, 0)).expect("plans");
            let counts: Vec<u8> = steps
                .iter()
                .filter_map(|step| match step {
                    Step::Press { count, .. } | Step::Release { count, .. } => Some(*count),
                    _ => None,
                })
                .collect();
            assert_eq!(counts, vec![1, 1, 2, 2]);
        }

        // Was a_zero_count_click_is_refused: a zero-count click is refused.
        {
            assert!(matches!(
                plan(&click(1, 1, 0), (0, 0)),
                Err(DesktopError::Invalid(_))
            ));
        }

        // Was down_and_up_happen_at_the_pointer: button down and up happen at the pointer.
        {
            let (steps, pointer) = plan(
                &InputAction::Down {
                    button: Button::Right,
                },
                (7, 8),
            )
            .expect("plans");
            assert_eq!(
                steps,
                vec![Step::Press {
                    button: Button::Right,
                    x: 7,
                    y: 8,
                    count: 1,
                }]
            );
            assert_eq!(pointer, (7, 8));
            let (steps, _) = plan(
                &InputAction::Up {
                    button: Button::Right,
                },
                (7, 8),
            )
            .expect("plans");
            assert!(matches!(
                steps.as_slice(),
                [Step::Release { x: 7, y: 8, .. }]
            ));
        }

        // Was a_scroll_moves_to_its_point_first: a scroll moves to its point first.
        {
            let (steps, pointer) = plan(
                &InputAction::Scroll {
                    x: 100,
                    y: 200,
                    dx: 0,
                    dy: -3,
                },
                (0, 0),
            )
            .expect("plans");
            assert_eq!(
                steps,
                vec![
                    Step::Move { x: 100, y: 200 },
                    Step::Scroll {
                        x: 100,
                        y: 200,
                        dx: 0,
                        dy: 3,
                    },
                ]
            );
            assert_eq!(pointer, (100, 200));
        }

        // Was a_scroll_down_and_right_is_a_negative_wheel: scrolling down and right is a negative wheel.
        {
            let (steps, _) = plan(
                &InputAction::Scroll {
                    x: 1,
                    y: 2,
                    dx: 4,
                    dy: 5,
                },
                (0, 0),
            )
            .expect("plans");
            assert!(matches!(
                steps.last(),
                Some(Step::Scroll { dx: -4, dy: -5, .. })
            ));
        }

        // Was a_drag_presses_at_the_start_and_releases_at_the_end: a drag presses at its start and releases at its end.
        {
            let (steps, pointer) = plan(
                &InputAction::Drag {
                    from: (0, 0),
                    to: (120, 60),
                },
                (5, 5),
            )
            .expect("plans");
            assert_eq!(steps.first(), Some(&Step::Move { x: 0, y: 0 }));
            assert!(matches!(
                steps.get(1),
                Some(Step::Press {
                    x: 0,
                    y: 0,
                    count: 1,
                    ..
                })
            ));
            assert_eq!(
                steps.last(),
                Some(&Step::Release {
                    button: Button::Left,
                    x: 120,
                    y: 60,
                    count: 1,
                })
            );
            let dragged: Vec<(i32, i32)> = steps
                .iter()
                .filter_map(|step| match step {
                    Step::Dragged { x, y, .. } => Some((*x, *y)),
                    _ => None,
                })
                .collect();
            assert_eq!(dragged.len(), DRAG_STEPS as usize);
            assert_eq!(dragged.last(), Some(&(120, 60)));
            assert_eq!(pointer, (120, 60));
        }

        // Was a_key_chord_is_pressed_and_released_with_its_flags: a key chord is pressed and released with its flags.
        {
            let (steps, pointer) = plan(
                &InputAction::Key {
                    combo: "ctrl+shift+s".into(),
                },
                (1, 1),
            )
            .expect("plans");
            let flags = keys::FLAG_CONTROL | keys::FLAG_SHIFT;
            assert_eq!(
                steps,
                vec![
                    Step::Key {
                        code: 0x01,
                        flags,
                        down: true,
                    },
                    Step::Key {
                        code: 0x01,
                        flags,
                        down: false,
                    },
                ]
            );
            assert_eq!(pointer, (1, 1));
        }

        // Was a_bad_chord_is_refused_before_anything_is_posted: a bad chord is refused before anything is posted.
        {
            assert!(
                plan(
                    &InputAction::Key {
                        combo: "hyper+a".into()
                    },
                    (0, 0)
                )
                .is_err()
            );
        }

        // Was empty_text_posts_nothing: empty text posts nothing.
        {
            let (steps, _) = plan(
                &InputAction::Type {
                    text: String::new(),
                },
                (0, 0),
            )
            .expect("plans");
            assert!(steps.is_empty());
        }
    }

    #[test]
    fn type_chunks_cap_each_chunk_at_twenty_units() {
        // Was typed_text_is_split_into_chunks_of_twenty_units: typed text is split into chunks of twenty units.
        {
            let text = "a".repeat(45);
            let chunks = type_chunks(&text);
            let lengths: Vec<usize> = chunks.iter().map(Vec::len).collect();
            assert_eq!(lengths, vec![20, 20, 5]);
        }

        // Was a_character_is_never_split_across_chunks: a character is never split across chunks.
        {
            // 19 units, then a character of two UTF-16 units: it starts a new chunk rather than overflowing.
            let text = format!("{}{}", "a".repeat(19), "\u{1F600}");
            let chunks = type_chunks(&text);
            assert_eq!(chunks.len(), 2);
            assert_eq!(chunks[0].len(), 19);
            assert_eq!(chunks[1], "\u{1F600}".encode_utf16().collect::<Vec<u16>>());
        }
    }

    #[test]
    fn buttons_have_their_cg_numbers() {
        assert_eq!(button_number(Button::Left), 0);
        assert_eq!(button_number(Button::Right), 1);
        assert_eq!(button_number(Button::Middle), 2);
    }
}
