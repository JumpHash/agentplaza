//! The first-run wizard and the tour, in the web console's words (`apps/shared/onboarding.json`,
//! read by `apps/web/src/ui/onboarding.ts`). On the first start the wizard takes the window (five
//! steps: welcome, hub, dependencies, model keys, checks); then the tour is offered; the tour is a
//! callout beside each sidebar screen's entry. The flags are `setup_done` and `tour_done` in the
//! desktop settings ([`boot::saved_setting`]). The Setup screen replays both.

use std::sync::OnceLock;

use iced::futures::channel::mpsc;
use iced::futures::{SinkExt, StreamExt};
use iced::widget::{column, container, row, Space};
use iced::{Alignment, Border, Color, Element, Length, Task, Theme};
use serde_json::{json, Value};

use super::screens::{Id, NAV};
use super::setup::{self, Check, Need};
use super::ui::{self, Kind, Tone};
use super::{boot, client, Ctx};

const COPY_JSON: &str = include_str!("../../../shared/onboarding.json");

fn copy() -> &'static Value {
    static COPY: OnceLock<Value> = OnceLock::new();
    COPY.get_or_init(|| serde_json::from_str(COPY_JSON).expect("apps/shared/onboarding.json is valid JSON"))
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
    copy()["wizard"]["steps"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// The index of a wizard step by its id (`checks`, `deps`, …); 0 when there is no such step.
pub fn step_index(id: &str) -> usize {
    wizard_steps().iter().position(|step| field(step, "id") == id).unwrap_or(0)
}

fn tour_steps() -> &'static [Value] {
    copy()["tour"]["steps"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// Fills `{name}` placeholders of a copy string.
pub fn fill(template: &str, values: &[(&str, String)]) -> String {
    values.iter().fold(template.to_string(), |text, (name, value)| text.replace(&format!("{{{name}}}"), value))
}

/// The screen a tour step points at (its key is the screen's label in lower case).
fn screen_id(key: &str) -> Option<Id> {
    NAV.iter().find(|(_, label, ..)| label.eq_ignore_ascii_case(key)).map(|(id, ..)| id.clone())
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
    let Some(id) = highlighted(state) else { return Task::none() };
    // The list starts under the brand block; the entry sits 300 px down from the list's top.
    let y = (anchor_y(&id, open) - BRAND_H - 300.0).max(0.0);
    let scroll: Task<()> = iced::widget::operation::scroll_to(super::shell::SIDEBAR_SCROLL.clone(), iced::widget::scrollable::AbsoluteOffset { x: 0.0, y });
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
}

fn status_of(overview: Option<Value>, providers: Option<Value>) -> Status {
    Status {
        reachable: overview.is_some(),
        hub: overview.as_ref().and_then(|o| o["hub"].as_bool()).unwrap_or(false),
        node: overview.as_ref().and_then(|o| o["noded"].as_bool()).unwrap_or(false),
        keys: providers.and_then(|list| {
            let list = list.as_array()?;
            let set = list.iter().filter(|p| p["credential"].as_str() != Some("missing")).count();
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
    Task::perform(async { setup::check_all(setup::Mode::Local) }, |checks| checks)
}

/// One row of the checks: a label, a tone and the words beside it.
#[derive(Debug, Clone)]
pub struct Row {
    pub label: String,
    pub tone: Tone,
    pub detail: String,
}

/// The checks as the wizard's last step and the Setup screen show them (the same rows, the same words
/// as the web console, plus this machine's requirements).
pub fn rows(status: Option<&Status>, local: Option<&[Check]>) -> Vec<Row> {
    let label = |key: &str| -> String { copy()["wizard"]["checks_labels"][key].as_str().unwrap_or("").to_string() };
    let Some(s) = status else {
        return vec![Row { label: label("hub"), tone: Tone::Muted, detail: t("wizard.checking").into() }];
    };
    let mut out = vec![
        if s.reachable && s.hub {
            Row { label: label("hub"), tone: Tone::Ok, detail: t("wizard.reachable").into() }
        } else {
            Row { label: label("hub"), tone: Tone::Danger, detail: t("wizard.unreachable").into() }
        },
        if s.node {
            Row { label: label("node"), tone: Tone::Ok, detail: "Running".into() }
        } else {
            Row { label: label("node"), tone: Tone::Warn, detail: "Not running".into() }
        },
    ];
    out.push(match s.keys {
        Some((n, m)) => Row {
            label: label("keys"),
            tone: if n > 0 { Tone::Ok } else { Tone::Warn },
            detail: fill(t("wizard.keys_count"), &[("n", n.to_string()), ("m", m.to_string())]),
        },
        None => Row { label: label("keys"), tone: Tone::Muted, detail: "Unknown".into() },
    });
    if let Some(checks) = local {
        let ready = checks.iter().filter(|c| c.state == setup::State::Ok).count();
        out.push(Row {
            label: label("local"),
            tone: if setup::ready(checks) { Tone::Ok } else { Tone::Warn },
            detail: fill(t("wizard.checks_ready"), &[("n", ready.to_string()), ("m", checks.len().to_string())]),
        });
    }
    out
}

/// What the console must do after a message.
pub enum Effect {
    None,
    /// Show another screen (the wizard's "Open Models").
    Navigate(Id),
    /// Back to the startup chooser (the wizard's "Change hub").
    Chooser,
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
    wizard_steps().get(index).map(|s| field(s, "id")).unwrap_or("")
}

/// Whether the wizard may go on from its current step: a dependencies step waits for every required
/// check (or "Skip for now"), and nothing goes on while a fix runs.
pub fn can_go_on(state: &State) -> bool {
    if state.fixing.is_some() {
        return false;
    }
    let deps = state.wizard.is_some_and(|index| wizard_step_id(index) == "deps");
    !deps || state.deps_skipped || state.local.as_deref().is_some_and(setup::ready)
}

/// Runs a fix off the UI thread: its log lines reach the step as they are written, then its result.
fn run_fix(need: Need) -> Task<Msg> {
    enum Step {
        Line(String),
        Done(Result<(), String>),
    }
    Task::stream(iced::stream::channel(64, move |mut out: mpsc::Sender<Msg>| async move {
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
    }))
}

pub fn update(state: &mut State, msg: Msg) -> (Task<Msg>, Effect) {
    let count = wizard_steps().len();
    match msg {
        Msg::Next | Msg::Back => {
            let Some(index) = state.wizard else { return (Task::none(), Effect::None) };
            if matches!(msg, Msg::Next) && !can_go_on(state) {
                return (Task::none(), Effect::None);
            }
            let next = if matches!(msg, Msg::Next) { (index + 1).min(count - 1) } else { index.saturating_sub(1) };
            state.wizard = Some(next);
            state.confirm = None;
            let task = if matches!(wizard_step_id(next), "deps" | "checks") { local_task().map(Msg::Local) } else { Task::none() };
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
                state.tour = Some(if matches!(msg, Msg::TourNext) { (index + 1).min(last) } else { index.saturating_sub(1) });
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

fn badge_row<'a>(r: &Row) -> Element<'a, Msg> {
    row![ui::mono(r.label.clone(), 12.0, ui::INK_2), ui::badge(&r.detail, r.tone)].spacing(10).align_y(Alignment::Center).into()
}

fn step_dot<'a>(on: bool) -> Element<'a, Msg> {
    container(Space::new().width(Length::Fixed(9.0)).height(Length::Fixed(9.0)))
        .style(move |_: &Theme| container::Style {
            background: on.then_some(ui::SIGNAL).map(Into::into),
            border: Border { color: ui::INK, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

/// A box on the paper: the sheet colour, the ink rule.
fn boxed(_: &Theme) -> container::Style {
    container::Style {
        background: Some(ui::SHEET.into()),
        border: Border { color: ui::INK, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}

/// The dependencies step: each requirement and its state, the fix that sets it up (shown first, run
/// only once confirmed), the fix's log while it runs, and the way on ("Skip for now").
fn deps_content<'a>(state: &State) -> Vec<Element<'a, Msg>> {
    let mut out: Vec<Element<'a, Msg>> = Vec::new();
    let Some(checks) = state.local.as_deref() else {
        out.push(ui::faint(t("wizard.checking")).into());
        return out;
    };
    let busy = state.fixing.is_some();
    for check in checks {
        let (word, tone) = match check.state {
            setup::State::Ok => ("Ready", Tone::Ok),
            setup::State::Missing => ("Missing", Tone::Warn),
            setup::State::Warning => ("Warning", Tone::Warn),
            setup::State::Blocked => ("Needs you", Tone::Danger),
        };
        let action: Element<'a, Msg> = if state.fixing == Some(check.need) {
            ui::faint(t("wizard.deps.running")).into()
        } else if check.state == setup::State::Missing && check.fix.is_some() {
            let label = if check.need == Need::Docker { t("wizard.deps.install") } else { t("wizard.deps.setup") };
            ui::btn(label, None, Kind::Secondary, (!busy).then_some(Msg::AskFix(check.need)))
        } else {
            Space::new().width(Length::Shrink).into()
        };
        out.push(
            column![
                row![ui::mono(check.label.to_string(), 12.0, ui::INK_2), ui::badge(word, tone), Space::new().width(Length::Fill), action]
                    .spacing(10)
                    .align_y(Alignment::Center),
                ui::faint(check.detail.clone()),
            ]
            .spacing(4)
            .into(),
        );
        if state.confirm == Some(check.need) {
            if let Some(fix) = &check.fix {
                out.push(
                    container(column![
                        ui::prose(t("wizard.deps.confirm")),
                        ui::prose(fix.clone()),
                        row![
                            ui::btn(t("wizard.deps.run"), None, Kind::Primary, Some(Msg::RunFix(check.need))),
                            Space::new().width(Length::Fill),
                            ui::btn(t("wizard.deps.cancel"), None, Kind::Ghost, Some(Msg::CancelFix)),
                        ]
                        .align_y(Alignment::Center),
                    ])
                    .padding(12)
                    .style(boxed)
                    .into(),
                );
            }
        }
    }
    if !state.log.is_empty() {
        let tail: Vec<Element<'a, Msg>> = state.log.iter().rev().take(8).rev().map(|line| ui::mono(line.clone(), 11.0, ui::INK_3).into()).collect();
        out.push(container(column(tail).spacing(2)).padding(8).style(boxed).into());
    }
    if let Some(error) = &state.fix_error {
        out.push(ui::error_line(error));
    }
    out.push(ui::faint(t(if setup::ready(checks) { "wizard.deps.ready" } else { "wizard.deps.not_ready" })).into());
    out.push(ui::btn(t("wizard.deps.skip"), None, Kind::Ghost, (!busy).then_some(Msg::SkipDeps)));
    out
}

/// The wizard: the window's page, one step at a time.
pub fn wizard_view<'a>(state: &'a State, hub: &str) -> Element<'a, Msg> {
    let steps = wizard_steps();
    let index = state.wizard.unwrap_or(0).min(steps.len() - 1);
    let step = &steps[index];
    let checks = rows(state.status.as_ref(), state.local.as_deref());
    let mut content: Vec<Element<'a, Msg>> =
        vec![ui::mono(field(step, "title").to_string(), 16.0, ui::INK).into(), ui::prose(field(step, "body")).into()];
    match field(step, "id") {
        "hub" => {
            content.push(ui::prose(format!("{} {}", t("wizard.connected_to"), hub)).into());
            content.push(badge_row(&checks[0]));
            content.push(ui::btn(t("wizard.change_hub"), None, Kind::Secondary, Some(Msg::Change)));
        }
        "deps" => content.extend(deps_content(state)),
        "keys" => {
            content.push(badge_row(&checks[2]));
            content.push(ui::btn(t("wizard.open_models"), None, Kind::Secondary, Some(Msg::OpenScreen(Id::Models))));
        }
        "checks" => content.extend(checks.iter().map(badge_row)),
        _ => {}
    }
    let last = index + 1 == steps.len();
    let dots: Vec<Element<'a, Msg>> = (0..steps.len()).map(|i| step_dot(i == index)).collect();
    let head = row![
        iced::widget::row(dots).spacing(6),
        Space::new().width(Length::Fill),
        ui::btn(t("wizard.skip"), None, Kind::Ghost, Some(Msg::Finish)),
    ]
    .align_y(Alignment::Center);
    let foot = row![
        ui::btn(t("wizard.back"), None, Kind::Ghost, (index > 0).then_some(Msg::Back)),
        Space::new().width(Length::Fill),
        if last {
            ui::btn(t("wizard.finish"), None, Kind::Primary, Some(Msg::Finish))
        } else {
            ui::btn(t("wizard.next"), None, Kind::Primary, can_go_on(state).then_some(Msg::Next))
        },
    ]
    .align_y(Alignment::Center);
    let card =
        container(column![head, iced::widget::column(content).spacing(12), foot].spacing(22)).padding(28).max_width(560.0).style(boxed);
    container(card)
        .padding(24)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(ui::paper)
        .into()
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
    let top = screen_id(field(step, "screen")).map(|id| anchor_y(&id, open)).unwrap_or(BRAND_H);
    let count = fill(t("tour.count"), &[("n", (index + 1).to_string()), ("m", steps.len().to_string())]);
    let card = container(
        column![
            ui::mono(count, 10.5, ui::INK_3),
            ui::mono(field(step, "title").to_string(), 13.5, ui::INK),
            ui::prose(field(step, "body")),
            row![
                ui::btn(t("tour.skip"), None, Kind::Ghost, Some(Msg::EndTour)),
                Space::new().width(Length::Fill),
                ui::btn(t("tour.back"), None, Kind::Ghost, (index > 0).then_some(Msg::TourBack)),
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
    .style(boxed);
    column![Space::new().height(Length::Fixed((top - 30.0).max(8.0))), row![Space::new().width(Length::Fixed(shell_width + 12.0)), card],]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn offer_layer<'a>() -> Element<'a, Msg> {
    let card = container(
        column![
            ui::mono(t("offer.title").to_string(), 14.0, ui::INK),
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
    .style(boxed);
    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(|_: &Theme| container::Style { background: Some(Color { a: 0.35, ..ui::INK }.into()), ..Default::default() })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wizard_has_five_steps_and_the_tour_six_to_eight() {
        assert!(wizard_steps().len() <= 5, "the wizard has at most five steps");
        assert_eq!(field(&wizard_steps()[2], "id"), "deps", "the dependencies step is third, after the hub");
        assert_eq!(step_index("deps"), 2);
        assert!((6..=8).contains(&tour_steps().len()));
    }

    #[test]
    fn every_tour_step_points_at_a_sidebar_screen() {
        for step in tour_steps() {
            assert!(screen_id(field(step, "screen")).is_some(), "no screen for {}", field(step, "screen"));
        }
    }

    #[test]
    fn copy_fills_its_placeholders() {
        assert_eq!(fill(t("wizard.keys_count"), &[("n", "1".into()), ("m", "3".into())]), "1 of 3 providers have a key");
        assert_eq!(t("wizard.next"), "Next");
        assert_eq!(t("no.such.key"), "");
    }

    #[test]
    fn the_tour_anchors_sit_inside_the_sidebar() {
        // Below the brand block, and past the fold of a 900px window for the last entries: `reveal` scrolls
        // the sidebar to them, so only the order and the start are fixed here.
        for step in tour_steps() {
            let id = screen_id(field(step, "screen")).expect("a sidebar screen");
            let y = anchor_y(&id, true);
            assert!(y > BRAND_H, "{} at {y}", field(step, "screen"));
        }
        assert!(anchor_y(&Id::Overview, true) < anchor_y(&Id::Setup, true));
    }

    #[test]
    fn keys_and_hub_status_read_as_the_web_reads_them() {
        let providers = json!([{ "credential": "missing" }, { "credential": "env" }, { "credential": "secret" }]);
        let overview = json!({ "hub": true, "noded": false });
        let status = status_of(Some(overview), Some(providers));
        assert_eq!(status, Status { reachable: true, hub: true, node: false, keys: Some((2, 3)) });
        let rows = rows(Some(&status), None);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].detail, "Reachable");
        assert_eq!(rows[1].detail, "Not running");
        assert_eq!(rows[2].detail, "2 of 3 providers have a key");
    }

    #[test]
    fn an_unreachable_hub_says_so() {
        let status = status_of(None, None);
        assert!(!status.reachable);
        assert_eq!(rows(Some(&status), None)[0].tone, Tone::Danger);
    }

    #[test]
    fn the_dependencies_step_waits_for_the_required_checks() {
        let check = |need, state, required| Check { need, label: "row", state, detail: String::new(), fix: None, required };
        let mut state = State { wizard: Some(2), ..Default::default() };
        assert!(!can_go_on(&state), "not read yet");
        state.local = Some(vec![check(Need::Docker, setup::State::Ok, true), check(Need::Garage, setup::State::Missing, false)]);
        assert!(can_go_on(&state), "an optional requirement missing does not hold the wizard");
        state.local = Some(vec![check(Need::Docker, setup::State::Missing, true)]);
        assert!(!can_go_on(&state), "a required one missing does");
        state.deps_skipped = true;
        assert!(can_go_on(&state), "Skip for now goes on");
        state.fixing = Some(Need::Docker);
        assert!(!can_go_on(&state), "nothing goes on while a fix runs");
    }

    #[test]
    fn step_moves_stay_inside_the_steps() {
        // Skipped dependencies: the wizard goes on past its step without the checks read.
        let mut state = State { wizard: Some(0), deps_skipped: true, ..Default::default() };
        let _ = update(&mut state, Msg::Back);
        assert_eq!(state.wizard, Some(0));
        for _ in 0..10 {
            let _ = update(&mut state, Msg::Next);
        }
        assert_eq!(state.wizard, Some(wizard_steps().len() - 1));
    }
}
