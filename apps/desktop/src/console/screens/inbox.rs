//! The Inbox screen (the web's `inbox` view): the cards waiting for the owner, each a decision the plaza needs before it
//! goes on. A card shows its kind, its title and body as plain text, the project, task, session and chat it is about
//! (links), and its choices as buttons. A choice that takes an amount has a field before its button, in the unit of
//! the owner's budget. Answering sends the choice (and the amount); the card leaves the list when the hub settles it.
//! The expert view adds each card's id, its raw kind and subject, and the raw ids of its choices. The chat screen
//! answers its questions with the same buttons ([`Answers`]).

use std::collections::{HashMap, HashSet};

use iced::widget::{column, container, text_input};
use iced::{Alignment, Element, Length, Task};

use super::Id;
use super::chat_model::{self as chat, AmountUnit};
use super::work_ui;
use crate::console::client::Client;
use crate::console::events;
use crate::console::work_api::{self, BudgetUnit, Card, CardAnswer, CardChoice};
use crate::console::{Ctx, client, scope, ui};
use crate::theme;

#[derive(Debug, Default)]
pub struct State {
    /// The open cards as the hub last answered.
    cards: Vec<Card>,
    /// The first load has answered (until then the page is blank, as on the web).
    loaded: bool,
    /// The open cards could not be read.
    failed: bool,
    answers: Answers,
    expert: bool,
    /// A line for the last answer ("Budget raised") or for why an answer was refused.
    note: Option<String>,
    /// A load is in flight; a card frame that comes meanwhile only asks for one more.
    loading: bool,
    again: bool,
}

/// What a load read: the open cards, and the unit of each card that takes an amount.
#[derive(Debug, Clone, Default)]
pub struct Loaded {
    cards: Vec<Card>,
    units: HashMap<String, AmountUnit>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Loaded, String>),
    Expert(bool),
    /// The amount typed for a card: the card's id and the text.
    Amount(String, String),
    /// A choice of a card pressed: the card's id and the choice's id.
    Choose(String, String),
    /// The hub answered a card: its id, and the answer or the hub's words.
    Answered(String, Result<CardAnswer, String>),
    /// A hub event: a card frame reloads the list.
    Event(events::Event),
}

/// The owner's answers to cards, for one screen: the amounts typed (kept until the card is answered), the answers in
/// flight (their buttons are off meanwhile), and the unit each card's amount is counted in (a card with no entry is
/// refused, as its budget did not read).
#[derive(Debug, Default)]
pub struct Answers {
    units: HashMap<String, AmountUnit>,
    amounts: HashMap<String, String>,
    busy: HashSet<String>,
}

impl Answers {
    /// Whether an answer of the card is in flight.
    pub fn busy(&self, id: &str) -> bool {
        self.busy.contains(id)
    }

    /// Keeps the unit each card's amount is counted in, as the screen last read it.
    pub fn set_units(&mut self, units: HashMap<String, AmountUnit>) {
        self.units = units;
    }

    /// Keeps the amount typed for a card; an empty text keeps nothing.
    pub fn set_amount(&mut self, id: &str, text: String) {
        if text.is_empty() {
            self.amounts.remove(id);
        } else {
            self.amounts.insert(id.to_string(), text);
        }
    }

    /// An answer landed or failed: the card's buttons are back, and its typed amount is forgotten when it was answered.
    pub fn settled(&mut self, id: &str, answered: bool) {
        self.busy.remove(id);
        if answered {
            self.amounts.remove(id);
        }
    }

    /// Sends the owner's choice of a card. A choice that takes an amount sends the typed one, in the card's unit; an
    /// amount refused says why, and nothing is sent.
    pub fn send(&mut self, ctx: &Ctx, card: &Card, choice: &str) -> Result<Task<Msg>, String> {
        let takes_amount = card
            .choices
            .iter()
            .any(|c| c.id == choice && c.takes_amount);
        let amount = if takes_amount {
            let unit = self
                .units
                .get(&card.id)
                .copied()
                .unwrap_or(AmountUnit::Unknown);
            let typed = self.amounts.get(&card.id).map_or("", String::as_str);
            Some(chat::amount_wire(unit, typed)?)
        } else {
            None
        };
        self.busy.insert(card.id.clone());
        let client = ctx.client.clone();
        let (id, choice) = (card.id.clone(), choice.to_string());
        let answered = id.clone();
        Ok(client::task(
            move || work_api::answer_card(&client, &id, &choice, amount),
            move |result| Msg::Answered(answered.clone(), result),
        ))
    }

