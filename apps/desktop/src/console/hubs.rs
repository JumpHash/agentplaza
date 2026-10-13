//! Connect to a hub elsewhere: the Connect screen (the hub URL and its certificate pin, the hub's
//! identity card, the owner token), the saved hubs and switching between them, and sign-out.
//!
//! Saved hubs live in `cli.json`, the file `agentplaza hub login` writes in the same directory, so
//! the CLI and the desktop share them. A remote hub is HTTPS with its own certificate: a pin (the
//! SHA-256 the hub shows) is trusted in place of a CA, as the CLI does. Without a pin the hub must
//! have a certificate a public CA signed.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{column, row, text_input, Space};
use iced::{Alignment, Element, Length, Task};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::boot::LOCAL_HUB;
use super::client::{self, Client};
use super::services;
use super::ui::{self, Kind};

/// A saved hub: one `hubs` entry of `cli.json`, the same fields the CLI writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedHub {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The hub certificate's SHA-256 pin, hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_token: Option<String>,
    /// The hub identity first seen on a successful connection (trust on first use), until forgotten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<Pinned>,
}

/// A remembered hub identity: the certificate fingerprint the hub showed, and the UTC day it was pinned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pinned {
    pub fingerprint: String,
    pub since: String,
}

/// Today's UTC date as `YYYY-MM-DD`, from the Unix clock (civil calendar, no time crate).
fn today() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The first 16 characters of a fingerprint, for the warning.
fn short(fingerprint: &str) -> String {
    format!("{}…", fingerprint.chars().take(16).collect::<String>())
}

/// `cli.json`: the saved hubs and the default one. Private to this user: it holds owner tokens.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub hubs: BTreeMap<String, SavedHub>,
    #[serde(default)]
    pub default: Option<String>,
}

/// `GET /v1/hub/identity`: what a hub says it is, shown before the owner signs in.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Identity {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub nodes: u64,
}

/// `cli.json` in the platform's config directory, as the CLI reads it.
pub fn default_path() -> PathBuf {
    apz_os::dirs::config().join("cli.json")
}

impl Config {
    pub fn load() -> Config {
        Self::load_from(&default_path())
    }

    /// Reads `path`; a missing or unreadable file is an empty config.
    pub fn load_from(path: &Path) -> Config {
        fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&default_path())
    }

    /// Writes `path` privately, creating its directory.
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            apz_os::private::create_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        apz_os::private::write(path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The default hub, when it is saved.
    pub fn active(&self) -> Option<&SavedHub> {
        self.default.as_deref().and_then(|name| self.hubs.get(name))
    }

    /// Makes a saved hub the default; an unknown name changes nothing.
    pub fn set_active(&mut self, name: &str) -> Result<(), String> {
        if !self.hubs.contains_key(name) {
            return Err(format!("no saved hub `{name}`"));
        }
        self.default = Some(name.to_string());
        Ok(())
    }

    /// Forgets the owner token of a saved hub and keeps its entry. Signing out of the default
    /// clears the default, so the CLI falls back to the local hub. True when a token was removed.
    pub fn sign_out(&mut self, name: &str) -> bool {
        let Some(hub) = self.hubs.get_mut(name) else {
            return false;
        };
        let had_token = hub.token.take().is_some();
        if self.default.as_deref() == Some(name) {
            self.default = None;
        }
        had_token
    }
}

#[derive(Debug, Default)]
pub struct State {
    pub config: Config,
    pub url: String,
    /// The hub certificate's pin, optional (a hub with a public certificate needs none).
    pub pin: String,
    pub token: String,
    /// The identity card of the URL last checked; cleared when the URL or pin changes.
    pub identity: Option<Identity>,
    /// The checked URL answers with an identity other than the pinned one: nothing is sent until trusted.
    pub mismatch: Option<Mismatch>,
    pub busy: bool,
    pub error: Option<String>,
    pub confirm_sign_out: bool,
}

