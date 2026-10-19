//! The model picker (the providers brief, "The model picker"): a popover with a rail on the left (Favourites
//! first, then one icon per enabled API instance in its accent), a search field, and the models as rows: the
//! model id in mono, the instance label in ink-3, and a star that toggles a favourite. An instance whose model
//! listing failed lists no models: its message and "Open provider setup" stand in for them.
//!
//! The choices come from the hub's model list ([`choices`]). The rail, the accents, the favourites and the
//! failed instances come from the same list through [`sync`], which the caller runs when it loads the models;
//! until then the rail still shows one icon per provider in the choices. Starring, opening the setup and
//! choosing a model are for the caller to act on (see [`PickerMsg`]).

use std::collections::BTreeSet;

use iced::widget::{Space, button, column, container, pick_list, row, text_input};
use iced::{Alignment, Border, Color, Element, Length, Theme};
use serde_json::Value;

use super::brands;
use super::ui::{self, Tone};

/// One model the picker offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// `provider/model`, as agents and settings store it.
    pub id: String,
    /// The brand key for `ui::brand_logo` (e.g. "anthropic", "openai", "zai"); for an API instance, its id.
    pub provider: String,
    pub label: String,
}

/// What the models list says about one API instance: its label, accent, and the listing error when it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    pub id: String,
    pub label: String,
    /// A `#rrggbb` accent, or None when the instance has none.
    pub accent: Option<String>,
    /// The message of the instance's failed listing (its health is crit): it lists no models.
    pub crit: Option<String>,
}

/// The instances and favourites the models list carries (see [`meta`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
    pub instances: Vec<Instance>,
    /// The starred models, `provider/model`. The list carries only the models it lists; the owner's full list is
    /// the settings' `favourite_models`, which the caller saves ([`with_favourite`]).
    pub favourites: Vec<String>,
}

/// Rows a group or subgroup shows before its "Show N more" control.
pub const SHOW_FIRST: usize = 12;
/// The provider whose models are split by upstream vendor (the second segment of the model id).
const VENDORED: &str = "openrouter";
/// The vendor bucket for models with no vendor segment; listed last.
const OTHER: &str = "other";
/// The note the hub's label carries for contributor models (`trains_on_data`).
const TRAINS_NOTE: &str = " · WARNING: trains on your data";
/// The popover's list height.
const LIST_HEIGHT: f32 = 280.0;

/// The rail's selection: the starred models, or one instance's models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rail {
    Favourites,
    Instance(String),
}

/// The picker's own state (open or closed, the search text, the rail, the sections opened past their first rows).
#[derive(Debug, Clone, Default)]
pub struct Picker {
    pub open: bool,
    pub query: String,
    /// The rail's selection; None lists every model.
    pub rail: Option<Rail>,
    /// The instances and favourites the models list says (see [`sync`]).
    pub meta: Meta,
    /// The `section_key`s whose "Show N more" was pressed.
    expanded: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub enum PickerMsg {
    Toggle,
    Query(String),
    /// A model was chosen (its id).
    Pick(String),
    /// The reasoning effort changed ("", "low", "medium", "high", "max", …).
    Effort(String),
    /// "Show N more" of the section with this `section_key`.
    More(String),
    /// A rail item was pressed: its models show; pressed again, every model shows.
    Rail(Rail),
    /// A model's star was pressed (its `provider/model` id). The caller saves the owner's favourites with it
    /// ([`with_favourite`] over the settings' list).
    Favourite(String),
    /// "Open provider setup" of a failed instance (its id): the caller opens Settings › Models with it selected.
    Setup(String),
}

/// A reasoning effort the picker offers; `value` "" is the model's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EffortOpt {
    value: &'static str,
    label: &'static str,
}

impl std::fmt::Display for EffortOpt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label)
    }
}

/// The efforts the web picker offers (`apps/web/src/views/agent-detail.ts`).
const EFFORTS: &[EffortOpt] = &[
    EffortOpt {
        value: "",
        label: "Model default",
    },
    EffortOpt {
        value: "none",
        label: "None",
    },
    EffortOpt {
        value: "minimal",
        label: "Minimal",
    },
    EffortOpt {
        value: "low",
        label: "Low",
    },
    EffortOpt {
        value: "medium",
        label: "Medium",
    },
    EffortOpt {
        value: "high",
        label: "High",
    },
    EffortOpt {
        value: "xhigh",
        label: "Extra high",
    },
    EffortOpt {
        value: "max",
        label: "Max",
    },
];