    /// The card's choices as buttons: the first is primary, a stop-like one is danger. A choice that takes an amount has
    /// its field before it, with the unit named beside the field; every button is off while the answer is in flight.
    pub fn choices<'a>(&'a self, card: &'a Card) -> Element<'a, Msg> {
        let busy = self.busy.contains(&card.id);
        let unit = self
            .units
            .get(&card.id)
            .copied()
            .unwrap_or(AmountUnit::Unknown);
        let mut items: Vec<Element<'a, Msg>> = Vec::new();
        for (i, choice) in card.choices.iter().enumerate() {
            if choice.takes_amount {
                items.push(self.amount_field(card, unit, busy));
            }
            items.push(choice_button(card, choice, i == 0, busy));
        }
        iced::widget::Row::with_children(items)
            .spacing(8)
            .align_y(Alignment::Center)
            .wrap()
            .into()
    }

    /// The amount a choice takes: a field in the card's unit, its unit named beside it.
    fn amount_field<'a>(
        &'a self,
        card: &'a Card,
        unit: AmountUnit,
        busy: bool,
    ) -> Element<'a, Msg> {
        let placeholder = match unit {
            AmountUnit::Unknown => "Amount",
            AmountUnit::Dollars => "$0.00",
            AmountUnit::Tokens => "0",
        };
        let value = self.amounts.get(&card.id).map_or("", String::as_str);
        let id = card.id.clone();
        let field = text_input(placeholder, value).width(Length::Fixed(140.0));
        let field = if busy {
            field
        } else {
            field.on_input(move |text: String| Msg::Amount(id.clone(), text))
        };
        iced::widget::Row::with_children(vec![
            field.into(),
            ui::faint(chat::amount_unit_name(unit).to_string()).into(),
        ])
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    }
}

fn choice_button<'a>(
    card: &Card,
    choice: &CardChoice,
    primary: bool,
    busy: bool,
) -> Element<'a, Msg> {
    let kind = if chat::is_danger_choice(&choice.id) {
        ui::Kind::Danger
    } else if primary {
        ui::Kind::Primary
    } else {
        ui::Kind::Secondary
    };
    let press = (!busy).then(|| Msg::Choose(card.id.clone(), choice.id.clone()));
    ui::btn(&choice.label, None, kind, press)
}

pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// The hub's events while the screen is open: the card frames reload the list.
pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    events::subscription(&ctx.client).map(Msg::Event)
}

/// Loads the open cards, and the unit of each card's amount: its task's budget, else its project's, else dollars. A card
/// whose budget does not read has no unit, so its amount is refused, and the other cards still load.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = true;
    let client = ctx.client.clone();
    client::task(
        move || {
            let cards = work_api::cards(&client, true, None)?;
            let units = amount_units(&client, &cards);
            Ok(Loaded { cards, units })
        },
        Msg::Loaded,
    )
}

/// The unit each card's amount is counted in: its task's budget's, else its project's, else dollars for a card with
/// neither. A card whose budget did not read has no entry, so its amount is refused. Only the cards with a choice that
/// takes an amount are read.
pub fn amount_units(client: &Client, cards: &[Card]) -> HashMap<String, AmountUnit> {
    let mut units = HashMap::new();
    for card in cards
        .iter()
        .filter(|c| c.choices.iter().any(|x| x.takes_amount))
    {
        let budget = if let Some(task) = &card.task {
            Some(work_api::task_budget(client, task))
        } else {
            card.project
                .as_ref()
                .map(|project| work_api::project_budget(client, project))
        };
        match budget {
            None => units.insert(card.id.clone(), AmountUnit::Dollars),
            Some(Ok(budget)) => units.insert(
                card.id.clone(),
                budget.unit.map_or(AmountUnit::Dollars, unit_of),
            ),
            Some(Err(_)) => None,
        };
    }
    units
}

/// The amount unit of a budget's unit.
pub fn unit_of(unit: BudgetUnit) -> AmountUnit {
    match unit {
        BudgetUnit::Usd => AmountUnit::Dollars,
        BudgetUnit::Tokens => AmountUnit::Tokens,
    }
}