/// A hub identity that differs from the pinned one.
#[derive(Debug, Clone)]
pub struct Mismatch {
    pub expected: String,
    pub received: Option<String>,
}

/// The pin remembered for a URL: its saved hub's entry, keyed as `connect` saves it.
fn pinned_for<'a>(config: &'a Config, url: &str) -> Option<&'a Pinned> {
    config.hubs.get(&host_of(url.trim()))?.pinned.as_ref()
}

impl State {
    /// The Connect screen with the saved hubs from `cli.json`.
    pub fn new() -> State {
        State { config: Config::load(), ..State::default() }
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Url(String),
    Pin(String),
    Token(String),
    /// Fetch the identity card of the URL.
    Check,
    Identity(Result<Identity, String>),
    /// Verify the owner token, save the hub as the default, and open it.
    Connect,
    /// The hub answered (Ok carries the client), or why not.
    Connected(Result<Client, String>),
    /// Trust the identity the checked URL shows now: re-pin it.
    TrustIdentity,
    /// Drop the mismatch and the checked identity; nothing is connected.
    CancelIdentity,
    /// Forget the pinned identity of the URL.
    ForgetPin,
    /// Switch to a saved hub.
    Use(String),
    AskSignOut,
    CancelSignOut,
    SignOut,
    /// After sign-out: the local hub's client if it answers.
    SignedOut(Result<Option<Client>, String>),
}

/// Handles one message; Some(client) means open that hub.
pub fn update(state: &mut State, msg: Msg) -> (Task<Msg>, Option<Client>) {
    match msg {
        Msg::Url(value) => {
            state.url = value;
            state.identity = None;
            state.mismatch = None;
            (Task::none(), None)
        }
        Msg::Pin(value) => {
            state.pin = value;
            state.identity = None;
            state.mismatch = None;
            (Task::none(), None)
        }
        Msg::Token(value) => {
            state.token = value;
            (Task::none(), None)
        }
        Msg::Check => {
            let url = state.url.trim().to_string();
            if state.busy || url.is_empty() {
                return (Task::none(), None);
            }
            let pin = state.pin.trim().to_string();
            state.busy = true;
            state.error = None;
            state.identity = None;
            state.mismatch = None;
            (client::task(move || fetch_identity(&url, &pin), Msg::Identity), None)
        }
        Msg::Identity(result) => {
            state.busy = false;
            match result {
                Ok(identity) => {
                    // Trust on first use: a hand-typed pin already binds the certificate, so only
                    // an unpinned URL is compared with the identity remembered for it.
                    state.mismatch = match pinned_for(&state.config, &state.url) {
                        Some(pinned) if state.pin.trim().is_empty() && identity.fingerprint.as_deref() != Some(pinned.fingerprint.as_str()) => {
                            Some(Mismatch { expected: pinned.fingerprint.clone(), received: identity.fingerprint.clone() })
                        }
                        _ => None,
                    };
                    state.identity = Some(identity);
                }
                Err(error) => state.error = Some(error),
            }
            (Task::none(), None)
        }
        Msg::Connect => {
            let url = state.url.trim().to_string();
            let pin = state.pin.trim().to_string();
            let token = state.token.trim().to_string();
            if state.busy || state.mismatch.is_some() || token.is_empty() {
                return (Task::none(), None);
            }
            let Some(checked) = state.identity.as_ref().map(|identity| identity.fingerprint.clone()) else {
                return (Task::none(), None);
            };
            state.busy = true;
            state.error = None;
            (client::task(move || connect(&url, &pin, &token, checked), Msg::Connected), None)
        }
        Msg::TrustIdentity => {
            let Some(mismatch) = state.mismatch.take() else {
                return (Task::none(), None);
            };
            let name = host_of(state.url.trim());
            if let Some(hub) = state.config.hubs.get_mut(&name) {
                hub.pinned = mismatch.received.map(|fingerprint| Pinned { fingerprint, since: today() });
            }
            if let Err(error) = state.config.save() {
                state.error = Some(error);
            }
            (Task::none(), None)
        }
        Msg::CancelIdentity => {
            state.mismatch = None;
            state.identity = None;
            (Task::none(), None)
        }
        Msg::ForgetPin => {
            let name = host_of(state.url.trim());
            if let Some(hub) = state.config.hubs.get_mut(&name) {
                hub.pinned = None;
            }
            state.mismatch = None;
            if let Err(error) = state.config.save() {
                state.error = Some(error);
            }
            (Task::none(), None)
        }
        Msg::Connected(result) => {
            state.busy = false;
            match result {
                Ok(hub) => {
                    state.config = Config::load();
                    state.token.clear();
                    state.confirm_sign_out = false;
                    (Task::none(), Some(hub))
                }
                Err(error) => {
                    state.error = Some(error);
                    (Task::none(), None)
                }
            }
        }
        Msg::Use(name) => {
            if state.busy {
                return (Task::none(), None);
            }
            if let Err(error) = state.config.set_active(&name) {
                state.error = Some(error);
                return (Task::none(), None);
            }
            if let Err(error) = state.config.save() {
                state.error = Some(error);
                return (Task::none(), None);
            }
            let Some(hub) = state.config.hubs.get(&name).cloned() else {
                return (Task::none(), None);
            };
            state.busy = true;
            state.error = None;
            (client::task(move || verify(&hub), Msg::Connected), None)
        }
        Msg::AskSignOut => {
            state.confirm_sign_out = true;
            (Task::none(), None)
        }
        Msg::CancelSignOut => {
            state.confirm_sign_out = false;
            (Task::none(), None)
        }
        Msg::SignOut => {
            let Some(name) = state.config.default.clone() else {
                return (Task::none(), None);
            };
            if state.busy || !state.config.sign_out(&name) {
                return (Task::none(), None);
            }
            state.confirm_sign_out = false;
            state.token.clear();
            state.identity = None;
            state.error = None;
            if let Err(error) = state.config.save() {
                state.error = Some(error);
                return (Task::none(), None);
            }
            state.busy = true;
            (client::task(|| Ok::<Option<Client>, String>(local_hub()), Msg::SignedOut), None)
        }
        Msg::SignedOut(result) => {
            state.busy = false;
            state.config = Config::load();
            match result {
                Ok(Some(local)) => (Task::none(), Some(local)),
                Ok(None) => {
                    state.error = Some("Signed out. No hub on this machine answers: connect to a hub.".into());
                    (Task::none(), None)
                }
                Err(error) => {
                    state.error = Some(error);
                    (Task::none(), None)
                }
            }
        }
    }
}

/// The identity card of a hub, fetched with its pin when one is given.
fn fetch_identity(url: &str, pin: &str) -> Result<Identity, String> {
    Client::new(url, parse_optional_pin(pin)?, None).get::<Identity>("/v1/hub/identity")
}

/// Verifies the owner token against a remote hub and saves the hub as the default. Returns the
/// client to open it with.
fn connect(url: &str, pin: &str, token: &str, checked: Option<String>) -> Result<Client, String> {
    // The identity is read again, unauthenticated, before the owner token is sent: it must still be
    // the one the user checked.
    let fresh = fetch_identity(url, pin)?;
    if fresh.fingerprint != checked {
        return Err("The hub's identity changed since it was checked. Check the hub again.".into());
    }
    let client = Client::new(url, parse_optional_pin(pin)?, Some(token.to_string()));
    client.get::<Value>("/v1/auth/check")?;
    let name = host_of(url);
    let mut config = Config::load();
    let (mcp_token, existing) = config.hubs.get(&name).map(|hub| (hub.mcp_token.clone(), hub.pinned.clone())).unwrap_or_default();
    // A hand-typed pin overrides trust on first use; otherwise the identity is remembered on first connect.
    let pinned = match (pin.is_empty(), fresh.fingerprint) {
        (true, Some(fingerprint)) => match existing {
            Some(pinned) if pinned.fingerprint == fingerprint => Some(pinned),
            _ => Some(Pinned { fingerprint, since: today() }),
        },
        _ => existing,
    };
    config.hubs.insert(
        name.clone(),
        SavedHub {
            url: client.base().to_string(),
            token: Some(token.to_string()),
            pin: (!pin.is_empty()).then(|| pin.to_string()),
            mcp_token,
            pinned,
        },
    );
    config.default = Some(name);
    config.save()?;
    Ok(client)
}

/// A saved hub's client, after checking its owner token still answers. A remembered identity that
/// no longer matches stops the token from being sent.
fn verify(hub: &SavedHub) -> Result<Client, String> {
    let token = hub.token.clone().ok_or_else(|| "Sign in to this hub first: paste its owner token.".to_string())?;
    if hub.pin.is_none() {
        if let Some(pinned) = &hub.pinned {
            let fresh = fetch_identity(&hub.url, "")?;
            if fresh.fingerprint.as_deref() != Some(pinned.fingerprint.as_str()) {
                return Err(format!(
                    "{}'s identity changed: expected {}, received {}. Check it on the Connect screen before signing in.",
                    hub.url,
                    short(&pinned.fingerprint),
                    fresh.fingerprint.as_deref().map(short).unwrap_or_else(|| "no certificate".into()),
                ));
            }
        }
    }
    let client = Client::new(&hub.url, parse_optional_pin(hub.pin.as_deref().unwrap_or(""))?, Some(token));
    client.get::<Value>("/v1/auth/check")?;
    Ok(client)
}

/// The local hub's client when it answers with this machine's owner token.
fn local_hub() -> Option<Client> {
    let client = Client::new(LOCAL_HUB, None, services::local_owner_token());
    client.get::<Value>("/v1/auth/check").ok().map(|_| client)
}

/// The name a hub is saved under: its host, as `agentplaza hub login` names it.
fn host_of(url: &str) -> String {
    let rest = url.split("://").last().unwrap_or(url);
    let host = rest.split('/').next().unwrap_or(rest);
    match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name.to_string(),
        _ => host.to_string(),
    }
}

