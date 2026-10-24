//! The hardware lab (sidebar `Lab`, and `APZ_SCREEN=HardwareLab`): the machine model kit on the engine's stage with the
//! web lab's controls, as the web's hidden `#/hardware-lab` route. The options come from the engine's own catalog
//! (`apz_engine::lab_catalog`). Every change starts the stage again with its options set, since the engine reads them
//! while it builds, as the web's lab restarts its engine.

use std::collections::BTreeMap;

use iced::widget::{Space, checkbox, column, container, pick_list, row};
use iced::{Alignment, Element, Length, Subscription, Task};
use serde_json::Value;

use super::Id;
use super::floor;
use crate::console::{Ctx, ui};
use crate::theme;

/// The lab's options and their defaults: the web lab's `readState` with an empty route.
fn defaults() -> BTreeMap<&'static str, String> {
    [
        ("model", "all"),
        ("lod", "0"),
        ("style", "ink"),
        ("theme", "dark"),
        ("status", "working"),
        ("cam", "iso"),
    ]
    .into_iter()
    .map(|(key, value)| (key, value.to_string()))
    .collect()
}

/// The lab screen: its options, and the engine on its stage once the screen has been opened.
pub struct State {
    values: BTreeMap<&'static str, String>,
    /// The engine's stage, built the first time the screen is opened (None until then).
    stage: Option<floor::State>,
    catalog: Value,
}

impl Default for State {
    fn default() -> State {
        let catalog =
            serde_json::from_str(&apz_engine::lab_catalog::catalog_json()).unwrap_or(Value::Null);
        State {
            values: defaults(),
            stage: None,
            catalog,
        }
    }
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HardwareLab")
            .field("values", &self.values)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// The engine's own messages (frames, size, orbit).
    Stage(floor::Msg),
    /// An option changed: the stage starts again with it.
    Set(&'static str, String),
}

/// The env names the engine's hardware lab reads (apz-engine-web's `HARDWARE_LAB_OPTIONS`). APZ_HW_ORTHO is set only
/// for 1: the engine reads it as a flag, and the web's lab leaves it out for 0.
fn env_of(values: &BTreeMap<&'static str, String>) -> BTreeMap<String, String> {
    const NAMES: [(&str, &str); 7] = [
        ("model", "APZ_HW_LAB"),
        ("lod", "APZ_HW_LOD"),
        ("style", "APZ_HW_STYLE"),
        ("theme", "APZ_HW_THEME"),
        ("status", "APZ_HW_STATUS"),
        ("cam", "APZ_HW_CAM"),
        ("ortho", "APZ_HW_ORTHO"),
    ];
    NAMES
        .iter()
        .filter_map(|&(key, name)| {
            let value = values.get(key).filter(|v| !v.is_empty())?;
            (name != "APZ_HW_ORTHO" || value == "1").then(|| (name.to_string(), value.clone()))
        })
        .collect()
}

/// Starts the stage again with the lab's options: they are set while the stage builds (the engine reads them then),
/// as the web's `start_hardware_lab` sets them for its engine.
fn restart(state: &mut State) {
    apz_engine::set_env_overrides(env_of(&state.values));
    state.stage = Some(floor::State::default());
    apz_engine::set_env_overrides(BTreeMap::new());
}

/// Opens the lab: its stage is built the first time the screen shows, and kept while the sidebar moves on.
pub fn load(state: &mut State, _ctx: &Ctx) -> Task<Msg> {
    if state.stage.is_none() {
        restart(state);
    }
    Task::none()
}

/// The lab's messages name no screen of their own.
pub fn go(_msg: &Msg) -> Option<Id> {
    None
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Stage(m) => match state.stage.as_mut() {
            Some(stage) => floor::update(stage, m, ctx).map(Msg::Stage),
            None => Task::none(),
        },
        Msg::Set(key, value) => {
            state.values.insert(key, value);
            restart(state);
            Task::none()
        }
    }
}

/// The engine's frames while the lab is shown.
pub fn subscription(state: &State) -> Subscription<Msg> {
    match &state.stage {
        Some(stage) => floor::subscription(stage).map(Msg::Stage),
        None => Subscription::none(),
    }
}

