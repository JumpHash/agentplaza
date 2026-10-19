//! The first-run wizard and the tour, in the web console's words (`apps/shared/onboarding.json`,
//! read by `apps/web/src/ui/onboarding.ts`). On the first start the wizard opens as the web's dialog
//! (six steps: welcome, look, hub, dependencies, model keys, checks); then the tour is offered; the
//! tour is a callout beside each sidebar screen's entry. The flags are `setup_done` and `tour_done` in
//! the desktop settings ([`boot::saved_setting`]). The Setup screen replays both.

use std::sync::OnceLock;

use iced::futures::channel::mpsc;
use iced::futures::{SinkExt, StreamExt};
use iced::widget::{Space, button, column, container, mouse_area, row, text};
use iced::{Alignment, Border, Color, Element, Length, Padding, Task, Theme};
use serde_json::{Value, json};

use super::screens::{Id, NAV, appearance};
use super::setup::{self, Check, Need};
use super::ui::{self, Kind, Tone};
use super::{Ctx, boot, client};

const COPY_JSON: &str = include_str!("../../../shared/onboarding.json");

/// The wizard's sheet width (the web's `.sheet.onboard`).
const WIZARD_WIDTH: f32 = 600.0;

fn copy() -> &'static Value {
    static COPY: OnceLock<Value> = OnceLock::new();
    COPY.get_or_init(|| {
        serde_json::from_str(COPY_JSON).expect("apps/shared/onboarding.json is valid JSON")
    })
}

/// A copy string by its dotted path (`wizard.next`, `wizard.steps.3.title`); empty when the path is missing.
pub fn copy_text(path: &str) -> &'static str {
    t(path)
}

fn t(path: &str) -> &'static str {
    let mut value = copy();
    for part in path.split('.') {
        let next = match part.parse::<usize>() {
            Ok(index) => value.get(index),
            Err(_) => value.get(part),
        };
        match next {
            Some(found) => value = found,
            None => return "",
        }
    }
    value.as_str().unwrap_or("")
}

/// A string field of a copy object.
fn field<'v>(value: &'v Value, key: &str) -> &'v str {
    value[key].as_str().unwrap_or("")
}

fn wizard_steps() -> &'static [Value] {
    copy()["wizard"]["steps"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// The index of a wizard step by its id (`checks`, `deps`, …); 0 when there is no such step.
pub fn step_index(id: &str) -> usize {
    wizard_steps()
        .iter()
        .position(|step| field(step, "id") == id)
        .unwrap_or(0)
}

fn tour_steps() -> &'static [Value] {
    copy()["tour"]["steps"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// Fills `{name}` placeholders of a copy string.
pub fn fill(template: &str, values: &[(&str, String)]) -> String {
    values
        .iter()
        .fold(template.to_string(), |text, (name, value)| {
            text.replace(&format!("{{{name}}}"), value)
        })
}

/// The screen a tour step points at (its key is the screen's label in lower case).
fn screen_id(key: &str) -> Option<Id> {
    NAV.iter()
        .find(|(_, label, ..)| label.eq_ignore_ascii_case(key))
        .map(|(id, ..)| id.clone())
}

// The sidebar's geometry (shell.rs): the brand block, a section heading, a screen's entry, and the
// gap between entries. The tour's callouts sit beside the entry they describe.
const BRAND_H: f32 = 42.0;
const HEAD_H: f32 = 32.0;
const ITEM_H: f32 = 30.0;
const GAP: f32 = 2.0;

/// The height of a screen's entry's centre, from the window's top (the sidebar open or as a rail).
pub fn anchor_y(target: &Id, open: bool) -> f32 {
    let mut y = BRAND_H;
    let mut section = "";
    for (id, _, sec, _) in NAV {
        if open && *sec != section {
            section = sec;
            y += HEAD_H + GAP;
        }
        if id == target {
            return y + ITEM_H / 2.0;
        }
        y += ITEM_H + GAP;
    }
    y
}

/// Scrolls the sidebar so the tour's entry sits near its top: a long sidebar (the Setup entry is below a
/// 900 px window's fold) scrolls, and the callout and the mark must still reach the eye.
pub fn reveal<T: Send + 'static>(state: &State, open: bool) -> Task<T> {
    let Some(id) = highlighted(state) else {
        return Task::none();
    };
    // The list starts under the brand block; the entry sits 300 px down from the list's top.
    let y = (anchor_y(&id, open) - BRAND_H - 300.0).max(0.0);
    let scroll: Task<()> = iced::widget::operation::scroll_to(
        super::shell::SIDEBAR_SCROLL.clone(),
        iced::widget::scrollable::AbsoluteOffset { x: 0.0, y },
    );
    scroll.discard()
}

/// What the hub and the plaza answered. `keys` is None when the providers could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub reachable: bool,
    pub hub: bool,
    pub node: bool,
    /// Providers with a key, and all providers.
    pub keys: Option<(usize, usize)>,
    /// Each provider's id and whether it has a key (the model keys step lists them).
    pub providers: Vec<(String, bool)>,
}