/// Parses a 64-hex-digit SHA-256 pin (a `sha256:` prefix and colons are allowed), as the CLI does.
pub fn parse_pin(hex: &str) -> Result<[u8; 32], String> {
    let hex = hex.trim().trim_start_matches("sha256:").replace(':', "");
    if hex.len() != 64 {
        return Err("a pin is 64 hex digits (the hub certificate's SHA-256)".into());
    }
    let mut pin = [0u8; 32];
    for (i, byte) in pin.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| "a pin is hex digits".to_string())?;
    }
    Ok(pin)
}

/// An empty pin is none; otherwise it must parse.
fn parse_optional_pin(text: &str) -> Result<Option<[u8; 32]>, String> {
    let text = text.trim();
    if text.is_empty() {
        Ok(None)
    } else {
        parse_pin(text).map(Some)
    }
}

pub fn view(state: &State) -> Element<'_, Msg> {
    let busy = state.busy;
    let mut page: Vec<Element<'_, Msg>> = Vec::new();

    if !state.config.hubs.is_empty() {
        page.push(ui::label("Saved hubs").into());
        for (name, hub) in &state.config.hubs {
            let active = state.config.default.as_deref() == Some(name.as_str());
            let signed_in = hub.token.is_some();
            let title = if active { format!("{name} (active)") } else { name.clone() };
            let status = if signed_in { "signed in" } else { "signed out" };
            page.push(
                row![
                    column![ui::mono(title, 15.0, ui::INK), ui::faint(format!("{} · {status}", hub.url))].spacing(4),
                    Space::new().width(Length::Fill),
                    ui::btn("Use", None, Kind::Ghost, (!busy && signed_in && !active).then(|| Msg::Use(name.clone()))),
                ]
                .align_y(Alignment::Center)
                .into(),
            );
        }
    }

    page.push(ui::label("Hub URL").into());
    page.push(text_input("https://hub.example:7700", &state.url).on_input(Msg::Url).on_submit(Msg::Check).into());
    page.push(ui::label("Certificate pin (for a hub with its own certificate)").into());
    page.push(text_input("64 hex digits", &state.pin).on_input(Msg::Pin).on_submit(Msg::Check).into());
    page.push(ui::btn("Check hub", None, Kind::Ghost, (!busy && !state.url.trim().is_empty()).then_some(Msg::Check)));
    if let Some(identity) = &state.identity {
        let pinned = pinned_for(&state.config, &state.url).filter(|_| state.pin.trim().is_empty());
        page.push(identity_card(identity, pinned, busy));
    }
    if let Some(mismatch) = &state.mismatch {
        let received = mismatch.received.as_deref().map(short).unwrap_or_else(|| "no certificate".into());
        let warning = format!(
            "This URL answers with a different identity. Pinned {}, received {received}. A reinstalled hub has a new identity; anything else answering here is not your hub. No token is sent until you trust the new identity.",
            short(&mismatch.expected)
        );
        page.push(
            column![
                ui::error_line(&warning),
                row![
                    ui::btn("Trust the new identity", None, Kind::Primary, (!busy).then_some(Msg::TrustIdentity)),
                    ui::btn("Cancel", None, Kind::Ghost, Some(Msg::CancelIdentity)),
                ]
                .spacing(8),
            ]
            .spacing(10)
            .into(),
        );
    }
    page.push(ui::label("Owner token").into());
    page.push(text_input("owner token", &state.token).on_input(Msg::Token).on_submit(Msg::Connect).secure(true).into());
    page.push(ui::faint("Check that the identity above is the hub you mean, then connect.").into());
    let connectable = !busy && state.identity.is_some() && state.mismatch.is_none() && !state.token.trim().is_empty();
    page.push(ui::btn("Connect", Some("external"), Kind::Primary, connectable.then_some(Msg::Connect)));

    if let (Some(name), Some(_)) = (state.config.default.as_deref(), state.config.active().and_then(|hub| hub.token.as_ref())) {
        if state.confirm_sign_out {
            page.push(ui::faint(format!("Forget the owner token for {name}? This machine stops signing in to it until the token is pasted again.")).into());
            page.push(
                row![
                    ui::btn("Sign out", None, Kind::Primary, (!busy).then_some(Msg::SignOut)),
                    ui::btn("Cancel", None, Kind::Ghost, Some(Msg::CancelSignOut)),
                ]
                .spacing(8)
                .into(),
            );
        } else {
            page.push(ui::btn("Sign out", None, Kind::Ghost, (!busy).then_some(Msg::AskSignOut)));
        }
    }

    if busy {
        page.push(ui::loading());
    }
    if let Some(error) = &state.error {
        page.push(ui::error_line(error));
    }
    column(page).spacing(14).into()
}

