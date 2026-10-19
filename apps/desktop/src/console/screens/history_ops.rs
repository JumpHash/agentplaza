//! The repository tabs Queue, Keys, Sync and Ops of the History screen, as the web's history view draws them (its
//! `queueBody`, `keysBody`, `syncBody` and `opsBody`): the landing line, the signing keys with the verify panel under
//! them, the connections to outside git with the export, its dry run and the adoption of a moved remote in a dialog,
//! and the operations log with its filter. Map, Files and Trace are not here.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{Space, column, container, pick_list, row, text_input};
use iced::{Alignment, Border, Color, Element, Length, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::history::{Connection, RepoRow, Tab};
use crate::console::Ctx;
use crate::console::client::{self, Client, enc};
use crate::console::ui::{self, Kind, Tone};
use crate::theme;

// ---- Data (the hub's answers) -------------------------------------------------------------------------------------

/// `GET /v1/weft/repos/{repo}/queue`: the changes in line for main, in order, and the head that moves main.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Queue {
    pub head: String,
    pub queue: Vec<Line>,
}

/// One change in the landing line.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Line {
    pub change: String,
    pub title: String,
    pub task: Option<String>,
    pub author: String,
    pub state: String,
    pub at_ms: u64,
}

/// `GET /v1/weft/keys`: the hub's signing keys, the owner's, the hub's and the agents'.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Keys {
    pub keys: Vec<Key>,
}

/// One signing key: its id and public key, who holds it, what it may sign, and when it was revoked.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Key {
    pub id: String,
    pub key: String,
    pub holder: Holder,
    pub caps: Vec<String>,
    pub issued_seq: u64,
    pub revoked_seq: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Holder {
    pub actor: Actor,
    pub name: Option<String>,
}

/// Who signed or did a thing: the owner, the hub, or an agent by its cid.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Actor {
    pub kind: String,
    pub cid: Option<u64>,
}

/// `GET /v1/weft/ops`: the operations log (the screen shows it newest first).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct OpsList {
    ops: Vec<Op>,
}

/// One operation of the hub's log.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Op {
    pub at_ms: u64,
    pub actor: Actor,
    pub kind: String,
    pub summary: String,
}

/// `GET /v1/weft/verify?subject=`: the signature check of a change, a revision or a release.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Verify {
    pub ok: bool,
    pub subject: String,
    pub records: Vec<Signed>,
}

/// One signed record the check walked, with the chain of signatures up to the owner's key.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Signed {
    pub seq: u64,
    pub kind: String,
    pub actor: String,
    pub key: String,
    pub ok: bool,
    pub via_console: bool,
    pub chain: Vec<String>,
}

/// `POST /v1/weft/repos/{repo}/export`: what a dry run would push, or what an export pushed.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Report {
    pub remote: String,
    pub changes: Vec<Pushed>,
}

/// One change a export carries, with its commits (one per intent).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Pushed {
    pub change: String,
    pub commits: Vec<Commit>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Commit {
    pub intent: String,
    pub git: String,
    pub files: u64,
}

/// An agent's name by its cid (`GET /v1/agents`).
#[derive(Deserialize)]
struct AgentName {
    cid: u64,
    #[serde(default)]
    name: String,
}

/// What a load fetched for the tab it answers.
#[derive(Debug, Clone, Default)]
pub struct Data {
    queue: Queue,
    keys: Keys,
    ops: Vec<Op>,
    /// The agents' names by cid; an agent without a name shows as `agent N`.
    names: BTreeMap<u64, String>,
}

/// Fetches what one of the four tabs shows (the others fetch nothing here).
pub fn fetch(client: &Client, repo: &str, tab: Tab) -> Result<Data, String> {
    let mut data = Data::default();
    match tab {
        Tab::Queue => {
            data.queue = client.get(&format!("/v1/weft/repos/{}/queue", enc(repo)))?;
        }
        Tab::Keys => {
            data.keys = client.get("/v1/weft/keys")?;
            data.names = names(client);
        }
        Tab::Ops => {
            data.ops = client.get::<OpsList>("/v1/weft/ops?limit=200")?.ops;
            data.names = names(client);
        }
        Tab::Changes | Tab::Releases | Tab::Sync => {}
    }
    Ok(data)
}

/// The agents' names: a missing list leaves the names out, and each agent shows as `agent N` until it has one.
fn names(client: &Client) -> BTreeMap<u64, String> {
    client
        .get::<Vec<AgentName>>("/v1/agents")
        .unwrap_or_default()
        .into_iter()
        .filter(|a| !a.name.trim().is_empty())
        .map(|a| (a.cid, a.name))
        .collect()
}

