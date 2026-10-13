//! The Treasury screen (parity with the web console's `treasury` view): the plaza budget, where it
//! sits, where it went, every wallet and the journal. The owner mints and fines here; everything
//! else is read.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::font::{self, Font};
use iced::widget::canvas::{self, Path, Stroke};
use iced::widget::{button, column, container, pick_list, row, text_input, Space};
use iced::{mouse, Alignment, Border, Element, Length, Padding, Pixels, Point, Rectangle, Size, Task, Theme};
use serde_json::{json, Value};

use super::Id;
use crate::console::{client, ui, Ctx};
use crate::theme::{self, INK, INK_2, INK_3, MONO, SHEET};

const VALUE: Font = Font { weight: font::Weight::Bold, ..theme::MONO };
const AMOUNT: Font = Font { weight: font::Weight::Semibold, ..theme::MONO };

#[derive(Debug, Default)]
pub struct State {
    /// The last failed call's message.
    pub error: Option<String>,
    /// The last answered action's note.
    notice: Option<String>,
    data: Option<Data>,
    kind_filter: Option<String>,
    busy: bool,
    mint_open: bool,
    mint_amount: String,
    fine_open: bool,
    fine_target: Option<AgentChoice>,
    fine_amount: String,
    fine_memo: String,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Data, String>),
    KindPicked(KindChoice),
    ToggleMint,
    MintAmount(String),
    SubmitMint,
    Minted(i64, Result<Value, String>),
    ToggleFine,
    FinePick(AgentChoice),
    FineAmount(String),
    FineMemo(String),
    SubmitFine,
    Fined(i64, Result<Value, String>),
}

/// An agent in the fine form's picker.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentChoice {
    cid: u64,
    label: String,
}

impl fmt::Display for AgentChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// A journal kind in the ledger's filter; `None` is every kind.
#[derive(Debug, Clone, PartialEq)]
pub struct KindChoice {
    kind: Option<String>,
    label: String,
}