fn status_of(overview: Option<Value>, providers: Option<Value>) -> Status {
    let listed: Vec<(String, bool)> = providers
        .as_ref()
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|p| {
                    (
                        p["id"].as_str().unwrap_or("").to_string(),
                        p["credential"].as_str() != Some("missing"),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Status {
        providers: listed,
        reachable: overview.is_some(),
        hub: overview
            .as_ref()
            .and_then(|o| o["hub"].as_bool())
            .unwrap_or(false),
        node: overview
            .as_ref()
            .and_then(|o| o["noded"].as_bool())
            .unwrap_or(false),
        keys: providers.and_then(|list| {
            let list = list.as_array()?;
            let set = list
                .iter()
                .filter(|p| p["credential"].as_str() != Some("missing"))
                .count();
            Some((set, list.len()))
        }),
    }
}

/// Reads the hub's status (the overview and the providers) off the UI thread.
pub fn status_task(ctx: &Ctx) -> Task<Status> {
    let client = ctx.client.clone();
    client::task(
        move || {
            let overview = client.get::<Value>("/v1/overview").ok();
            let providers = client.get::<Value>("/v1/providers").ok();
            Ok::<Status, String>(status_of(overview, providers))
        },
        |status| status.unwrap_or_default(),
    )
}

/// The plaza's local requirements (Docker, Postgres, …), checked off the UI thread.
pub fn local_task() -> Task<Vec<Check>> {
    Task::perform(async { setup::check_all(setup::Mode::Local) }, |checks| {
        checks
    })
}

/// The checks the checks step shows, in the web's order: the hub, the node, the model keys and this machine's
/// requirements. `Some(true)` is ready, `Some(false)` not ready, `None` not read yet.
fn check_states(
    status: Option<&Status>,
    local: Option<&[Check]>,
) -> [(&'static str, Option<bool>); 4] {
    [
        (
            "wizard.checks_labels.hub",
            status.map(|s| s.reachable && s.hub),
        ),
        ("wizard.checks_labels.node", status.map(|s| s.node)),
        (
            "wizard.checks_labels.keys",
            status.map(|s| matches!(s.keys, Some((n, _)) if n > 0)),
        ),
        ("wizard.checks_labels.local", local.map(setup::ready)),
    ]
    .map(|(path, ok)| (t(path), ok))
}

/// What the console must do after a message.
pub enum Effect {
    None,
    /// Show another screen (the wizard's "Open Models").
    Navigate(Id),
    /// Back to the startup chooser (the wizard's "Change hub").
    Chooser,
    /// The wizard's look was applied; the console saves it to the hub.
    Save(apz_theme::appearance::Appearance),
}

#[derive(Debug, Default)]
pub struct State {
    /// The wizard's step (None when it is not open).
    pub wizard: Option<usize>,
    /// The tour's step (None when it is not running).
    pub tour: Option<usize>,
    /// The tour offer is showing.
    pub offer: bool,
    pub status: Option<Status>,
    /// This machine's requirements (the dependencies step and the checks).
    pub local: Option<Vec<Check>>,
    /// The requirement whose fix is waiting for the user's confirmation.
    pub confirm: Option<Need>,
    /// The fix running now, its log lines so far, and how it ended when it failed.
    pub fixing: Option<Need>,
    pub log: Vec<String>,
    pub fix_error: Option<String>,
    /// "Skip for now" on the dependencies step: the wizard goes on with requirements missing.
    pub deps_skipped: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Next,
    Back,
    /// Finish, or Skip setup: the wizard ends and the tour is offered.
    Finish,
    OpenScreen(Id),
    /// The wizard's "Change hub".
    Change,
    /// The close button or a click on the scrim: the wizard closes and opens again on the next start.
    Dismiss,
    /// A press on the sheet itself: it is taken, so it does not reach the scrim.
    Keep,
    Status(Status),
    Local(Vec<Check>),
    StartTour,
    SkipOffer,
    TourNext,
    TourBack,
    /// Done, Skip tour or Escape.
    EndTour,
    /// Dependencies: the user is asked to confirm a fix, may cancel it, runs it, and the fix reports.
    AskFix(Need),
    CancelFix,
    RunFix(Need),
    FixLog(String),
    FixDone(Result<(), String>),
    SkipDeps,
    /// The look step: Loom (`loom-clay`) or Graphite (`graphite-cobalt`) is chosen.
    Look(&'static str),
}

fn flag(key: &str) -> bool {
    boot::saved_setting(key) == Some(json!(true))
}

fn set_flag(key: &str) {
    // Remembering is a convenience: a failed write must not block the console.
    let _ = boot::save_setting(key, json!(true));
}

/// Opens the wizard at its first step (from the Setup screen, or on the first start).
pub fn start_wizard(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.wizard = Some(0);
    state.tour = None;
    state.offer = false;
    state.deps_skipped = false;
    state.confirm = None;
    status_task(ctx).map(Msg::Status)
}

/// On connecting: the wizard when the setup flag is not set; a refresh of the status when the wizard
/// is already open (it came back from the chooser).
pub fn first_start(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    if state.wizard.is_some() {
        return status_task(ctx).map(Msg::Status);
    }
    if flag("setup_done") {
        return Task::none();
    }
    start_wizard(state, ctx)
}

pub fn start_tour(state: &mut State) {
    state.tour = Some(0);
    state.offer = false;
}

/// The screen the running tour step points at (the console highlights its entry).
pub fn highlighted(state: &State) -> Option<Id> {
    let step = tour_steps().get(state.tour?)?;
    screen_id(field(step, "screen"))
}

fn wizard_step_id(index: usize) -> &'static str {
    wizard_steps()
        .get(index)
        .map(|s| field(s, "id"))
        .unwrap_or("")
}

/// Whether the wizard may go on from its current step: a dependencies step waits for every required
/// check (or "Skip for now"), and nothing goes on while a fix runs.
pub fn can_go_on(state: &State) -> bool {
    if state.fixing.is_some() {
        return false;
    }
    let deps = state
        .wizard
        .is_some_and(|index| wizard_step_id(index) == "deps");
    !deps || state.deps_skipped || state.local.as_deref().is_some_and(setup::ready)
}

/// Runs a fix off the UI thread: its log lines reach the step as they are written, then its result.
fn run_fix(need: Need) -> Task<Msg> {
    enum Step {
        Line(String),
        Done(Result<(), String>),
    }
    Task::stream(iced::stream::channel(
        64,
        move |mut out: mpsc::Sender<Msg>| async move {
            let (tx, mut rx) = mpsc::unbounded::<Step>();
            std::thread::spawn(move || {
                let result = {
                    let mut log = |line: String| {
                        let _ = tx.unbounded_send(Step::Line(line));
                    };
                    setup::fix(need, &mut log)
                };
                let _ = tx.unbounded_send(Step::Done(result));
            });
            while let Some(step) = rx.next().await {
                let (msg, last) = match step {
                    Step::Line(line) => (Msg::FixLog(line), false),
                    Step::Done(result) => (Msg::FixDone(result), true),
                };
                if out.send(msg).await.is_err() || last {
                    break;
                }
            }
        },
    ))
}

pub fn update(state: &mut State, msg: Msg) -> (Task<Msg>, Effect) {
    let count = wizard_steps().len();
    match msg {
        Msg::Next | Msg::Back => {
            let Some(index) = state.wizard else {
                return (Task::none(), Effect::None);
            };
            if matches!(msg, Msg::Next) && !can_go_on(state) {
                return (Task::none(), Effect::None);
            }
            let next = if matches!(msg, Msg::Next) {
                (index + 1).min(count - 1)
            } else {
                index.saturating_sub(1)
            };
            state.wizard = Some(next);
            state.confirm = None;
            let task = if matches!(wizard_step_id(next), "deps" | "checks") {
                local_task().map(Msg::Local)
            } else {
                Task::none()
            };
            (task, Effect::None)
        }
        Msg::SkipDeps => {
            state.deps_skipped = true;
            update(state, Msg::Next)
        }
        Msg::AskFix(need) => {
            if state.fixing.is_none() {
                state.confirm = Some(need);
            }
            (Task::none(), Effect::None)
        }
        Msg::CancelFix => {
            state.confirm = None;
            (Task::none(), Effect::None)
        }
        Msg::RunFix(need) => {
            if state.fixing.is_some() {
                return (Task::none(), Effect::None);
            }
            state.confirm = None;
            state.fixing = Some(need);
            state.log.clear();
            state.fix_error = None;
            (run_fix(need), Effect::None)
        }
        Msg::FixLog(line) => {
            state.log.push(line);
            if state.log.len() > 200 {
                state.log.remove(0);
            }
            (Task::none(), Effect::None)
        }
        // The fix is over (it worked or not): the checks are read again.
        Msg::FixDone(result) => {
            state.fixing = None;
            state.fix_error = result.err();
            (local_task().map(Msg::Local), Effect::None)
        }
        Msg::Finish => {
            set_flag("setup_done");
            state.wizard = None;
            state.offer = !flag("tour_done");
            (Task::none(), Effect::None)
        }
        // The wizard steps aside for the screen; it comes back on the next start until Finish or Skip.
        Msg::OpenScreen(id) => {
            state.wizard = None;
            (Task::none(), Effect::Navigate(id))
        }
        Msg::Change => (Task::none(), Effect::Chooser),
        Msg::Dismiss => {
            state.wizard = None;
            state.confirm = None;
            (Task::none(), Effect::None)
        }
        Msg::Keep => (Task::none(), Effect::None),
        Msg::Look(id) => (Task::none(), Effect::Save(appearance::choose_look(id))),
        Msg::Status(status) => {
            state.status = Some(status);
            (Task::none(), Effect::None)
        }
        Msg::Local(checks) => {
            state.local = Some(checks);
            (Task::none(), Effect::None)
        }
        Msg::StartTour => {
            start_tour(state);
            (Task::none(), Effect::None)
        }
        Msg::SkipOffer => {
            set_flag("tour_done");
            state.offer = false;
            (Task::none(), Effect::None)
        }
        Msg::TourNext | Msg::TourBack => {
            if let Some(index) = state.tour {
                let last = tour_steps().len() - 1;
                state.tour = Some(if matches!(msg, Msg::TourNext) {
                    (index + 1).min(last)
                } else {
                    index.saturating_sub(1)
                });
            }
            (Task::none(), Effect::None)
        }
        Msg::EndTour => {
            if state.tour.take().is_some() {
                set_flag("tour_done");
            }
            (Task::none(), Effect::None)
        }
    }
}

/// A tile of the wizard's lists: the sheet-2 ground and the skin's radius (the web's `.onb-row` and `.onb-box`).
fn tile<'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content)
        .padding(Padding {
            top: 10.0,
            right: 12.0,
            bottom: 10.0,
            left: 12.0,
        })
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().sheet_2.into()),
            border: Border::default().rounded(crate::theme::skin().radius),
            ..Default::default()
        })
        .into()
}

