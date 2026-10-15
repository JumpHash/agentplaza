//! The Egress credentials screen (parity with the web console's egress view): the owner's rules for the hub's HTTPS
//! proxy. A rule names a host, and the header the hub adds to an agent's request to it; the value comes from a vault
//! secret bound to the `egress` consumer, so the secret never enters the VM. The form checks a draft the way the hub
//! does (`crates/apid/src/egress_inject.rs`), so a refusal shows before the request; the hub stays the authority.

use iced::widget::{checkbox, column, row, text_input};
use iced::{Alignment, Element, Length, Task};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Id;
use super::agent;
use super::nodes::Load;
use crate::console::{Ctx, client, ui};

/// The placeholder the format must carry; the hub puts the secret's value in its place.
const PLACEHOLDER: &str = "${SECRET}";
/// Headers a rule may not set: hop-by-hop headers, and the ones that frame or route the request itself.
const REFUSED_HEADERS: [&str; 11] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

/// One rule, as `GET /v1/egress/inject` lists it. A missing field takes the hub's default.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub id: String,
    /// The roles the rule applies to; empty means every role.
    pub roles: Vec<String>,
    /// An exact host, or `*.suffix` for its subdomains.
    pub host: String,
    pub port: u16,
    /// The methods the rule applies to; empty means any method.
    pub methods: Vec<String>,
    pub path_prefix: String,
    pub header: String,
    /// The header's value, with `${SECRET}` where the secret's value goes.
    pub format: String,
    /// The vault secret's name.
    pub secret: String,
    pub enabled: bool,
    pub created_ms: u64,
}

impl Default for Rule {
    fn default() -> Self {
        Rule {
            id: String::new(),
            roles: Vec::new(),
            host: String::new(),
            port: 443,
            methods: Vec::new(),
            path_prefix: "/".to_string(),
            header: String::new(),
            format: "Bearer ${SECRET}".to_string(),
            secret: String::new(),
            enabled: true,
            created_ms: 0,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RuleList {
    rules: Vec<Rule>,
}

/// The add form's fields as typed. `Default` is the form's starting values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub host: String,
    pub port: String,
    pub roles: String,
    pub methods: String,
    pub path_prefix: String,
    pub header: String,
    pub format: String,
    pub secret: String,
}

impl Default for Draft {
    fn default() -> Self {
        Draft {
            host: String::new(),
            port: "443".to_string(),
            roles: String::new(),
            methods: String::new(),
            path_prefix: "/".to_string(),
            header: "Authorization".to_string(),
            format: "Bearer ${SECRET}".to_string(),
            secret: String::new(),
        }
    }
}

/// One field of the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Host,
    Port,
    Roles,
    Methods,
    PathPrefix,
    Header,
    Format,
    Secret,
}

impl Draft {
    fn set(&mut self, field: Field, value: String) {
        match field {
            Field::Host => self.host = value,
            Field::Port => self.port = value,
            Field::Roles => self.roles = value,
            Field::Methods => self.methods = value,
            Field::PathPrefix => self.path_prefix = value,
            Field::Header => self.header = value,
            Field::Format => self.format = value,
            Field::Secret => self.secret = value,
        }
    }
}

/// The request body for `POST /v1/egress/inject` from the draft, or the first thing the hub would refuse in it.
pub fn rule_body(draft: &Draft) -> Result<Value, String> {
    let host = draft.host.trim().trim_end_matches('.').to_ascii_lowercase();
    check_host(&host)?;
    let port: u16 = draft
        .port
        .trim()
        .parse()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| "the port is 1 to 65535".to_string())?;
    // A credential goes only inside TLS: over plain HTTP it would cross the network readable.
    if port == 80 {
        return Err("a credential is only added to HTTPS: the port cannot be 80".to_string());
    }
    let methods: Vec<String> = split_list(&draft.methods)
        .into_iter()
        .map(|method| method.to_ascii_uppercase())
        .collect();
    if methods.iter().any(|method| !is_token(method)) {
        return Err("a method is a token such as GET".to_string());
    }
    let path = draft.path_prefix.trim();
    if !path.starts_with('/') || path.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the path prefix starts with / and has no spaces".to_string());
    }
    let header = draft.header.trim();
    if !is_token(header) {
        return Err("the header is a name such as Authorization".to_string());
    }
    if REFUSED_HEADERS
        .iter()
        .any(|refused| refused.eq_ignore_ascii_case(header))
    {
        return Err(format!(
            "{header} is a hop-by-hop or framing header; a rule may not set it"
        ));
    }
    if !draft.format.contains(PLACEHOLDER) {
        return Err("the format must contain ${SECRET}".to_string());
    }
    if draft.format.chars().any(char::is_control) {
        return Err("the format has no control characters".to_string());
    }
    let secret = draft.secret.trim();
    if secret.is_empty() {
        return Err("a rule names a vault secret".to_string());
    }
    Ok(json!({
        "roles": split_list(&draft.roles),
        "host": host,
        "port": port,
        "methods": methods,
        "path_prefix": path,
        "header": header,
        "format": draft.format,
        "secret": secret,
        "enabled": true,
    }))
}

