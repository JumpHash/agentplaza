//! The Treasury screen (parity with the web console's `treasury` view): the plaza budget, where it
//! sits, where it went, every wallet and the journal. The owner mints and fines here; everything
//! else is read. Figures count up, the charts ease between ranges, the flow's dashes run while the
//! screen is shown, and the series, wallets and journal page through their own tables.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iced::font::{self, Font};
use iced::widget::canvas::{self, Path, Stroke};
use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, pick_list, responsive, row, text_input, Space};
use iced::{alignment, mouse, Alignment, Border, Color, Element, Event, Length, Padding, Pixels, Point, Rectangle, Renderer, Size, Subscription, Task, Theme};
use serde_json::{json, Value};

use super::usage::usd;
use super::Id;
use crate::console::{chart, client::{self, Client}, events, ui, Ctx};
use crate::theme::{self, MONO};

const VALUE: Font = Font { weight: font::Weight::Bold, ..theme::MONO };
const AMOUNT: Font = Font { weight: font::Weight::Semibold, ..theme::MONO };
const ITALIC: Font = Font { style: font::Style::Italic, ..theme::SERIF };

const ANIM: Duration = Duration::from_millis(600);
const DAY_MS: u64 = 86_400_000;
const HOUR_MS: u64 = 3_600_000;
const JOURNAL_PAGE: usize = 50;
const SERIES_PAGE: usize = 25;
/// One credit is a tenth of a cent.
const CREDIT_USD: f64 = 0.001;
const KNOWN_KINDS: [&str; 11] = ["mint", "budget_move", "payment", "refund", "fine", "fee", "clawback", "spend", "subcontract", "transfer", "model"];
const BLUE: Color = Color::from_rgb8(0x6f, 0xa8, 0xdc);
const PURPLE: Color = Color::from_rgb8(0xb4, 0x8e, 0xd6);
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DASH: f32 = 7.0;
const DASH_PERIOD: f32 = 14.0;
const BAR_PAD_LEFT: f32 = 58.0;
const BAR_PAD_BOTTOM: f32 = 22.0;
/// The gap between tiles, cards and sections.
const GAP: f32 = 16.0;
/// The narrowest a tile may get; below five of them plus gaps, the tiles wrap to rows of three.
const TILE_MIN: f32 = 150.0;
/// Estimated advance of one 11px mono glyph, and of one 15px serif glyph (for fitting text).
const MONO11_CHAR: f32 = 7.6;
const SERIF15_CHAR: f32 = 7.6;
/// The gap between legend entries, and an entry's swatch with the gaps inside it.
const LEGEND_GAP: f32 = 18.0;
const LEGEND_SWATCH: f32 = 22.0;
/// A journal row's fixed columns (id, time, kind, amount) with their gaps, and the memo's share.
const JOURNAL_FIXED: f32 = 56.0 + 92.0 + 120.0 + 90.0 + 4.0 * 12.0;
const SERIES_COLUMNS: [(SeriesSort, &str, u16); 11] = [
    (SeriesSort::Time, "When", 3),
    (SeriesSort::Minted, "Minted", 2),
    (SeriesSort::Escrowed, "Escrowed", 2),
    (SeriesSort::Paid, "Paid", 2),
    (SeriesSort::Refunded, "Refunded", 2),
    (SeriesSort::Fined, "Fined", 2),
    (SeriesSort::Spent, "Spent", 2),
    (SeriesSort::Entries, "Entries", 2),
    (SeriesSort::Budget, "Budget", 2),
    (SeriesSort::Circulating, "Circulating", 2),
    (SeriesSort::Treasury, "Treasury", 2),
];

#[derive(Debug, Default)]
pub struct State {
    /// The last failed call's message.
    pub error: Option<String>,
    /// The last answered action's note.
    notice: Option<String>,
    data: Option<Data>,
    busy: bool,
    mint_open: bool,
    mint_amount: String,
    fine_open: bool,
    fine_target: Option<AgentChoice>,
    fine_amount: String,
    fine_memo: String,
    range: Range,
    mode: Mode,
    balances: Animated,
    flows: Animated,
    counters: Counters,
    series_sort: SeriesSort,
    series_asc: bool,
    series_page: usize,
    balance_sort: BalanceSort,
    balance_asc: bool,
    flow_sort: FlowSort,
    flow_asc: bool,
    wallet_sort: WalletSort,
    wallet_asc: bool,
    /// The journal's filter as applied, and the draft typed into the task box.
    filter: JournalFilter,
    task_draft: String,
    journal: Vec<Entry>,
    journal_total: u64,
    journal_next: Option<i64>,
    older_busy: bool,
    /// Rows that arrived on a poll, with the moment they arrived (they fade in).
    fresh: Vec<(i64, Instant)>,
    /// A load is in flight; a live event meanwhile marks the screen dirty, and the answer starts the next load.
    loading: bool,
    dirty: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Snapshot, String>),
    Range(Range),
    Mode(Mode),
    SeriesSort(SeriesSort),
    /// Moves the series table by this many pages.
    SeriesPage(i64),
    WalletSort(WalletSort),
    BalanceSort(BalanceSort),
    FlowSort(FlowSort),
    KindPicked(KindChoice),
    WalletPicked(WalletChoice),
    TaskDraft(String),
    TaskApply,
    OlderPressed,
    Older(JournalFilter, Result<JournalPage, String>),
    /// An animation frame while the screen shows data.
    Frame,
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
    /// A live event from the hub; a `treasury` one reloads the screen.
    Event(events::Event),
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

/// An agent in the journal's filter; `None` is every agent.
#[derive(Debug, Clone, PartialEq)]
pub struct WalletChoice {
    cid: Option<u64>,
    label: String,
}

impl fmt::Display for WalletChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// The window the balances and flows cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Range {
    Week,
    #[default]
    Month,
    Quarter,
    /// The last year: the series is bucketed by day, so "all" is bounded here.
    All,
}

impl Range {
    const ALL: [(Range, &'static str); 4] = [(Range::Week, "7 d"), (Range::Month, "30 d"), (Range::Quarter, "90 d"), (Range::All, "All")];

    fn days(self) -> u64 {
        match self {
            Range::Week => 7,
            Range::Month => 30,
            Range::Quarter => 90,
            Range::All => 365,
        }
    }

    fn hourly(self) -> bool {
        self == Range::Week
    }

    fn bucket(self) -> &'static str {
        if self.hourly() {
            "hour"
        } else {
            "day"
        }
    }
}

/// The chart view or the table view of the same series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Chart,
    Table,
}

impl Mode {
    const ALL: [(Mode, &'static str); 2] = [(Mode::Chart, "Chart"), (Mode::Table, "Table")];
}

/// The series table's columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SeriesSort {
    #[default]
    Time,
    Minted,
    Escrowed,
    Paid,
    Refunded,
    Fined,
    Spent,
    Entries,
    Budget,
    Circulating,
    Treasury,
}

/// The wallets table's columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WalletSort {
    Name,
    Role,
    #[default]
    Balance,
    Earned,
    Spent,
    Share,
}

/// The balance table's columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BalanceSort {
    #[default]
    Time,
    Budget,
    Circulating,
    Treasury,
}

/// The flow table's columns: the bucket, the entries, then the kinds that moved money (by index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlowSort {
    #[default]
    Time,
    Entries,
    Kind(usize),
}

/// The journal's filters as applied; a change starts the journal over from its newest page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct JournalFilter {
    kind: Option<String>,
    wallet: Option<u64>,
    task: String,
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// The animation frames while the screen shows data: the figures, the charts, the new journal rows
/// and the flow's dashes. Subscribed by `screens::subscription` while the Treasury is open.
pub fn subscription(state: &State, ctx: &Ctx) -> Subscription<Msg> {
    let frames = if state.data.is_some() {
        iced::time::every(Duration::from_millis(33)).map(|_| Msg::Frame)
    } else {
        Subscription::none()
    };
    Subscription::batch([frames, events::subscription(&ctx.client).map(Msg::Event)])
}

/// Loads the treasury, its series, wallets, roster, real cost and the journal's newest page (called
/// when it opens and on every poll tick).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = true;
    let to = now_ms() as u64;
    let query = Query { from: to.saturating_sub(state.range.days() * DAY_MS), to, bucket: state.range.bucket(), filter: state.filter.clone() };
    let client = ctx.client.clone();
    client::task(move || fetch(&client, &query), Msg::Loaded)
}

struct Query {
    from: u64,
    to: u64,
    bucket: &'static str,
    filter: JournalFilter,
}

fn fetch(c: &Client, q: &Query) -> Result<Snapshot, String> {
    let treasury: Value = c.get("/v1/treasury")?;
    let wallets: Value = c.get("/v1/treasury/wallets")?;
    let roster: Value = c.get("/v1/agents")?;
    let series: Value = c.get(&format!("/v1/treasury/series?from={}&to={}&bucket={}", q.from, q.to, q.bucket))?;
    let usage: Value = c.get(&format!("/v1/usage/summary?from={}&to={}", q.from, q.to))?;
    let page = journal_page(c, &q.filter, None)?;
    let mut data = Data::parse(&treasury, &wallets, &roster, &series, &usage);
    // Earned and spent come from each wallet's agent profile; a profile that fails leaves them out (shown as a dash).
    for w in &mut data.wallets {
        let profile: Result<Value, String> = c.get(&format!("/v1/agents/{}/profile", w.cid));
        if let Ok(p) = profile {
            w.earned = Some(num(&p, "earned"));
            w.spent = Some(num(&p, "spent"));
        }
    }
    Ok(Snapshot { data, filter: q.filter.clone(), page })
}