/// A ruled box with the sheet-2 ground: a fix waiting for confirmation, a fix's log.
fn ruled<'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    container(content)
        .padding(12)
        .width(Length::Fill)
        .style(ui::frame(Some(p.sheet_2), p.rule))
        .into()
}

/// The progress rail: a bar and the title of each step. The current one is ink, the done ones dimmed (the web's
/// `.onb-rail`).
fn rail<'a>(index: usize) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let cells: Vec<Element<'a, Msg>> = wizard_steps()
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let (on, done) = (i == index, i < index);
            let color = if on || done { p.signal } else { p.rule_strong };
            let alpha = if done && !on { 0.45 } else { 1.0 };
            let bar = container(Space::new().width(Length::Fill).height(Length::Fixed(4.0)))
                .width(Length::Fill)
                .style(move |_: &Theme| container::Style {
                    background: Some(Color { a: alpha, ..color }.into()),
                    border: Border::default().rounded(2.0),
                    ..Default::default()
                });
            let ink = if on {
                p.ink
            } else if done {
                p.ink_2
            } else {
                p.ink_3
            };
            column![
                bar,
                ui::sans(field(step, "title"), 11.0, ink).width(Length::Fill)
            ]
            .spacing(7)
            .width(Length::FillPortion(1))
            .into()
        })
        .collect();
    row(cells).spacing(8).into()
}

