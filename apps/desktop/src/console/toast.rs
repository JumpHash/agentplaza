//! Toasts (the web's `toast()`): a short note that confirms an action (a save, a removal, a test) and
//! fades after `LIFETIME`. The shell keeps the list (`shell::State::toasts`), so a note outlives the
//! screen that raised it and lays over whichever screen is open. Expiry takes its clock from the
//! caller, so the rule runs on a made-up timeline in the tests.

use std::time::{Duration, Instant};

use iced::alignment::Horizontal;
use iced::widget::{column, container, text};
use iced::{Element, Length};

use crate::theme::{self, Tone};

/// How long a toast stays on screen.
pub const LIFETIME: Duration = Duration::from_secs(4);

/// A success (green) or an error (red) note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub kind: Kind,
    pub text: String,
    /// When the toast went up, on the clock the shell ticks with.
    pub shown: Instant,
}

impl Toast {
    /// Whether the toast is still on screen at `now`. A clock that reads earlier than `shown` keeps it.
    pub fn live_at(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.shown) < LIFETIME
    }
}

/// Puts a toast up at `now`; the newest goes last.
pub fn push(list: &mut Vec<Toast>, kind: Kind, text: impl Into<String>, now: Instant) {
    list.push(Toast { kind, text: text.into(), shown: now });
}

/// Takes down the toasts whose time is up at `now`. Returns true when any went.
pub fn expire(list: &mut Vec<Toast>, now: Instant) -> bool {
    let before = list.len();
    list.retain(|t| t.live_at(now));
    list.len() != before
}

/// The toasts as a column at the window's bottom right, laid over the screen (the shell stacks it).
pub fn view<'a, M: 'a>(list: &'a [Toast]) -> Element<'a, M> {
    let cards: Vec<Element<'a, M>> = list
        .iter()
        .map(|t| {
            let tone = match t.kind {
                Kind::Success => Tone::Ok,
                Kind::Error => Tone::Crit,
            };
            container(text(t.text.as_str()).size(14.0)).padding(12).max_width(360.0).style(theme::notice(tone)).into()
        })
        .collect();
    container(column(cards).spacing(8).align_x(Horizontal::Right))
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(16)
        .align_x(Horizontal::Right)
        .align_y(iced::alignment::Vertical::Bottom)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_toast_stays_for_four_seconds_then_goes() {
        let t0 = Instant::now();
        let mut list = Vec::new();
        push(&mut list, Kind::Success, "Saved", t0);
        assert!(!expire(&mut list, t0 + Duration::from_millis(3_999)));
        assert_eq!(list.len(), 1);
        assert!(expire(&mut list, t0 + LIFETIME));
        assert!(list.is_empty());
    }

    #[test]
    fn only_the_expired_toasts_go() {
        let t0 = Instant::now();
        let mut list = Vec::new();
        push(&mut list, Kind::Success, "Saved", t0);
        push(&mut list, Kind::Error, "Test failed", t0 + Duration::from_secs(2));
        assert!(expire(&mut list, t0 + Duration::from_millis(4_500)));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, Kind::Error);
        assert_eq!(list[0].text, "Test failed");
    }

    #[test]
    fn a_clock_behind_the_toast_keeps_it() {
        let t0 = Instant::now();
        let mut list = Vec::new();
        push(&mut list, Kind::Error, "Removed", t0 + Duration::from_secs(10));
        assert!(!expire(&mut list, t0));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn newest_toast_goes_last_and_each_keeps_its_own_clock() {
        let t0 = Instant::now();
        let mut list = Vec::new();
        push(&mut list, Kind::Success, "first", t0);
        push(&mut list, Kind::Success, "second", t0 + Duration::from_secs(1));
        assert_eq!(list.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(), ["first", "second"]);
        assert!(expire(&mut list, t0 + Duration::from_secs(4)));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].text, "second");
    }
}