fn journal_page(c: &Client, filter: &JournalFilter, before: Option<i64>) -> Result<JournalPage, String> {
    let v: Value = c.get(&journal_path(filter, before))?;
    Ok(JournalPage::parse(&v))
}

/// `GET /v1/treasury/journal` with its filters; `before` pages back from an entry id.
pub fn journal_path(filter: &JournalFilter, before: Option<i64>) -> String {
    let mut path = format!("/v1/treasury/journal?limit={JOURNAL_PAGE}");
    if let Some(b) = before {
        path.push_str(&format!("&before={b}"));
    }
    if let Some(kind) = &filter.kind {
        path.push_str(&format!("&kind={}", enc(kind)));
    }
    if let Some(wallet) = filter.wallet {
        path.push_str(&format!("&wallet={wallet}"));
    }
    if !filter.task.is_empty() {
        path.push_str(&format!("&task={}", enc(&filter.task)));
    }
    path
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Frame => {
            let now = Instant::now();
            state.fresh.retain(|(_, at)| now.saturating_duration_since(*at) < ANIM);
            Task::none()
        }
        Msg::Loaded(Ok(snap)) => {
            state.error = None;
            // A poll that answers an old filter is stale: the new filter's own load is on its way.
            if snap.filter == state.filter {
                apply(state, snap);
            }
            settle(state, ctx)
        }
        Msg::Loaded(Err(err)) => {
            state.error = Some(err);
            settle(state, ctx)
        }
        Msg::Event(event) => {
            if event.kind != "treasury" {
                return Task::none();
            }
            if state.loading {
                state.dirty = true;
                Task::none()
            } else {
                load(state, ctx)
            }
        }
        Msg::Range(range) => {
            state.range = range;
            state.series_page = 0;
            load(state, ctx)
        }
        Msg::Mode(mode) => {
            state.mode = mode;
            Task::none()
        }
        Msg::SeriesSort(sort) => {
            resort(&mut state.series_sort, &mut state.series_asc, sort, false);
            state.series_page = 0;
            Task::none()
        }
        Msg::SeriesPage(delta) => {
            state.series_page = (state.series_page as i64 + delta).max(0) as usize;
            Task::none()
        }
        Msg::BalanceSort(sort) => {
            resort(&mut state.balance_sort, &mut state.balance_asc, sort, false);
            Task::none()
        }
        Msg::FlowSort(sort) => {
            resort(&mut state.flow_sort, &mut state.flow_asc, sort, false);
            Task::none()
        }
        Msg::WalletSort(sort) => {
            resort(&mut state.wallet_sort, &mut state.wallet_asc, sort, matches!(sort, WalletSort::Name | WalletSort::Role));
            Task::none()
        }
        Msg::KindPicked(choice) => {
            state.filter.kind = choice.kind;
            refilter(state, ctx)
        }
        Msg::WalletPicked(choice) => {
            state.filter.wallet = choice.cid;
            refilter(state, ctx)
        }
        Msg::TaskDraft(value) => {
            state.task_draft = value;
            Task::none()
        }
        Msg::TaskApply => {
            state.filter.task = state.task_draft.trim().to_string();
            refilter(state, ctx)
        }
        Msg::OlderPressed => {
            let Some(before) = state.journal_next else { return Task::none() };
            if state.older_busy {
                return Task::none();
            }
            state.older_busy = true;
            let filter = state.filter.clone();
            let client = ctx.client.clone();
            let for_reply = filter.clone();
            client::task(move || journal_page(&client, &filter, Some(before)), move |res| Msg::Older(for_reply.clone(), res))
        }
        Msg::Older(filter, res) => {
            state.older_busy = false;
            match res {
                Ok(page) if filter == state.filter => {
                    for e in page.entries {
                        if !state.journal.iter().any(|x| x.id == e.id) {
                            state.journal.push(e);
                        }
                    }
                    state.journal.sort_by(|a, b| b.id.cmp(&a.id));
                    state.journal_next = page.next_before;
                    state.journal_total = page.total;
                }
                Ok(_) => {}
                Err(err) => state.error = Some(err),
            }
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

/// A load has answered: a live event that came meanwhile starts the next one, so loads never pile up.
fn settle(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = false;
    if std::mem::take(&mut state.dirty) {
        load(state, ctx)
    } else {
        Task::none()
    }
}

/// Starts the journal over under the current filter and loads its newest page.
fn refilter(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.journal.clear();
    state.journal_next = None;
    state.journal_total = 0;
    state.fresh.clear();
    load(state, ctx)
}

/// Takes a fresh snapshot: the newest journal rows merge over the older pages already loaded, rows
/// new since the last poll fade in, and the charts and figures ease to their new targets.
fn apply(state: &mut State, snap: Snapshot) {
    let now = Instant::now();
    let head = snap.page.entries;
    let oldest = head.iter().map(|e| e.id).min();
    let older_loaded = oldest.is_some_and(|o| state.journal.iter().any(|e| e.id < o));
    if !state.journal.is_empty() {
        for e in &head {
            if !state.journal.iter().any(|x| x.id == e.id) {
                state.fresh.push((e.id, now));
            }
        }
    }
    state.journal = merge_head(&state.journal, head);
    state.journal_total = snap.page.total;
    if !older_loaded {
        state.journal_next = snap.page.next_before;
    }
    state.data = Some(snap.data);
    sync(state, now);
}

/// Points the animated series and figures at the loaded data.
fn sync(state: &mut State, now: Instant) {
    let Some(d) = state.data.as_ref() else { return };
    state.balances.retarget(balance_series(&d.points), now);
    state.flows.retarget(flow_series(&d.points), now);
    state.counters.budget.aim(d.budget, now);
    state.counters.circulating.aim(d.circulating(), now);
    state.counters.paid.aim(d.paid(), now);
    state.counters.usd.aim(d.cost, now);
    state.counters.treasury.aim(d.treasury_now(), now);
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let now = Instant::now();
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
            parts.extend(sections(data, state, now));
        }
    }
    ui::sheet(head, ui::scroll(column(parts).spacing(GAP).width(Length::Fill)).spacing(GAP).height(Length::Fill).into())
}

/// The read-only body: tallies, the range and view controls, the charts or the series table, the
/// flow, the wallets and the journal.
fn sections<'a>(d: &Data, state: &State, now: Instant) -> Vec<Element<'a, Msg>> {
    let mut out: Vec<Element<'a, Msg>> = vec![tallies(d, state, now), controls(state)];
    match state.mode {
        Mode::Chart => out.extend(charts(d, state, now)),
        Mode::Table => {
            out.push(ui::card("Balances", balance_table(d, state)));
            out.push(ui::card("Flows", flow_table(d, state)));
            out.push(ui::card("Series", series_table(d, state)));
        }
    }
    out.push(ui::card("Where the money goes", flow(d)));
    out.push(wallets_card(d, state));
    out.push(journal_card(Some(d), state, now));
    out
}

/// The five tallies, in one row when there is room for five tiles, else rows of three.
fn tallies<'a>(d: &Data, state: &State, now: Instant) -> Element<'a, Msg> {
    let c = &state.counters;
    let cost = c.usd.at(now);
    let stats: Vec<(&'static str, String, String)> = vec![
        ("Plaza budget", money(c.budget.at(now)), String::new()),
        ("In circulation", money(c.circulating.at(now)), "held in agents’ wallets".to_string()),
        ("Paid out", money(c.paid.at(now)), "margins for verified work".to_string()),
        ("Real model cost", usd(cost), format!("≈ {} credits metered", money(cost / CREDIT_USD))),
        ("Treasury", money(c.treasury.at(now)), format!("{} fined in range", money(d.fined()))),
    ];
    responsive(move |size| {
        let per_row = if size.width >= 5.0 * TILE_MIN + 4.0 * GAP { 5 } else { 3 };
        let rows: Vec<Element<'a, Msg>> = stats
            .chunks(per_row)
            .map(|chunk| {
                let mut line = row![].spacing(GAP);
                for (title, value, note) in chunk {
                    line = line.push(tile(title, value.clone(), note.clone()));
                }
                // A short last row keeps the grid's columns: the empty slots take the same share.
                for _ in chunk.len()..per_row {
                    line = line.push(Space::new().width(Length::FillPortion(1)));
                }
                line.into()
            })
            .collect();
        column(rows).spacing(GAP).width(Length::Fill).into()
    })
    .into()
}

fn controls<'a>(state: &State) -> Element<'a, Msg> {
    row![segmented(&Range::ALL, state.range, Msg::Range), Space::new().width(Length::Fill), segmented(&Mode::ALL, state.mode, Msg::Mode)]
        .spacing(16)
        .align_y(Alignment::Center)
        .into()
}