/// A catalog list (`models` …) as strings.
fn list(catalog: &Value, key: &str) -> Vec<String> {
    catalog[key]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// A dropdown choice: its value and the label shown (an empty value reads as `none`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Choice {
    value: String,
    label: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// A labelled dropdown over `options`, an empty first option labelled `none` when given.
fn select<'a>(
    state: &State,
    label: &str,
    key: &'static str,
    none: Option<&str>,
    options: Vec<String>,
) -> Element<'a, Msg> {
    let mut choices: Vec<Choice> = none
        .map(|n| Choice {
            value: String::new(),
            label: n.to_string(),
        })
        .into_iter()
        .collect();
    choices.extend(options.into_iter().map(|v| Choice {
        label: v.clone(),
        value: v,
    }));
    let current = state.values.get(key).cloned().unwrap_or_default();
    let selected = choices.iter().find(|c| c.value == current).cloned();
    field(
        label,
        pick_list(choices, selected, move |c: Choice| Msg::Set(key, c.value))
            .style(theme::pick_list)
            .text_size(13.0)
            .into(),
    )
}

/// A labelled checkbox stored as 1 or 0.
fn toggle<'a>(state: &State, label: &str, key: &'static str) -> Element<'a, Msg> {
    let on = state.values.get(key).is_some_and(|v| v == "1");
    checkbox(on)
        .label(label.to_string())
        .on_toggle(move |b| Msg::Set(key, if b { "1" } else { "0" }.into()))
        .text_size(13.0)
        .into()
}

/// The option buttons (the camera, the level of detail): each a (value, label), the selected one in signal orange,
/// as the web's segmented buttons.
fn buttons<'a>(
    state: &State,
    key: &'static str,
    options: &[(&'static str, &'static str)],
) -> Element<'a, Msg> {
    let current = state.values.get(key).cloned().unwrap_or_default();
    let mut r = row![].spacing(4);
    for &(value, label) in options {
        let kind = if current == value {
            ui::Kind::Primary
        } else {
            ui::Kind::Secondary
        };
        r = r.push(ui::btn_sm(
            label,
            None,
            kind,
            Some(Msg::Set(key, value.to_string())),
        ));
    }
    r.into()
}

fn field<'a>(label: &str, input: Element<'a, Msg>) -> Element<'a, Msg> {
    column![ui::label(label), input].spacing(4).into()
}

/// The lab's controls, in the web lab's order.
fn controls<'a>(state: &State) -> Vec<Element<'a, Msg>> {
    let mut models = vec!["all".to_string()];
    models.extend(list(&state.catalog, "models"));
    vec![
        select(state, "Model", "model", None, models),
        buttons(state, "lod", &[("0", "FULL"), ("1", "MID"), ("2", "BLOCK")]),
        select(
            state,
            "Style",
            "style",
            None,
            ["ink", "blueprint", "night"].map(String::from).to_vec(),
        ),
        buttons(state, "theme", &[("light", "LIGHT"), ("dark", "DARK")]),
        select(
            state,
            "Status",
            "status",
            None,
            ["working", "idle", "strained", "offline"]
                .map(String::from)
                .to_vec(),
        ),
        toggle(state, "Orthographic", "ortho"),
        buttons(
            state,
            "cam",
            &[
                ("iso", "ISO"),
                ("top", "TOP"),
                ("front", "FRONT"),
                ("side", "SIDE"),
                ("back", "BACK"),
            ],
        ),
    ]
}

/// The lab's readout: the engine's summary of what it shows, in red when the engine reports errors.
fn readout(state: &State) -> (String, bool) {
    let info: Value = state
        .stage
        .as_ref()
        .and_then(|stage| serde_json::from_str(&stage.lab_info()).ok())
        .unwrap_or(Value::Null);
    let failed = info["errors"]
        .as_array()
        .is_some_and(|errors| !errors.is_empty());
    (
        info["summary"].as_str().unwrap_or_default().to_string(),
        failed,
    )
}

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let head = ui::view_head("Hardware lab", Vec::new());
    let bar = row(controls(state))
        .spacing(16)
        .align_y(Alignment::End)
        .wrap()
        .vertical_spacing(10);
    let (summary, failed) = readout(state);
    let p = theme::palette();
    let body = column![
        bar,
        ui::sans(summary, 12.0, if failed { p.crit } else { p.ink_2 })
    ]
    .spacing(12);
    let stage: Element<'a, Msg> = match &state.stage {
        Some(stage) => floor::stage(stage, ctx).map(Msg::Stage),
        None => Space::new().into(),
    };
    // The web lab's canvas height: the field of view is vertical, so the same height frames the same.
    let stage = container(stage)
        .width(Length::Fill)
        .height(Length::Fixed(420.0));
    let page = ui::scroll(body.push(stage).height(Length::Shrink)).height(Length::Fill);
    ui::sheet(head, page.into())
}