impl fmt::Display for KindChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the treasury, its journal, its wallets and the roster (called when it opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || {
            let treasury: Value = client.get("/v1/treasury")?;
            let journal: Value = client.get("/v1/treasury/journal")?;
            let wallets: Value = client.get("/v1/treasury/wallets")?;
            let roster: Value = client.get("/v1/agents")?;
            Ok(Data::parse(&treasury, &journal, &wallets, &roster))
        },
        Msg::Loaded,
    )
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(data)) => {
            state.data = Some(data);
            state.error = None;
            Task::none()
        }
        Msg::Loaded(Err(err)) => {
            state.error = Some(err);
            Task::none()
        }
        Msg::KindPicked(choice) => {
            state.kind_filter = choice.kind;
            Task::none()
        }
        Msg::ToggleMint => {
            state.mint_open = !state.mint_open;
            state.fine_open = false;
            Task::none()
        }
        Msg::MintAmount(value) => {
            state.mint_amount = value;
            Task::none()
        }
        Msg::SubmitMint => {
            let Some(amount) = whole(&state.mint_amount) else {
                state.error = Some("Enter a whole amount above zero.".to_string());
                return Task::none();
            };
            state.busy = true;
            state.error = None;
            state.notice = None;
            let client = ctx.client.clone();
            client::task(
                move || client.post::<Value>("/v1/treasury/mint", &json!({ "amount": amount })),
                move |res| Msg::Minted(amount, res),
            )
        }
        Msg::Minted(amount, Ok(_)) => {
            state.busy = false;
            state.mint_open = false;
            state.mint_amount.clear();
            state.notice = Some(format!("Minted {} into the plaza budget.", money(amount as f64)));
            load(state, ctx)
        }
        Msg::Minted(_, Err(err)) => {
            state.busy = false;
            state.error = Some(err);
            Task::none()
        }
        Msg::ToggleFine => {
            state.fine_open = !state.fine_open;
            state.mint_open = false;
            Task::none()
        }
        Msg::FinePick(choice) => {
            state.fine_target = Some(choice);
            Task::none()
        }
        Msg::FineAmount(value) => {
            state.fine_amount = value;
            Task::none()
        }
        Msg::FineMemo(value) => {
            state.fine_memo = value;
            Task::none()
        }
        Msg::SubmitFine => {
            let Some(target) = fine_choice(state) else {
                state.error = Some("There are no agents to fine.".to_string());
                return Task::none();
            };
            let Some(amount) = whole(&state.fine_amount) else {
                state.error = Some("Enter a whole amount above zero.".to_string());
                return Task::none();
            };
            let memo = state.fine_memo.trim().to_string();
            if memo.is_empty() {
                state.error = Some("A fine needs a memo.".to_string());
                return Task::none();
            }
            state.busy = true;
            state.error = None;
            state.notice = None;
            let client = ctx.client.clone();
            let cid = target.cid;
            client::task(
                move || client.post::<Value>("/v1/treasury/fine", &json!({ "cid": cid, "amount": amount, "memo": memo })),
                move |res| Msg::Fined(amount, res),
            )
        }
        Msg::Fined(amount, Ok(_)) => {
            state.busy = false;
            state.fine_open = false;
            state.fine_amount.clear();
            state.fine_memo.clear();
            state.notice = Some(format!("Fined {}.", money(amount as f64)));
            load(state, ctx)
        }
        Msg::Fined(_, Err(err)) => {
            state.busy = false;
            state.error = Some(err);
            Task::none()
        }
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let head = ui::view_head(
        "Treasury",
        vec![
            ui::btn("Fine an agent", None, ui::Kind::Secondary, Some(Msg::ToggleFine)),
            ui::btn("Mint budget", None, ui::Kind::Primary, Some(Msg::ToggleMint)),
        ],
    );
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if state.mint_open {
        parts.push(ui::card("Mint budget", mint_form(state)));
    }
    if state.fine_open {
        parts.push(ui::card("Fine an agent", fine_form(state)));
    }
    if let Some(note) = &state.notice {
        parts.push(ui::faint(note.clone()).into());
    }
    match &state.data {
        None => parts.push(match &state.error {
            Some(err) => ui::empty("alert", "Treasury unreachable", err),
            None => ui::loading(),
        }),
        Some(data) => {
            if let Some(err) = &state.error {
                parts.push(ui::error_line(err));
            }
            parts.extend(sections(data, state));
        }
    }
    ui::sheet(head, column(parts).spacing(22).width(Length::Fill).into())
}

/// The read-only body: tallies, the flow, the wallets and the ledger.
fn sections<'a>(d: &Data, state: &State) -> Vec<Element<'a, Msg>> {
    let escrow: f64 = d.accounts.iter().filter(|a| a.kind == "escrow").map(|a| a.balance).sum();
    let paid: f64 = d.journal.iter().filter(|e| e.kind == "payment").map(Entry::amount).sum();
    let wallets_total: f64 = d.wallets.iter().map(|w| w.balance).sum();

    let tallies: Element<'a, Msg> = row![
        tile("Plaza budget", money(d.plaza_budget), ""),
        tile("In escrow", money(escrow), "held for open tasks"),
        tile("Spent", money(d.spent), "to providers and compute"),
        tile("Paid to agents", money(paid), "margins for verified work"),
        tile("Agents’ wallets", money(wallets_total), ""),
    ]
    .spacing(12)
    .into();

    let mut out: Vec<Element<'a, Msg>> = Vec::new();
    out.push(tallies);
    out.push(ui::card("Flow", flow(d.plaza_budget, escrow, d.spent, paid, wallets_total)));
    out.push(wallets_card(d));
    out.push(ledger_card(d, state));
    out
}