/// The chart view: balances over time, and the flows per bucket stacked by kind.
fn charts<'a>(d: &Data, state: &State, now: Instant) -> Vec<Element<'a, Msg>> {
    let labels: Vec<String> = d.points.iter().map(|p| point_label(p.at as i64, state.range.hourly())).collect();
    let balances = state.balances.current(now);
    let flows = state.flows.current(now);
    let balance_card = ui::card(
        "Balances over time",
        column![legend(&balances, money, true), chart::stacked_area(balances, labels.clone(), money, "No balances in this range")].spacing(12).into(),
    );
    let flow_card = ui::card(
        "Flows per bucket",
        column![
            legend(&flows, money, false),
            iced::widget::canvas(StackedBars { series: flows, labels }).width(Length::Fill).height(Length::Fixed(220.0)),
        ]
        .spacing(12)
        .into(),
    );
    vec![balance_card, flow_card]
}

/// A tally tile (the web's `.stat`): the figure, its label, then an italic note when there is one.
fn tile<'a>(title: &str, value: String, note: String) -> Element<'a, Msg> {
    // Long figures step down a size so they stay inside the tile.
    let size = if value.chars().count() > 8 { 22.0 } else { 28.0 };
    let mut stack = column![iced::widget::text(value).font(VALUE).size(size).color(crate::theme::palette().ink).wrapping(Wrapping::None), ui::label(title).wrapping(Wrapping::None)]
        .spacing(6);
    // A note wraps onto as many lines as it needs and the tile grows with it; a tile without one keeps
    // a blank line, so the tiles beside it stay the same height.
    let note = if note.is_empty() { "\u{a0}".to_string() } else { note };
    stack = stack.push(iced::widget::text(note).font(ITALIC).size(13.0).color(crate::theme::palette().ink_2));
    container(stack)
        .padding(Padding::from(16.0))
        .width(Length::FillPortion(1))
        .style(|_: &Theme| container::Style { border: Border { color: crate::theme::palette().rule, width: 1.0, radius: 0.0.into() }, ..Default::default() })
        .into()
}

/// The budget's path (the web's `flow`): budget, escrow, then spend (providers, compute) or payment
/// (agents), then wallets. The arrows carry dashes that run along them, faster for larger flows.
fn flow<'a>(d: &Data) -> Element<'a, Msg> {
    // The same range totals as the flows chart's legend: the series summed over the selected range.
    let diagram = FlowChart { budget: d.budget, escrow: d.escrow, escrowed: d.escrowed(), refunded: d.refunded(), spent: d.spent(), paid: d.paid(), wallets: d.wallets_total(), t: now_ms() / 1000.0 };
    iced::widget::canvas(diagram).width(Length::Fill).height(Length::Fixed(180.0)).into()
}

struct FlowChart {
    budget: f64,
    escrow: f64,
    /// The range's escrowed and refunded flows, on the arrows between budget and escrow.
    escrowed: f64,
    refunded: f64,
    spent: f64,
    paid: f64,
    wallets: f64,
    /// Seconds, for the dashes.
    t: f64,
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
            frame.fill_rectangle(tl, size, crate::theme::palette().sheet);
            frame.stroke(&Path::rectangle(tl, size), Stroke::default().with_color(crate::theme::palette().ink).with_width(1.0));
            frame.fill_text(canvas::Text {
                content: label.to_uppercase(),
                position: at(x + 12.0, y + 12.0),
                color: crate::theme::palette().ink_3,
                size: Pixels(10.5 * scale),
                font: MONO,
                ..canvas::Text::default()
            });
            frame.fill_text(canvas::Text {
                content: money(value),
                position: at(x + 12.0, y + 33.0),
                color: crate::theme::palette().ink,
                size: Pixels(24.0 * scale),
                font: VALUE,
                ..canvas::Text::default()
            });
        };
        let arrow = |frame: &mut canvas::Frame, from: (f32, f32), to: (f32, f32), flow: f64, max: f64| {
            let share = (flow.max(0.0) / max) as f32;
            let (dx, dy) = (to.0 - from.0, to.1 - from.1);
            let len = (dx * dx + dy * dy).sqrt().max(1.0);
            let (ux, uy) = (dx / len, dy / len);
            let head = 8.75;
            let base = (to.0 - ux * head, to.1 - uy * head);
            frame.stroke(&Path::line(at(from.0, from.1), at(base.0, base.1)), Stroke::default().with_color(crate::theme::palette().ink_2).with_width((1.0 + 3.0 * share) * scale));
            let (px, py) = (-uy * head * 0.5, ux * head * 0.5);
            let (wing_a, wing_b, tip) = (at(base.0 + px, base.1 + py), at(base.0 - px, base.1 - py), at(to.0, to.1));
            let head_path = Path::new(|b| {
                b.move_to(tip);
                b.line_to(wing_a);
                b.line_to(wing_b);
                b.close();
            });
            frame.fill(&head_path, crate::theme::palette().ink_2);
            // The dashes: they travel from the source toward the head, at a speed that grows with the flow.
            let speed = (20.0 + 30.0 * (1.0 + flow.max(0.0)).log10()).min(140.0) as f32;
            let travel = (self.t as f32 * speed) % DASH_PERIOD;
            let run = len - head;
            let mut s = travel - DASH_PERIOD;
            while s < run {
                let (a, b) = (s.max(0.0), (s + DASH).min(run));
                if b > a {
                    let p = at(from.0 + ux * a, from.1 + uy * a);
                    let q = at(from.0 + ux * b, from.1 + uy * b);
                    frame.stroke(&Path::line(p, q), Stroke::default().with_color(crate::theme::palette().signal).with_width(2.0 * scale));
                }
                s += DASH_PERIOD;
            }
            if flow > 0.0 {
                frame.fill_text(canvas::Text {
                    content: money(flow),
                    position: at((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0 - 6.0),
                    color: crate::theme::palette().ink_2,
                    size: Pixels(10.5 * scale),
                    font: MONO,
                    align_x: alignment::Horizontal::Center.into(),
                    ..canvas::Text::default()
                });
            }
        };

        boxed(&mut frame, 0.0, 55.0, "Budget · now", self.budget);
        boxed(&mut frame, 220.0, 55.0, "Escrow · now", self.escrow);
        boxed(&mut frame, 440.0, 10.0, "Spend · range", self.spent);
        boxed(&mut frame, 440.0, 100.0, "Payment · range", self.paid);
        boxed(&mut frame, 660.0, 55.0, "Wallets · now", self.wallets);
        let arrows = [
            ((170.0, 78.0), (220.0, 78.0), self.escrowed),
            ((220.0, 102.0), (170.0, 102.0), self.refunded),
            ((390.0, 70.0), (440.0, 45.0), self.spent),
            ((390.0, 110.0), (440.0, 135.0), self.paid),
            ((610.0, 135.0), (660.0, 100.0), self.paid),
        ];
        let max = arrows.iter().map(|a| a.2).fold(1.0, f64::max);
        for (from, to, flow) in arrows {
            arrow(&mut frame, from, to, flow, max);
        }
        vec![frame.into_geometry()]
    }
}

/// Stacked bars per bucket, one colour per series, with a hover column and a tooltip.
struct StackedBars {
    series: Vec<chart::Series>,
    labels: Vec<String>,
}

/// The plot inside a bars chart's bounds: (left, top, width, height).
fn bars_plot(bounds: Rectangle) -> (f32, f32, f32, f32) {
    let width = (bounds.width - BAR_PAD_LEFT - 10.0).max(1.0);
    let height = (bounds.height - 10.0 - BAR_PAD_BOTTOM).max(1.0);
    (BAR_PAD_LEFT, 10.0, width, height)
}

/// The bar under `x`, or None when `x` is off the plot.
fn bar_at(x: f32, n: usize, left: f32, width: f32) -> Option<usize> {
    if n == 0 || x < left || x > left + width {
        return None;
    }
    let slot = width / n as f32;
    Some((((x - left) / slot) as usize).min(n - 1))
}

fn caption(frame: &mut canvas::Frame, content: String, at: Point, color: Color, size: f32, right: bool) {
    frame.fill_text(canvas::Text {
        content,
        position: at,
        color,
        size: Pixels(size),
        font: MONO,
        align_x: if right { alignment::Horizontal::Right.into() } else { alignment::Horizontal::Left.into() },
        ..canvas::Text::default()
    });
}

impl canvas::Program<Msg> for StackedBars {
    type State = Option<usize>;