// ---- State and messages -------------------------------------------------------------------------------------------

/// The tabs' own state: the verify draft and its answer, the operations filter, the connection picked and its dry
/// run, the open dialog, and the note of the last action.
#[derive(Debug)]
pub struct State {
    pub error: Option<String>,
    pub notice: Option<String>,
    verify_draft: String,
    verify: Option<Verify>,
    /// The operations group shown (`all` for every one).
    filter: &'static str,
    /// The connection picked in Sync; the first connection while none is picked.
    conn: String,
    dry: Option<Dry>,
    adopt: Option<Adopt>,
}

impl Default for State {
    fn default() -> Self {
        State {
            error: None,
            notice: None,
            verify_draft: String::new(),
            verify: None,
            filter: "all",
            conn: String::new(),
            dry: None,
            adopt: None,
        }
    }
}

impl State {
    /// Drops the note of the last action (a tab was left).
    pub fn clear_notes(&mut self) {
        self.error = None;
        self.notice = None;
    }
}

/// A dry run's answer, for the repository and connection it was run for.
#[derive(Debug)]
struct Dry {
    repo: String,
    conn: String,
    report: Report,
}

/// The adoption a dialog asks the owner to confirm: the connection, and the dialog's title.
#[derive(Debug)]
struct Adopt {
    conn: String,
    title: String,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Draft(String),
    Verify,
    Verified(Result<Verify, String>),
    Filter(&'static str),
    Pick(String),
    DryRun,
    DryRunOf(String),
    Dried(String, String, Result<Report, String>),
    Export,
    OpenAdopt(String),
    CloseAdopt,
    Adopt,
    /// An action's answer: its note on success, its error otherwise. A success reloads the screen.
    Done(Result<String, String>),
}

/// The connection the Sync tab shows as picked: the one chosen, or the first while none is (or it is gone).
fn picked(ui: &State, conns: &[Connection]) -> String {
    if conns.iter().any(|c| c.id == ui.conn) {
        ui.conn.clone()
    } else {
        conns.first().map(|c| c.id.clone()).unwrap_or_default()
    }
}

pub fn update(ui: &mut State, msg: Msg, ctx: &Ctx, repo: &str, conns: &[Connection]) -> Task<Msg> {
    match msg {
        Msg::Draft(text) => {
            ui.verify_draft = text;
            Task::none()
        }
        Msg::Verify => {
            let subject = ui.verify_draft.trim().to_string();
            if subject.is_empty() {
                ui.error = Some("Name what to verify: a change, a revision or a release.".into());
                return Task::none();
            }
            ui.error = None;
            let client = ctx.client.clone();
            let path = format!("/v1/weft/verify?subject={}", enc(&subject));
            client::task(move || client.get::<Verify>(&path), Msg::Verified)
        }
        Msg::Verified(res) => {
            match res {
                Ok(v) if v.ok => {
                    ui.error = None;
                    ui.notice = Some(format!("{} checks out", v.subject));
                    ui.verify = Some(v);
                }
                Ok(v) => {
                    ui.notice = None;
                    ui.error = Some(format!("{}: the signature check failed", v.subject));
                    ui.verify = Some(v);
                }
                Err(e) => {
                    ui.verify = None;
                    ui.error = Some(e);
                }
            }
            Task::none()
        }
        Msg::Filter(group) => {
            ui.filter = group;
            Task::none()
        }
        Msg::Pick(conn) => {
            ui.conn = conn;
            ui.dry = None;
            Task::none()
        }
        Msg::DryRun => {
            let conn = picked(ui, conns);
            if conn.is_empty() {
                ui.error = Some("Pick a connection first.".into());
                return Task::none();
            }
            ui.error = None;
            dry_run(ctx, repo, conn)
        }
        Msg::DryRunOf(conn) => {
            ui.conn = conn.clone();
            ui.error = None;
            dry_run(ctx, repo, conn)
        }
        Msg::Dried(repo, conn, res) => {
            match res {
                Ok(report) => {
                    ui.error = None;
                    ui.notice = Some(format!("Dry run for {conn}. Nothing was pushed."));
                    ui.dry = Some(Dry { repo, conn, report });
                }
                Err(e) => {
                    ui.dry = None;
                    ui.error = Some(e);
                }
            }
            Task::none()
        }
        Msg::Export => {
            let conn = picked(ui, conns);
            let ready = ui
                .dry
                .as_ref()
                .filter(|d| d.repo == repo && d.conn == conn)
                .map(|d| (d.report.changes.len(), d.report.remote.clone()));
            let Some((n, remote)) = ready else {
                ui.error = Some("Run a dry run first, so you see what would be pushed.".into());
                return Task::none();
            };
            ui.error = None;
            if n == 0 {
                ui.notice = Some(format!("Nothing to export: {remote} is in step with main."));
                return Task::none();
            }
            ui.dry = None;
            let client = ctx.client.clone();
            let path = format!("/v1/weft/repos/{}/export", enc(repo));
            let body = json!({ "connection": conn });
            let done = format!("Exported {} to {remote}", changes_word(n));
            client::task(
                move || client.post::<Value>(&path, &body),
                move |res| Msg::Done(res.map(|_| done.clone())),
            )
        }
        Msg::OpenAdopt(conn) => {
            ui.adopt = Some(Adopt {
                title: format!("Adopt {conn}?"),
                conn,
            });
            Task::none()
        }
        Msg::CloseAdopt => {
            ui.adopt = None;
            Task::none()
        }
        Msg::Adopt => {
            let Some(a) = ui.adopt.take() else {
                return Task::none();
            };
            ui.error = None;
            let client = ctx.client.clone();
            let path = format!("/v1/weft/repos/{}/adopt", enc(repo));
            let body = json!({ "connection": a.conn });
            let done = format!("Adopting the commits of {}", a.conn);
            client::task(
                move || client.post::<Value>(&path, &body),
                move |res| Msg::Done(res.map(|_| done.clone())),
            )
        }
        Msg::Done(res) => {
            match res {
                Ok(text) => {
                    ui.error = None;
                    ui.notice = Some(text);
                }
                Err(e) => {
                    ui.notice = None;
                    ui.error = Some(e);
                }
            }
            Task::none()
        }
    }
}

/// A dry run of the repository's export to a connection: what would be pushed, nothing written.
fn dry_run(ctx: &Ctx, repo: &str, conn: String) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = format!("/v1/weft/repos/{}/export", enc(repo));
    let body = json!({ "connection": conn, "dry_run": true });
    let tag = (repo.to_string(), conn);
    client::task(
        move || client.post::<Report>(&path, &body),
        move |res| Msg::Dried(tag.0.clone(), tag.1.clone(), res),
    )
}

