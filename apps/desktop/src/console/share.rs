//! The Share dialog: one artifact or file shared as a public link and a network link (the web's
//! `apps/web/src/ui/share.ts`, drawn natively). It reads the target's live share from the hub, and every
//! change the owner makes is the hub's answer: the dialog reloads what the hub now holds after it lands.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{checkbox, column, opaque, row, text_input, toggler};
use iced::{Alignment, Element, Length, Task};
use serde::Deserialize;
use serde_json::{Value, json};

use super::client::{self, Client};
use super::ui;
use crate::theme;

const DAY_MS: i64 = 86_400_000;

/// The target a share belongs to, as the hub names it.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Target {
    pub kind: String,
    pub id: String,
}

/// The two links a live share offers: the public one and the network one.
#[derive(Debug, Clone, Deserialize)]
pub struct Links {
    pub public: String,
    pub network: String,
}

/// A share as the hub reports it. `state` is "live", "revoked" or "expired".
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ShareView {
    pub id: String,
    pub target: Target,
    #[serde(default)]
    pub version: Option<u32>,
    #[serde(default)]
    pub has_password: bool,
    #[serde(default)]
    pub expires_ms: Option<i64>,
    #[serde(default)]
    pub allow_download: bool,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub created_ms: i64,
    #[serde(default)]
    pub revoked_ms: Option<i64>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub views: u64,
    #[serde(default)]
    pub downloads: u64,
    pub state: String,
    pub links: Links,
}

/// One entry of a share's activity: a view, a download or a failed unlock, and how it arrived.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Event {
    pub at_ms: i64,
    pub kind: String,
    pub via: String,
}

/// The expiry the owner picks: never, a number of days from now, or a calendar date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expiry {
    Never,
    Days(u32),
    Date,
}

/// The dialog's state: the target, the live share the hub holds, and the drafts the owner has typed.
pub struct Dialog {
    client: Client,
    kind: String,
    id: String,
    /// The dialog's title: "Share" and the target's title.
    heading: String,
    /// The version the dialog was opened on (a version of the artifact), if any.
    version: Option<u32>,
    /// The first load has answered.
    loaded: bool,
    /// A change is in flight (the controls are disabled).
    busy: bool,
    /// The live share, or None while private.
    share: Option<ShareView>,
    /// The target had a share that expired and none is live.
    expired: bool,
    /// The last events of the live share, newest first.
    events: Vec<Event>,
    /// The expiry choice the owner made, until the hub answers; None shows the share's own expiry.
    expiry: Option<Expiry>,
    /// The date draft, `YYYY-MM-DD`.
    date: String,
    /// The password draft (never shown).
    password: String,
    /// The password last sent to the hub while its change is in flight; the draft is cleared only once the hub takes it.
    password_sent: Option<String>,
    /// Whether the new share pins the version the dialog was opened on.
    pinned: bool,
    /// Whether visitors may download the data and files.
    allow_download: bool,
    error: Option<String>,
    copied_public: bool,
    copied_network: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Loaded(Result<Vec<ShareView>, String>),
    /// Turn the share on (true, a POST) or off (false, a DELETE that revokes both links).
    Toggle(bool),
    Expiry(Expiry),
    /// The date draft changes.
    Date(String),
    /// Apply the date draft (Enter in the date field).
    ApplyDate,
    Password(String),
    SetPassword,
    ClearPassword,
    AllowDownload(bool),
    /// Pin the version the dialog was opened on (true) or follow the latest (false).
    Pin(bool),
    CopyPublic,
    CopyNetwork,
    Saved(Result<ShareView, String>),
    Events(Result<Vec<Event>, String>),
    Close,
}

/// Opens the dialog for one target. `version` is the version the owner is looking at, if the target has versions.
pub fn open(
    client: &Client,
    kind: &str,
    id: &str,
    title: &str,
    version: Option<u32>,
) -> (Dialog, Task<Msg>) {
    let dialog = Dialog {
        client: client.clone(),
        kind: kind.to_string(),
        id: id.to_string(),
        heading: format!("Share {title}"),
        version,
        loaded: false,
        busy: false,
        share: None,
        expired: false,
        events: Vec::new(),
        expiry: None,
        date: String::new(),
        password: String::new(),
        password_sent: None,
        pinned: false,
        allow_download: false,
        error: None,
        copied_public: false,
        copied_network: false,
    };
    let task = dialog.reload();
    (dialog, task)
}