    fn update(&self, hover: &mut Option<usize>, event: &Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<canvas::Action<Msg>> {
        if !matches!(event, Event::Mouse(_)) {
            return None;
        }
        let (left, _, width, _) = bars_plot(bounds);
        let next = cursor.position_in(bounds).and_then(|p| bar_at(p.x, self.labels.len(), left, width));
        if next != *hover {
            *hover = next;
            return Some(canvas::Action::request_redraw());
        }
        None
    }

    fn draw(&self, hover: &Option<usize>, renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (left, top, width, height) = bars_plot(bounds);
        let tops = chart::stacked_tops(&self.series);
        let n = tops.first().map_or(0, Vec::len);
        let peak = tops.last().map_or(0.0, |t| t.iter().copied().fold(0.0, f64::max));
        let ceiling = chart::nice_ceiling(peak);
        let y = |v: f64| top + height - (v / ceiling) as f32 * height;
        let at = |v: &[f64], i: usize| v.get(i).copied().unwrap_or(0.0);

        for k in 0..=4 {
            let v = ceiling * k as f64 / 4.0;
            let yy = y(v);
            let rule = if k == 0 { crate::theme::palette().rule } else { crate::theme::palette().rule_faint };
            frame.stroke(&Path::line(Point::new(left, yy), Point::new(left + width, yy)), Stroke::default().with_color(rule).with_width(1.0));
            caption(&mut frame, money(v), Point::new(left - 8.0, yy - 6.0), crate::theme::palette().ink_3, 10.5, true);
        }
        if n == 0 || peak <= 0.0 {
            caption(&mut frame, "No flows in this range".to_string(), Point::new(left + width / 2.0, top + height / 2.0 - 7.0), crate::theme::palette().ink_3, 13.0, false);
            return vec![frame.into_geometry()];
        }

        let slot = width / n as f32;
        let bar = (slot * 0.7).max(1.0);
        for (s, series) in self.series.iter().enumerate() {
            for i in 0..n {
                let lo = if s == 0 { 0.0 } else { at(&tops[s - 1], i) };
                let hi = at(&tops[s], i);
                if hi > lo {
                    let bx = left + slot * i as f32 + (slot - bar) / 2.0;
                    let top_y = y(hi);
                    frame.fill_rectangle(Point::new(bx, top_y), Size::new(bar, y(lo) - top_y), series.color);
                }
            }
        }

        let ticks = n.min(6);
        for k in 0..ticks {
            let i = if ticks <= 1 { 0 } else { k * (n - 1) / (ticks - 1) };
            let x = left + slot * i as f32 + slot / 2.0;
            let last_tick = k + 1 == ticks && ticks > 1;
            let anchor = if last_tick { Point::new(left + width, top + height + 6.0) } else { Point::new(x, top + height + 6.0) };
            caption(&mut frame, self.labels.get(i).cloned().unwrap_or_default(), anchor, crate::theme::palette().ink_3, 10.5, last_tick);
        }

        if let Some(i) = hover.filter(|i| *i < n) {
            let hx = left + slot * i as f32;
            frame.fill_rectangle(Point::new(hx, top), Size::new(slot, height), crate::theme::palette().hover);
            let rows: Vec<(Color, String)> = self
                .series
                .iter()
                .enumerate()
                .filter_map(|(s, series)| {
                    let v = at(&tops[s], i) - if s == 0 { 0.0 } else { at(&tops[s - 1], i) };
                    (v > 0.0).then(|| (series.color, format!("{}  {}", series.key, money(v))))
                })
                .collect();
            let box_w = 200.0;
            let box_h = 14.0 + 16.0 * (rows.len() as f32 + 1.0) + 6.0;
            let bx = if hx > left + width / 2.0 { hx - 12.0 - box_w } else { hx + slot + 12.0 };
            let by = top + 4.0;
            frame.fill_rectangle(Point::new(bx, by), Size::new(box_w, box_h), crate::theme::palette().sheet);
            frame.stroke(&Path::rectangle(Point::new(bx, by), Size::new(box_w, box_h)), Stroke::default().with_color(crate::theme::palette().rule).with_width(1.0));
            caption(&mut frame, self.labels.get(i).cloned().unwrap_or_default(), Point::new(bx + 10.0, by + 8.0), crate::theme::palette().ink, 11.0, false);
            for (k, (color, text)) in rows.into_iter().enumerate() {
                let row_y = by + 8.0 + 16.0 * (k as f32 + 1.0);
                frame.fill(&Path::circle(Point::new(bx + 14.0, row_y + 6.0), 3.5), color);
                caption(&mut frame, text, Point::new(bx + 24.0, row_y), crate::theme::palette().ink_2, 11.0, false);
            }
        }
        vec![frame.into_geometry()]
    }
}

/// A row of swatches, each series' name and its value: the total over the range, or the last value
/// when `last` (for balances, which are levels rather than flows).
fn legend<'a>(series: &[chart::Series], format: fn(f64) -> String, last: bool) -> Element<'a, Msg> {
    let items: Vec<(Color, String, String)> = series
        .iter()
        .map(|s| {
            let total: f64 = if last { s.values.last().copied().unwrap_or(0.0) } else { s.values.iter().sum() };
            (s.color, s.key.clone(), format(total))
        })
        .collect();
    responsive(move |size| {
        // Entries fill a line left to right; an entry that would pass the width starts the next line.
        let mut lines: Vec<Vec<&(Color, String, String)>> = vec![Vec::new()];
        let mut used = 0.0_f32;
        for item in &items {
            let width = LEGEND_SWATCH + (item.1.chars().count() + item.2.chars().count() + 1) as f32 * MONO11_CHAR;
            let gap = if used > 0.0 { LEGEND_GAP } else { 0.0 };
            if used > 0.0 && used + gap + width > size.width {
                lines.push(Vec::new());
                used = 0.0;
            }
            used += if used > 0.0 { LEGEND_GAP } else { 0.0 } + width;
            if let Some(line) = lines.last_mut() {
                line.push(item);
            }
        }
        let rows: Vec<Element<'a, Msg>> = lines
            .into_iter()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let mut r = row![].spacing(LEGEND_GAP).align_y(Alignment::Center);
                for (color, key, value) in line {
                    let color = *color;
                    let swatch = container(Space::new().width(10.0).height(10.0)).style(move |_: &Theme| container::Style {
                        background: Some(color.into()),
                        ..Default::default()
                    });
                    r = r.push(
                        row![
                            swatch,
                            iced::widget::text(key.clone()).font(MONO).size(11.0).color(crate::theme::palette().ink_2).wrapping(Wrapping::None),
                            iced::widget::text(value.clone()).font(MONO).size(11.0).color(crate::theme::palette().ink).wrapping(Wrapping::None),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    );
                }
                r.into()
            })
            .collect();
        column(rows).spacing(6).width(Length::Fill).into()
    })
    .into()
}

/// The series as a table: sortable, paged, with a totals row (flows summed, balances at their end).
fn series_table<'a>(d: &Data, state: &State) -> Element<'a, Msg> {
    if d.points.is_empty() {
        return ui::empty("clock", "No buckets in this range.", "");
    }
    let hourly = state.range.hourly();
    let mut points: Vec<&Bucket> = d.points.iter().collect();
    points.sort_by(|a, b| {
        let o = series_value(a, state.series_sort).total_cmp(&series_value(b, state.series_sort));
        if state.series_asc {
            o
        } else {
            o.reverse()
        }
    });
    let pages = page_count(points.len(), SERIES_PAGE);
    let page = state.series_page.min(pages - 1);
    let heads: Vec<Element<'a, Msg>> = SERIES_COLUMNS
        .iter()
        .map(|(sort, label, portion)| sort_head(label, *portion, *sort, state.series_sort, state.series_asc, Msg::SeriesSort))
        .collect();
    let mut lines = column![row(heads).spacing(8)].spacing(8);
    for p in points.iter().copied().skip(page * SERIES_PAGE).take(SERIES_PAGE) {
        lines = lines.push(ui::hairline());
        lines = lines.push(row(series_cells(p, point_label(p.at as i64, hourly))).spacing(8).align_y(Alignment::Center));
    }
    let all = totals(&d.points);
    lines = lines.push(ui::hairline());
    lines = lines.push(row(series_cells(&all, "Total · end".to_string())).spacing(8).align_y(Alignment::Center));
    let footer = row![
        ui::faint(format!("{} buckets · page {} of {}", points.len(), page + 1, pages)),
        Space::new().width(Length::Fill),
        ui::btn("Prev", None, ui::Kind::Secondary, (page > 0).then_some(Msg::SeriesPage(-1))),
        ui::btn("Next", None, ui::Kind::Secondary, (page + 1 < pages).then_some(Msg::SeriesPage(1))),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    column![lines, footer].spacing(14).into()
}

/// The balance table: the plaza budget, the money in circulation and the treasury at each bucket's end.
/// Newest first until a column is picked.
fn balance_table<'a>(d: &Data, state: &State) -> Element<'a, Msg> {
    if d.points.is_empty() {
        return ui::empty("clock", "No buckets in this range.", "");
    }
    let hourly = state.range.hourly();
    let mut points: Vec<&Bucket> = d.points.iter().collect();
    points.sort_by(|a, b| {
        let o = balance_value(a, state.balance_sort).total_cmp(&balance_value(b, state.balance_sort));
        if state.balance_asc {
            o
        } else {
            o.reverse()
        }
    });
    let heads: Vec<Element<'a, Msg>> = vec![
        sort_head("When", 3, BalanceSort::Time, state.balance_sort, state.balance_asc, Msg::BalanceSort),
        sort_head("Plaza budget", 2, BalanceSort::Budget, state.balance_sort, state.balance_asc, Msg::BalanceSort),
        sort_head("In circulation", 2, BalanceSort::Circulating, state.balance_sort, state.balance_asc, Msg::BalanceSort),
        sort_head("Treasury", 2, BalanceSort::Treasury, state.balance_sort, state.balance_asc, Msg::BalanceSort),
    ];
    let figure = |x: f64| cell(2, true, ui::mono(money(x), 12.0, crate::theme::palette().ink));
    let mut lines = column![row(heads).spacing(8)].spacing(8);
    for p in points {
        let label = cell(3, false, ui::mono(point_label(p.at as i64, hourly), 11.5, crate::theme::palette().ink_2));
        lines = lines.push(ui::hairline());
        lines = lines.push(row![label, figure(p.budget), figure(p.circulating), figure(p.treasury)].spacing(8).align_y(Alignment::Center));
    }
    lines.into()
}