/// The choices from the hub's model list (`GET /v1/models` and the provider catalogue, as the web
/// picker reads them). Decision models (group "decisions") are left out: agents never chat with them.
pub fn choices(models: &Value) -> Vec<Choice> {
    models
        .get("models")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter(|m| m.get("group").and_then(Value::as_str) != Some("decisions"))
                .filter_map(|m| {
                    let provider = m.get("provider")?.as_str()?.to_string();
                    let model = m.get("id")?.as_str()?;
                    let name = m
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|n| !n.is_empty())
                        .unwrap_or(model);
                    let label = if m.get("trains_on_data").and_then(Value::as_bool) == Some(true) {
                        format!("{name}{TRAINS_NOTE}")
                    } else {
                        name.to_string()
                    };
                    Some(Choice {
                        id: format!("{provider}/{model}"),
                        provider,
                        label,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The instances, favourites and failed listings the hub's model list (`GET /v1/models`) carries: one
/// instance per provider in the order the list first names it (its `provider_label` and `accent`), the starred
/// entries (`favourite`), and each `errors` entry as the crit message of its instance.
pub fn meta(models: &Value) -> Meta {
    let mut out = Meta::default();
    for m in models
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = m.get("provider").and_then(Value::as_str) else {
            continue;
        };
        if !out.instances.iter().any(|i| i.id == id) {
            out.instances.push(Instance {
                id: id.to_string(),
                label: m
                    .get("provider_label")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(id)
                    .to_string(),
                accent: m.get("accent").and_then(Value::as_str).map(str::to_string),
                crit: None,
            });
        }
        let starred = m.get("favourite").and_then(Value::as_bool) == Some(true);
        if let (true, Some(model)) = (starred, m.get("id").and_then(Value::as_str)) {
            let key = format!("{id}/{model}");
            if !out.favourites.contains(&key) {
                out.favourites.push(key);
            }
        }
    }
    for e in models
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = e.get("provider").and_then(Value::as_str) else {
            continue;
        };
        let message = e
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("The provider did not list its models.")
            .to_string();
        match out.instances.iter_mut().find(|i| i.id == id) {
            Some(i) => i.crit = Some(message),
            None => out.instances.push(Instance {
                id: id.to_string(),
                label: id.to_string(),
                accent: None,
                crit: Some(message),
            }),
        }
    }
    out
}

/// Takes the instances and favourites of `models` into the picker (the caller runs it when it loads the models).
pub fn sync(state: &mut Picker, models: &Value) {
    state.meta = meta(models);
}

/// `current` with `id` starred, or unstarred when it is already in it: the list the caller saves as the
/// settings' `favourite_models`.
pub fn with_favourite(current: &[String], id: &str) -> Vec<String> {
    if current.iter().any(|f| f == id) {
        current
            .iter()
            .filter(|f| f.as_str() != id)
            .cloned()
            .collect()
    } else {
        let mut out = current.to_vec();
        out.push(id.to_string());
        out
    }
}

/// Stars or unstars `id` (`provider/model`) in the owner's favourites on the hub: the settings are read, the list
/// changed with [`with_favourite`] and saved. The favourites as saved.
pub fn save_favourite(client: &super::client::Client, id: &str) -> Result<Vec<String>, String> {
    let mut settings: Value = client.get("/v1/settings")?;
    let current: Vec<String> = settings["favourite_models"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let next = with_favourite(&current, id);
    settings["favourite_models"] = serde_json::json!(next);
    let saved: Value = client.put("/v1/settings", &settings)?;
    Ok(saved["favourite_models"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or(next))
}

/// `models` (a `/v1/models` answer) with each entry's `favourite` flag set from `favourites`.
pub fn mark_favourites(models: &mut Value, favourites: &[String]) {
    if let Some(list) = models["models"].as_array_mut() {
        for m in list {
            let name = format!(
                "{}/{}",
                m["provider"].as_str().unwrap_or(""),
                m["id"].as_str().unwrap_or("")
            );
            m["favourite"] = Value::Bool(favourites.contains(&name));
        }
    }
}

/// The colour of a `#rrggbb` accent, or None for anything else (an unset or malformed accent).
pub fn accent_color(hex: &str) -> Option<Color> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(digits, 16).ok()?;
    Some(Color::from_rgb8((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// The letters of a monogram for `key`: the first letters of its first two words ("my-hub" is "MH"), or its
/// first two characters for one word.
fn initials(key: &str) -> String {
    let parts: Vec<&str> = key
        .split(|c: char| !c.is_alphanumeric())
        .filter(|p| !p.is_empty())
        .collect();
    let letters: String = match parts.as_slice() {
        [] => "?".to_string(),
        [one] => one.chars().take(2).collect(),
        [first, second, ..] => first
            .chars()
            .take(1)
            .chain(second.chars().take(1))
            .collect(),
    };
    letters.to_uppercase()
}

/// A provider's mark: its brand logo when the brands know the key, else a monogram of the key on a tint of
/// its accent (`size` px square).
pub fn mark<'a, M: 'a>(key: &str, accent: Option<&str>, size: f32) -> Element<'a, M> {
    if brands::get(key).is_some() {
        return ui::brand_logo(key, size);
    }
    let tint = accent
        .and_then(accent_color)
        .unwrap_or_else(|| crate::theme::palette().ink_3);
    container(ui::mono(
        initials(key),
        size * 0.42,
        crate::theme::palette().ink,
    ))
    .center(Length::Fixed(size))
    .style(move |_: &Theme| container::Style {
        background: Some(Color { a: 0.22, ..tint }.into()),
        border: Border::default().rounded(size * 0.28),
        ..Default::default()
    })
    .into()
}

/// Whether the hub lists the `provider/model` `id` as a contributor model (`trains_on_data`).
pub fn trains_on_data(models: &Value, id: &str) -> bool {
    models
        .get("models")
        .and_then(Value::as_array)
        .is_some_and(|list| {
            list.iter().any(|m| {
                m.get("trains_on_data").and_then(Value::as_bool) == Some(true)
                    && m.get("provider")
                        .and_then(Value::as_str)
                        .zip(m.get("id").and_then(Value::as_str))
                        .is_some_and(|(p, i)| format!("{p}/{i}") == id)
            })
        })
}

/// Splits a `provider/model` id at its first slash, so the model keeps its own slashes
/// (`openrouter/anthropic/claude-sonnet-4.5` is `openrouter` and `anthropic/claude-sonnet-4.5`).
pub fn split_id(id: &str) -> (&str, &str) {
    id.split_once('/').unwrap_or((id, ""))
}

/// Splits a model at its first slash: the upstream vendor (None without a slash) and the rest,
/// the model's name (`anthropic/claude-sonnet-4.5` is `anthropic` and `claude-sonnet-4.5`).
pub fn split_vendor(model: &str) -> (Option<&str>, &str) {
    match model.split_once('/') {
        Some((vendor, name)) => (Some(vendor), name),
        None => (None, model),
    }
}

fn is_vendored(provider: &str) -> bool {
    provider == VENDORED
}

/// The label without the contributor note.
fn base_label(c: &Choice) -> &str {
    c.label.strip_suffix(TRAINS_NOTE).unwrap_or(&c.label)
}

/// Whether the hub flags the model as training on the user's data.
fn trains(c: &Choice) -> bool {
    c.label.ends_with(TRAINS_NOTE)
}

/// The name a row shows: for openrouter the model's name after its vendor, else the label.
pub fn display_name(c: &Choice) -> &str {
    if is_vendored(&c.provider) {
        split_vendor(split_id(&c.id).1).1
    } else {
        base_label(c)
    }
}

/// The choices every whitespace-separated term of `query` matches, case-insensitively, in the id
/// or the display name; all of them when the query is blank.
pub fn matching<'a>(choices: &'a [Choice], query: &str) -> Vec<&'a Choice> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    choices
        .iter()
        .filter(|c| {
            if terms.is_empty() {
                return true;
            }
            let haystack = format!("{} {} {}", c.id, base_label(c), display_name(c)).to_lowercase();
            terms.iter().all(|t| haystack.contains(t.as_str()))
        })
        .collect()
}

/// The upstream vendor of an openrouter model; "other" when its id has no vendor segment.
fn vendor_of(c: &Choice) -> &str {
    split_vendor(split_id(&c.id).1)
        .0
        .filter(|v| !v.is_empty())
        .unwrap_or(OTHER)
}

/// One provider's matching models, split into sections.
pub struct Group<'a> {
    pub provider: &'a str,
    /// The matching models of all its sections.
    pub count: usize,
    pub sections: Vec<Section<'a>>,
}

/// The models of one provider under one vendor. `vendor` is "" for a provider that is not split.
pub struct Section<'a> {
    pub vendor: &'a str,
    pub rows: Vec<&'a Choice>,
}

/// Groups matching models by provider, in the order the providers first appear. Openrouter's
/// sections are its vendors, sorted by name with "other" last; the rows keep their order.
pub fn group<'a>(hits: &[&'a Choice]) -> Vec<Group<'a>> {
    let mut groups: Vec<Group<'a>> = Vec::new();
    for &c in hits {
        let i = match groups
            .iter()
            .position(|g| g.provider == c.provider.as_str())
        {
            Some(i) => i,
            None => {
                groups.push(Group {
                    provider: c.provider.as_str(),
                    count: 0,
                    sections: Vec::new(),
                });
                groups.len() - 1
            }
        };
        let g = &mut groups[i];
        g.count += 1;
        let vendor = if is_vendored(&c.provider) {
            vendor_of(c)
        } else {
            ""
        };
        match g.sections.iter_mut().find(|s| s.vendor == vendor) {
            Some(s) => s.rows.push(c),
            None => g.sections.push(Section {
                vendor,
                rows: vec![c],
            }),
        }
    }
    for g in &mut groups {
        g.sections
            .sort_by(|a, b| (a.vendor == OTHER, a.vendor).cmp(&(b.vendor == OTHER, b.vendor)));
    }
    groups
}