fn identity_card(identity: &Identity, pinned: Option<&Pinned>, busy: bool) -> Element<'static, Msg> {
    let name = if identity.name.is_empty() { "Unnamed hub".to_string() } else { identity.name.clone() };
    let certificate = match &identity.fingerprint {
        Some(fingerprint) => format!("certificate {fingerprint}"),
        None => "no certificate fingerprint".to_string(),
    };
    let mut card: Vec<Element<'static, Msg>> = vec![
        ui::mono(name, 16.0, ui::INK).into(),
        ui::faint(format!("{} · version {} · {} nodes", identity.url, identity.version, identity.nodes)).into(),
        ui::faint(certificate).into(),
    ];
    if let Some(pinned) = pinned {
        card.push(
            row![
                ui::faint(format!("Pinned since {}", pinned.since)),
                Space::new().width(Length::Fill),
                ui::btn("Forget pin", None, Kind::Ghost, (!busy).then_some(Msg::ForgetPin)),
            ]
            .align_y(Alignment::Center)
            .into(),
        );
    }
    column(card).spacing(4).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch `cli.json` path in a fresh directory for this test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-hubs-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("cli.json")
    }

    fn hub(url: &str, token: Option<&str>) -> SavedHub {
        SavedHub { url: url.into(), token: token.map(Into::into), pin: None, mcp_token: None, pinned: None }
    }

    #[test]
    fn cli_json_round_trips_in_the_cli_format() {
        let path = scratch("round-trip");
        let mut config = Config::default();
        config.hubs.insert("home".into(), SavedHub { pin: Some("ab".repeat(32)), ..hub("https://home.example:7700", Some("tok")) });
        config.hubs.insert("lab".into(), hub("https://lab.example:7700", None));
        config.default = Some("home".into());
        config.save_to(&path).expect("saved");

        let text = fs::read_to_string(&path).expect("read back");
        assert!(text.contains("\"default\": \"home\""));
        assert!(!text.contains("mcp_token"), "absent optional fields are not written");
        let back = Config::load_from(&path);
        assert_eq!(back.default.as_deref(), Some("home"));
        assert_eq!(back.hubs["home"].token.as_deref(), Some("tok"));
        assert_eq!(back.hubs["home"].pin, Some("ab".repeat(32)));
        assert_eq!(back.hubs["lab"].token, None);

        // A file the CLI wrote reads the same way.
        fs::write(&path, r#"{"hubs":{"x":{"url":"https://x:7700","token":"t"}},"default":"x"}"#).expect("write");
        assert_eq!(Config::load_from(&path).active().map(|h| h.url.as_str()), Some("https://x:7700"));
        let _ = fs::remove_dir_all(path.parent().expect("a directory"));
    }

    #[test]
    fn switching_chooses_the_active_hub() {
        let mut config = Config::default();
        config.hubs.insert("a".into(), hub("https://a:7700", Some("ta")));
        config.hubs.insert("b".into(), hub("https://b:7700", Some("tb")));
        assert!(config.active().is_none(), "no default, so nothing is active");
        config.set_active("b").expect("a saved hub");
        assert_eq!(config.active().map(|h| h.url.as_str()), Some("https://b:7700"));
        config.set_active("a").expect("a saved hub");
        assert_eq!(config.default.as_deref(), Some("a"));
        assert!(config.set_active("nope").is_err());
        assert_eq!(config.default.as_deref(), Some("a"), "an unknown name changes nothing");
    }

    #[test]
    fn sign_out_forgets_the_token_and_keeps_the_entry() {
        let path = scratch("sign-out");
        let mut config = Config::default();
        config.hubs.insert("home".into(), SavedHub { pin: Some("cd".repeat(32)), ..hub("https://home:7700", Some("secret")) });
        config.default = Some("home".into());
        config.save_to(&path).expect("saved");

        assert!(config.sign_out("home"));
        assert!(!config.sign_out("home"), "already signed out");
        assert!(!config.sign_out("missing"));
        config.save_to(&path).expect("saved");

        let back = Config::load_from(&path);
        assert_eq!(back.hubs["home"].token, None);
        assert_eq!(back.hubs["home"].url, "https://home:7700");
        assert_eq!(back.hubs["home"].pin, Some("cd".repeat(32)));
        assert_eq!(back.default, None, "signing out of the default returns to the local hub");
        assert!(!fs::read_to_string(&path).expect("read back").contains("secret"));
        let _ = fs::remove_dir_all(path.parent().expect("a directory"));
    }

    #[test]
    fn pins_and_saved_names_parse_as_the_cli_writes_them() {
        assert_eq!(parse_pin(&"ab".repeat(32)).expect("hex")[0], 0xab);
        let colons = vec!["ab"; 32].join(":");
        assert!(parse_pin(&format!("sha256:{colons}")).is_ok());
        assert!(parse_pin("abcd").is_err());
        assert_eq!(parse_optional_pin("   ").expect("empty is fine"), None);
        assert_eq!(host_of("https://plaza.example:7700/x"), "plaza.example");
        assert_eq!(host_of("https://10.0.0.5"), "10.0.0.5");
    }
}