/// Applies one message. It returns the task to run and whether the dialog closed.
pub fn update(d: &mut Dialog, msg: Msg) -> (Task<Msg>, bool) {
    match msg {
        Msg::Loaded(res) => {
            d.busy = false;
            d.loaded = true;
            match res {
                Ok(all) => {
                    d.expired = all.iter().all(|s| s.state != "live")
                        && all.iter().any(|s| s.state == "expired");
                    let live = all.into_iter().find(|s| s.state == "live");
                    d.error = None;
                    d.absorb(live);
                    (d.events_task(), false)
                }
                Err(e) => {
                    d.error = Some(e);
                    (Task::none(), false)
                }
            }
        }
        Msg::Saved(res) => {
            d.busy = false;
            match res {
                Ok(view) => {
                    d.error = None;
                    if let Some(sent) = d.password_sent.take()
                        && d.password == sent
                    {
                        d.password.clear();
                    }
                    let live = (view.state == "live").then_some(view);
                    d.absorb(live);
                    (d.events_task(), false)
                }
                Err(e) => {
                    // The toggles and the expiry show what the hub holds, so the confirmed values come back.
                    d.password_sent = None;
                    if let Some(share) = &d.share {
                        d.pinned = share.version.is_some();
                        d.allow_download = share.allow_download;
                    }
                    d.expiry = None;
                    d.error = Some(e);
                    (Task::none(), false)
                }
            }
        }
        Msg::Events(res) => {
            match res {
                Ok(list) => d.events = list,
                Err(e) => d.error = Some(e),
            }
            (Task::none(), false)
        }
        Msg::Toggle(on) => {
            if d.busy {
                return (Task::none(), false);
            }
            let task = if on { d.create() } else { d.revoke() };
            (task, false)
        }
        Msg::Expiry(choice) => {
            d.expiry = Some(choice);
            let task = match choice {
                Expiry::Never => d.patch(json!({ "expires_ms": null })),
                Expiry::Days(n) => {
                    d.patch(json!({ "expires_ms": now_ms() + i64::from(n) * DAY_MS }))
                }
                Expiry::Date => Task::none(),
            };
            (task, false)
        }
        Msg::Date(s) => {
            d.date = s;
            (Task::none(), false)
        }
        Msg::ApplyDate => match parse_day_end(&d.date) {
            Some(ms) if ms > now_ms() => (d.patch(json!({ "expires_ms": ms })), false),
            Some(_) => {
                d.error = Some("Pick a date after today.".to_string());
                (Task::none(), false)
            }
            None => {
                d.error = Some("Type a real date as YYYY-MM-DD, from 2000 to 9999.".to_string());
                (Task::none(), false)
            }
        },
        Msg::Password(s) => {
            d.password = s;
            (Task::none(), false)
        }
        Msg::SetPassword => {
            if d.busy {
                return (Task::none(), false);
            }
            if d.password.is_empty() {
                d.error = Some("Type a password first.".to_string());
                return (Task::none(), false);
            }
            let task = d.patch(json!({ "password": d.password.clone() }));
            // The patch starts only when the dialog is idle and the share is live; then the draft waits for the hub.
            if d.busy {
                d.password_sent = Some(d.password.clone());
            }
            (task, false)
        }
        Msg::ClearPassword => (d.patch(json!({ "password": null })), false),
        Msg::AllowDownload(on) => (d.patch(json!({ "allow_download": on })), false),
        Msg::Pin(on) => {
            let version = if on { d.version } else { None };
            (d.patch(json!({ "version": version })), false)
        }
        Msg::CopyPublic => match d.share.as_ref() {
            Some(share) => {
                d.copied_public = true;
                d.copied_network = false;
                (iced::clipboard::write(share.links.public.clone()), false)
            }
            None => (Task::none(), false),
        },
        Msg::CopyNetwork => match d.share.as_ref() {
            Some(share) => {
                d.copied_network = true;
                d.copied_public = false;
                (iced::clipboard::write(share.links.network.clone()), false)
            }
            None => (Task::none(), false),
        },
        Msg::Close => (Task::none(), true),
    }
}

