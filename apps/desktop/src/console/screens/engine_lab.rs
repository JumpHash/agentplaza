//! The engine's labs, as the web's hidden `#/robot-lab`, `#/room-lab` and `#/hardware-lab` routes: the engine on a lab
//! stage with the web lab's controls (the options come from the engine's own catalog, `apz_engine::lab_catalog`). Every
//! change starts the stage again with the options set, as the web's lab restarts its engine. Hidden from the sidebar as
//! on the web: `APZ_SCREEN=RobotLab|RoomLab|HardwareLab` opens one.

use std::collections::BTreeMap;

use iced::widget::{checkbox, column, container, pick_list, row, slider, text_input};
use iced::{Alignment, Element, Length, Subscription, Task};
use serde_json::Value;

use super::floor;
use super::Id;
use crate::console::{ui, Ctx};
use crate::theme;

/// Which lab the screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Robot,
    Room,
    Hardware,
}

impl Kind {
    pub fn id(self) -> Id {
        match self {
            Kind::Robot => Id::RobotLab,
            Kind::Room => Id::RoomLab,
            Kind::Hardware => Id::HardwareLab,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Kind::Robot => "Robot lab",
            Kind::Room => "Room lab",
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
            Kind::Room => &[
                ("role", "orchestrator"),
                ("w", "4"),
                ("d", "3.5"),
                ("door", "s"),
                ("seed", "7"),
                ("grid", "0"),
                ("cam", "oblique"),
                ("debug", "0"),
            ],
            Kind::Hardware => &[
                ("model", "exterior/workstation"),
                ("gpus", "0"),
                ("status", "online"),
                ("working", "0"),
                ("open", "0"),
                ("traces", "1"),
                ("cam", "iso"),
            ],
        };
        pairs.iter().map(|&(k, v)| (k, v.to_string())).collect()
    }
}

/// The lab screen: which lab, its options, the model's own params (the hardware lab), and the engine on its stage.
pub struct State {
    kind: Kind,
    values: BTreeMap<&'static str, String>,
    params: BTreeMap<String, String>,
    /// The typed text of the number fields, not yet applied (applied on Enter).
    drafts: BTreeMap<&'static str, String>,
    /// The typed text of the model params, not yet applied (applied on Enter).
    param_drafts: BTreeMap<String, String>,
    stage: floor::State,
    catalog: Value,
    /// The kind the stage was last started for (None: not yet).
    started: Option<Kind>,
}