/// The dialog that asks before a moved remote's own commits come into main, if one is open.
pub fn dialog(ui: &State) -> Option<Element<'_, Msg>> {
    let a = ui.adopt.as_ref()?;
    let p = theme::palette();
    let body = ui::prose(format!(
        "{} moved on its own. Adopting brings its commits into main as imported changes, with their authors and dates kept.",
        a.conn
    ))
    .size(13.0)
    .color(p.ink_2);
    Some(ui::modal(
        a.title.as_str(),
        body.into(),
        vec![
            ui::btn("Cancel", None, Kind::Ghost, Some(Msg::CloseAdopt)),
            ui::btn("Adopt", None, Kind::Primary, Some(Msg::Adopt)),
        ],
        Msg::CloseAdopt,
        440.0,
    ))
}

// ---- View ---------------------------------------------------------------------------------------------------------

/// The body of one of the four tabs: the notes of the last action, then the tab's panels.
pub fn body<'a>(ui: &'a State, tab: Tab, repo: &'a RepoRow, data: &'a Data) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &ui.error {
        items.push(ui::error_line(e));
    }
    if let Some(n) = &ui.notice {
        items.push(ui::faint(n.clone()).into());
    }
    items.push(match tab {
        Tab::Queue => queue_view(&data.queue),
        Tab::Keys => keys_view(ui, data),
        Tab::Sync => sync_view(ui, repo),
        Tab::Ops => ops_view(ui, data),
        Tab::Changes | Tab::Releases => Space::new().into(),
    });
    column(items).spacing(14).into()
}

/// A table as the web's `.tbl`: a head row in the label voice over a strong rule, then each row under a hairline.
/// Columns share the width in the given portions; a row may be tinted.
fn table<'a>(
    cols: &[(u16, &str)],
    rows: Vec<(Option<Color>, Vec<Element<'a, Msg>>)>,
) -> Element<'a, Msg> {
    let heads: Vec<Element<'a, Msg>> = cols.iter().map(|(_, t)| ui::label(t).into()).collect();
    let mut lines: Vec<Element<'a, Msg>> = vec![cells(cols, heads), strong_rule()];
    for (tint, items) in rows {
        let line = cells(cols, items);
        lines.push(match tint {
            Some(c) => container(line)
                .width(Length::Fill)
                .style(move |_: &Theme| container::Style {
                    background: Some(c.into()),
                    ..Default::default()
                })
                .into(),
            None => line,
        });
        lines.push(ui::hairline());
    }
    column(lines).width(Length::Fill).into()
}