/// A plain name (`example.com`) or `*.example.com`; no scheme, path, port or other character.
fn check_host(host: &str) -> Result<(), String> {
    let bare = host.strip_prefix("*.").unwrap_or(host);
    let plain = !bare.is_empty()
        && !bare.starts_with('.')
        && !bare.ends_with('.')
        && bare
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
    if plain {
        Ok(())
    } else {
        Err(
            "the host is a name such as example.com or *.example.com (no scheme, path or port)"
                .to_string(),
        )
    }
}

/// An RFC 7230 token: the characters a header name or a method may hold.
fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// A comma-separated list, trimmed, with the empty items dropped.
fn split_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// The rule in one line: where it applies, and what header it sets. The secret's value never appears; only its name.
pub fn summary(rule: &Rule) -> String {
    let roles = if rule.roles.is_empty() {
        "all roles".to_string()
    } else {
        rule.roles.join(", ")
    };
    let methods = if rule.methods.is_empty() {
        "any method".to_string()
    } else {
        rule.methods.join(", ")
    };
    format!(
        "{} · {} · {} · {}: {} · secret {}",
        rule.path_prefix, methods, roles, rule.header, rule.format, rule.secret
    )
}

#[derive(Debug, Default)]
pub struct State {
    pub rules: Load<Vec<Rule>>,
    /// The add form is open while true; the draft is its typed fields.
    pub adding: bool,
    pub draft: Draft,
    /// A write is on its way to the hub.
    pub busy: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    /// The rule waiting for its delete confirmation.
    pub confirm_delete: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Vec<Rule>, String>),
    OpenAdd,
    CloseAdd,
    Edit(Field, String),
    Create,
    Created(Result<(), String>),
    /// Turns a rule on or off.
    Toggle {
        id: String,
        on: bool,
    },
    /// A rule was changed or deleted: its result and the line to confirm it with.
    Acted(Result<(), String>, String),
    AskDelete(String),
    CancelDelete,
    Delete(String),
}

pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

fn settle<T>(result: Result<T, String>) -> Load<T> {
    match result {
        Ok(value) => Load::Ready(value),
        Err(_) => Load::Failed,
    }
}

/// Loads the rules (called when the screen opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || {
            client
                .get::<RuleList>("/v1/egress/inject")
                .map(|list| list.rules)
        },
        Msg::Loaded,
    )
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(result) => {
            state.rules = settle(result);
            Task::none()
        }
        Msg::OpenAdd => {
            state.adding = true;
            state.draft = Draft::default();
            state.error = None;
            Task::none()
        }
        Msg::CloseAdd => {
            state.adding = false;
            state.error = None;
            Task::none()
        }
        Msg::Edit(field, value) => {
            state.draft.set(field, value);
            Task::none()
        }
        Msg::Create => {
            if state.busy {
                return Task::none();
            }
            let body = match rule_body(&state.draft) {
                Ok(body) => body,
                Err(error) => {
                    state.error = Some(error);
                    return Task::none();
                }
            };
            state.busy = true;
            state.error = None;
            let client = ctx.client.clone();
            client::task(
                move || client.post::<Value>("/v1/egress/inject", &body).map(|_| ()),
                Msg::Created,
            )
        }
        Msg::Created(Ok(())) => {
            state.busy = false;
            state.adding = false;
            state.draft = Draft::default();
            state.error = None;
            state.notice = Some("Rule added".to_string());
            load(state, ctx)
        }
        Msg::Created(Err(error)) => {
            state.busy = false;
            state.error = Some(error);
            Task::none()
        }
        Msg::Toggle { id, on } => {
            if state.busy {
                return Task::none();
            }
            state.busy = true;
            let path = format!("/v1/egress/inject/{id}");
            let body = json!({ "enabled": on });
            let notice = if on { "Rule on" } else { "Rule off" };
            let client = ctx.client.clone();
            client::task(
                move || client.patch::<Value>(&path, &body).map(|_| ()),
                move |result| Msg::Acted(result, notice.to_string()),
            )
        }
        Msg::Acted(result, notice) => {
            state.busy = false;
            match result {
                Ok(()) => {
                    state.error = None;
                    state.notice = Some(notice);
                }
                Err(error) => {
                    state.notice = None;
                    state.error = Some(error);
                }
            }
            load(state, ctx)
        }
        Msg::AskDelete(id) => {
            state.confirm_delete = Some(id);
            state.error = None;
            Task::none()
        }
        Msg::CancelDelete => {
            state.confirm_delete = None;
            Task::none()
        }
        Msg::Delete(id) => {
            if state.busy {
                return Task::none();
            }
            state.busy = true;
            state.confirm_delete = None;
            let path = format!("/v1/egress/inject/{id}");
            let client = ctx.client.clone();
            client::task(
                move || client.delete(&path).map(|_| ()),
                |result| Msg::Acted(result, "Rule deleted".to_string()),
            )
        }
    }
}