/// A load landed: one more runs if card frames came in while it was in flight.
fn landed(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = false;
    if std::mem::take(&mut state.again) {
        load(state, ctx)
    } else {
        Task::none()
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(loaded)) => {
            state.cards = loaded.cards;
            state.answers.set_units(loaded.units);
            state.failed = false;
            state.loaded = true;
            landed(state, ctx)
        }
        Msg::Loaded(Err(_)) => {
            state.failed = true;
            state.loaded = true;
            landed(state, ctx)
        }
        Msg::Expert(on) => {
            state.expert = on;
            Task::none()
        }
        Msg::Amount(id, text) => {
            state.answers.set_amount(&id, text);
            Task::none()
        }
        Msg::Choose(id, choice) => {
            let Some(card) = state.cards.iter().find(|c| c.id == id) else {
                return Task::none();
            };
            match state.answers.send(ctx, card, &choice) {
                Ok(task) => task,
                Err(message) => {
                    state.note = Some(message);
                    Task::none()
                }
            }
        }
        Msg::Answered(id, Ok(answer)) => {
            state.answers.settled(&id, true);
            state.cards.retain(|c| c.id != id);
            state.note = Some(chat::done_text(&answer.done));
            load(state, ctx)
        }
        Msg::Answered(id, Err(message)) => {
            // The hub refuses an answer to a card that is already settled: the list reloads, and that card leaves it.
            state.answers.settled(&id, false);
            state.note = Some(message);
            load(state, ctx)
        }
        Msg::Event(event) => {
            if event.kind != "card" {
                return Task::none();
            }
            if state.loading {
                state.again = true;
                return Task::none();
            }
            load(state, ctx)
        }
    }
}

pub fn view<'a>(state: &'a State) -> Element<'a, Msg> {
    let waiting = match state.cards.len() {
        0 => "Nothing waits for you".to_string(),
        1 => "1 waiting for you".to_string(),
        n => format!("{n} waiting for you"),
    };
    let head = ui::view_head_with_intro(
        "Inbox",
        &waiting,
        vec![work_ui::switch("Expert", state.expert, Msg::Expert)],
    );
    let body: Element<'a, Msg> = if !state.loaded {
        ui::loading()
    } else if state.failed {
        ui::error_line("Could not load the inbox.")
    } else if state.cards.is_empty() {
        ui::empty(
            "inbox",
            "Nothing waits for you",
            "Decisions the plaza needs from you appear here.",
        )
    } else {
        let cards: Vec<Element<'a, Msg>> =
            state.cards.iter().map(|c| card_view(state, c)).collect();
        column(cards).spacing(12).width(Length::Fill).into()
    };
    let mut lines: Vec<Element<'a, Msg>> = vec![head];
    if let Some(note) = &state.note {
        lines.push(ui::faint(note.clone()).into());
    }
    lines.push(body);
    column(lines).spacing(16).width(Length::Fill).into()
}

/// One card: its kind, its title and body, its facts as links, the expert line, and its choices as buttons.
fn card_view<'a>(state: &'a State, c: &'a Card) -> Element<'a, Msg> {
    let mut facts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(project) = &c.project {
        facts.push(ui::text_link(
            &scope::title(project),
            Msg::Go(Id::Project(project.clone())),
        ));
    }
    if let Some(task) = &c.task {
        facts.push(ui::text_link("Open task", Msg::Go(Id::Task(task.clone()))));
    }
    if let Some(session) = &c.session {
        facts.push(ui::text_link(
            "Open session",
            Msg::Go(Id::Session(session.clone())),
        ));
    }
    if let Some(chat_id) = &c.chat {
        facts.push(ui::text_link(
            "Open the chat",
            Msg::Go(Id::Chat(chat_id.clone())),
        ));
    }
    facts.push(ui::faint(format!("Created {}", ui::ago(c.created_ms))).into());
    let mut lines: Vec<Element<'a, Msg>> = vec![
        ui::badge(&chat::card_kind_label(&c.kind), chat::kind_tone(&c.kind)),
        ui::sans(c.title.clone(), 15.0, theme::palette().ink).into(),
    ];
    if let Some(body) = c.body.as_deref().filter(|b| !b.trim().is_empty()) {
        lines.push(ui::prose(body.to_string()).into());
    }
    lines.push(iced::widget::Row::with_children(facts).spacing(14).into());
    if state.expert {
        lines.push(ui::mono(chat::card_expert_line(c), 12.0, theme::palette().ink_3).into());
    }
    lines.push(state.answers.choices(c));
    container(column(lines).spacing(10).width(Length::Fill))
        .padding(16)
        .width(Length::Fill)
        .style(ui::card_style)
        .into()
}

#[cfg(test)]
#[path = "inbox_tests.rs"]
mod tests;