/// One table row: its cells in proportional columns, padded 9 by 12 as the web's cells are.
fn cells<'a>(cols: &[(u16, &str)], items: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let laid: Vec<Element<'a, Msg>> = cols
        .iter()
        .zip(items)
        .map(|(col, item)| container(item).width(Length::FillPortion(col.0)).into())
        .collect();
    container(row(laid).spacing(12).align_y(Alignment::Center))
        .padding([9, 12])
        .width(Length::Fill)
        .into()
}

fn strong_rule<'a>() -> Element<'a, Msg> {
    container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().rule_strong.into()),
            ..Default::default()
        })
        .into()
}

/// A labelled control with its hint under it (the web's `field`: the label in sans ink-3, the hint under the control).
fn field<'a>(label: &str, control: Element<'a, Msg>, hint: &str) -> Element<'a, Msg> {
    let caption = ui::sans(label.to_string(), 12.0, theme::palette().ink_3);
    column![caption, control, ui::faint(hint.to_string())]
        .spacing(6)
        .width(Length::Fill)
        .into()
}

/// A paragraph under a panel's head, in the ink-2 of the web's `.wmr-desc`.
fn intro<'a>(line: &str) -> Element<'a, Msg> {
    ui::prose(line.to_string())
        .size(13.0)
        .color(theme::palette().ink_2)
        .into()
}

/// A row of a table with no tint.
fn plain<'a>(items: Vec<Element<'a, Msg>>) -> (Option<Color>, Vec<Element<'a, Msg>>) {
    (None, items)
}

// ---- Queue --------------------------------------------------------------------------------------------------------

/// The landing line onto main: the changes in order, the head that moves main.
fn queue_view<'a>(q: &'a Queue) -> Element<'a, Msg> {
    let p = theme::palette();
    let n = q.queue.len();
    let line = if n > 0 {
        format!(
            "{} in line for main. Only the head moves main.",
            changes_word(n)
        )
    } else {
        "Nothing is in line for main.".to_string()
    };
    let head = if q.head.is_empty() {
        "root".to_string()
    } else {
        q.head.clone()
    };
    let bar = row![
        ui::prose(line).size(13.0).color(p.ink_2),
        Space::new().width(Length::Fill),
        ui::prose("main is at ").size(13.0).color(p.ink_2),
        ui::mono(head, 12.0, p.ink),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let list: Element<'a, Msg> = if n == 0 {
        ui::empty(
            "git",
            "Nothing is in line",
            "Changes join the queue when they are ready to land.",
        )
    } else {
        let now = now_ms();
        let rows: Vec<(Option<Color>, Vec<Element<'a, Msg>>)> = q
            .queue
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let (label, tone) = queue_state(&x.state);
                let tint = match x.state.as_str() {
                    "conflict" => Some(p.crit_soft),
                    "landing" => Some(p.signal_soft),
                    _ => None,
                };
                let change = column![
                    ui::mono(x.change.clone(), 12.0, p.ink),
                    ui::faint(x.title.clone()).size(11.0),
                ]
                .spacing(2);
                let cells: Vec<Element<'a, Msg>> = vec![
                    ui::mono((i + 1).to_string(), 12.0, p.ink_2).into(),
                    change.into(),
                    ui::sans(x.task.clone().unwrap_or_else(|| "—".into()), 13.0, p.ink).into(),
                    ui::sans(x.author.clone(), 13.0, p.ink).into(),
                    ui::badge_dot(label, tone),
                    ui::sans(wait_text(now.saturating_sub(x.at_ms)), 13.0, p.ink).into(),
                ];
                (tint, cells)
            })
            .collect();
        table(
            &[
                (1, "#"),
                (5, "Change"),
                (2, "Task"),
                (2, "Agent"),
                (2, "State"),
                (2, "Waiting"),
            ],
            rows,
        )
    };
    let hint = ui::faint(
        "Only the head moves main. A change that shares no file with the head may land first. A clash leaves the line: rebase it onto main, or send it back to its agent.",
    );
    ui::card("Landing queue", column![bar, list, hint].spacing(12).into())
}

fn queue_state(state: &str) -> (&'static str, Tone) {
    match state {
        "queued" => ("Queued", Tone::Info),
        "landing" => ("Landing", Tone::Signal),
        "conflict" => ("Conflict", Tone::Danger),
        _ => ("Other", Tone::Muted),
    }
}

// ---- Keys and the verify panel ------------------------------------------------------------------------------------