impl Dialog {
    /// The hub's list of the target's shares, as a task.
    fn reload(&self) -> Task<Msg> {
        let c = self.client.clone();
        let path = self.list_path();
        client::task(move || c.get::<Vec<ShareView>>(&path), Msg::Loaded)
    }

    fn list_path(&self) -> String {
        format!(
            "/v1/shares?target={}",
            client::enc(&format!("{}:{}", self.kind, self.id))
        )
    }

    /// The live share's recent events, as a task (none while private).
    fn events_task(&self) -> Task<Msg> {
        let Some(id) = self.share.as_ref().map(|s| s.id.clone()) else {
            return Task::none();
        };
        let c = self.client.clone();
        let path = format!("/v1/shares/{}/events?limit=20", client::enc(&id));
        client::task(move || c.get::<Vec<Event>>(&path), Msg::Events)
    }

    /// The version a new share pins: the one the dialog was opened on, when the owner chose it.
    fn pin_version(&self) -> Option<u32> {
        if self.pinned { self.version } else { None }
    }

    /// Takes the hub's answer for the live share (or None when it is not live) and resets the drafts it replaces.
    fn absorb(&mut self, share: Option<ShareView>) {
        if let Some(s) = &share {
            self.pinned = s.version.is_some();
            self.allow_download = s.allow_download;
            self.date = s.expires_ms.map(date_of).unwrap_or_default();
        }
        self.share = share;
        self.expiry = None;
    }

    /// Turns the share on: a POST that creates the live share with the drafts the owner set.
    fn create(&mut self) -> Task<Msg> {
        let body = json!({
            "target": { "kind": self.kind, "id": self.id },
            "version": self.pin_version(),
            "allow_download": self.allow_download,
        });
        self.busy = true;
        self.error = None;
        let c = self.client.clone();
        client::task(move || c.post::<ShareView>("/v1/shares", &body), Msg::Saved)
    }

    /// Turns the share off: a DELETE that revokes both links, then the reload of what the hub holds.
    fn revoke(&mut self) -> Task<Msg> {
        let Some(id) = self.share.as_ref().map(|s| s.id.clone()) else {
            return Task::none();
        };
        self.busy = true;
        self.error = None;
        let c = self.client.clone();
        let path = format!("/v1/shares/{}", client::enc(&id));
        let list = self.list_path();
        client::task(
            move || -> Result<Vec<ShareView>, String> {
                c.delete(&path)?;
                c.get::<Vec<ShareView>>(&list)
            },
            Msg::Loaded,
        )
    }

    /// Changes the live share with a PATCH of the given fields. It does nothing while private or busy.
    fn patch(&mut self, body: Value) -> Task<Msg> {
        if self.busy {
            return Task::none();
        }
        let Some(id) = self.share.as_ref().map(|s| s.id.clone()) else {
            return Task::none();
        };
        self.busy = true;
        self.error = None;
        let c = self.client.clone();
        let path = format!("/v1/shares/{}", client::enc(&id));
        client::task(move || c.patch::<ShareView>(&path, &body), Msg::Saved)
    }
}

/// The Share dialog: the kit's modal (no foot), its body the switch, and once shared the links, the settings, the
/// activity and Stop sharing. The caller stacks it over the screen it opened from.
pub fn view(d: &Dialog) -> Element<'_, Msg> {
    let mut parts: Vec<Element<'_, Msg>> = Vec::new();
    if d.loaded {
        parts.push(switch_row(d));
        if let Some(share) = &d.share {
            parts.push(links(d, share));
            parts.push(settings(d));
            parts.push(activity(d));
            parts.push(ui::btn(
                "Stop sharing",
                None,
                ui::Kind::DangerOutline,
                action(d, Msg::Toggle(false)),
            ));
        }
    } else {
        parts.push(ui::loading());
    }
    if let Some(error) = &d.error {
        parts.push(ui::error_line(error));
    }
    let body = column(parts).spacing(16).width(Length::Fill);
    // Opaque, so a press on the dialog does not reach the page beneath it.
    opaque(ui::modal(
        &d.heading,
        body.into(),
        Vec::new(),
        Msg::Close,
        WIDTH,
    ))
}