/// A look's choice: the one in use is outlined in the action colour on the signal ground (the web's
/// `.look-pick[aria-pressed="true"]`).
fn pick_style(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_: &Theme, status: button::Status| {
        let p = crate::theme::palette();
        let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (background, edge, width) = if on {
            (p.signal_soft, p.signal, 2.0)
        } else if hot {
            (p.hover, p.rule_strong, 1.0)
        } else {
            (p.raised, p.rule_strong, 1.0)
        };
        button::Style {
            background: Some(background.into()),
            text_color: p.ink,
            border: Border {
                color: edge,
                width,
                radius: crate::theme::skin().radius.into(),
            },
            ..Default::default()
        }
    }
}

/// The look step: Loom and Graphite side by side, each with its words, and a link to Appearance for the rest.
fn look_choices<'a>() -> Element<'a, Msg> {
    let current = crate::theme::appearance().theme;
    let pick = |id: &'static str, name: &'static str, about: &'static str| -> Element<'a, Msg> {
        let p = crate::theme::palette();
        button(
            column![
                ui::sans(t(name), 14.5, p.ink).font(crate::theme::SANS_SEMIBOLD),
                ui::sans(t(about), 12.5, p.ink_3),
            ]
            .spacing(4),
        )
        .on_press(Msg::Look(id))
        .padding(12)
        .width(Length::FillPortion(1))
        .style(pick_style(current == id))
        .into()
    };
    column![
        row![
            pick("loom-clay", "wizard.look.loom", "wizard.look.loom_about"),
            pick(
                "graphite-cobalt",
                "wizard.look.graphite",
                "wizard.look.graphite_about"
            ),
        ]
        .spacing(10),
        ui::btn_sm(
            t("wizard.look.open_appearance"),
            None,
            Kind::Ghost,
            Some(Msg::OpenScreen(Id::Appearance)),
        ),
    ]
    .spacing(12)
    .into()
}