/// The hub's signing keys (the owner's, the hub's, the agents') and the verify panel under them.
fn keys_view<'a>(ui: &'a State, data: &'a Data) -> Element<'a, Msg> {
    let p = theme::palette();
    let keys = &data.keys.keys;
    let list: Element<'a, Msg> = if keys.is_empty() {
        ui::empty(
            "git",
            "No keys yet",
            "Signing keys show here once this repository has signed records.",
        )
    } else {
        let mut rows: Vec<(Option<Color>, Vec<Element<'a, Msg>>)> = Vec::new();
        for k in keys {
            let revoked = k.revoked_seq.is_some();
            let ink = if revoked { p.ink_3 } else { p.ink };
            let (kind, tone) = key_kind(&k.holder.actor.kind);
            let holder = k
                .holder
                .name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| actor_name(&k.holder.actor, &data.names));
            let signs = if k.caps.is_empty() {
                "—".to_string()
            } else {
                k.caps.join(", ")
            };
            let status = if revoked {
                ui::badge_dot("Revoked", Tone::Danger)
            } else {
                ui::badge_dot("Active", Tone::Ok)
            };
            let cells: Vec<Element<'a, Msg>> = vec![
                column![ui::sans(holder, 13.0, ink), ui::badge(&kind, tone)]
                    .spacing(4)
                    .into(),
                ui::mono(k.id.chars().take(12).collect::<String>(), 12.0, ink).into(),
                ui::mono(ui::ellipsize(&k.key, 24), 12.0, ink).into(),
                ui::sans(format!("record {}", k.issued_seq), 13.0, ink).into(),
                ui::sans(signs, 13.0, ink).into(),
                status,
            ];
            rows.push(plain(cells));
        }
        table(
            &[
                (3, "Holder"),
                (2, "Id"),
                (3, "Key"),
                (2, "Issued"),
                (2, "Signs"),
                (2, "Status"),
            ],
            rows,
        )
    };
    let keys_panel = ui::card(
        "Signing keys",
        column![
            intro("Ed25519 keys, each named by its id. A revoked key signs nothing new, and the records it signed stay as they were."),
            list,
        ]
        .spacing(12)
        .into(),
    );

    let subject = text_input("w-212, w-206.r1 or acme-web@v0.3.0", &ui.verify_draft)
        .on_input(Msg::Draft)
        .on_submit(Msg::Verify)
        .padding(8)
        .width(Length::Fill);
    let verify_row = row![
        field(
            "Subject",
            subject.into(),
            "A change, a revision or a release."
        ),
        ui::btn("Verify", None, Kind::Primary, Some(Msg::Verify)),
    ]
    .spacing(10)
    .align_y(Alignment::End);
    let mut verify_items: Vec<Element<'a, Msg>> = vec![verify_row.into()];
    if let Some(v) = &ui.verify {
        verify_items.extend(verify_view(v, &data.names));
    }
    let verify_panel = ui::card(
        "Verify a signature",
        column(verify_items).spacing(12).into(),
    );

    column![keys_panel, verify_panel].spacing(14).into()
}