// ---- The view ---------------------------------------------------------------------------------------------

/// The button or checkbox's message, unless a write is running.
fn gate(busy: bool, msg: Msg) -> Option<Msg> {
    if busy { None } else { Some(msg) }
}

pub fn view(state: &State) -> Element<'_, Msg> {
    let mut parts: Vec<Element<'_, Msg>> = vec![
        ui::faint("The hub adds this header to the agent's HTTPS requests to the host; the secret never enters the VM. Only hosts with a rule are intercepted.")
            .into(),
    ];
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error));
    }
    if let Some(notice) = &state.notice {
        parts.push(ui::faint(notice.clone()).into());
    }
    if state.adding {
        parts.push(ui::card("Add a rule", add_form(state)));
    }
    parts.push(ui::card("Rules", rules_view(state)));
    let head = ui::view_head(
        "Egress credentials",
        vec![ui::btn(
            "Add a rule",
            None,
            ui::Kind::Primary,
            (!state.adding).then_some(Msg::OpenAdd),
        )],
    );
    ui::sheet(head, column(parts).spacing(22).width(Length::Fill).into())
}

/// One labelled field of the add form, with a line on what it does.
fn field<'a>(
    label: &str,
    placeholder: &str,
    value: &'a str,
    hint: &str,
    which: Field,
) -> Element<'a, Msg> {
    column![
        ui::label(label),
        text_input(placeholder, value)
            .on_input(move |text| Msg::Edit(which, text))
            .padding(8.0)
            .style(crate::theme::input),
        ui::faint(hint.to_string()),
    ]
    .spacing(6)
    .into()
}

fn add_form(state: &State) -> Element<'_, Msg> {
    let d = &state.draft;
    column![
        field(
            "Host",
            "api.example.com or *.example.com",
            &d.host,
            "The name the request goes to. Only this host gets the header.",
            Field::Host,
        ),
        field(
            "Port",
            "443",
            &d.port,
            "The HTTPS port. 80 is refused: a credential is only added over TLS.",
            Field::Port,
        ),
        field(
            "Roles",
            "coder, writer",
            &d.roles,
            "Comma-separated. Empty means every role.",
            Field::Roles,
        ),
        field(
            "Methods",
            "GET, POST",
            &d.methods,
            "Comma-separated. Empty means any method.",
            Field::Methods,
        ),
        field(
            "Path prefix",
            "/",
            &d.path_prefix,
            "Only requests whose path starts with this.",
            Field::PathPrefix,
        ),
        field(
            "Header",
            "Authorization",
            &d.header,
            "The header the hub sets on the request.",
            Field::Header,
        ),
        field(
            "Format",
            "Bearer ${SECRET}",
            &d.format,
            "${SECRET} is replaced by the secret's value when the request goes out.",
            Field::Format,
        ),
        field(
            "Secret",
            "github_token",
            &d.secret,
            "The vault entry's name. It must be bound to the egress consumer.",
            Field::Secret,
        ),
        row![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseAdd)),
            ui::btn(
                "Add rule",
                None,
                ui::Kind::Primary,
                gate(state.busy, Msg::Create)
            ),
        ]
        .spacing(8),
    ]
    .spacing(12)
    .width(Length::Fill)
    .into()
}

fn rules_view(state: &State) -> Element<'_, Msg> {
    match &state.rules {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the rules.", ""),
        Load::Ready(list) if list.is_empty() => ui::empty(
            "external",
            "No rules yet",
            "Add a rule to have the hub add a vault credential to an agent's request to a host.",
        ),
        Load::Ready(list) => column(
            list.iter()
                .map(|rule| rule_row(state, rule))
                .collect::<Vec<_>>(),
        )
        .spacing(16)
        .width(Length::Fill)
        .into(),
    }
}