/// The dialog's width: the web's default sheet.
const WIDTH: f32 = 560.0;

/// The message a control sends, or None while a change is in flight (the control is disabled).
fn action(d: &Dialog, msg: Msg) -> Option<Msg> {
    (!d.busy).then_some(msg)
}

/// The switch: private, or anyone with the link can view.
fn switch_row(d: &Dialog) -> Element<'_, Msg> {
    let live = d.share.is_some();
    let label = if live {
        "Anyone with the link (can view)"
    } else {
        "Private (you only)"
    };
    let note = if live {
        "Turning this off stops sharing and revokes both links at once."
    } else if d.expired {
        "This link has expired. Turning sharing on makes a new link."
    } else {
        "Only you can open this."
    };
    column![
        toggler(live).label(label).on_toggle(Msg::Toggle),
        ui::faint(note)
    ]
    .spacing(6)
    .into()
}

/// The public link and the network link, each with its copy button.
fn links<'a>(d: &Dialog, share: &'a ShareView) -> Element<'a, Msg> {
    column![
        link_row(
            "Link",
            &share.links.public,
            d.copied_public,
            "Copy link",
            Msg::CopyPublic
        ),
        link_row(
            "Network link",
            &share.links.network,
            d.copied_network,
            "Copy network link",
            Msg::CopyNetwork,
        ),
    ]
    .spacing(12)
    .into()
}