/// The key a section's "Show N more" is remembered under.
pub fn section_key(provider: &str, vendor: &str) -> String {
    if vendor.is_empty() {
        provider.to_string()
    } else {
        format!("{provider}/{vendor}")
    }
}

/// The first `SHOW_FIRST` of `rows` (all of them once `expanded`), and how many are left out.
pub fn cap<T>(rows: &[T], expanded: bool) -> (&[T], usize) {
    if expanded || rows.len() <= SHOW_FIRST {
        (rows, 0)
    } else {
        (&rows[..SHOW_FIRST], rows.len() - SHOW_FIRST)
    }
}

/// Applies a message to the picker's own state (Toggle, Query, Pick, More, Rail, Favourite, Setup); the chosen
/// id or effort is for the caller to act on.
pub fn update(state: &mut Picker, msg: &PickerMsg) {
    match msg {
        PickerMsg::Toggle => {
            state.open = !state.open;
            state.query.clear();
            state.expanded.clear();
        }
        PickerMsg::Query(q) => state.query = q.clone(),
        PickerMsg::Pick(_) => {
            state.open = false;
            state.query.clear();
            state.expanded.clear();
        }
        PickerMsg::More(key) => {
            state.expanded.insert(key.clone());
        }
        PickerMsg::Rail(r) => {
            state.rail = if state.rail.as_ref() == Some(r) {
                None
            } else {
                Some(r.clone())
            };
            state.expanded.clear();
        }
        PickerMsg::Favourite(id) => {
            state.meta.favourites = with_favourite(&state.meta.favourites, id);
        }
        PickerMsg::Setup(_) => state.open = false,
        PickerMsg::Effort(_) => {}
    }
}