impl Default for State {
    fn default() -> State {
        let catalog = serde_json::from_str(&apz_engine::lab_catalog::catalog_json()).unwrap_or(Value::Null);
        State {
            kind: Kind::Robot,
            values: Kind::Robot.defaults(),
            params: BTreeMap::new(),
            drafts: BTreeMap::new(),
            param_drafts: BTreeMap::new(),
            stage: floor::State::default(),
            catalog,
            started: None,
        }
    }
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineLab").field("kind", &self.kind).field("values", &self.values).finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// The engine's own messages (frames, size, orbit).
    Stage(floor::Msg),
    /// An option changed: the stage starts again with it.
    Set(&'static str, String),
    /// A number field's text changed: kept as a draft until Enter applies it.
    Draft(&'static str, String),
    /// A number field was submitted: its draft is applied (or reverted when it is not a number).
    Commit(&'static str),
    /// A model param's text changed (the hardware lab): kept as a draft until Enter applies it.
    ParamDraft(String, String),
    /// A model param was submitted: applied when it is a number, else reverted.
    ParamCommit(String),
}

/// The env names the engine's labs read (apz-engine-web's `LAB_OPTIONS`, `ROOM_LAB_OPTIONS`, `HARDWARE_LAB_OPTIONS`).
fn env_of(kind: Kind, values: &BTreeMap<&'static str, String>, params: &BTreeMap<String, String>) -> BTreeMap<String, String> {
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
        Kind::Room => &[
            ("role", "APZ_ROOM_LAB"),
            ("seed", "APZ_ROOM_SEED"),
            ("door", "APZ_ROOM_DOOR"),
            ("grid", "APZ_ROOM_GRID"),
            ("cam", "APZ_ROOM_CAM"),
            ("debug", "APZ_ROOM_DEBUG"),
        ],
        Kind::Hardware => &[
            ("model", "APZ_HW_LAB"),
            ("gpus", "APZ_HW_GPUS"),
            ("status", "APZ_HW_STATUS"),
            ("working", "APZ_HW_WORKING"),
            ("open", "APZ_HW_OPEN"),
            ("traces", "APZ_HW_TRACES"),
            ("cam", "APZ_HW_CAM"),
        ],
    };
    let mut env: BTreeMap<String, String> =
        names.iter().filter_map(|&(k, name)| values.get(k).filter(|v| !v.is_empty()).map(|v| (name.to_string(), v.clone()))).collect();
    match kind {
        Kind::Room => {
            let (w, d) = (values.get("w").cloned().unwrap_or_default(), values.get("d").cloned().unwrap_or_default());
            env.insert("APZ_ROOM_SIZE".into(), format!("{w}x{d}"));
        }
        Kind::Hardware if !params.is_empty() => {
            let pairs: Vec<String> = params.iter().filter(|(_, v)| v.trim().parse::<f64>().is_ok()).map(|(k, v)| format!("{k}={}", v.trim())).collect();
            env.insert("APZ_HW_PARAMS".into(), pairs.join(","));
        }
        _ => {}
    }
    env
}

/// Starts the stage again with the lab's options: they are set while the stage builds (the labs read them then), as
/// the web's `start_lab` sets them for its engine.
fn restart(state: &mut State) {
    apz_engine::set_env_overrides(env_of(state.kind, &state.values, &state.params));
    state.stage = floor::State::default();
    apz_engine::set_env_overrides(BTreeMap::new());
    state.started = Some(state.kind);
}

/// Opens lab `kind`: its defaults, unless it is the lab already shown.
pub fn open(state: &mut State, kind: Kind) -> Task<Msg> {
    if state.started != Some(kind) {
        state.kind = kind;
        state.values = kind.defaults();
        state.params.clear();
        state.drafts.clear();
        state.param_drafts.clear();
        restart(state);
    }
    Task::none()
}

/// The stored value of a number field's typed text, read as the web's lab does: a seed from 0 to 4294967295, floored;
/// a width or depth from 1 to 12, kept as typed. Out of range, or not a number, is None, and the field keeps its last
/// value.
fn number_value(key: &str, text: &str) -> Option<String> {
    let x = text.trim().parse::<f64>().ok().filter(|x| x.is_finite())?;
    if key == "seed" {
        return (0.0..=4_294_967_295.0).contains(&x).then(|| (x.floor() as u64).to_string());
    }
    (1.0..=12.0).contains(&x).then(|| format!("{}", x))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_value_reads_the_size_as_typed() {
        assert_eq!(number_value("width", "3.3").as_deref(), Some("3.3"));
        assert_eq!(number_value("depth", " 12 ").as_deref(), Some("12"));
        assert_eq!(number_value("width", "13"), None);
        assert_eq!(number_value("width", "0.5"), None);
    }

    #[test]
    fn number_value_floors_the_seed_within_range() {
        assert_eq!(number_value("seed", "12.7").as_deref(), Some("12"));
        assert_eq!(number_value("seed", "0").as_deref(), Some("0"));
        assert_eq!(number_value("seed", "4294967295").as_deref(), Some("4294967295"));
        assert_eq!(number_value("seed", "-1"), None);
        assert_eq!(number_value("seed", "4294967296"), None);
    }

    #[test]
    fn number_value_rejects_text_that_is_not_a_number() {
        assert_eq!(number_value("width", "abc"), None);
        assert_eq!(number_value("seed", "abc"), None);
        assert_eq!(number_value("width", "NaN"), None);
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Stage(m) => floor::update(&mut state.stage, m, ctx).map(Msg::Stage),
        Msg::Set(key, value) => {
            if key == "model" {
                state.params.clear(); // another model has its own params
                state.param_drafts.clear();
            }
            if key == "species" && state.kind == Kind::Robot {
                state.values.insert("clip", String::new()); // the clip list follows the species
            }
            state.values.insert(key, value);
            restart(state);
            Task::none()
        }
        Msg::Draft(key, text) => {
            state.drafts.insert(key, text);
            Task::none()
        }
        Msg::Commit(key) => {
            if let Some(value) = state.drafts.remove(key).and_then(|text| number_value(key, &text)) {
                state.values.insert(key, value);
                restart(state);
            }
            Task::none()
        }
        Msg::ParamDraft(key, text) => {
            state.param_drafts.insert(key, text);
            Task::none()
        }
        Msg::ParamCommit(key) => {
            if let Some(text) = state.param_drafts.remove(&key) {
                let text = text.trim().to_string();
                if text.parse::<f64>().is_ok() {
                    state.params.insert(key, text);
                    restart(state);
                }
            }
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
    catalog[key].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
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
fn select<'a>(state: &State, label: &str, key: &'static str, none: Option<&str>, options: Vec<String>) -> Element<'a, Msg> {
    let mut choices: Vec<Choice> = none.map(|n| Choice { value: String::new(), label: n.to_string() }).into_iter().collect();
    choices.extend(options.into_iter().map(|v| Choice { label: v.clone(), value: v }));
    let current = state.values.get(key).cloned().unwrap_or_default();
    let selected = choices.iter().find(|c| c.value == current).cloned();
    field(label, pick_list(choices, selected, move |c: Choice| Msg::Set(key, c.value)).style(theme::pick_list).text_size(13.0).into())
}

/// A labelled number field: the typed text is a draft until Enter applies it (see [`number_value`]).
fn number<'a>(state: &State, label: &str, key: &'static str) -> Element<'a, Msg> {
    let value = state.drafts.get(key).or_else(|| state.values.get(key)).cloned().unwrap_or_default();
    field(
        label,
        text_input("", &value)
            .on_input(move |v| Msg::Draft(key, v))
            .on_submit(Msg::Commit(key))
            .style(theme::input)
            .padding(6.0)
            .width(Length::Fixed(72.0))
            .into(),
    )
}

/// A labelled checkbox stored as 1 or 0.
fn toggle<'a>(state: &State, label: &str, key: &'static str) -> Element<'a, Msg> {
    let on = state.values.get(key).is_some_and(|v| v == "1");
    checkbox(on).label(label.to_string()).on_toggle(move |b| Msg::Set(key, if b { "1" } else { "0" }.into())).text_size(13.0).into()
}