/// The hub step: the console's host and whether the hub answers, and the way to another hub.
fn hub_content<'a>(state: &State, hub: &str) -> Vec<Element<'a, Msg>> {
    let p = crate::theme::palette();
    let host = hub
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    let (word, tone) = match state.status.as_ref().map(|s| s.reachable && s.hub) {
        None => (t("wizard.checking"), Tone::Muted),
        Some(true) => (t("wizard.reachable"), Tone::Ok),
        Some(false) => (t("wizard.unreachable"), Tone::Danger),
    };
    vec![
        tile(
            row![
                ui::sans(t("wizard.connected_to"), 13.0, p.ink_2),
                ui::mono(host, 13.0, p.ink).font(crate::theme::MONO_MEDIUM),
                ui::badge_dot::<Msg>(word, tone),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ),
        ui::btn_sm(t("wizard.change_hub"), None, Kind::Ghost, Some(Msg::Change)),
    ]
}

/// The dependencies step: each requirement with its state and, when missing, the fix that sets it up (shown
/// first, run only once confirmed), the fix's log while it runs, and the way on ("Skip for now").
fn deps_content<'a>(state: &State) -> Vec<Element<'a, Msg>> {
    let p = crate::theme::palette();
    let Some(checks) = state.local.as_deref() else {
        return vec![ui::faint(t("wizard.checking")).into()];
    };
    let busy = state.fixing.is_some();
    let mut list: Vec<Element<'a, Msg>> = Vec::new();
    for check in checks {
        let (word, tone) = match check.state {
            setup::State::Ok => ("Ready", Tone::Ok),
            setup::State::Missing => ("Not ready", Tone::Warn),
            setup::State::Warning => ("Warning", Tone::Warn),
            setup::State::Blocked => ("Needs you", Tone::Danger),
        };
        let action: Element<'a, Msg> = if state.fixing == Some(check.need) {
            ui::faint(t("wizard.deps.running")).into()
        } else if check.state == setup::State::Missing && check.fix.is_some() {
            let label = if check.need == Need::Docker {
                t("wizard.deps.install")
            } else {
                t("wizard.deps.setup")
            };
            ui::btn_sm(
                label,
                None,
                Kind::Secondary,
                (!busy).then_some(Msg::AskFix(check.need)),
            )
        } else {
            Space::new().width(Length::Shrink).into()
        };
        list.push(tile(
            row![
                column![
                    ui::sans(check.label, 13.0, p.ink).font(crate::theme::SANS_MEDIUM),
                    ui::faint(check.detail.clone()),
                ]
                .spacing(3)
                .width(Length::Fill),
                ui::badge_dot::<Msg>(word, tone),
                action,
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        ));
        if state.confirm == Some(check.need)
            && let Some(fix) = &check.fix
        {
            list.push(ruled(
                column![
                    ui::prose(t("wizard.deps.confirm")),
                    ui::prose(fix.clone()),
                    row![
                        ui::btn_sm(
                            t("wizard.deps.run"),
                            None,
                            Kind::Primary,
                            Some(Msg::RunFix(check.need))
                        ),
                        Space::new().width(Length::Fill),
                        ui::btn_sm(
                            t("wizard.deps.cancel"),
                            None,
                            Kind::Ghost,
                            Some(Msg::CancelFix)
                        ),
                    ]
                    .align_y(Alignment::Center),
                ]
                .spacing(10),
            ));
        }
    }
    if !state.log.is_empty() {
        let tail: Vec<Element<'a, Msg>> = state
            .log
            .iter()
            .rev()
            .take(8)
            .rev()
            .map(|line| ui::mono(line.clone(), 11.0, p.ink_3).into())
            .collect();
        list.push(ruled(column(tail).spacing(2)));
    }
    let mut notes: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(error) = &state.fix_error {
        notes.push(ui::error_line(error));
    }
    notes.push(
        ui::prose(t(if setup::ready(checks) {
            "wizard.deps.ready"
        } else {
            "wizard.deps.not_ready"
        }))
        .color(p.ink_2)
        .into(),
    );
    notes.push(ui::btn_sm(
        t("wizard.deps.skip"),
        None,
        Kind::Ghost,
        (!busy).then_some(Msg::SkipDeps),
    ));
    vec![
        column(list).spacing(6).into(),
        column(notes).spacing(12).into(),
    ]
}

/// The model keys step: the count of providers with a key, each provider with its dot, and Open Models.
fn keys_content<'a>(state: &State) -> Vec<Element<'a, Msg>> {
    let p = crate::theme::palette();
    let count = match state.status.as_ref() {
        None => t("wizard.checking").to_string(),
        Some(s) => match s.keys {
            Some((n, m)) => fill(
                t("wizard.keys_count"),
                &[("n", n.to_string()), ("m", m.to_string())],
            ),
            None => "Unknown".into(),
        },
    };
    let mut out: Vec<Element<'a, Msg>> = vec![ui::prose(count).into()];
    if let Some(s) = state.status.as_ref() {
        let providers: Vec<Element<'a, Msg>> = s
            .providers
            .iter()
            .map(|(id, has_key)| {
                tile(
                    row![
                        ui::mono(id.clone(), 12.5, p.ink),
                        Space::new().width(Length::Fill),
                        ui::dot::<Msg>(if *has_key { p.ok } else { p.ink_3 }),
                    ]
                    .align_y(Alignment::Center),
                )
            })
            .collect();
        out.push(column(providers).spacing(6).into());
    }
    out.push(ui::btn_sm(
        t("wizard.open_models"),
        None,
        Kind::Ghost,
        Some(Msg::OpenScreen(Id::Models)),
    ));
    out
}

/// The checks step: the four rows, each Ready, Not ready or Checking, and how many are ready.
fn checks_content<'a>(state: &State) -> Vec<Element<'a, Msg>> {
    let p = crate::theme::palette();
    let states = check_states(state.status.as_ref(), state.local.as_deref());
    let rows: Vec<Element<'a, Msg>> = states
        .iter()
        .map(|(label, ok)| {
            let (word, tone) = match ok {
                None => (t("wizard.checking"), Tone::Muted),
                Some(true) => ("Ready", Tone::Ok),
                Some(false) => ("Not ready", Tone::Warn),
            };
            tile(
                row![
                    ui::sans(*label, 13.0, p.ink).width(Length::Fill),
                    ui::badge_dot::<Msg>(word, tone),
                ]
                .align_y(Alignment::Center),
            )
        })
        .collect();
    let mut out: Vec<Element<'a, Msg>> = vec![column(rows).spacing(6).into()];
    if state.status.is_some() {
        let ready = states.iter().filter(|(_, ok)| *ok == Some(true)).count();
        out.push(
            ui::faint(fill(
                t("wizard.checks_ready"),
                &[("n", ready.to_string()), ("m", states.len().to_string())],
            ))
            .into(),
        );
    }
    out
}

/// The wizard as a dialog (the web's `.scrim > .sheet.onboard`): the head with the step's title, Skip setup and
/// the close button; the progress rail, the step's words and its content; the foot with Back and Next. The close
/// button and a click on the scrim end it without the setup flag, so it opens again on the next start.
pub fn wizard_dialog<'a>(state: &State, hub: &str) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let steps = wizard_steps();
    let index = state.wizard.unwrap_or(0).min(steps.len() - 1);
    let step = &steps[index];
    let content: Vec<Element<'a, Msg>> = match field(step, "id") {
        "hub" => hub_content(state, hub),
        "look" => vec![look_choices()],
        "deps" => deps_content(state),
        "keys" => keys_content(state),
        "checks" => checks_content(state),
        _ => Vec::new(),
    };
    let head = container(
        row![
            text(field(step, "title"))
                .font(crate::theme::SANS_SEMIBOLD)
                .size(15.0)
                .color(p.ink)
                .width(Length::Fill),
            ui::btn_sm(t("wizard.skip"), None, Kind::Ghost, Some(Msg::Finish)),
            ui::btn_sm("", Some("x"), Kind::Ghost, Some(Msg::Dismiss)),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding(Padding {
        top: 16.0,
        right: 12.0,
        bottom: 6.0,
        left: 20.0,
    });
    let body = column![
        rail(index),
        column![ui::prose(field(step, "body")), column(content).spacing(12),].spacing(12),
    ]
    .spacing(18);
    let last = index + 1 == steps.len();
    let foot = container(
        row![
            ui::btn_sm(
                t("wizard.back"),
                None,
                Kind::Ghost,
                (index > 0).then_some(Msg::Back)
            ),
            Space::new().width(Length::Fill),
            if last {
                ui::btn_sm(t("wizard.finish"), None, Kind::Primary, Some(Msg::Finish))
            } else {
                ui::btn_sm(
                    t("wizard.next"),
                    None,
                    Kind::Primary,
                    can_go_on(state).then_some(Msg::Next),
                )
            },
        ]
        .align_y(Alignment::Center),
    )
    .padding(Padding {
        top: 12.0,
        right: 16.0,
        bottom: 12.0,
        left: 16.0,
    });
    let rule = container(Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(p.rule.into()),
            ..Default::default()
        });
    let sheet = container(column![
        head,
        ui::scroll(container(body).padding(Padding {
            top: 6.0,
            right: 20.0,
            bottom: 16.0,
            left: 20.0,
        }))
        .height(Length::Shrink),
        rule,
        foot,
    ])
    .width(Length::Fixed(WIZARD_WIDTH))
    .max_height(760.0)
    .style(crate::theme::pop);
    // A click on the sheet is taken by the sheet; a click on the scrim closes the wizard.
    let scrim = container(mouse_area(sheet).on_press(Msg::Keep))
        .width(Length::Fill)
        .height(Length::Fill)
        .center(Length::Fill)
        .style(crate::theme::scrim);
    mouse_area(scrim).on_press(Msg::Dismiss).into()
}