/// The picker: the trigger (the instance's mark, the current model, a chevron), and when open the popover;
/// the effort selector beside it when `effort` is Some.
pub fn view<'a, M: Clone + 'a>(
    state: &'a Picker,
    choices: &'a [Choice],
    current: &'a str,
    effort: Option<&'a str>,
    wrap: fn(PickerMsg) -> M,
) -> Element<'a, M> {
    let cur = choices.iter().find(|c| c.id == current);
    let mut trigger_row = row![].spacing(8).align_y(Alignment::Center);
    if let Some(c) = cur {
        trigger_row = trigger_row.push(mark(&c.provider, accent_of(state, &c.provider), 16.0));
    }
    let (shown, color) = if current.is_empty() {
        ("Choose a model".to_string(), crate::theme::palette().ink_3)
    } else {
        (current.to_string(), crate::theme::palette().ink)
    };
    trigger_row = trigger_row.push(ui::sans(shown, 12.0, color));
    trigger_row = trigger_row.push(Space::new().width(Length::Fill));
    trigger_row = trigger_row.push(ui::icon(
        "chevron-down",
        14.0,
        crate::theme::palette().ink_3,
    ));
    let trigger = button(trigger_row)
        .on_press(wrap(PickerMsg::Toggle))
        .width(Length::Fill)
        .padding([6, 10])
        .style(plain);

    let mut head = row![trigger].spacing(8).align_y(Alignment::Center);
    if let Some(sel) = effort {
        let selected = EFFORTS.iter().copied().find(|o| o.value == sel);
        head = head.push(pick_list(EFFORTS, selected, move |o: EffortOpt| {
            wrap(PickerMsg::Effort(o.value.to_string()))
        }));
    }
    let mut col = column![head].spacing(6);
    if state.open {
        col = col.push(popover(state, choices, current, wrap));
    }
    col.into()
}