/// A labelled slider over `lo..=hi`.
fn range<'a>(state: &State, label: &str, key: &'static str, lo: f32, hi: f32, step: f32) -> Element<'a, Msg> {
    let v = state.values.get(key).and_then(|v| v.parse::<f32>().ok()).unwrap_or(lo);
    field(
        label,
        row![
            slider(lo..=hi, v, move |x| Msg::Set(key, format!("{}", (x / step).round() * step))).step(step).width(Length::Fixed(120.0)),
            ui::mono(format!("{v:.2}"), 12.0, theme::palette().ink_2),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    )
}

/// The option buttons (the camera, the GPU count): each a (value, label), the selected one in signal orange, as the
/// web's segmented buttons.
fn buttons<'a>(state: &State, key: &'static str, options: &[(&'static str, &'static str)]) -> Element<'a, Msg> {
    let current = state.values.get(key).cloned().unwrap_or_default();
    let mut r = row![].spacing(4);
    for &(value, label) in options {
        let kind = if current == value { ui::Kind::Primary } else { ui::Kind::Secondary };
        r = r.push(ui::btn_sm(label, None, kind, Some(Msg::Set(key, value.to_string()))));
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
                c["speciesClips"].get(&species).and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(String::from)),
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
                select(state, "State", "state", None, ["idle", "working", "walking", "offline"].map(String::from).to_vec()),
                range(state, "Hue", "hue", 0.0, 360.0, 1.0),
                toggle(state, "Orchestrator", "orch"),
                buttons(state, "cam", &[("front", "FRONT"), ("iso", "ISO"), ("side", "SIDE"), ("q34", "Q34"), ("top", "TOP")]),
            ]
        }
        Kind::Room => {
            let mut roles = vec!["all".to_string()];
            roles.extend(list(c, "roles"));
            vec![
                select(state, "Role", "role", None, roles),
                number(state, "Width", "w"),
                number(state, "Depth", "d"),
                select(state, "Door", "door", None, ["n", "e", "s", "w"].map(String::from).to_vec()),
                number(state, "Seed", "seed"),
                toggle(state, "Twelve seeds", "grid"),
                select(state, "Camera", "cam", None, ["oblique", "top"].map(String::from).to_vec()),
                toggle(state, "Debug", "debug"),
            ]
        }
        Kind::Hardware => {
            let model = state.values.get("model").cloned().unwrap_or_default();
            let mut out = vec![
                select(state, "Model", "model", None, list(c, "models")),
                buttons(state, "gpus", &[("0", "0 GPUs"), ("1", "1 GPU"), ("2", "2 GPUs")]),
                select(state, "Status", "status", None, ["online", "draining", "offline", "revoked"].map(String::from).to_vec()),
                range(state, "Working", "working", 0.0, 6.0, 1.0),
                range(state, "Open", "open", 0.0, 1.0, 0.01),
                toggle(state, "Traces (boards)", "traces"),
            ];
            if let Some(defaults) = c["params"].get(&model).and_then(Value::as_object) {
                // The model's params by name, as the web's lab lists them.
                let mut names: Vec<&String> = defaults.keys().collect();
                names.sort();
                for k in names {
                    let shown = state.param_drafts.get(k).or_else(|| state.params.get(k)).cloned().unwrap_or_else(|| defaults[k].to_string());
                    let (draft, commit) = (k.clone(), k.clone());
                    out.push(field(
                        k,
                        text_input("", &shown)
                            .on_input(move |x| Msg::ParamDraft(draft.clone(), x))
                            .on_submit(Msg::ParamCommit(commit))
                            .style(theme::input)
                            .padding(6.0)
                            .width(Length::Fixed(72.0))
                            .into(),
                    ));
                }
            }
            out.push(buttons(state, "cam", &[("iso", "ISO"), ("top", "TOP"), ("front", "FRONT"), ("side", "SIDE"), ("anchor", "ANCHOR")]));
            out
        }
    }
}

