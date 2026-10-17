//! The engine's labs, as the web's hidden `#/robot-lab` and `#/hardware-lab` routes: the engine on a lab
//! stage with the web lab's controls (the options come from the engine's own catalog, `apz_engine::lab_catalog`). Every
//! change starts the stage again with the options set, as the web's lab restarts its engine. Hidden from the sidebar as
//! on the web: `APZ_SCREEN=RobotLab|HardwareLab` opens one.

use std::collections::BTreeMap;

use iced::widget::{checkbox, column, container, pick_list, row, slider};
use iced::{Alignment, Element, Length, Subscription, Task};
use serde_json::Value;

use super::Id;
use super::floor;
use crate::console::{Ctx, ui};
use crate::theme;

/// Which lab the screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Robot,
    Hardware,
}

impl Kind {
    pub fn id(self) -> Id {
        match self {
            Kind::Robot => Id::RobotLab,
            Kind::Hardware => Id::HardwareLab,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Kind::Robot => "Robot lab",
            Kind::Hardware => "Hardware lab",
        }
    }

    /// The web lab's defaults (its `readState` with an empty route).
    fn defaults(self) -> BTreeMap<&'static str, String> {
        let pairs: &[(&str, &str)] = match self {
            Kind::Robot => &[
                ("species", "all"),
                ("prop", ""),
                ("clip", ""),
                ("tool", ""),
                ("carry", ""),
                ("fx", ""),
                ("state", "idle"),
                ("hue", "200"),
                ("orch", "0"),
                ("cam", "front"),
            ],
            Kind::Hardware => &[
                ("model", "all"),
                ("lod", "0"),
                ("style", "ink"),
                ("theme", "dark"),
                ("status", "working"),
                ("cam", "iso"),
            ],
        };
        pairs.iter().map(|&(k, v)| (k, v.to_string())).collect()
    }
}

/// The lab screen: which lab, its options, and the engine on its stage.
pub struct State {
    kind: Kind,
    values: BTreeMap<&'static str, String>,
    stage: floor::State,
    catalog: Value,
    /// The kind the stage was last started for (None: not yet).
    started: Option<Kind>,
}

impl Default for State {
    fn default() -> State {
        let catalog =
            serde_json::from_str(&apz_engine::lab_catalog::catalog_json()).unwrap_or(Value::Null);
        State {
            kind: Kind::Robot,
            values: Kind::Robot.defaults(),
            stage: floor::State::default(),
            catalog,
            started: None,
        }
    }
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineLab")
            .field("kind", &self.kind)
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

/// The env names the engine's labs read (apz-engine-web's `LAB_OPTIONS` and `HARDWARE_LAB_OPTIONS`).
fn env_of(kind: Kind, values: &BTreeMap<&'static str, String>) -> BTreeMap<String, String> {
    let names: &[(&str, &str)] = match kind {
        Kind::Robot => &[
            ("species", "APZ_ROBOT_LAB"),
            ("clip", "APZ_ROBOT_CLIP"),
            ("tool", "APZ_ROBOT_TOOL"),
            ("carry", "APZ_ROBOT_CARRY"),
            ("state", "APZ_ROBOT_STATE"),
            ("hue", "APZ_ROBOT_HUE"),
            ("orch", "APZ_ROBOT_ORCH"),
            ("cam", "APZ_ROBOT_CAM"),
            ("prop", "APZ_ROBOT_PROP"),
            ("fx", "APZ_ROBOT_FX"),
        ],
        Kind::Hardware => &[
            ("model", "APZ_HW_LAB"),
            ("lod", "APZ_HW_LOD"),
            ("style", "APZ_HW_STYLE"),
            ("theme", "APZ_HW_THEME"),
            ("status", "APZ_HW_STATUS"),
            ("cam", "APZ_HW_CAM"),
            ("ortho", "APZ_HW_ORTHO"),
        ],
    };
    let env: BTreeMap<String, String> = names
        .iter()
        .filter_map(|&(k, name)| {
            values
                .get(k)
                .filter(|v| !v.is_empty())
                .map(|v| (name.to_string(), v.clone()))
        })
        .collect();
    env
}

/// Starts the stage again with the lab's options: they are set while the stage builds (the labs read them then), as
/// the web's `start_lab` sets them for its engine.
fn restart(state: &mut State) {
    apz_engine::set_env_overrides(env_of(state.kind, &state.values));
    state.stage = floor::State::default();
    apz_engine::set_env_overrides(BTreeMap::new());
    state.started = Some(state.kind);
}

/// Opens lab `kind`: its defaults, unless it is the lab already shown.
pub fn open(state: &mut State, kind: Kind) -> Task<Msg> {
    if state.started != Some(kind) {
        state.kind = kind;
        state.values = kind.defaults();
        restart(state);
    }
    Task::none()
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Stage(m) => floor::update(&mut state.stage, m, ctx).map(Msg::Stage),
        Msg::Set(key, value) => {
            if key == "species" && state.kind == Kind::Robot {
                state.values.insert("clip", String::new()); // the clip list follows the species
            }
            state.values.insert(key, value);
            restart(state);
            Task::none()
        }
    }
}

/// The engine's frames while the lab is shown.
pub fn subscription(state: &State) -> Subscription<Msg> {
    floor::subscription(&state.stage).map(Msg::Stage)
}

/// A catalog list (`species`, `props` …) as strings.
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

