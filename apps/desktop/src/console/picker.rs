//! The model picker (`apps/web/src/ui/model-picker.ts`): model ids grouped by provider with the
//! provider's mark, a search box, the current choice, and the reasoning effort beside it.

use iced::Element;
use serde_json::Value;

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

/// The choices from the hub's model list (`GET /v1/models` and the provider catalogue, as the web
/// picker reads them).
pub fn choices(models: &Value) -> Vec<Choice> {
    let _ = models;
    Vec::new() // contract: PICK
}

/// Applies a message to the picker's own state (Toggle, Query); returns the chosen id or effort
/// for the caller to act on.
pub fn update(state: &mut Picker, msg: &PickerMsg) {
    let _ = (state, msg); // contract: PICK
}

/// The picker: the trigger (current model with its mark), and when open the grouped, searchable
/// list; the effort selector beside it when `effort` is Some.
pub fn view<'a, M: Clone + 'a>(state: &'a Picker, choices: &'a [Choice], current: &'a str, effort: Option<&'a str>, wrap: fn(PickerMsg) -> M) -> Element<'a, M> {
    let _ = (state, choices, effort, wrap);
    super::ui::mono(current, 12.0, super::ui::INK).into() // contract: PICK
}