/// What the hub signed under a subject, record by record, each walked to the owner key.
fn verify_view<'a>(v: &'a Verify, names: &BTreeMap<u64, String>) -> Vec<Element<'a, Msg>> {
    let p = theme::palette();
    let head = row![
        ui::badge_dot(
            if v.ok {
                "Signature checks out"
            } else {
                "Signature check failed"
            },
            if v.ok { Tone::Ok } else { Tone::Danger },
        ),
        ui::prose(format!(
            "{}: {}, each walked to the owner key.",
            v.subject,
            plural(v.records.len(), "signed record", "signed records")
        ))
        .size(13.0)
        .color(p.ink_2),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let rows: Vec<(Option<Color>, Vec<Element<'a, Msg>>)> = v
        .records
        .iter()
        .map(|r| {
            let result = if r.ok {
                ui::badge_dot("Ok", Tone::Ok)
            } else {
                ui::badge_dot("Failed", Tone::Danger)
            };
            plain(vec![
                ui::mono(r.seq.to_string(), 12.0, p.ink).into(),
                ui::sans(r.kind.clone(), 13.0, p.ink).into(),
                ui::sans(signer_name(&r.actor, names), 13.0, p.ink).into(),
                ui::mono(r.key.chars().take(8).collect::<String>(), 12.0, p.ink).into(),
                ui::mono(r.chain.join(" → "), 11.0, p.ink_2).into(),
                ui::sans(if r.via_console { "Yes" } else { "No" }, 13.0, p.ink).into(),
                result,
            ])
        })
        .collect();
    vec![
        head.into(),
        table(
            &[
                (1, "Seq"),
                (2, "Kind"),
                (2, "Signed by"),
                (2, "Key"),
                (4, "Chain to owner key"),
                (2, "Via console"),
                (2, "Result"),
            ],
            rows,
        ),
    ]
}

fn key_kind(kind: &str) -> (String, Tone) {
    match kind {
        "owner" => ("Owner".into(), Tone::Signal),
        "hub" => ("Hub".into(), Tone::Info),
        "agent" => ("Agent".into(), Tone::Muted),
        other => (other.into(), Tone::Muted),
    }
}

// ---- Sync: the connections to outside git, the export with its dry run ---------------------------------------------

fn sync_view<'a>(ui: &'a State, repo: &'a RepoRow) -> Element<'a, Msg> {
    let conns = &repo.connections;
    let conn = picked(ui, conns);
    let list: Element<'a, Msg> = if conns.is_empty() {
        ui::faint("No outside git connections yet.").into()
    } else {
        let mut items: Vec<Element<'a, Msg>> = Vec::new();
        for (i, x) in conns.iter().enumerate() {
            if i > 0 {
                items.push(ui::hairline());
            }
            items.push(connection_row(x));
        }
        column(items).into()
    };
    let connections = ui::card(
        "Connections to outside git",
        column![
            intro("Where this repository meets outside git. A connection that moved on its own is adopted, never overwritten."),
            list,
        ]
        .spacing(12)
        .into(),
    );

    let ids: Vec<String> = conns.iter().map(|c| c.id.clone()).collect();
    let picker = pick_list(ids, Some(conn.clone()), Msg::Pick).width(Length::Fixed(220.0));
    let export_row = row![
        field(
            "Connection",
            picker.into(),
            "Export is fast-forward only: a connection that moved on its own is not pushed to.",
        ),
        ui::btn("Dry run", None, Kind::Ghost, Some(Msg::DryRun)),
        ui::btn("Export", None, Kind::Primary, Some(Msg::Export)),
    ]
    .spacing(10)
    .align_y(Alignment::End);
    let mut export_items: Vec<Element<'a, Msg>> = vec![
        intro("One commit per intent, authored by the owner and signed with the hub's key."),
        export_row.into(),
    ];
    if let Some(d) = ui
        .dry
        .as_ref()
        .filter(|d| d.repo == repo.name && d.conn == conn)
    {
        export_items.push(dry_view(&d.report));
    }
    let export = ui::card("Export to git", column(export_items).spacing(12).into());

    column![connections, export].spacing(14).into()
}

/// One connection: its state, id and remote, what its state says, and the action its state allows.
fn connection_row<'a>(x: &Connection) -> Element<'a, Msg> {
    let p = theme::palette();
    let action: Option<Element<'a, Msg>> = match x.state.as_str() {
        "in-step" => Some(ui::btn(
            "Dry run export",
            None,
            Kind::Ghost,
            Some(Msg::DryRunOf(x.id.clone())),
        )),
        "moved" => Some(ui::btn(
            "Adopt",
            None,
            Kind::Primary,
            Some(Msg::OpenAdopt(x.id.clone())),
        )),
        _ => None,
    };
    let mut text: Vec<Element<'a, Msg>> = vec![
        row![
            ui::badge_dot(connection_text(&x.state), connection_tone(&x.state)),
            ui::sans(x.id.clone(), 13.0, p.ink),
            ui::mono(x.remote.clone(), 12.0, p.ink_2),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    ];
    if !x.detail.is_empty() {
        text.push(ui::faint(x.detail.clone()).into());
    }
    let mut line = row![container(column(text).spacing(4)).width(Length::Fill)]
        .spacing(12)
        .align_y(Alignment::Center);
    if let Some(a) = action {
        line = line.push(a);
    }
    container(line).padding([12, 0]).width(Length::Fill).into()
}

/// What a dry run would push: the changes and their commits, nothing written.
fn dry_view<'a>(r: &Report) -> Element<'a, Msg> {
    let p = theme::palette();
    let n = r.changes.len();
    let summary = if n > 0 {
        format!(
            "Would push {} to {}. Nothing is pushed by a dry run.",
            changes_word(n),
            r.remote
        )
    } else {
        format!("Nothing to push: {} is in step with main.", r.remote)
    };
    let mut items: Vec<Element<'a, Msg>> = vec![
        row![
            ui::badge_dot("Dry run", Tone::Info),
            ui::prose(summary).size(13.0).color(p.ink),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    ];
    for ch in &r.changes {
        let mut lines: Vec<Element<'a, Msg>> =
            vec![ui::mono(ch.change.clone(), 12.0, p.ink).into()];
        for m in &ch.commits {
            lines.push(
                ui::faint(format!(
                    "{} · {} · {}",
                    m.intent,
                    m.git,
                    files_word(m.files)
                ))
                .into(),
            );
        }
        items.push(column(lines).spacing(2).into());
    }
    let radius = theme::skin().radius;
    container(column(items).spacing(10))
        .padding(12)
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(p.sheet_2.into()),
            border: Border {
                color: p.rule,
                width: 1.0,
                radius: radius.into(),
            },
            ..Default::default()
        })
        .into()
}