/// A tally tile (the web's `.stat`): the figure, its label, then an italic note when there is one.
fn tile<'a>(title: &str, value: String, note: &str) -> Element<'a, Msg> {
    let italic = Font { style: font::Style::Italic, ..theme::SERIF };
    let mut stack = column![iced::widget::text(value).font(VALUE).size(30.0).color(theme::INK), ui::label(title)].spacing(4);
    if !note.is_empty() {
        stack = stack.push(iced::widget::text(note.to_string()).font(italic).size(13.0).color(theme::INK_2));
    }
    container(stack)
        .padding(Padding { top: 14.0, right: 16.0, bottom: 16.0, left: 16.0 })
        .width(Length::FillPortion(1))
        .height(Length::Fixed(100.0))
        .style(|_: &Theme| container::Style { border: Border { color: theme::RULE, width: 1.0, radius: 0.0.into() }, ..Default::default() })
        .into()
}

/// The budget's path (the web's `flow`): budget, escrow, then spend (providers, compute) or payment
/// (agents), then wallets. Drawn on an 830 x 180 grid, scaled to the width and centred.
fn flow<'a>(budget: f64, escrow: f64, spent: f64, paid: f64, wallets: f64) -> Element<'a, Msg> {
    iced::widget::canvas(FlowChart { budget, escrow, spent, paid, wallets }).width(Length::Fill).height(Length::Fixed(200.0)).into()
}

struct FlowChart {
    budget: f64,
    escrow: f64,
    spent: f64,
    paid: f64,
    wallets: f64,
}

impl canvas::Program<Msg> for FlowChart {
    type State = ();