/// The accent the picker knows for `provider`'s instance, if any.
fn accent_of<'a>(state: &'a Picker, provider: &str) -> Option<&'a str> {
    state
        .meta
        .instances
        .iter()
        .find(|i| i.id == provider)
        .and_then(|i| i.accent.as_deref())
}

/// The instances the rail shows: the picker's own (enabled, with their accents) and any provider the choices
/// name that the list did not describe.
fn known_instances(state: &Picker, choices: &[Choice]) -> Vec<Instance> {
    let mut out = state.meta.instances.clone();
    for c in choices {
        if !out.iter().any(|i| i.id == c.provider) {
            out.push(Instance {
                id: c.provider.clone(),
                label: c.provider.clone(),
                accent: None,
                crit: None,
            });
        }
    }
    out
}

/// Whether the rail's selection lists `c`.
fn in_rail(state: &Picker, c: &Choice) -> bool {
    match &state.rail {
        None => true,
        Some(Rail::Favourites) => state.meta.favourites.contains(&c.id),
        Some(Rail::Instance(id)) => c.provider == *id,
    }
}

/// The label of the instance that serves `c`.
fn instance_label<'a>(state: &'a Picker, c: &'a Choice) -> &'a str {
    state
        .meta
        .instances
        .iter()
        .find(|i| i.id == c.provider)
        .map_or(c.provider.as_str(), |i| i.label.as_str())
}