/// A connection's state in the web's words.
pub fn connection_text(state: &str) -> &str {
    match state {
        "in-step" => "In step",
        "moved" => "Moved on its own",
        "adopting" => "Adopting",
        "error" => "Error",
        other => other,
    }
}

/// A connection's tone: in step is ok, a moved remote warns, an error is a danger.
pub fn connection_tone(state: &str) -> Tone {
    match state {
        "in-step" => Tone::Ok,
        "moved" => Tone::Warn,
        "adopting" => Tone::Info,
        "error" => Tone::Danger,
        _ => Tone::Muted,
    }
}

// ---- Ops ----------------------------------------------------------------------------------------------------------

const OP_GROUPS: &[(&str, &str)] = &[
    ("all", "All"),
    ("land", "Land"),
    ("seal", "Seal"),
    ("sign", "Sign"),
    ("export", "Export"),
    ("import", "Import"),
    ("adopt", "Adopt"),
    ("revert", "Revert"),
    ("queue", "Queue"),
    ("key", "Key"),
];

/// The hub's operations log, newest first, filtered by its group.
fn ops_view<'a>(ui: &'a State, data: &'a Data) -> Element<'a, Msg> {
    let p = theme::palette();
    let seg = ui::seg(OP_GROUPS, ui.filter, Msg::Filter);
    let shown: Vec<&Op> = data
        .ops
        .iter()
        .rev()
        .filter(|o| ui.filter == "all" || op_group(&o.kind) == ui.filter)
        .collect();
    let list: Element<'a, Msg> = if shown.is_empty() {
        ui::empty(
            "git",
            "Nothing logged for this op",
            "Pick All to see every operation.",
        )
    } else {
        let rows: Vec<(Option<Color>, Vec<Element<'a, Msg>>)> = shown
            .iter()
            .map(|o| {
                plain(vec![
                    ui::sans(ago(o.at_ms), 13.0, p.ink).into(),
                    ui::sans(actor_name(&o.actor, &data.names), 13.0, p.ink).into(),
                    ui::badge(&o.kind, op_tone(op_group(&o.kind))),
                    ui::prose(o.summary.clone()).size(13.0).color(p.ink).into(),
                ])
            })
            .collect();
        table(&[(2, "When"), (3, "Actor"), (3, "Op"), (6, "Detail")], rows)
    };
    ui::card("Operations log", column![seg, list].spacing(12).into())
}

/// The group an operation's kind belongs to (`revision.seal` is a seal, `cert.issue` a key); `other` when none.
fn op_group(kind: &str) -> &'static str {
    if kind.starts_with("revision.seal") {
        "seal"
    } else if kind.starts_with("release.sign") {
        "sign"
    } else if kind.contains("export") {
        "export"
    } else if kind.contains("import") {
        "import"
    } else if kind.contains("adopt") {
        "adopt"
    } else if kind.contains("revert") {
        "revert"
    } else if kind.starts_with("queue.") {
        "queue"
    } else if kind.starts_with("cert.") || kind.starts_with("key") {
        "key"
    } else if kind.contains("land") {
        "land"
    } else {
        "other"
    }
}

fn op_tone(group: &str) -> Tone {
    match group {
        "land" => Tone::Ok,
        "seal" | "export" | "import" => Tone::Info,
        "sign" => Tone::Signal,
        "adopt" | "revert" => Tone::Warn,
        _ => Tone::Muted,
    }
}

// ---- Names and times ----------------------------------------------------------------------------------------------

/// A hub actor as a name: the owner and the hub keep their words, an agent is its name.
fn actor_name(a: &Actor, names: &BTreeMap<u64, String>) -> String {
    match a.kind.as_str() {
        "owner" => "Owner".into(),
        "hub" => "Hub".into(),
        _ => match a.cid {
            None => a.kind.clone(),
            Some(cid) => names
                .get(&cid)
                .cloned()
                .unwrap_or_else(|| format!("agent {cid}")),
        },
    }
}

