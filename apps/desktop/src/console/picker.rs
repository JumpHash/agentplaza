//! The model picker (`apps/web/src/ui/model-picker.ts`): model ids grouped by provider with the
//! provider's mark, a search box, the current choice, and the reasoning effort beside it.

use std::collections::BTreeSet;

use iced::widget::{button, column, container, pick_list, row, text_input, Space};
use iced::{Alignment, Border, Element, Length, Theme};
use serde_json::Value;

use super::ui::{self, Tone};

/// One model the picker offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// `provider/model`, as agents and settings store it.
    pub id: String,
    /// The brand key for `ui::brand_logo` (e.g. "anthropic", "openai", "zai").
    pub provider: String,
    pub label: String,
}

/// Rows a group or subgroup shows before its "Show N more" control.
pub const SHOW_FIRST: usize = 12;
/// The provider whose models are split by upstream vendor (the second segment of the model id).
const VENDORED: &str = "openrouter";
/// The vendor bucket for models with no vendor segment; listed last.
const OTHER: &str = "other";
/// The note the hub's label carries for contributor models (`trains_on_data`).
const TRAINS_NOTE: &str = " · WARNING: trains on your data";

/// The picker's own state (open or closed, the search text, the sections opened past their first rows).
#[derive(Debug, Clone, Default)]
pub struct Picker {
    pub open: bool,
    pub query: String,
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
    EffortOpt { value: "", label: "Model default" },
    EffortOpt { value: "none", label: "None" },
    EffortOpt { value: "minimal", label: "Minimal" },
    EffortOpt { value: "low", label: "Low" },
    EffortOpt { value: "medium", label: "Medium" },
    EffortOpt { value: "high", label: "High" },
    EffortOpt { value: "xhigh", label: "Extra high" },
    EffortOpt { value: "max", label: "Max" },
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
                    let name = m.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or(model);
                    let label = if m.get("trains_on_data").and_then(Value::as_bool) == Some(true) {
                        format!("{name}{TRAINS_NOTE}")
                    } else {
                        name.to_string()
                    };
                    Some(Choice { id: format!("{provider}/{model}"), provider, label })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the hub lists the `provider/model` `id` as a contributor model (`trains_on_data`).
pub fn trains_on_data(models: &Value, id: &str) -> bool {
    models.get("models").and_then(Value::as_array).is_some_and(|list| {
        list.iter().any(|m| {
            m.get("trains_on_data").and_then(Value::as_bool) == Some(true)
                && m.get("provider").and_then(Value::as_str).zip(m.get("id").and_then(Value::as_str)).is_some_and(|(p, i)| format!("{p}/{i}") == id)
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
    split_vendor(split_id(&c.id).1).0.filter(|v| !v.is_empty()).unwrap_or(OTHER)
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
        let i = match groups.iter().position(|g| g.provider == c.provider.as_str()) {
            Some(i) => i,
            None => {
                groups.push(Group { provider: c.provider.as_str(), count: 0, sections: Vec::new() });
                groups.len() - 1
            }
        };
        let g = &mut groups[i];
        g.count += 1;
        let vendor = if is_vendored(&c.provider) { vendor_of(c) } else { "" };
        match g.sections.iter_mut().find(|s| s.vendor == vendor) {
            Some(s) => s.rows.push(c),
            None => g.sections.push(Section { vendor, rows: vec![c] }),
        }
    }
    for g in &mut groups {
        g.sections.sort_by(|a, b| (a.vendor == OTHER, a.vendor).cmp(&(b.vendor == OTHER, b.vendor)));
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

/// Applies a message to the picker's own state (Toggle, Query, Pick, More); the chosen id or
/// effort is for the caller to act on.
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
        PickerMsg::Effort(_) => {}
    }
}

/// The picker: the trigger (current model with its mark), and when open the grouped, searchable
/// list; the effort selector beside it when `effort` is Some.
pub fn view<'a, M: Clone + 'a>(state: &'a Picker, choices: &'a [Choice], current: &'a str, effort: Option<&'a str>, wrap: fn(PickerMsg) -> M) -> Element<'a, M> {
    let cur = choices.iter().find(|c| c.id == current);
    let mut trigger_row = row![].spacing(8).align_y(Alignment::Center);
    if let Some(c) = cur {
        trigger_row = trigger_row.push(ui::brand_logo(&c.provider, 16.0));
    }
    let (shown, color) = if current.is_empty() { ("Choose a model".to_string(), crate::theme::palette().ink_3) } else { (current.to_string(), crate::theme::palette().ink) };
    trigger_row = trigger_row.push(ui::mono(shown, 12.0, color));
    trigger_row = trigger_row.push(Space::new().width(Length::Fill));
    trigger_row = trigger_row.push(ui::icon("chevron-down", 14.0, crate::theme::palette().ink_3));
    let trigger = button(trigger_row).on_press(wrap(PickerMsg::Toggle)).width(Length::Fill).padding([6, 10]).style(plain);

    let mut head = row![trigger].spacing(8).align_y(Alignment::Center);
    if let Some(sel) = effort {
        let selected = EFFORTS.iter().copied().find(|o| o.value == sel);
        head = head.push(pick_list(EFFORTS, selected, move |o: EffortOpt| wrap(PickerMsg::Effort(o.value.to_string()))));
    }
    let mut col = column![head].spacing(6);

    if state.open {
        let hits = matching(choices, &state.query);
        let search = text_input("Search models", &state.query)
            .on_input(move |s| wrap(PickerMsg::Query(s)))
            .font(crate::theme::MONO)
            .padding(6)
            .size(12.0)
            .style(crate::theme::input);

        let mut list = column![].spacing(2);
        for g in group(&hits) {
            list = list.push(row![ui::brand_logo(g.provider, 14.0), ui::label(&format!("{} · {}", g.provider, g.count))].spacing(8).align_y(Alignment::Center));
            for s in &g.sections {
                if !s.vendor.is_empty() {
                    list = list.push(container(ui::faint(format!("{} · {}", s.vendor, s.rows.len()))).padding([0, 10]));
                }
                let key = section_key(g.provider, s.vendor);
                let (shown, hidden) = cap(&s.rows, state.expanded.contains(&key));
                for m in shown.iter().copied() {
                    list = list.push(model_row(m, current, wrap));
                }
                if hidden > 0 {
                    let more = button(ui::faint(format!("Show {hidden} more"))).on_press(wrap(PickerMsg::More(key))).width(Length::Fill).padding([6, 10]).style(plain);
                    list = list.push(more);
                }
            }
        }
        if hits.is_empty() {
            list = list.push(ui::faint("No models match"));
        }

        let panel = container(column![search, ui::scroll(list).height(Length::Fixed(280.0))].spacing(8))
            .padding(8)
            .width(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().sheet.into()),
                border: Border { color: crate::theme::palette().rule, width: 1.0, radius: 0.0.into() },
                ..Default::default()
            });
        col = col.push(panel);
    }
    col.into()
}

/// One model row: its display name in mono, then the full model id and the contributor warning in
/// faint text when they add to it; the current one marked.
fn model_row<'a, M: Clone + 'a>(c: &'a Choice, current: &str, wrap: fn(PickerMsg) -> M) -> Element<'a, M> {
    let is_current = c.id == current;
    let name = display_name(c);
    let model = split_id(&c.id).1;
    let mut line = row![ui::mono(name.to_string(), 12.0, crate::theme::palette().ink)].spacing(8).align_y(Alignment::Center);
    if model != name {
        line = line.push(ui::faint(model.to_string()));
    }
    if trains(c) {
        line = line.push(ui::faint(TRAINS_NOTE.trim_start().to_string()));
    }
    line = line.push(Space::new().width(Length::Fill));
    if is_current {
        line = line.push(ui::badge("current", Tone::Signal));
    }
    button(line).on_press(wrap(PickerMsg::Pick(c.id.clone()))).width(Length::Fill).padding([6, 10]).style(plain).into()
}

/// The picker's plain button: paper-coloured, ink text, a 1px rule, a faint wash on hover.
fn plain(_: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered);
    button::Style {
        background: if hovered { Some(crate::theme::palette().rule_faint.into()) } else { None },
        text_color: crate::theme::palette().ink,
        border: Border { color: crate::theme::palette().rule, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_read_the_hub_model_list() {
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

    #[test]
    fn contributor_models_are_flagged_in_the_picker() {
        let models = serde_json::json!({
            "models": [
                {"provider": "meta", "id": "muse-spark-1.3-contributor", "name": "Muse Spark 1.3 (contributor)", "trains_on_data": true},
                {"provider": "meta", "id": "muse-spark-1.3", "name": "Muse Spark 1.3", "trains_on_data": false}
            ]
        });
        let out = choices(&models);
        assert_eq!(out[0].label, "Muse Spark 1.3 (contributor) · WARNING: trains on your data");
        assert_eq!(out[1].label, "Muse Spark 1.3");
        assert!(trains_on_data(&models, "meta/muse-spark-1.3-contributor"));
        assert!(!trains_on_data(&models, "meta/muse-spark-1.3"));
    }

    fn choice(id: &str, provider: &str, label: &str) -> Choice {
        Choice { id: id.to_string(), provider: provider.to_string(), label: label.to_string() }
    }

    fn router_choices() -> Vec<Choice> {
        choices(&serde_json::json!({
            "models": [
                {"provider": "openrouter", "id": "anthropic/claude-sonnet-4.5", "name": "Claude Sonnet 4.5"},
                {"provider": "openrouter", "id": "anthropic/claude-haiku-4.5", "name": null},
                {"provider": "openrouter", "id": "openai/gpt-5", "name": "GPT-5"},
                {"provider": "openrouter", "id": "llama-free", "name": "Llama free", "trains_on_data": true},
                {"provider": "opencode", "id": "big-pickle", "name": "Big Pickle"}
            ]
        }))
    }

    fn ids(hits: &[&Choice]) -> Vec<String> {
        hits.iter().map(|c| c.id.clone()).collect()
    }

    #[test]
    fn ids_split_at_the_first_slash_only() {
        assert_eq!(split_id("openrouter/anthropic/claude-sonnet-4.5"), ("openrouter", "anthropic/claude-sonnet-4.5"));
        assert_eq!(split_id("anthropic/claude-haiku-5-5"), ("anthropic", "claude-haiku-5-5"));
        assert_eq!(split_vendor("anthropic/claude-sonnet-4.5"), (Some("anthropic"), "claude-sonnet-4.5"));
        assert_eq!(split_vendor("a/b/c"), (Some("a"), "b/c"));
        assert_eq!(split_vendor("gpt-x"), (None, "gpt-x"));
    }

    #[test]
    fn openrouter_rows_show_the_name_after_the_vendor() {
        let all = router_choices();
        assert_eq!(all[0].id, "openrouter/anthropic/claude-sonnet-4.5");
        assert_eq!(display_name(&all[0]), "claude-sonnet-4.5");
        assert_eq!(display_name(&all[1]), "claude-haiku-4.5");
        assert_eq!(display_name(&all[4]), "Big Pickle");
        assert_eq!(display_name(&all[3]), "llama-free");
        assert!(trains(&all[3]));
        assert!(!trains(&all[0]));
    }

    #[test]
    fn every_term_must_match_the_id_or_the_name() {
        let all = router_choices();
        assert_eq!(matching(&all, "").len(), 5);
        assert_eq!(matching(&all, "   ").len(), 5);
        assert_eq!(ids(&matching(&all, "SONNET 4.5")), vec!["openrouter/anthropic/claude-sonnet-4.5"]);
        assert_eq!(ids(&matching(&all, "claude 4.5")), vec!["openrouter/anthropic/claude-sonnet-4.5", "openrouter/anthropic/claude-haiku-4.5"]);
        assert_eq!(ids(&matching(&all, "openai gpt")), vec!["openrouter/openai/gpt-5"]);
        assert!(matching(&all, "claude opus").is_empty());
    }

    #[test]
    fn openrouter_models_group_by_vendor_with_other_last() {
        let all = router_choices();
        let hits = matching(&all, "");
        let groups = group(&hits);
        let counts: Vec<(&str, usize)> = groups.iter().map(|g| (g.provider, g.count)).collect();
        assert_eq!(counts, vec![("openrouter", 4), ("opencode", 1)]);
        let vendors: Vec<(&str, usize)> = groups[0].sections.iter().map(|s| (s.vendor, s.rows.len())).collect();
        assert_eq!(vendors, vec![("anthropic", 2), ("openai", 1), ("other", 1)]);
        assert_eq!(groups[1].sections[0].vendor, "");

        let narrowed = matching(&all, "anthropic");
        let groups = group(&narrowed);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].count, 2);
    }

    #[test]
    fn a_section_shows_twelve_rows_and_counts_the_rest() {
        let many: Vec<Choice> = (0..30).map(|i| choice(&format!("openrouter/acme/model-{i:02}"), "openrouter", &format!("model-{i:02}"))).collect();
        let hits = matching(&many, "acme");
        let groups = group(&hits);
        let rows = &groups[0].sections[0].rows;
        assert_eq!(rows.len(), 30);
        let (shown, hidden) = cap(&rows[..], false);
        assert_eq!((shown.len(), hidden), (12, 18));
        assert_eq!(cap(&rows[..], true), (&rows[..], 0));
        assert_eq!(cap(&rows[..12], false), (&rows[..12], 0));
        assert_eq!(cap(&rows[..13], false).1, 1);
    }

    #[test]
    fn section_keys_are_per_vendor_for_openrouter_only() {
        assert_eq!(section_key("openrouter", "anthropic"), "openrouter/anthropic");
        assert_eq!(section_key("opencode", ""), "opencode");
    }

    #[test]
    fn show_more_expands_until_the_picker_closes() {
        let mut state = Picker::default();
        update(&mut state, &PickerMsg::Toggle);
        update(&mut state, &PickerMsg::More("openrouter/acme".to_string()));
        assert!(state.expanded.contains("openrouter/acme"));
        update(&mut state, &PickerMsg::Toggle);
        assert!(state.expanded.is_empty());
        assert!(!state.open);
    }
}