/// The flow table: the entries and the money of each kind that moved in each bucket, with a total row.
/// Newest first until a column is picked.
fn flow_table<'a>(d: &Data, state: &State) -> Element<'a, Msg> {
    if d.points.is_empty() {
        return ui::empty("clock", "No buckets in this range.", "");
    }
    let hourly = state.range.hourly();
    let kinds = flow_kinds(&d.points);
    let mut points: Vec<&Bucket> = d.points.iter().collect();
    points.sort_by(|a, b| {
        let o = flow_value(a, state.flow_sort, &kinds).total_cmp(&flow_value(b, state.flow_sort, &kinds));
        if state.flow_asc {
            o
        } else {
            o.reverse()
        }
    });
    let mut heads: Vec<Element<'a, Msg>> = vec![
        sort_head("When", 3, FlowSort::Time, state.flow_sort, state.flow_asc, Msg::FlowSort),
        sort_head("Entries", 2, FlowSort::Entries, state.flow_sort, state.flow_asc, Msg::FlowSort),
    ];
    for (i, kind) in kinds.iter().enumerate() {
        heads.push(sort_head(&kind_label(kind), 2, FlowSort::Kind(i), state.flow_sort, state.flow_asc, Msg::FlowSort));
    }
    let figure = |x: f64| cell(2, true, ui::mono(money(x), 12.0, crate::theme::palette().ink));
    let mut lines = column![row(heads).spacing(8)].spacing(8);
    for p in points {
        let mut cells: Vec<Element<'a, Msg>> = vec![
            cell(3, false, ui::mono(point_label(p.at as i64, hourly), 11.5, crate::theme::palette().ink_2)),
            figure(p.entries),
        ];
        cells.extend(kinds.iter().map(|k| figure(p.by_kind.get(k).copied().unwrap_or(0.0))));
        lines = lines.push(ui::hairline());
        lines = lines.push(row(cells).spacing(8).align_y(Alignment::Center));
    }
    let mut foot: Vec<Element<'a, Msg>> = vec![
        cell(3, false, ui::mono("Total".to_string(), 11.5, crate::theme::palette().ink_2)),
        figure(d.points.iter().map(|p| p.entries).sum::<f64>()),
    ];
    foot.extend(kinds.iter().map(|k| figure(d.points.iter().map(|p| p.by_kind.get(k).copied().unwrap_or(0.0)).sum::<f64>())));
    lines = lines.push(ui::hairline());
    lines = lines.push(row(foot).spacing(8).align_y(Alignment::Center));
    lines.into()
}

fn balance_value(p: &Bucket, sort: BalanceSort) -> f64 {
    match sort {
        BalanceSort::Time => p.at,
        BalanceSort::Budget => p.budget,
        BalanceSort::Circulating => p.circulating,
        BalanceSort::Treasury => p.treasury,
    }
}

fn flow_value(p: &Bucket, sort: FlowSort, kinds: &[String]) -> f64 {
    match sort {
        FlowSort::Time => p.at,
        FlowSort::Entries => p.entries,
        FlowSort::Kind(i) => kinds.get(i).and_then(|k| p.by_kind.get(k)).copied().unwrap_or(0.0),
    }
}

/// One series row, in the order of `SERIES_COLUMNS`.
fn series_cells<'a>(p: &Bucket, label: String) -> Vec<Element<'a, Msg>> {
    let figure = |x: f64| cell(2, true, ui::mono(money(x), 12.0, crate::theme::palette().ink));
    vec![
        cell(3, false, ui::mono(label, 11.5, crate::theme::palette().ink_2)),
        figure(p.minted),
        figure(p.escrowed),
        figure(p.paid),
        figure(p.refunded),
        figure(p.fined),
        figure(p.spent),
        figure(p.entries),
        figure(p.budget),
        figure(p.circulating),
        figure(p.treasury),
    ]
}

fn series_value(p: &Bucket, sort: SeriesSort) -> f64 {
    match sort {
        SeriesSort::Time => p.at,
        SeriesSort::Minted => p.minted,
        SeriesSort::Escrowed => p.escrowed,
        SeriesSort::Paid => p.paid,
        SeriesSort::Refunded => p.refunded,
        SeriesSort::Fined => p.fined,
        SeriesSort::Spent => p.spent,
        SeriesSort::Entries => p.entries,
        SeriesSort::Budget => p.budget,
        SeriesSort::Circulating => p.circulating,
        SeriesSort::Treasury => p.treasury,
    }
}

/// The sum of the flows over `points`, with the balances at the last point's end.
fn totals(points: &[Bucket]) -> Bucket {
    let mut t = Bucket::default();
    for p in points {
        t.entries += p.entries;
        t.minted += p.minted;
        t.escrowed += p.escrowed;
        t.paid += p.paid;
        t.refunded += p.refunded;
        t.fined += p.fined;
        t.spent += p.spent;
    }
    if let Some(last) = points.last() {
        t.budget = last.budget;
        t.circulating = last.circulating;
        t.treasury = last.treasury;
    }
    t
}