fn link_row<'a>(
    label: &'static str,
    url: &'a str,
    copied: bool,
    copy_label: &'static str,
    msg: Msg,
) -> Element<'a, Msg> {
    let mut line = row![
        text_input("", url)
            .style(theme::input)
            .size(12.0)
            .font(theme::MONO)
            .width(Length::Fill),
        ui::btn_sm(copy_label, None, ui::Kind::Secondary, Some(msg)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if copied {
        line = line.push(ui::faint("Copied"));
    }
    column![ui::label(label), line].spacing(6).into()
}

/// The live share's settings: expiry, password, version and downloads.
fn settings(d: &Dialog) -> Element<'_, Msg> {
    let live_expiry = match &d.share {
        Some(share) if share.expires_ms.is_some() => Expiry::Date,
        _ => Expiry::Never,
    };
    let selected = d.expiry.unwrap_or(live_expiry);
    let choices = [
        (Expiry::Never, "Never"),
        (Expiry::Days(1), "1 day"),
        (Expiry::Days(7), "7 days"),
        (Expiry::Days(30), "30 days"),
        (Expiry::Date, "On a date"),
    ];
    let expiry_buttons: Vec<Element<'_, Msg>> = choices
        .iter()
        .map(|(e, label)| choice(label, selected == *e, Msg::Expiry(*e), d.busy))
        .collect();
    let mut block = column![ui::label("Expires"), row(expiry_buttons).spacing(6)].spacing(6);
    if selected == Expiry::Date {
        block = block.push(
            text_input("YYYY-MM-DD", &d.date)
                .on_input(Msg::Date)
                .on_submit(Msg::ApplyDate)
                .style(theme::input)
                .size(13.0),
        );
        block = block.push(ui::faint("The link stops working at the end of this day."));
    }

    let has_password = d.share.as_ref().is_some_and(|s| s.has_password);
    let password = text_input(
        if has_password {
            "New password"
        } else {
            "Password"
        },
        &d.password,
    )
    .secure(true)
    .on_input(Msg::Password)
    .on_submit(Msg::SetPassword)
    .style(theme::input)
    .size(13.0)
    .width(Length::Fill);
    let mut password_row = row![
        password,
        ui::btn_sm(
            if has_password {
                "Change"
            } else {
                "Set password"
            },
            None,
            ui::Kind::Ghost,
            action(d, Msg::SetPassword),
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if has_password {
        password_row = password_row.push(ui::btn_sm(
            "Remove",
            None,
            ui::Kind::Ghost,
            action(d, Msg::ClearPassword),
        ));
    }
    let password_note = if has_password {
        "Visitors need this password to open the link."
    } else {
        "Anyone with the link can open it."
    };
    block = block.push(
        column![
            ui::label("Password"),
            password_row,
            ui::faint(password_note)
        ]
        .spacing(6),
    );

    if let Some(version) = d.version {
        let version_buttons = row![
            choice("Latest", !d.pinned, Msg::Pin(false), d.busy),
            choice(
                &format!("This version (v{version})"),
                d.pinned,
                Msg::Pin(true),
                d.busy
            ),
        ]
        .spacing(6);
        block = block.push(column![ui::label("Version"), version_buttons].spacing(6));
    }

    block = block.push(
        checkbox(d.allow_download)
            .label("Allow downloading data and files")
            .on_toggle(Msg::AllowDownload),
    );
    block.into()
}

/// One choice of a row of choices: the kit's button, solid when selected, outlined otherwise.
fn choice<'a>(label: &str, selected: bool, msg: Msg, busy: bool) -> Element<'a, Msg> {
    ui::btn_sm(
        label,
        None,
        if selected {
            ui::Kind::Primary
        } else {
            ui::Kind::Secondary
        },
        (!busy).then_some(msg),
    )
}

/// The counts and the last events of the live share, in a card as the web's.
fn activity(d: &Dialog) -> Element<'_, Msg> {
    let palette = theme::palette();
    let (views, downloads) = d.share.as_ref().map_or((0, 0), |s| (s.views, s.downloads));
    let mut lines: Vec<Element<'_, Msg>> = Vec::new();
    if d.events.is_empty() {
        lines.push(ui::faint("Nobody has opened this link yet.").into());
    }
    for event in &d.events {
        lines.push(
            ui::sans(
                format!(
                    "{} · via {} · {}",
                    event_label(&event.kind),
                    via_label(&event.via),
                    ago(event.at_ms)
                ),
                theme::TYPE_UI,
                palette.ink_2,
            )
            .into(),
        );
    }
    let body = column![
        ui::sans(
            format!("{views} views · {downloads} downloads"),
            theme::TYPE_UI,
            palette.ink
        ),
        column(lines).spacing(4),
    ]
    .spacing(8);
    ui::card("Activity", body.into())
}

fn event_label(kind: &str) -> &str {
    match kind {
        "view" => "Viewed",
        "download" => "Downloaded",
        "unlock_failed" => "Wrong password",
        other => other,
    }
}

fn via_label(via: &str) -> &str {
    match via {
        "relay" => "agentplaza.io",
        "network" => "your network",
        other => other,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// How long ago a millisecond timestamp was, in the web's words ("2 h ago").
fn ago(ms: i64) -> String {
    let secs = (now_ms() - ms).max(0) / 1000;
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

/// The end of a `YYYY-MM-DD` day in UTC, in milliseconds, or None when the text is not a real calendar day from 2000
/// to 9999. The arithmetic is checked, so no input can overflow.
fn parse_day_end(s: &str) -> Option<i64> {
    let mut parts = s.trim().split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some()
        || !(2000..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
    {
        return None;
    }
    let next_day = days_from_civil(year, month, day).checked_add(1)?;
    next_day.checked_mul(DAY_MS)?.checked_sub(1)
}

/// The number of days in a month of a Gregorian year; February counts its leap day.
fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// A millisecond timestamp as the `YYYY-MM-DD` day it falls on, in UTC.
fn date_of(ms: i64) -> String {
    let (y, m, d) = civil_from_days(ms.div_euclid(DAY_MS));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The proleptic Gregorian date of a count of days since 1970-01-01 (the inverse of `days_from_civil`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
