//! The model picker (`apps/web/src/ui/model-picker.ts`): model ids grouped by provider with the
//! provider's mark, a search box, the current choice, and the reasoning effort beside it.

use iced::widget::{button, column, container, pick_list, row, scrollable, text_input, Space};
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

/// The picker's own state (open or closed, the search text).
#[derive(Debug, Clone, Default)]
pub struct Picker {
    pub open: bool,
    pub query: String,
}

#[derive(Debug, Clone)]
pub enum PickerMsg {
    Toggle,
    Query(String),
    /// A model was chosen (its id).
    Pick(String),
    /// The reasoning effort changed ("", "low", "medium", "high", "max", …).
    Effort(String),
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
/// picker reads them).
pub fn choices(models: &Value) -> Vec<Choice> {
    models
        .get("models")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|m| {
                    let provider = m.get("provider")?.as_str()?.to_string();
                    let model = m.get("id")?.as_str()?;
                    let label = m.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or(model).to_string();
                    Some(Choice { id: format!("{provider}/{model}"), provider, label })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Applies a message to the picker's own state (Toggle, Query); returns the chosen id or effort
/// for the caller to act on.
pub fn update(state: &mut Picker, msg: &PickerMsg) {
    match msg {
        PickerMsg::Toggle => {
            state.open = !state.open;
            state.query.clear();
        }
        PickerMsg::Query(q) => state.query = q.clone(),
        PickerMsg::Pick(_) => {
            state.open = false;
            state.query.clear();
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
    let (shown, color) = if current.is_empty() { ("Choose a model".to_string(), ui::INK_3) } else { (current.to_string(), ui::INK) };
    trigger_row = trigger_row.push(ui::mono(shown, 12.0, color));
    trigger_row = trigger_row.push(Space::new().width(Length::Fill));
    trigger_row = trigger_row.push(ui::icon("chevron-down", 14.0, ui::INK_3));
    let trigger = button(trigger_row).on_press(wrap(PickerMsg::Toggle)).width(Length::Fill).padding([6, 10]).style(plain);

    let mut head = row![trigger].spacing(8).align_y(Alignment::Center);
    if let Some(sel) = effort {
        let selected = EFFORTS.iter().copied().find(|o| o.value == sel);
        head = head.push(pick_list(EFFORTS, selected, move |o: EffortOpt| wrap(PickerMsg::Effort(o.value.to_string()))));
    }
    let mut col = column![head].spacing(6);

    if state.open {
        let q = state.query.trim().to_lowercase();
        let hits: Vec<&Choice> = choices.iter().filter(|c| q.is_empty() || c.label.to_lowercase().contains(&q) || c.id.to_lowercase().contains(&q)).collect();
        let search = text_input("Search models", &state.query).on_input(move |s| wrap(PickerMsg::Query(s))).font(crate::theme::MONO).padding(6).size(12.0);

        let mut list = column![].spacing(2);
        let mut seen: Vec<&str> = Vec::new();
        for c in hits.iter().copied() {
            if seen.contains(&c.provider.as_str()) {
                continue;
            }
            seen.push(c.provider.as_str());
            list = list.push(row![ui::brand_logo(&c.provider, 14.0), ui::label(&c.provider)].spacing(8).align_y(Alignment::Center));
            for m in hits.iter().copied().filter(|m| m.provider == c.provider) {
                list = list.push(model_row(m, current, wrap));
            }
        }
        if hits.is_empty() {
            list = list.push(ui::faint("No models match"));
        }

        let panel = container(column![search, scrollable(list).height(Length::Fixed(280.0))].spacing(8))
            .padding(8)
            .width(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(ui::SHEET.into()),
                border: Border { color: ui::RULE, width: 1.0, radius: 0.0.into() },
                ..Default::default()
            });
        col = col.push(panel);
    }
    col.into()
}

/// One model row: its id in mono (the label beside it when it differs), the current one marked.
fn model_row<'a, M: Clone + 'a>(c: &'a Choice, current: &str, wrap: fn(PickerMsg) -> M) -> Element<'a, M> {
    let is_current = c.id == current;
    let model = c.id.split_once('/').map_or(c.id.as_str(), |(_, m)| m);
    let mut line = row![ui::mono(model.to_string(), 12.0, ui::INK)].spacing(8).align_y(Alignment::Center);
    if c.label != model {
        line = line.push(ui::faint(c.label.clone()));
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
        background: if hovered { Some(ui::RULE_FAINT.into()) } else { None },
        text_color: ui::INK,
        border: Border { color: ui::RULE, width: 1.0, radius: 0.0.into() },
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
                {"provider": "anthropic", "id": "claude-haiku-5-5", "name": "Claude Haiku 5.5"},
                {"provider": "openai", "id": "gpt-x", "name": null}
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
}