/// The tour's callout (beside its entry) or the offer (centred over a scrim), when either shows.
pub fn overlay<'a>(state: &State, shell_width: f32, open: bool) -> Option<Element<'a, Msg>> {
    if let Some(index) = state.tour {
        return Some(tour_layer(index, shell_width, open));
    }
    state.offer.then(offer_layer)
}

fn tour_layer<'a>(index: usize, shell_width: f32, open: bool) -> Element<'a, Msg> {
    let steps = tour_steps();
    let index = index.min(steps.len() - 1);
    let step = &steps[index];
    let last = index + 1 == steps.len();
    let top = screen_id(field(step, "screen"))
        .map(|id| anchor_y(&id, open))
        .unwrap_or(BRAND_H);
    let count = fill(
        t("tour.count"),
        &[
            ("n", (index + 1).to_string()),
            ("m", steps.len().to_string()),
        ],
    );
    let card = container(
        column![
            ui::sans(count, 10.5, crate::theme::palette().ink_3),
            ui::sans(
                field(step, "title").to_string(),
                13.5,
                crate::theme::palette().ink
            ),
            ui::prose(field(step, "body")),
            row![
                ui::btn(t("tour.skip"), None, Kind::Ghost, Some(Msg::EndTour)),
                Space::new().width(Length::Fill),
                ui::btn(
                    t("tour.back"),
                    None,
                    Kind::Ghost,
                    (index > 0).then_some(Msg::TourBack)
                ),
                if last {
                    ui::btn(t("tour.done"), None, Kind::Primary, Some(Msg::EndTour))
                } else {
                    ui::btn(t("tour.next"), None, Kind::Primary, Some(Msg::TourNext))
                },
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(8),
    )
    .padding(14)
    .width(Length::Fixed(320.0))
    .style(crate::theme::pop);
    column![
        Space::new().height(Length::Fixed((top - 30.0).max(8.0))),
        row![Space::new().width(Length::Fixed(shell_width + 12.0)), card],
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn offer_layer<'a>() -> Element<'a, Msg> {
    let card = container(
        column![
            ui::sans(
                t("offer.title").to_string(),
                14.0,
                crate::theme::palette().ink
            ),
            row![
                ui::btn(t("offer.skip"), None, Kind::Ghost, Some(Msg::SkipOffer)),
                Space::new().width(Length::Fill),
                ui::btn(t("offer.start"), None, Kind::Primary, Some(Msg::StartTour)),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(16),
    )
    .padding(20)
    .width(Length::Fixed(360.0))
    .style(crate::theme::pop);
    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(|_: &Theme| container::Style {
            background: Some(
                Color {
                    a: 0.35,
                    ..crate::theme::palette().ink
                }
                .into(),
            ),
            ..Default::default()
        })
        .into()
}