    fn draw(&self, _state: &(), renderer: &iced::Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<canvas::Geometry> {
        let scale = (bounds.width / 830.0).min(bounds.height / 180.0).max(0.1);
        let ox = (bounds.width - 830.0 * scale) / 2.0;
        let oy = (bounds.height - 180.0 * scale) / 2.0;
        let at = |x: f32, y: f32| Point::new(ox + x * scale, oy + y * scale);
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        let boxed = |frame: &mut canvas::Frame, x: f32, y: f32, label: &str, value: f64| {
            let (tl, size) = (at(x, y), Size::new(170.0 * scale, 70.0 * scale));
            frame.fill_rectangle(tl, size, SHEET);
            frame.stroke(&Path::rectangle(tl, size), Stroke::default().with_color(INK).with_width(1.0));
            frame.fill_text(canvas::Text {
                content: label.to_uppercase(),
                position: at(x + 12.0, y + 12.0),
                color: INK_3,
                size: Pixels(10.5 * scale),
                font: MONO,
                ..canvas::Text::default()
            });
            frame.fill_text(canvas::Text {
                content: money(value),
                position: at(x + 12.0, y + 33.0),
                color: INK,
                size: Pixels(24.0 * scale),
                font: VALUE,
                ..canvas::Text::default()
            });
        };
        let arrow = |frame: &mut canvas::Frame, from: (f32, f32), to: (f32, f32)| {
            let (dx, dy) = (to.0 - from.0, to.1 - from.1);
            let len = (dx * dx + dy * dy).sqrt().max(1.0);
            let (ux, uy) = (dx / len, dy / len);
            let head = 8.75;
            let base = (to.0 - ux * head, to.1 - uy * head);
            frame.stroke(&Path::line(at(from.0, from.1), at(base.0, base.1)), Stroke::default().with_color(INK_2).with_width(1.25 * scale));
            let (px, py) = (-uy * head * 0.5, ux * head * 0.5);
            let (wing_a, wing_b, tip) = (at(base.0 + px, base.1 + py), at(base.0 - px, base.1 - py), at(to.0, to.1));
            let head_path = Path::new(|b| {
                b.move_to(tip);
                b.line_to(wing_a);
                b.line_to(wing_b);
                b.close();
            });
            frame.fill(&head_path, INK_2);
        };

        boxed(&mut frame, 0.0, 55.0, "Budget", self.budget);
        boxed(&mut frame, 220.0, 55.0, "Escrow", self.escrow);
        boxed(&mut frame, 440.0, 10.0, "Spend", self.spent);
        boxed(&mut frame, 440.0, 100.0, "Payment", self.paid);
        boxed(&mut frame, 660.0, 55.0, "Wallets", self.wallets);
        arrow(&mut frame, (170.0, 90.0), (220.0, 90.0));
        arrow(&mut frame, (390.0, 90.0), (440.0, 45.0));
        arrow(&mut frame, (390.0, 90.0), (440.0, 135.0));
        arrow(&mut frame, (610.0, 135.0), (660.0, 90.0));
        vec![frame.into_geometry()]
    }
}

fn wallets_card<'a>(d: &Data) -> Element<'a, Msg> {
    if d.wallets.is_empty() {
        return ui::card("Wallets", ui::empty("user", "No wallets yet.", ""));
    }
    let mut wallets: Vec<&Wallet> = d.wallets.iter().collect();
    wallets.sort_by(|a, b| b.balance.total_cmp(&a.balance));
    let mut lines: Vec<Element<'a, Msg>> = Vec::new();
    lines.push(
        row![cell(3, false, ui::label("Agent")), cell(2, false, ui::label("Role")), cell(2, true, ui::label("Balance"))].into(),
    );
    for w in wallets {
        let role = d
            .roster
            .iter()
            .find(|m| m.cid == w.cid)
            .map_or_else(|| "—".to_string(), |m| m.role_label.clone());
        lines.push(ui::hairline());
        let name = button(iced::widget::text(w.name.clone()).font(theme::SERIF).size(14.0).color(theme::INK))
            .padding(0.0)
            .style(link)
            .on_press(Msg::Go(Id::Agent(w.cid)));
        lines.push(
            row![
                cell(3, false, name),
                cell(2, false, iced::widget::text(role).font(theme::SERIF).size(14.0).color(theme::INK_2)),
                cell(2, true, ui::mono(money(w.balance), 12.5, theme::INK)),
            ]
            .align_y(Alignment::Center)
            .into(),
        );
    }
    ui::card("Wallets", column(lines).spacing(8).into())
}

fn ledger_card<'a>(d: &Data, state: &State) -> Element<'a, Msg> {
    let mut kinds: Vec<String> = d.journal.iter().map(|e| e.kind.clone()).collect();
    kinds.sort();
    kinds.dedup();
    let mut options = vec![KindChoice { kind: None, label: "All kinds".to_string() }];
    options.extend(kinds.into_iter().map(|k| KindChoice { kind: Some(k.clone()), label: k }));
    let selected = options.iter().find(|o| o.kind == state.kind_filter).cloned();
    let filter = pick_list(options, selected, Msg::KindPicked);

    let shown: Vec<&Entry> = d
        .journal
        .iter()
        .filter(|e| state.kind_filter.as_deref().map_or(true, |k| e.kind == k))
        .take(50)
        .collect();
    let body: Element<'a, Msg> = if shown.is_empty() {
        ui::empty::<Msg>("clock", "No entries.", "")
    } else {
        let rows: Vec<Element<'a, Msg>> = shown.into_iter().map(entry_row).collect();
        let last = rows.len().saturating_sub(1);
        let mut ledger = column![].width(Length::Fill);
        for (i, r) in rows.into_iter().enumerate() {
            ledger = ledger.push(r);
            if i < last {
                ledger = ledger.push(container(Space::new().height(1.0)).width(Length::Fill).style(|_: &Theme| container::Style {
                    background: Some(theme::RULE_FAINT.into()),
                    ..Default::default()
                }));
            }
        }
        ledger.into()
    };
    let head = container(
        row![ui::label("Journal"), Space::new().width(Length::Fill), filter].spacing(12).align_y(Alignment::Center),
    )
    .padding(Padding { top: 8.0, right: 12.0, bottom: 8.0, left: 12.0 })
    .width(Length::Fill)
    .style(|_: &Theme| container::Style { border: Border { color: theme::RULE, width: 1.0, radius: 0.0.into() }, ..Default::default() });
    let frame = container(body).width(Length::Fill).style(|_: &Theme| container::Style {
        border: Border { color: theme::RULE, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    });
    column![head, frame].into()
}

fn entry_row<'a>(e: &Entry) -> Element<'a, Msg> {
    let memo = if e.memo.is_empty() {
        match &e.task {
            Some(task) => format!("{} for {task}", e.kind),
            None => e.kind.clone(),
        }
    } else {
        e.memo.clone()
    };
    row![
        container(ui::mono(format!("#{}", e.id), 11.5, theme::INK_3)).width(Length::Fixed(56.0)),
        container(ui::mono(ago(e.at), 11.5, theme::INK_3)).width(Length::Fixed(92.0)),
        container(ui::badge::<Msg>(&e.kind, kind_tone(&e.kind))).width(Length::Fixed(110.0)),
        container(iced::widget::text(memo).font(theme::SERIF).size(15.0).color(theme::INK)).width(Length::Fill),
        iced::widget::text(money(e.amount())).font(AMOUNT).size(14.0).color(theme::INK),
    ]
    .spacing(14)
    .padding(Padding { top: 9.0, right: 14.0, bottom: 9.0, left: 14.0 })
    .align_y(Alignment::Center)
    .into()
}

fn mint_form<'a>(state: &State) -> Element<'a, Msg> {
    let amount = text_input("Amount", &state.mint_amount)
        .on_input(Msg::MintAmount)
        .on_submit(Msg::SubmitMint)
        .style(theme::input);
    column![
        field("Amount", amount.into(), "Whole units added to the plaza budget."),
        ui::btn("Mint", None, ui::Kind::Primary, (!state.busy).then_some(Msg::SubmitMint)),
    ]
    .spacing(12)
    .into()
}

fn fine_form<'a>(state: &State) -> Element<'a, Msg> {
    let choices = roster_choices(state);
    if choices.is_empty() {
        return ui::faint("There are no agents to fine.").into();
    }
    let who = pick_list(choices, fine_choice(state), Msg::FinePick);
    let amount = text_input("Amount", &state.fine_amount)
        .on_input(Msg::FineAmount)
        .on_submit(Msg::SubmitFine)
        .style(theme::input);
    let memo = text_input("What for", &state.fine_memo)
        .on_input(Msg::FineMemo)
        .on_submit(Msg::SubmitFine)
        .style(theme::input);
    column![
        field("Agent", who.into(), ""),
        field("Amount", amount.into(), "Taken from the agent’s wallet."),
        field("Memo", memo.into(), "Shown in the journal."),
        ui::btn("Fine", None, ui::Kind::Danger, (!state.busy).then_some(Msg::SubmitFine)),
    ]
    .spacing(12)
    .into()
}

fn field<'a>(title: &str, input: Element<'a, Msg>, hint: &str) -> Element<'a, Msg> {
    column![ui::label(title), input, ui::faint(hint.to_string())].spacing(6).into()
}