/// A signer of a record as a name: `agent:301` is the agent's name; the owner and the hub keep their words.
fn signer_name(label: &str, names: &BTreeMap<u64, String>) -> String {
    match label {
        "owner" => "Owner".into(),
        "hub" => "Hub".into(),
        _ => match label
            .strip_prefix("agent:")
            .and_then(|n| n.parse::<u64>().ok())
        {
            Some(cid) => names
                .get(&cid)
                .cloned()
                .unwrap_or_else(|| format!("agent {cid}")),
            None => label.to_string(),
        },
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// "just now", "5 min ago", "2 h ago", "3 d ago" (the web's `ago`); an em dash for no time.
fn ago(at_ms: u64) -> String {
    if at_ms == 0 {
        return "—".into();
    }
    let s = now_ms().saturating_sub(at_ms) / 1000;
    if s < 45 {
        "just now".into()
    } else if s < 3_600 {
        format!("{} min ago", ((s + 30) / 60).max(1))
    } else if s < 86_400 {
        format!("{} h ago", (s + 1_800) / 3_600)
    } else {
        format!("{} d ago", (s + 43_200) / 86_400)
    }
}

/// A wait as the queue shows it: under a minute, `N min`, or `N h M min`.
fn wait_text(ms: u64) -> String {
    let m = (ms + 30_000) / 60_000;
    if m < 1 {
        "under a minute".into()
    } else if m < 60 {
        format!("{m} min")
    } else if m.is_multiple_of(60) {
        format!("{} h", m / 60)
    } else {
        format!("{} h {} min", m / 60, m % 60)
    }
}

fn changes_word(n: usize) -> String {
    plural(n, "change", "changes")
}

fn files_word(n: u64) -> String {
    if n == 1 {
        "1 file".into()
    } else {
        format!("{n} files")
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_reads_in_minutes_and_hours() {
        assert_eq!(wait_text(10_000), "under a minute");
        assert_eq!(wait_text(5 * 60_000), "5 min");
        assert_eq!(wait_text(65 * 60_000), "1 h 5 min");
        assert_eq!(wait_text(120 * 60_000), "2 h");
    }

    #[test]
    fn an_operation_sorts_into_its_group() {
        assert_eq!(op_group("revision.seal"), "seal");
        assert_eq!(op_group("release.sign"), "sign");
        assert_eq!(op_group("cert.issue"), "key");
        assert_eq!(op_group("queue.add"), "queue");
        assert_eq!(op_group("land"), "land");
        assert_eq!(op_group("something"), "other");
    }

    #[test]
    fn the_connections_read_in_the_webs_words() {
        assert_eq!(connection_text("in-step"), "In step");
        assert_eq!(connection_text("moved"), "Moved on its own");
        assert_eq!(connection_text("pushed"), "pushed");
        assert_eq!(connection_tone("moved"), Tone::Warn);
    }

    #[test]
    fn a_signer_is_named_by_its_agent() {
        let mut names = BTreeMap::new();
        names.insert(301, "Ada".to_string());
        assert_eq!(signer_name("agent:301", &names), "Ada");
        assert_eq!(signer_name("agent:9", &names), "agent 9");
        assert_eq!(signer_name("hub", &names), "Hub");
    }

    #[test]
    fn the_hub_answers_read_into_the_tabs() {
        let queue: Queue = serde_json::from_value(serde_json::json!({
            "repo": "acme-web", "ref": "main", "head": "w-206.l",
            "queue": [{ "change": "w-210", "title": "Footer links", "task": null, "author": "agent:215", "state": "queued", "at_ms": 1 }]
        }))
        .expect("a queue of the hub's shape");
        assert_eq!(queue.queue.len(), 1);
        assert_eq!(queue.queue[0].author, "agent:215");

        let keys: Keys = serde_json::from_value(serde_json::json!({
            "owner": "4f9c", "keys": [{ "id": "k1", "key": "ab", "holder": { "actor": { "kind": "agent", "cid": 215 } },
            "caps": ["sign"], "issued_seq": 3, "issuer": "owner", "revoked_seq": null, "revoked_reason": null }]
        }))
        .expect("keys of the hub's shape");
        assert_eq!(keys.keys[0].holder.actor.cid, Some(215));
        assert!(keys.keys[0].revoked_seq.is_none());

        let verify: Verify = serde_json::from_value(serde_json::json!({
            "ok": true, "owner_key": "4f9c", "subject": "w-206",
            "records": [{ "seq": 4, "kind": "written", "actor": "agent:215", "key": "ab12cd34", "signer": "agent:215", "at_ms": 1, "ok": true, "via_console": false, "body": null, "chain": ["w-206.r1", "owner"] }]
        }))
        .expect("a check of the hub's shape");
        assert_eq!(verify.records[0].chain.len(), 2);
    }
}