/// The popover: the rail on the left, the search field and the rows on the right.
fn popover<'a, M: Clone + 'a>(
    state: &'a Picker,
    choices: &'a [Choice],
    current: &'a str,
    wrap: fn(PickerMsg) -> M,
) -> Element<'a, M> {
    let instances = known_instances(state, choices);
    let shown: Vec<&'a Choice> = matching(choices, &state.query)
        .into_iter()
        .filter(|c| in_rail(state, c))
        .collect();

    // Enter picks the first row the search leaves.
    let mut search = text_input("Search models…", &state.query)
        .on_input(move |s| wrap(PickerMsg::Query(s)))
        .font(crate::theme::SANS)
        .padding(6)
        .size(12.0)
        .style(crate::theme::input);
    if let Some(first) = shown.first() {
        search = search.on_submit(wrap(PickerMsg::Pick(first.id.clone())));
    }

    let mut list = column![].spacing(2);
    for inst in instances.iter().filter(|i| i.crit.is_some()) {
        let listed = match &state.rail {
            None => true,
            Some(Rail::Instance(id)) => *id == inst.id,
            Some(Rail::Favourites) => false,
        };
        if listed {
            list = list.push(failed_instance(inst, wrap));
        }
    }
    for g in group(&shown) {
        for s in &g.sections {
            if !s.vendor.is_empty() {
                list = list.push(
                    container(ui::faint(format!("{} · {}", s.vendor, s.rows.len())))
                        .padding([0, 10]),
                );
            }
            let key = section_key(g.provider, s.vendor);
            let (rows, hidden) = cap(&s.rows, state.expanded.contains(&key));
            for m in rows.iter().copied() {
                list = list.push(model_row(m, current, state, wrap));
            }
            if hidden > 0 {
                let more = button(ui::faint(format!("Show {hidden} more")))
                    .on_press(wrap(PickerMsg::More(key)))
                    .width(Length::Fill)
                    .padding([6, 10])
                    .style(plain);
                list = list.push(more);
            }
        }
    }
    if shown.is_empty() && !instances.iter().any(|i| i.crit.is_some()) {
        let none = if matches!(state.rail, Some(Rail::Favourites)) {
            "No favourites yet. Star a model to list it here."
        } else {
            "No models match"
        };
        list = list.push(ui::faint(none));
    }

    let body = column![search, ui::scroll(list).height(Length::Fixed(LIST_HEIGHT))].spacing(8);
    let panel = container(row![
        rail(state, &instances, wrap),
        container(body).padding(8).width(Length::Fill),
    ])
    .width(Length::Fill)
    .style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().sheet.into()),
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: crate::theme::skin().radius.into(),
        },
        ..Default::default()
    });
    panel.into()
}

/// The rail: the star for Favourites, a rule, then one icon per instance in its accent. The selected item has
/// the signal ground.
fn rail<'a, M: Clone + 'a>(
    state: &'a Picker,
    instances: &[Instance],
    wrap: fn(PickerMsg) -> M,
) -> Element<'a, M> {
    let fav_on = matches!(state.rail, Some(Rail::Favourites));
    let fav_ink = if fav_on {
        crate::theme::palette().signal
    } else {
        crate::theme::palette().ink_3
    };
    let mut col = column![rail_button(
        ui::icon("star", 14.0, fav_ink),
        fav_on,
        wrap(PickerMsg::Rail(Rail::Favourites)),
    )]
    .spacing(6)
    .align_x(Alignment::Center);
    if !instances.is_empty() {
        col = col.push(
            container(Space::new().height(1.0))
                .width(Length::Fixed(18.0))
                .style(|_: &Theme| container::Style {
                    background: Some(crate::theme::palette().rule_faint.into()),
                    ..Default::default()
                }),
        );
    }
    for inst in instances {
        let on = matches!(&state.rail, Some(Rail::Instance(id)) if *id == inst.id);
        col = col.push(rail_button(
            mark(&inst.id, inst.accent.as_deref(), 16.0),
            on,
            wrap(PickerMsg::Rail(Rail::Instance(inst.id.clone()))),
        ));
    }
    container(col)
        .padding([8, 6])
        .width(Length::Fixed(40.0))
        .height(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().paper_side.into()),
            ..Default::default()
        })
        .into()
}

/// One rail item: 28 px, the signal ground and ring when selected, a faint wash under the pointer.
fn rail_button<'a, M: Clone + 'a>(
    content: Element<'a, M>,
    selected: bool,
    msg: M,
) -> Element<'a, M> {
    button(container(content).center(Length::Fill))
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(28.0))
        .padding(0.0)
        .style(move |_: &Theme, status: button::Status| {
            let p = crate::theme::palette();
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: if selected {
                    Some(p.signal_soft.into())
                } else if hovered {
                    Some(p.hover.into())
                } else {
                    None
                },
                text_color: p.ink,
                border: Border {
                    color: if selected {
                        p.signal
                    } else {
                        Color::TRANSPARENT
                    },
                    width: 1.0,
                    radius: crate::theme::skin().radius.into(),
                },
                ..Default::default()
            }
        })
        .on_press(msg)
        .into()
}