/// A labelled slider over `lo..=hi`.
fn range<'a>(
    state: &State,
    label: &str,
    key: &'static str,
    lo: f32,
    hi: f32,
    step: f32,
) -> Element<'a, Msg> {
    let v = state
        .values
        .get(key)
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(lo);
    field(
        label,
        row![
            slider(lo..=hi, v, move |x| Msg::Set(
                key,
                format!("{}", (x / step).round() * step)
            ))
            .step(step)
            .width(Length::Fixed(120.0)),
            ui::sans(format!("{v:.2}"), 12.0, theme::palette().ink_2),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    )
}

/// The option buttons (the camera, the GPU count): each a (value, label), the selected one in signal orange, as the
/// web's segmented buttons.
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

/// The lab's controls, the web lab's in its order.
fn controls<'a>(state: &State) -> Vec<Element<'a, Msg>> {
    let c = &state.catalog;
    match state.kind {
        Kind::Robot => {
            let species = state.values.get("species").cloned().unwrap_or_default();
            // The clips: the shared ones and the species' own, de-duplicated and sorted as the web's list.
            let mut clips = list(c, "clips");
            clips.extend(
                c["speciesClips"]
                    .get(&species)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(String::from)),
            );
            clips.sort();
            clips.dedup();
            let mut all = vec!["all".to_string()];
            all.extend(list(c, "species"));
            let mut props = vec!["all".to_string()];
            props.extend(list(c, "props"));
            vec![
                select(state, "Species", "species", None, all),
                select(state, "Prop", "prop", Some("(no prop)"), props),
                select(state, "Clip", "clip", Some("(state clip)"), clips),
                select(state, "Tool", "tool", Some("(no tool)"), list(c, "tools")),
                select(state, "Carry", "carry", Some("(nothing)"), list(c, "carry")),
                select(state, "Effect", "fx", Some("(no effect)"), list(c, "fx")),
                select(
                    state,
                    "State",
                    "state",
                    None,
                    ["idle", "working", "walking", "offline"]
                        .map(String::from)
                        .to_vec(),
                ),
                range(state, "Hue", "hue", 0.0, 360.0, 1.0),
                toggle(state, "Orchestrator", "orch"),
                buttons(
                    state,
                    "cam",
                    &[
                        ("front", "FRONT"),
                        ("iso", "ISO"),
                        ("side", "SIDE"),
                        ("q34", "Q34"),
                        ("top", "TOP"),
                    ],
                ),
            ]
        }
        Kind::Hardware => {
            let mut models = vec!["all".to_string()];
            models.extend(list(c, "models"));
            let mut out = vec![
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
            ];
            out.push(buttons(
                state,
                "cam",
                &[
                    ("iso", "ISO"),
                    ("top", "TOP"),
                    ("front", "FRONT"),
                    ("side", "SIDE"),
                    ("anchor", "ANCHOR"),
                ],
            ));
            out
        }
    }
}

/// Whether the robot lab shows a prop (its prop screen): the same readout as the web's lab, which counts the props and
/// the whole frame there.
fn prop_screen(state: &State) -> bool {
    state.values.get("prop").is_some_and(|v| !v.is_empty())
}

/// The robot lab's readout line, in the web lab's wording: the species screen's draw calls per robot, or the prop
/// screen's props, robots and whole-frame calls (the main pass).
fn robot_draws_text(counts: &apz_engine::robots::lab::DrawCounts, prop: bool) -> String {
    if prop {
        return format!(
            "props: {} · robots: {} · draw calls (main pass, whole frame): {}",
            counts.props,
            counts.robots.len(),
            counts.calls
        );
    }
    let parts: Vec<String> = counts
        .robots
        .iter()
        .map(|r| format!("{} {}", r.species, r.calls))
        .collect();
    format!("draw calls per robot (main pass): {}", parts.join(" · "))
}

/// The lab's readout: the hardware lab's summary (red on errors), the robot lab's draw calls of the last frame.
fn readout(state: &State) -> (String, bool) {
    let info: Value = serde_json::from_str(&state.stage.lab_info()).unwrap_or(Value::Null);
    let lines = |key: &str| {
        info[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    match state.kind {
        Kind::Robot => {
            use crate::console::floor_engine::{LAST_MAIN_CALLS, LAST_OWNED_CALLS};
            let owned = LAST_OWNED_CALLS
                .lock()
                .map(|v| v.clone())
                .unwrap_or_default();
            let calls = LAST_MAIN_CALLS.load(std::sync::atomic::Ordering::Relaxed);
            let text =
                state.stage.lab.as_ref().map(|lab| {
                    robot_draws_text(&lab.draw_counts(&owned, calls), prop_screen(state))
                });
            (text.unwrap_or_default(), false)
        }
        Kind::Hardware => {
            let errors: Vec<String> = lines("errors");
            (
                info["summary"].as_str().unwrap_or_default().to_string(),
                !errors.is_empty(),
            )
        }
    }
}

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let head = ui::view_head(state.kind.title(), Vec::new());
    let bar = row(controls(state))
        .spacing(16)
        .align_y(Alignment::End)
        .wrap()
        .vertical_spacing(10);
    let (summary, bad) = readout(state);
    let p = theme::palette();
    let body = column![
        bar,
        ui::sans(summary, 12.0, if bad { p.crit } else { p.ink_2 })
    ]
    .spacing(12);
    // The web lab's canvas height: the field of view is vertical, so the same height frames the same.
    let stage = container(floor::stage(&state.stage, ctx).map(Msg::Stage))
        .width(Length::Fill)
        .height(Length::Fixed(420.0));
    let page = ui::scroll(body.push(stage).height(Length::Shrink)).height(Length::Fill);
    ui::sheet(head, page.into())
}