/// The lab's readout: the room lab's violations and rooms, the hardware lab's summary (red on errors), the robot lab's
/// draw calls of the last frame.
fn readout(state: &State) -> (String, Vec<String>, bool) {
    let info: Value = serde_json::from_str(&state.stage.lab_info()).unwrap_or(Value::Null);
    let lines = |key: &str| info[key].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    match state.kind {
        Kind::Robot => {
            let calls = crate::console::floor_engine::LAST_DRAW_CALLS.load(std::sync::atomic::Ordering::Relaxed);
            (format!("draw calls (last frame): {calls}"), Vec::new(), false)
        }
        Kind::Room => {
            let debug = state.values.get("debug").is_some_and(|v| v == "1");
            (info["summary"].as_str().unwrap_or_default().to_string(), if debug { lines("lines") } else { Vec::new() }, false)
        }
        Kind::Hardware => {
            let errors: Vec<String> = lines("errors");
            (info["summary"].as_str().unwrap_or_default().to_string(), Vec::new(), !errors.is_empty())
        }
    }
}

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let head = ui::view_head(state.kind.title(), Vec::new());
    let bar = row(controls(state)).spacing(16).align_y(Alignment::End).wrap().vertical_spacing(10);
    let (summary, lines, bad) = readout(state);
    let p = theme::palette();
    let mut body = column![bar, ui::mono(summary, 12.0, if bad { p.crit } else { p.ink_2 })].spacing(12);
    for l in lines {
        body = body.push(ui::mono(l, 12.0, p.ink));
    }
    // The web lab's canvas heights: the field of view is vertical, so the same height frames the same.
    let tall = match state.kind {
        Kind::Room if state.values.get("role").is_some_and(|r| r == "all") => 760.0,
        Kind::Room => 540.0,
        Kind::Robot | Kind::Hardware => 420.0,
    };
    let stage = container(floor::stage(&state.stage, ctx).map(Msg::Stage)).width(Length::Fill).height(Length::Fixed(tall));
    let page = ui::scroll(body.push(stage).height(Length::Shrink)).height(Length::Fill);
    ui::sheet(head, page.into())
}