fn rule_row<'a>(state: &State, rule: &Rule) -> Element<'a, Msg> {
    let (badge, tone) = if rule.enabled {
        ("on", ui::Tone::Ok)
    } else {
        ("off", ui::Tone::Muted)
    };
    let head: Vec<Element<'a, Msg>> = vec![
        ui::mono(
            format!("{}:{}", rule.host, rule.port),
            12.0,
            crate::theme::palette().ink,
        )
        .into(),
        ui::badge(badge, tone),
    ];
    let id = rule.id.clone();
    let enabled = checkbox(rule.enabled)
        .label("Enabled")
        .on_toggle(move |on| Msg::Toggle { id: id.clone(), on });
    let actions: Element<'a, Msg> = if state.confirm_delete.as_deref() == Some(rule.id.as_str()) {
        let confirm: Vec<Element<'a, Msg>> = vec![
            ui::faint(format!(
                "Delete the rule for {}? Its requests stop getting the header at once.",
                rule.host
            ))
            .into(),
            ui::btn_sm("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelDelete)),
            ui::btn_sm(
                "Delete",
                None,
                ui::Kind::Danger,
                gate(state.busy, Msg::Delete(rule.id.clone())),
            ),
        ];
        row(confirm).spacing(10).align_y(Alignment::Center).into()
    } else {
        ui::btn_sm(
            "Delete",
            None,
            ui::Kind::Danger,
            Some(Msg::AskDelete(rule.id.clone())),
        )
    };
    column![
        row(head).spacing(10).align_y(Alignment::Center),
        ui::faint(format!(
            "{} · added {}",
            summary(rule),
            agent::ago(rule.created_ms)
        )),
        row![enabled, actions]
            .spacing(16)
            .align_y(Alignment::Center),
    ]
    .spacing(8)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(host: &str) -> Draft {
        Draft {
            host: host.to_string(),
            secret: "github_token".to_string(),
            ..Draft::default()
        }
    }

    #[test]
    fn egress_rules_parse_with_the_hubs_defaults_and_from_the_owner_api() {
        {
            // egress_rule_parses_with_the_hubs_defaults
            let rule: Rule = serde_json::from_str(r#"{"id":"r1","host":"api.example.com"}"#)
                .expect("a rule parses");
            assert_eq!(rule.port, 443);
            assert_eq!(rule.path_prefix, "/");
            assert_eq!(rule.format, "Bearer ${SECRET}");
            assert!(rule.enabled, "a rule with no enabled flag is on");
            assert!(rule.roles.is_empty() && rule.methods.is_empty());
        }
        {
            // egress_rule_list_parses_from_the_owner_api
            let list: RuleList = serde_json::from_str(
                r#"{"rules":[{"id":"r2","roles":["coder"],"host":"*.example.com","port":8443,
                    "methods":["GET"],"path_prefix":"/v1","header":"X-Api-Key","format":"${SECRET}",
                    "secret":"k","enabled":false,"created_ms":9}]}"#,
            )
            .expect("the list parses");
            let rule = &list.rules[0];
            assert_eq!(rule.host, "*.example.com");
            assert_eq!(rule.port, 8443);
            assert_eq!(rule.roles, vec!["coder".to_string()]);
            assert!(!rule.enabled);
            assert_eq!(rule.created_ms, 9);
        }
    }

    #[test]
    fn egress_body_carries_the_draft_and_the_hubs_normalization() {
        let mut d = draft("API.Example.com.");
        d.methods = "get, post".to_string();
        d.roles = " coder ,, ".to_string();
        let body = rule_body(&d).expect("a valid draft builds a body");
        assert_eq!(body["host"], "api.example.com");
        assert_eq!(body["port"], 443);
        assert_eq!(body["methods"], json!(["GET", "POST"]));
        assert_eq!(body["roles"], json!(["coder"]));
        assert_eq!(body["header"], "Authorization");
        assert_eq!(body["format"], "Bearer ${SECRET}");
        assert_eq!(body["secret"], "github_token");
        assert_eq!(body["enabled"], true);
    }

    #[test]
    fn egress_summary_names_the_secret_and_never_its_value() {
        let rule = Rule {
            host: "example.com".to_string(),
            header: "Authorization".to_string(),
            secret: "github_token".to_string(),
            ..Rule::default()
        };
        let text = summary(&rule);
        assert!(text.contains("all roles"));
        assert!(text.contains("any method"));
        assert!(text.contains("Bearer ${SECRET}"));
        assert!(text.contains("secret github_token"));
    }
}