fn wallets_card<'a>(d: &Data, state: &State) -> Element<'a, Msg> {
    if d.wallets.is_empty() {
        return ui::card("Wallets", ui::empty("user", "No wallets yet.", ""));
    }
    let total = d.wallets_total();
    let mut wallets: Vec<&Wallet> = d.wallets.iter().collect();
    wallets.sort_by(|a, b| {
        let o = match state.wallet_sort {
            WalletSort::Name => a.name.cmp(&b.name),
            WalletSort::Role => role_of(d, a.cid).cmp(&role_of(d, b.cid)),
            WalletSort::Earned => a.earned.unwrap_or(0.0).total_cmp(&b.earned.unwrap_or(0.0)),
            WalletSort::Spent => a.spent.unwrap_or(0.0).total_cmp(&b.spent.unwrap_or(0.0)),
            _ => a.balance.total_cmp(&b.balance),
        };
        if state.wallet_asc {
            o
        } else {
            o.reverse()
        }
    });
    let heads: Vec<Element<'a, Msg>> = vec![
        sort_head("Agent", 3, WalletSort::Name, state.wallet_sort, state.wallet_asc, Msg::WalletSort),
        sort_head("Role", 2, WalletSort::Role, state.wallet_sort, state.wallet_asc, Msg::WalletSort),
        sort_head("Balance", 2, WalletSort::Balance, state.wallet_sort, state.wallet_asc, Msg::WalletSort),
        sort_head("Earned", 2, WalletSort::Earned, state.wallet_sort, state.wallet_asc, Msg::WalletSort),
        sort_head("Spent", 2, WalletSort::Spent, state.wallet_sort, state.wallet_asc, Msg::WalletSort),
        sort_head("Share", 3, WalletSort::Share, state.wallet_sort, state.wallet_asc, Msg::WalletSort),
    ];
    let mut lines = column![row(heads).spacing(8)].spacing(8);
    for w in wallets {
        let role = d.roster.iter().find(|m| m.cid == w.cid).map_or_else(|| "—".to_string(), |m| m.role_label.clone());
        let name = button(iced::widget::text(w.name.clone()).font(theme::SERIF).size(14.0).color(crate::theme::palette().ink))
            .padding(0.0)
            .style(link)
            .on_press(Msg::Go(Id::Agent(w.cid)));
        let share = if total > 0.0 { w.balance / total } else { 0.0 };
        lines = lines.push(ui::hairline());
        lines = lines.push(
            row![
                cell(3, false, name),
                cell(2, false, iced::widget::text(role).font(theme::SERIF).size(14.0).color(crate::theme::palette().ink_2)),
                cell(2, true, ui::mono(money(w.balance), 12.5, crate::theme::palette().ink)),
                cell(2, true, ui::mono(w.earned.map_or_else(|| "—".to_string(), money), 12.5, crate::theme::palette().ink)),
                cell(2, true, ui::mono(w.spent.map_or_else(|| "—".to_string(), money), 12.5, crate::theme::palette().ink)),
                cell(3, false, row![share_bar(share), ui::mono(format!("{:.1}%", share * 100.0), 11.0, crate::theme::palette().ink_2)].spacing(10).align_y(Alignment::Center)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    // The totals row, as the web's: balance, earned and spent summed (a wallet with no profile counts 0).
    let sum = |f: &dyn Fn(&Wallet) -> f64| d.wallets.iter().map(f).sum::<f64>();
    let bold = |v: String| ui::mono(v, 12.5, crate::theme::palette().ink);
    lines = lines.push(ui::hairline());
    lines = lines.push(
        row![
            cell(3, false, iced::widget::text("Total").font(theme::SERIF).size(14.0).color(crate::theme::palette().ink)),
            cell(2, false, Space::new()),
            cell(2, true, bold(money(sum(&|w| w.balance)))),
            cell(2, true, bold(money(sum(&|w| w.earned.unwrap_or(0.0))))),
            cell(2, true, bold(money(sum(&|w| w.spent.unwrap_or(0.0))))),
            cell(3, false, Space::new()),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    );
    ui::card("Wallets", lines.into())
}

/// The role label of the agent `cid` on the roster, or empty when it is not there.
fn role_of(d: &Data, cid: u64) -> String {
    d.roster.iter().find(|m| m.cid == cid).map_or_else(String::new, |m| m.role_label.clone())
}

/// A bar of `share` (0 to 1) of the row's width.
fn share_bar<'a>(share: f64) -> Element<'a, Msg> {
    let filled = (share.clamp(0.0, 1.0) * 100.0).round() as u16;
    let segment = |portion: u16, color: Color| -> Element<'a, Msg> {
        container(Space::new().width(Length::Fill).height(6.0))
            .width(Length::FillPortion(portion))
            .height(Length::Fixed(6.0))
            .style(move |_: &Theme| container::Style { background: Some(color.into()), ..Default::default() })
            .into()
    };
    let bar: Element<'a, Msg> = match filled {
        0 => segment(100, crate::theme::palette().rule_faint),
        100 => segment(100, crate::theme::palette().ok),
        f => row![segment(f, crate::theme::palette().ok), segment(100 - f, crate::theme::palette().rule_faint)].into(),
    };
    container(bar).width(Length::Fill).into()
}

/// The journal: filters, newest first, paged by "Load older", new rows fading in.
fn journal_card<'a>(d: Option<&Data>, state: &State, now: Instant) -> Element<'a, Msg> {
    let kinds = kind_options(d, &state.journal);
    let selected_kind = kinds.iter().find(|o| o.kind == state.filter.kind).cloned();
    let kind_pick = pick_list(kinds, selected_kind, Msg::KindPicked);
    let wallets = wallet_options(d);
    let selected_wallet = wallets.iter().find(|o| o.cid == state.filter.wallet).cloned();
    let wallet_pick = pick_list(wallets, selected_wallet, Msg::WalletPicked);
    let task = text_input("Task id", &state.task_draft).on_input(Msg::TaskDraft).on_submit(Msg::TaskApply).style(theme::input).width(Length::Fill);
    let filters = row![kind_pick, wallet_pick, task].spacing(12).align_y(Alignment::Center);

    let body: Element<'a, Msg> = if state.journal.is_empty() {
        ui::empty::<Msg>("clock", "No entries.", "")
    } else {
        let mut ledger = column![].width(Length::Fill);
        let last = state.journal.len() - 1;
        for (i, e) in state.journal.iter().enumerate() {
            ledger = ledger.push(entry_row(e, fade(&state.fresh, e.id, now)));
            if i < last {
                ledger = ledger.push(container(Space::new().height(1.0)).width(Length::Fill).style(|_: &Theme| container::Style {
                    background: Some(crate::theme::palette().rule_faint.into()),
                    ..Default::default()
                }));
            }
        }
        ledger.into()
    };
    let footer = row![
        ui::faint(format!("Newest first · {} of {} shown", state.journal.len(), state.journal_total)),
        Space::new().width(Length::Fill),
        ui::btn("Load older", None, ui::Kind::Secondary, (state.journal_next.is_some() && !state.older_busy).then_some(Msg::OlderPressed)),
    ]
    .align_y(Alignment::Center);
    ui::card("Journal", column![filters, body, footer].spacing(GAP).width(Length::Fill).into())
}

/// How far a fresh row has faded in (0 at its arrival, 1 once settled).
fn fade(fresh: &[(i64, Instant)], id: i64, now: Instant) -> f32 {
    fresh.iter().find(|(i, _)| *i == id).map_or(1.0, |(_, at)| chart::ease(progress(*at, now)) as f32)
}

/// One journal row: the id, the time, the kind's badge, the memo (cut with an ellipsis to the room
/// the fixed columns leave), and the amount, right-aligned.
fn entry_row<'a>(e: &Entry, alpha: f32) -> Element<'a, Msg> {
    let memo = if e.memo.is_empty() {
        match &e.task {
            Some(task) => format!("{} for {task}", e.kind),
            None => e.kind.clone(),
        }
    } else {
        e.memo.clone()
    };
    let (id, when, kind, amount, tone) = (format!("#{}", e.id), ago(e.at), kind_label(&e.kind), money(e.amount), kind_tone(&e.kind));
    responsive(move |size| {
        let fade = |c: Color| Color { a: c.a * alpha, ..c };
        let room = (size.width - JOURNAL_FIXED).max(0.0);
        let memo = ellipsize(&memo, (room / SERIF15_CHAR) as usize);
        row![
            container(ui::mono(id.clone(), 11.5, fade(crate::theme::palette().ink_3))).width(Length::Fixed(56.0)).clip(true),
            container(ui::mono(when.clone(), 11.5, fade(crate::theme::palette().ink_3))).width(Length::Fixed(92.0)).clip(true),
            container(ui::badge::<Msg>(&kind, tone)).width(Length::Fixed(120.0)).clip(true),
            container(iced::widget::text(memo).font(theme::SERIF).size(15.0).color(fade(crate::theme::palette().ink)).wrapping(Wrapping::None))
                .width(Length::Fill)
                .clip(true),
            container(iced::widget::text(amount.clone()).font(AMOUNT).size(14.0).color(fade(crate::theme::palette().ink)).wrapping(Wrapping::None))
                .width(Length::Fixed(90.0))
                .align_right(Length::Fixed(90.0)),
        ]
        .spacing(12)
        .padding(Padding { top: 9.0, right: 0.0, bottom: 9.0, left: 0.0 })
        .align_y(Alignment::Center)
        .into()
    })
    .into()
}

/// `text` cut to `max` characters, the last of them an ellipsis when it had to be cut.
fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let kept: String = text.chars().take(max - 1).collect();
    format!("{}…", kept.trim_end())
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

/// A sortable header cell: the active column in ink, with its direction.
fn sort_head<'a, T: Copy + PartialEq>(label: &str, portion: u16, value: T, current: T, asc: bool, make: fn(T) -> Msg) -> Element<'a, Msg> {
    let active = value == current;
    let arrow = if active {
        if asc {
            " ^"
        } else {
            " v"
        }
    } else {
        ""
    };
    let content = button(iced::widget::text(format!("{}{arrow}", label.to_uppercase())).font(MONO).size(11.0).color(if active { crate::theme::palette().ink } else { crate::theme::palette().ink_3 }))
        .style(link)
        .padding(0)
        .on_press(make(value));
    cell(portion, false, content)
}

/// Picks a column for a sort: the same column flips its direction; a new one starts in `asc_first`.
fn resort<T: Copy + PartialEq>(current: &mut T, asc: &mut bool, next: T, asc_first: bool) {
    if *current == next {
        *asc = !*asc;
    } else {
        *current = next;
        *asc = asc_first;
    }
}

/// A segmented control: the current option solid, the others plain.
fn segmented<'a, T: Copy + PartialEq + 'a>(options: &[(T, &str)], current: T, make: fn(T) -> Msg) -> Element<'a, Msg> {
    let mut line = row![].spacing(6);
    for (value, label) in options {
        let kind = if *value == current { ui::Kind::Primary } else { ui::Kind::Secondary };
        line = line.push(ui::btn(label, None, kind, Some(make(*value))));
    }
    line.into()
}