/// One table cell: a fixed share of the row, its content right-aligned when `right`.
fn cell<'a>(portion: u16, right: bool, content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content.into())
        .width(Length::FillPortion(portion))
        .align_x(if right { Alignment::End } else { Alignment::Start })
        .into()
}

/// A plain link-looking button for an agent's name.
fn link(_: &Theme, _: button::Status) -> button::Style {
    button::Style { text_color: ui::INK, ..Default::default() }
}

fn roster_choices(state: &State) -> Vec<AgentChoice> {
    state
        .data
        .as_ref()
        .map(|d| d.roster.iter().map(|a| AgentChoice { cid: a.cid, label: a.name.clone() }).collect())
        .unwrap_or_default()
}

/// The agent the fine form targets: the picked one, else the first on the roster (as the web's select).
fn fine_choice(state: &State) -> Option<AgentChoice> {
    state.fine_target.clone().or_else(|| roster_choices(state).into_iter().next())
}

/// A whole amount above zero, if the text is one.
fn whole(text: &str) -> Option<i64> {
    text.trim().parse::<i64>().ok().filter(|n| *n > 0)
}

fn kind_tone(kind: &str) -> ui::Tone {
    match kind {
        "mint" => ui::Tone::Ok,
        "spend" => ui::Tone::Warn,
        "payment" => ui::Tone::Signal,
        "refund" | "transfer" => ui::Tone::Info,
        "fine" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

/// Whole units with thousands separators, as the web's `money`.
fn money(n: f64) -> String {
    let rounded = n.round() as i64;
    let digits = rounded.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if rounded < 0 {
        format!("-{out}")
    } else {
        out
    }
}

fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
}

/// A relative time, as the web's `ago`; `ms` is epoch milliseconds.
fn ago(ms: f64) -> String {
    if ms <= 0.0 {
        return "—".to_string();
    }
    let s = ((now_ms() - ms) / 1000.0).round().max(0.0) as i64;
    match s {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// The data the screen shows, read from the hub's JSON.
#[derive(Debug, Clone)]
pub struct Data {
    plaza_budget: f64,
    spent: f64,
    accounts: Vec<Account>,
    journal: Vec<Entry>,
    wallets: Vec<Wallet>,
    roster: Vec<Member>,
}

#[derive(Debug, Clone)]
struct Account {
    kind: String,
    balance: f64,
}

#[derive(Debug, Clone)]
struct Entry {
    id: i64,
    kind: String,
    at: f64,
    postings: Vec<f64>,
    memo: String,
    task: Option<String>,
}

#[derive(Debug, Clone)]
struct Wallet {
    cid: u64,
    name: String,
    balance: f64,
}

#[derive(Debug, Clone)]
struct Member {
    cid: u64,
    name: String,
    role_label: String,
}

impl Data {
    fn parse(treasury: &Value, journal: &Value, wallets: &Value, roster: &Value) -> Data {
        let mut journal: Vec<Entry> = list(journal, "entries").iter().map(Entry::parse).collect();
        journal.sort_by(|a, b| b.at.total_cmp(&a.at));
        Data {
            plaza_budget: num(treasury, "plaza_budget"),
            spent: num(treasury, "spent"),
            accounts: list(treasury, "accounts")
                .iter()
                .map(|a| Account { kind: text(a, "kind"), balance: num(a, "balance") })
                .collect(),
            journal,
            wallets: list(wallets, "wallets")
                .iter()
                .map(|w| Wallet { cid: uint(w, "cid"), name: text(w, "name"), balance: num(w, "balance") })
                .collect(),
            roster: list(roster, "agents")
                .iter()
                .map(|a| Member { cid: uint(a, "cid"), name: text(a, "name"), role_label: text(a, "role_label") })
                .collect(),
        }
    }
}

impl Entry {
    fn parse(v: &Value) -> Entry {
        Entry {
            id: v.get("id").and_then(Value::as_i64).unwrap_or(0),
            kind: text(v, "kind"),
            at: num(v, "at"),
            postings: v
                .get("postings")
                .and_then(Value::as_array)
                .map(|ps| ps.iter().map(|p| num(p, "amount")).collect())
                .unwrap_or_default(),
            memo: text(v, "memo"),
            task: v.get("task").and_then(Value::as_str).map(str::to_string),
        }
    }

    /// A balanced entry carries its debit legs as positive amounts; the entry's size is their sum.
    fn amount(&self) -> f64 {
        self.postings.iter().copied().filter(|a| *a > 0.0).sum()
    }
}

/// An array at the root, or under `key` (the journal may come wrapped).
fn list<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.as_array().or_else(|| v.get(key).and_then(Value::as_array)).map(Vec::as_slice).unwrap_or(&[])
}

fn num(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn uint(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}