/// A failed instance: its mark and label, the listing's message, and "Open provider setup".
fn failed_instance<'a, M: Clone + 'a>(inst: &Instance, wrap: fn(PickerMsg) -> M) -> Element<'a, M> {
    column![
        row![
            mark(&inst.id, inst.accent.as_deref(), 14.0),
            ui::sans(inst.label.clone(), 12.0, crate::theme::palette().ink),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        ui::faint(inst.crit.clone().unwrap_or_default()),
        ui::text_link(
            "Open provider setup",
            wrap(PickerMsg::Setup(inst.id.clone()))
        ),
    ]
    .spacing(4)
    .padding([6, 10])
    .into()
}

/// One model row: the model id in mono with the contributor warning and the "current" mark beside it, the
/// instance label in faint text at the right, and the star. The name part picks the model; the star toggles
/// the favourite.
fn model_row<'a, M: Clone + 'a>(
    c: &'a Choice,
    current: &str,
    state: &'a Picker,
    wrap: fn(PickerMsg) -> M,
) -> Element<'a, M> {
    let is_current = c.id == current;
    let starred = state.meta.favourites.contains(&c.id);
    let mut line = row![ui::mono(
        split_id(&c.id).1.to_string(),
        12.0,
        crate::theme::palette().ink
    )]
    .spacing(8)
    .align_y(Alignment::Center);
    if trains(c) {
        line = line.push(ui::badge("Trains on data", Tone::Warn));
    }
    if is_current {
        line = line.push(ui::badge("current", Tone::Signal));
    }
    line = line.push(Space::new().width(Length::Fill));
    line = line.push(ui::faint(instance_label(state, c).to_string()));
    let pick = button(line)
        .on_press(wrap(PickerMsg::Pick(c.id.clone())))
        .width(Length::Fill)
        .padding([6, 10])
        .style(plain);
    let star_ink = if starred {
        crate::theme::palette().signal
    } else {
        crate::theme::palette().ink_3
    };
    let star = button(ui::icon("star", 13.0, star_ink))
        .on_press(wrap(PickerMsg::Favourite(c.id.clone())))
        .padding([6, 8])
        .style(plain);
    row![pick, star].align_y(Alignment::Center).into()
}