/// A plain link-looking button for an agent's name.
fn link(_: &Theme, _: button::Status) -> button::Style {
    button::Style { text_color: crate::theme::palette().ink, ..Default::default() }
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

/// The journal's kinds: all kinds first, then the known and seen ones, each once.
fn kind_options(d: Option<&Data>, journal: &[Entry]) -> Vec<KindChoice> {
    let mut kinds: Vec<String> = KNOWN_KINDS.iter().map(|k| k.to_string()).collect();
    kinds.extend(journal.iter().map(|e| e.kind.clone()));
    if let Some(d) = d {
        kinds.extend(d.points.iter().flat_map(|p| p.by_kind.keys().cloned()));
    }
    sort_kinds(&mut kinds);
    let mut options = vec![KindChoice { kind: None, label: "All kinds".to_string() }];
    options.extend(kinds.into_iter().map(|k| KindChoice { label: kind_label(&k), kind: Some(k) }));
    options
}

/// The journal's agents: every agent, by name, after "All agents".
fn wallet_options(d: Option<&Data>) -> Vec<WalletChoice> {
    let mut options = vec![WalletChoice { cid: None, label: "All agents".to_string() }];
    if let Some(d) = d {
        let mut roster: Vec<&Member> = d.roster.iter().collect();
        roster.sort_by(|a, b| a.name.cmp(&b.name));
        options.extend(roster.into_iter().map(|m| WalletChoice { cid: Some(m.cid), label: m.name.clone() }));
    }
    options
}

/// A whole amount above zero, if the text is one.
fn whole(text: &str) -> Option<i64> {
    text.trim().parse::<i64>().ok().filter(|n| *n > 0)
}

fn kind_tone(kind: &str) -> ui::Tone {
    match kind {
        "mint" => ui::Tone::Ok,
        "spend" | "model" | "fee" => ui::Tone::Warn,
        "payment" | "subcontract" => ui::Tone::Signal,
        "refund" | "transfer" => ui::Tone::Info,
        "fine" | "clawback" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

/// The colour a kind's flow takes in the charts (the same families as its badge).
fn kind_color(kind: &str) -> Color {
    match kind {
        "mint" => crate::theme::palette().ok,
        "spend" => crate::theme::palette().warn,
        "payment" => crate::theme::palette().signal,
        "refund" | "transfer" => BLUE,
        "fine" | "clawback" => crate::theme::palette().crit,
        "fee" => Color::from_rgb8(0xe8, 0xb8, 0x4a),
        "budget_move" => crate::theme::palette().ink_2,
        "subcontract" => Color::from_rgb8(0x6f, 0xc8, 0xb0),
        "escrow" => PURPLE,
        _ => crate::theme::palette().ink_3,
    }
}

/// The budget's balances over the buckets: the plaza budget, the circulating money and the treasury.
fn balance_series(points: &[Bucket]) -> Vec<chart::Series> {
    let pick = |key: &str, color: Color, f: fn(&Bucket) -> f64| chart::Series { key: key.to_string(), color, values: points.iter().map(f).collect() };
    vec![
        pick("Budget", crate::theme::palette().ok, |p| p.budget),
        pick("Circulating", BLUE, |p| p.circulating),
        pick("Treasury", crate::theme::palette().warn, |p| p.treasury),
    ]
}

/// The flows per bucket, one series per kind that moved money in the range, sorted by kind. A kind
/// that is zero across the whole range is left out, so the legend lists only what the chart shows.
fn flow_series(points: &[Bucket]) -> Vec<chart::Series> {
    flow_kinds(points)
        .into_iter()
        .map(|kind| {
            let values: Vec<f64> = points.iter().map(|p| p.by_kind.get(&kind).copied().unwrap_or(0.0)).collect();
            chart::Series { color: kind_color(&kind), key: kind_label(&kind), values }
        })
        .collect()
}

/// Kinds in the web's order (`KNOWN_KINDS`, as treasury.ts `KINDS`), then any other kind by name; duplicates dropped.
fn sort_kinds(kinds: &mut Vec<String>) {
    let rank = |k: &str| KNOWN_KINDS.iter().position(|known| *known == k).unwrap_or(KNOWN_KINDS.len());
    kinds.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    kinds.dedup();
}

/// The kinds that moved money in the range, in the web's kind order.
fn flow_kinds(points: &[Bucket]) -> Vec<String> {
    let mut kinds: Vec<String> = points.iter().flat_map(|p| p.by_kind.keys().cloned()).collect();
    sort_kinds(&mut kinds);
    kinds.retain(|kind| points.iter().any(|p| p.by_kind.get(kind).is_some_and(|v| *v != 0.0)));
    kinds
}

/// A kind's id as a reader sees it: "budget_move" is "Budget move", "subcontract" is "Subcontract".
fn kind_label(kind: &str) -> String {
    if kind == "model" {
        return "Model call".to_string();
    }
    let spaced = kind.replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
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

/// A chart or table label for a bucket that starts at `ms` (UTC): "14 Nov 22:00" hourly, else "14 Nov".
fn point_label(ms: i64, hourly: bool) -> String {
    let (_, m, d, h) = civil(ms);
    let month = MONTHS[(m - 1) as usize];
    if hourly {
        format!("{d} {month} {h:02}:00")
    } else {
        format!("{d} {month}")
    }
}

/// UTC calendar fields of epoch milliseconds: (year, month 1-12, day, hour).
fn civil(ms: i64) -> (i64, u32, u32, u32) {
    let days = ms.div_euclid(DAY_MS as i64);
    let hour = (ms.rem_euclid(DAY_MS as i64) / HOUR_MS as i64) as u32;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, hour)
}

/// The rows of a page size needed for `len` rows (at least one page).
fn page_count(len: usize, size: usize) -> usize {
    len.div_ceil(size).max(1)
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

/// Percent-encodes a query value.
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Eases from 0 to 1 over the animation's length, from `start`.
fn progress(start: Instant, now: Instant) -> f64 {
    (now.saturating_duration_since(start).as_secs_f64() / ANIM.as_secs_f64()).min(1.0)
}

/// The journal rows: `head` (the newest page) over the older rows already loaded, newest first.
fn merge_head(existing: &[Entry], head: Vec<Entry>) -> Vec<Entry> {
    let Some(oldest) = head.iter().map(|e| e.id).min() else { return head };
    let mut out = head;
    out.extend(existing.iter().filter(|e| e.id < oldest).cloned());
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

/// A figure that counts from its old value to a new one.
#[derive(Debug, Default, Clone, Copy)]
struct Tween {
    from: f64,
    to: f64,
    start: Option<Instant>,
}

impl Tween {
    fn at(&self, now: Instant) -> f64 {
        match self.start {
            Some(s) => self.from + (self.to - self.from) * chart::ease(progress(s, now)),
            None => self.to,
        }
    }

    fn aim(&mut self, to: f64, now: Instant) {
        if to == self.to {
            return;
        }
        self.from = self.at(now);
        self.to = to;
        self.start = Some(now);
    }
}

#[derive(Debug, Default)]
struct Counters {
    budget: Tween,
    circulating: Tween,
    paid: Tween,
    usd: Tween,
    treasury: Tween,
}

/// A chart's series, easing from the frame on screen to a new target.
#[derive(Debug, Default)]
struct Animated {
    from: Vec<chart::Series>,
    to: Vec<chart::Series>,
    start: Option<Instant>,
}

impl Animated {
    fn current(&self, now: Instant) -> Vec<chart::Series> {
        match self.start {
            Some(s) => chart::blend(&self.from, &self.to, chart::ease(progress(s, now))),
            None => self.to.clone(),
        }
    }

    fn retarget(&mut self, next: Vec<chart::Series>, now: Instant) {
        if next == self.to {
            return;
        }
        self.from = self.current(now);
        self.to = next;
        self.start = Some(now);
    }
}

/// The data the screen shows, read from the hub's JSON.
#[derive(Debug, Clone)]
pub struct Data {
    budget: f64,
    escrow: f64,
    treasury: f64,
    wallets: Vec<Wallet>,
    roster: Vec<Member>,
    points: Vec<Bucket>,
    /// The real model cost over the range, in dollars.
    cost: f64,
}

/// What one poll reads: the data, the filter it was read under, and the journal's newest page.
#[derive(Debug, Clone)]
pub struct Snapshot {
    data: Data,
    filter: JournalFilter,
    page: JournalPage,
}

/// One page of the journal, newest first.
#[derive(Debug, Clone)]
pub struct JournalPage {
    entries: Vec<Entry>,
    total: u64,
    next_before: Option<i64>,
}

#[derive(Debug, Clone, Default)]
struct Bucket {
    at: f64,
    minted: f64,
    escrowed: f64,
    paid: f64,
    refunded: f64,
    fined: f64,
    spent: f64,
    entries: f64,
    budget: f64,
    circulating: f64,
    treasury: f64,
    by_kind: BTreeMap<String, f64>,
}

#[derive(Debug, Clone)]
struct Entry {
    id: i64,
    kind: String,
    at: f64,
    amount: f64,
    memo: String,
    task: Option<String>,
}

#[derive(Debug, Clone)]
struct Wallet {
    cid: u64,
    name: String,
    balance: f64,
    earned: Option<f64>,
    spent: Option<f64>,
}

#[derive(Debug, Clone)]
struct Member {
    cid: u64,
    name: String,
    role_label: String,
}

impl Data {
    fn parse(treasury: &Value, wallets: &Value, roster: &Value, series: &Value, usage: &Value) -> Data {
        let accounts = list(treasury, "accounts");
        let balance = |kind: &str| accounts.iter().filter(|a| text(a, "kind") == kind).map(|a| num(a, "balance")).sum::<f64>();
        let totals = usage.get("totals").cloned().unwrap_or(Value::Null);
        Data {
            budget: num(treasury, "plaza_budget"),
            escrow: balance("task_escrow"),
            treasury: balance("treasury"),
            wallets: list(wallets, "wallets")
                .iter()
                .map(|w| Wallet { cid: uint(w, "cid"), name: text(w, "name"), balance: num(w, "balance"), earned: None, spent: None })
                .collect(),
            roster: list(roster, "agents")
                .iter()
                .map(|a| Member { cid: uint(a, "cid"), name: text(a, "name"), role_label: text(a, "role_label") })
                .collect(),
            points: list(series, "points").iter().map(Bucket::parse).collect(),
            cost: num(&totals, "usd"),
        }
    }

    fn paid(&self) -> f64 {
        self.points.iter().map(|p| p.paid).sum()
    }

    /// The range's spend and escrow: the series summed, as the flows' legend sums them.
    fn spent(&self) -> f64 {
        self.points.iter().map(|p| p.spent).sum()
    }

    fn escrowed(&self) -> f64 {
        self.points.iter().map(|p| p.escrowed).sum()
    }

    fn refunded(&self) -> f64 {
        self.points.iter().map(|p| p.refunded).sum()
    }

    fn fined(&self) -> f64 {
        self.points.iter().map(|p| p.fined).sum()
    }

    fn wallets_total(&self) -> f64 {
        self.wallets.iter().map(|w| w.balance).sum()
    }

    /// The money in circulation at the end of the range (the wallets, when there is no series).
    fn circulating(&self) -> f64 {
        self.points.last().map_or_else(|| self.wallets_total(), |p| p.circulating)
    }

    fn treasury_now(&self) -> f64 {
        self.points.last().map_or(self.treasury, |p| p.treasury)
    }
}

impl Bucket {
    fn parse(v: &Value) -> Bucket {
        Bucket {
            at: num(v, "at_ms"),
            minted: num(v, "minted"),
            escrowed: num(v, "escrowed"),
            paid: num(v, "paid"),
            refunded: num(v, "refunded"),
            fined: num(v, "fined"),
            spent: num(v, "spent"),
            entries: num(v, "entries"),
            budget: num(v, "plaza_budget_end"),
            circulating: num(v, "circulating_end"),
            treasury: num(v, "treasury_end"),
            by_kind: v
                .get("by_kind")
                .and_then(Value::as_object)
                .map(|m| m.iter().map(|(k, x)| (k.clone(), x.as_f64().unwrap_or(0.0))).collect())
                .unwrap_or_default(),
        }
    }
}

impl JournalPage {
    fn parse(v: &Value) -> JournalPage {
        JournalPage {
            entries: list(v, "entries").iter().map(Entry::parse).collect(),
            total: uint(v, "total"),
            next_before: v.get("next_before").and_then(Value::as_i64),
        }
    }
}

impl Entry {
    /// The hub's entry: `created_at` (or `at`), its net `amount` (or, when absent, the debit legs of
    /// the postings: a balanced entry's size), and its task under `task_id` (or `task`).
    fn parse(v: &Value) -> Entry {
        let legs: f64 = v
            .get("postings")
            .and_then(Value::as_array)
            .map(|ps| ps.iter().map(|p| num(p, "amount")).filter(|a| *a > 0.0).sum())
            .unwrap_or(0.0);
        let created = num(v, "created_at");
        Entry {
            id: v.get("id").and_then(Value::as_i64).unwrap_or(0),
            kind: text(v, "kind"),
            at: if created > 0.0 { created } else { num(v, "at") },
            amount: if v.get("amount").is_some() { num(v, "amount") } else { legs },
            memo: text(v, "memo"),
            task: v.get("task_id").or_else(|| v.get("task")).and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: i64, kind: &str) -> Entry {
        Entry { id, kind: kind.to_string(), at: 0.0, amount: 0.0, memo: String::new(), task: None }
    }

    fn ids(v: &[Entry]) -> Vec<i64> {
        v.iter().map(|e| e.id).collect()
    }

    #[test]
    fn money_groups_thousands() {
        assert_eq!(money(1_234_567.0), "1,234,567");
        assert_eq!(money(-1500.0), "-1,500");
        assert_eq!(money(0.4), "0");
    }

    #[test]
    fn whole_needs_a_positive_integer() {
        assert_eq!(whole(" 12 "), Some(12));
        assert_eq!(whole("0"), None);
        assert_eq!(whole("-3"), None);
        assert_eq!(whole("1.5"), None);
    }

    #[test]
    fn enc_escapes_reserved_bytes() {
        assert_eq!(enc("a b&c"), "a%20b%26c");
        assert_eq!(enc("ok-_.~"), "ok-_.~");
    }

    #[test]
    fn journal_path_carries_the_filters_and_the_cursor() {
        let f = JournalFilter { kind: Some("spend".into()), wallet: Some(7), task: "fix bug".into() };
        assert_eq!(journal_path(&f, None), "/v1/treasury/journal?limit=50&kind=spend&wallet=7&task=fix%20bug");
        assert_eq!(journal_path(&JournalFilter::default(), Some(42)), "/v1/treasury/journal?limit=50&before=42");
    }

    #[test]
    fn civil_dates_are_utc() {
        assert_eq!(civil(0), (1970, 1, 1, 0));
        assert_eq!(civil(1_700_000_000_000), (2023, 11, 14, 22));
        assert_eq!(point_label(1_700_000_000_000, true), "14 Nov 22:00");
        assert_eq!(point_label(1_700_000_000_000, false), "14 Nov");
    }

    #[test]
    fn a_head_page_merges_over_the_older_rows_already_loaded() {
        let merged = merge_head(&[entry(8, "spend"), entry(7, "fine")], vec![entry(9, "mint"), entry(8, "spend")]);
        assert_eq!(ids(&merged), vec![9, 8, 7]);
        assert!(ids(&merge_head(&[entry(3, "mint")], vec![])).is_empty());
    }

    #[test]
    fn resort_flips_the_same_column_and_starts_a_new_one_in_its_own_order() {
        let (mut cur, mut asc) = (SeriesSort::Time, false);
        resort(&mut cur, &mut asc, SeriesSort::Time, false);
        assert!(asc);
        resort(&mut cur, &mut asc, SeriesSort::Paid, false);
        assert_eq!((cur, asc), (SeriesSort::Paid, false));
    }

    #[test]
    fn pages_hold_at_least_one() {
        assert_eq!(page_count(0, 25), 1);
        assert_eq!(page_count(25, 25), 1);
        assert_eq!(page_count(26, 25), 2);
    }

    #[test]
    fn bar_at_picks_the_column_under_the_cursor() {
        assert_eq!(bar_at(10.0, 4, 10.0, 40.0), Some(0));
        assert_eq!(bar_at(35.0, 4, 10.0, 40.0), Some(2));
        assert_eq!(bar_at(50.0, 4, 10.0, 40.0), Some(3));
        assert_eq!(bar_at(9.0, 4, 10.0, 40.0), None);
        assert_eq!(bar_at(51.0, 4, 10.0, 40.0), None);
    }

    #[test]
    fn a_tween_counts_up_and_settles() {
        let t0 = Instant::now();
        let mut t = Tween::default();
        t.aim(100.0, t0);
        assert!(t.at(t0) < 1.0);
        assert_eq!(t.at(t0 + ANIM), 100.0);
    }

    #[test]
    fn credits_are_a_tenth_of_a_cent() {
        assert_eq!(money(0.5 / CREDIT_USD), "500");
    }

    #[test]
    fn kind_choices_list_every_kind_once_after_all_kinds() {
        let k = kind_options(None, &[entry(1, "zeta"), entry(2, "mint")]);
        assert_eq!(k[0].label, "All kinds");
        assert!(k.iter().any(|c| c.kind.as_deref() == Some("zeta")));
        assert_eq!(k.iter().filter(|c| c.kind.as_deref() == Some("mint")).count(), 1);
    }

    #[test]
    fn journal_parses_its_page_and_sums_debit_legs() {
        let v = json!({
            "entries": [{"id": 3, "kind": "mint", "at": 1.0, "postings": [{"amount": 5}, {"amount": -5}], "memo": "", "task": null}],
            "total": 9, "limit": 50, "truncated": true, "next_before": 3
        });
        let page = JournalPage::parse(&v);
        assert_eq!((page.total, page.next_before), (9, Some(3)));
        assert_eq!(page.entries[0].amount, 5.0);
    }

    #[test]
    fn the_series_parses_into_stacked_flows_and_balances() {
        let series = json!({"points": [
            {"at_ms": 1000, "entries": 2, "minted": 5, "escrowed": 0, "paid": 1, "refunded": 0, "fined": 0, "spent": 3,
             "by_kind": {"mint": 5, "spend": 3}, "plaza_budget_end": 10, "circulating_end": 4, "treasury_end": 2},
            {"at_ms": 2000, "entries": 1, "minted": 0, "escrowed": 0, "paid": 2, "refunded": 0, "fined": 1, "spent": 0,
             "by_kind": {"payment": 2, "fine": 1}, "plaza_budget_end": 8, "circulating_end": 6, "treasury_end": 3}
        ]});
        let points: Vec<Bucket> = list(&series, "points").iter().map(Bucket::parse).collect();
        let flows = flow_series(&points);
        let keys: Vec<&str> = flows.iter().map(|s| s.key.as_str()).collect();
        // The web's kind order (treasury.ts KINDS), not alphabetical.
        assert_eq!(keys, vec!["Mint", "Payment", "Fine", "Spend"]);
        assert_eq!(flows[0].values, vec![5.0, 0.0]);
        assert_eq!(balance_series(&points)[0].values, vec![10.0, 8.0]);
        let all = totals(&points);
        assert_eq!((all.minted, all.paid, all.budget, all.circulating), (5.0, 3.0, 8.0, 6.0));
    }
}