/// The picker's plain button: paper-coloured, ink text, a 1px rule, a faint wash on hover.
fn plain(_: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered);
    button::Style {
        background: if hovered {
            Some(crate::theme::palette().rule_faint.into())
        } else {
            None
        },
        text_color: crate::theme::palette().ink,
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: crate::theme::skin().radius.into(),
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_choices_list_hub_models_and_flag_contributors() {
        // The choices read the hub model list: decision-group models are left out.
        {
            let models = serde_json::json!({
                "models": [
                    {"provider": "anthropic", "id": "claude-haiku-5-5", "name": "Claude Haiku 5.5", "group": "chat"},
                    {"provider": "openai", "id": "gpt-x", "name": null},
                    {"provider": "openai-decisions", "id": "gpt-6-luna", "name": null, "group": "decisions"}
                ],
                "errors": []
            });
            let out = choices(&models);
            assert_eq!(out.len(), 2);
            assert_eq!(out[0].id, "anthropic/claude-haiku-5-5");
            assert_eq!(out[0].provider, "anthropic");
            assert_eq!(out[0].label, "Claude Haiku 5.5");
            assert_eq!(out[1].id, "openai/gpt-x");
            assert_eq!(out[1].label, "gpt-x");
        }
        // Contributor models (trains on data) are flagged in the picker label and by trains_on_data.
        {
            let models = serde_json::json!({
                "models": [
                    {"provider": "meta", "id": "muse-spark-1.3-contributor", "name": "Muse Spark 1.3 (contributor)", "trains_on_data": true},
                    {"provider": "meta", "id": "muse-spark-1.3", "name": "Muse Spark 1.3", "trains_on_data": false}
                ]
            });
            let out = choices(&models);
            assert_eq!(
                out[0].label,
                "Muse Spark 1.3 (contributor) · WARNING: trains on your data"
            );
            assert_eq!(out[1].label, "Muse Spark 1.3");
            assert!(trains_on_data(&models, "meta/muse-spark-1.3-contributor"));
            assert!(!trains_on_data(&models, "meta/muse-spark-1.3"));
        }
    }

    #[test]
    fn ids_split_at_the_first_slash_only() {
        assert_eq!(
            split_id("openrouter/anthropic/claude-sonnet-4.5"),
            ("openrouter", "anthropic/claude-sonnet-4.5")
        );
        assert_eq!(
            split_id("anthropic/claude-haiku-5-5"),
            ("anthropic", "claude-haiku-5-5")
        );
        assert_eq!(
            split_vendor("anthropic/claude-sonnet-4.5"),
            (Some("anthropic"), "claude-sonnet-4.5")
        );
        assert_eq!(split_vendor("a/b/c"), (Some("a"), "b/c"));
        assert_eq!(split_vendor("gpt-x"), (None, "gpt-x"));
    }

    #[test]
    fn the_models_list_gives_instances_favourites_and_failed_listings() {
        let models = serde_json::json!({
            "models": [
                {"provider": "openai", "provider_label": "OpenAI", "accent": "#5cc8ff", "id": "gpt-x", "favourite": true},
                {"provider": "openai", "provider_label": "OpenAI", "accent": "#5cc8ff", "id": "gpt-y", "favourite": false},
                {"provider": "zai", "provider_label": "Z.ai GLM", "accent": null, "id": "glm-5.3", "favourite": true}
            ],
            "errors": [{"provider": "groq", "error": "401 Unauthorized"}, {"provider": "zai", "error": "timeout"}]
        });
        let m = meta(&models);
        assert_eq!(m.instances.len(), 3);
        assert_eq!(m.instances[0].label, "OpenAI");
        assert_eq!(m.instances[0].accent.as_deref(), Some("#5cc8ff"));
        assert_eq!(m.instances[0].crit, None);
        assert_eq!(m.instances[1].crit.as_deref(), Some("timeout"));
        assert_eq!(m.instances[2].id, "groq");
        assert_eq!(m.instances[2].label, "groq");
        assert_eq!(
            m.favourites,
            vec!["openai/gpt-x".to_string(), "zai/glm-5.3".to_string()]
        );

        let mut state = Picker::default();
        sync(&mut state, &models);
        assert_eq!(state.meta, m);
    }

    #[test]
    fn stars_toggle_and_the_rail_filters_the_models() {
        let starred = with_favourite(&["a/x".to_string()], "b/y");
        assert_eq!(starred, vec!["a/x".to_string(), "b/y".to_string()]);
        assert_eq!(with_favourite(&starred, "a/x"), vec!["b/y".to_string()]);

        let choices = vec![
            Choice {
                id: "a/x".into(),
                provider: "a".into(),
                label: "x".into(),
            },
            Choice {
                id: "b/y".into(),
                provider: "b".into(),
                label: "y".into(),
            },
        ];
        let mut state = Picker::default();
        state.meta.favourites = vec!["b/y".to_string()];
        state.rail = Some(Rail::Favourites);
        let fav: Vec<&Choice> = matching(&choices, "")
            .into_iter()
            .filter(|c| in_rail(&state, c))
            .collect();
        assert_eq!(fav.len(), 1);
        assert_eq!(fav[0].id, "b/y");

        // Pressing the selected rail item again shows every model.
        update(&mut state, &PickerMsg::Rail(Rail::Favourites));
        assert_eq!(state.rail, None);
        update(&mut state, &PickerMsg::Rail(Rail::Instance("a".into())));
        assert_eq!(state.rail, Some(Rail::Instance("a".into())));
        update(&mut state, &PickerMsg::Favourite("a/x".into()));
        assert_eq!(
            state.meta.favourites,
            vec!["b/y".to_string(), "a/x".to_string()]
        );
    }

    #[test]
    fn accents_and_monograms_read_the_hub_forms() {
        assert_eq!(
            accent_color("#5cc8ff"),
            Some(Color::from_rgb8(0x5c, 0xc8, 0xff))
        );
        assert_eq!(accent_color("5cc8ff"), None);
        assert_eq!(accent_color("#zzzzzz"), None);
        assert_eq!(accent_color("#fff"), None);
        assert_eq!(initials("my-hub"), "MH");
        assert_eq!(initials("openai"), "OP");
        assert_eq!(initials("---"), "?");
    }
}
