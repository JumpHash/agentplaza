//! Egress credentials: the hub adds a secret to an agent's outbound HTTPS request, so the secret never
//! enters the VM.
//!
//! The owner writes **rules**: for agents of these roles (empty = every role), a request to this host
//! (exact, or `*.suffix`) on this port, with one of these methods (empty = any) and a path under this
//! prefix, gets this header, whose value is the format with `${SECRET}` replaced by a vault secret.
//! First match wins. Rules are owner-only; the secret is named, never shown.
//!
//! How the request is reached: vsockd's egress proxy asks the hub before it dials
//! (`/internal/egress/check`), and the answer says `intercept` only when a rule could match that agent
//! and host — every other host stays an opaque CONNECT tunnel. For an intercepted host the proxy
//! terminates the guest's TLS with a leaf certificate the hub signs ([`leaf_for`], CA in
//! [`ca_for_role`]), opens its own TLS to the real host, and asks the hub per request
//! (`/internal/egress/inject`) which header to add. Each injected request is a transcript entry
//! (host, method, path, rule — never the value). CubeSandbox's egress proxy does the same with
//! OpenResty for every HTTP(S) request; here only the hosts the owner named are opened.
//!
//! GET    /v1/egress/inject            → {rules: [rule]}
//! POST   /v1/egress/inject   rule     → rule
//! PATCH  /v1/egress/inject/{id}       → rule
//! DELETE /v1/egress/inject/{id}       → {ok}
//! POST   /internal/egress/cert   {cid, host}                       → {cert_pem, key_pem}
//! POST   /internal/egress/inject {cid, host, port, method, path}   → {header, value, rule} | {}

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use apz_http::{Params, Request, Response, Router};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, date_time_ymd,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::state::App;
use crate::vault::OnBehalf;

/// The vault consumer a rule's secret is read under. A secret is usable here when a vault entry of
/// its name is bound to this consumer (the same check `Store::value_for` makes).
const BINDING: &str = "egress";
/// What a rule's format writes the secret's value into.
const PLACEHOLDER: &str = "${SECRET}";
/// A rule's port when it names none: HTTPS.
const DEFAULT_PORT: u16 = 443;
/// The owner's rules, at `<state-dir>/egress-inject.json`.
const RULES_FILE: &str = "egress-inject.json";
/// The egress CA's certificate and its key (0600), under the state dir.
const CA_CERT_FILE: &str = "egress-ca.pem";
const CA_KEY_FILE: &str = "egress-ca.key";
const CA_NAME: &str = "AgentPlaza egress CA";
/// The CA's validity in days: about ten years.
const CA_DAYS: i64 = 3_652;
/// A leaf certificate's validity in days. A cached leaf is replaced a day before it ends.
const LEAF_DAYS: i64 = 7;
const DAY_MS: u64 = 86_400_000;
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

/// A refusal to answer a route: its status and a message that never carries a secret's value.
type Refusal = (u16, String);

/// One rule: for agents of `roles`, a request to `host:port` with one of `methods` under `path_prefix`
/// gets `header: format` with the secret's value in `${SECRET}`. `secret` is the vault's name for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub id: String,
    pub roles: Vec<String>,
    pub host: String,
    pub port: u16,
    pub methods: Vec<String>,
    pub path_prefix: String,
    pub header: String,
    pub format: String,
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
            port: DEFAULT_PORT,
            methods: Vec::new(),
            path_prefix: "/".into(),
            header: String::new(),
            format: "Bearer ${SECRET}".into(),
            secret: String::new(),
            enabled: true,
            created_ms: 0,
        }
    }
}

/// The port a proxy question names when it names none.
pub fn default_port() -> u16 {
    DEFAULT_PORT
}

/// Checks a rule and normalizes it: the host lower-cased, the methods upper-cased. `bound` says whether
/// a vault entry of the rule's secret name is bound to the egress consumer; its error is the vault's.
pub(crate) fn validate(
    rule: &mut Rule,
    bound: impl Fn(&str) -> Result<bool, String>,
) -> Result<(), String> {
    rule.host = normalize_host(&rule.host);
    check_host(&rule.host)?;
    if rule.port == 0 {
        return Err("the port is 1 to 65535".into());
    }
    // The proxy adds a credential only inside TLS it terminates: over plain HTTP the request would go
    // out readable by anyone on the path (and to whoever answers for the host's DNS name).
    if rule.port == 80 {
        return Err("a credential is only added to HTTPS: the port cannot be 80".into());
    }
    rule.roles = rule
        .roles
        .iter()
        .map(|role| role.trim().to_string())
        .filter(|role| !role.is_empty())
        .collect();
    rule.methods = rule
        .methods
        .iter()
        .map(|method| method.trim().to_ascii_uppercase())
        .collect();
    if rule.methods.iter().any(|method| !is_token(method)) {
        return Err("a method is a token such as GET".into());
    }
    if !rule.path_prefix.starts_with('/')
        || rule
            .path_prefix
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("the path prefix starts with / and has no spaces".into());
    }
    rule.header = rule.header.trim().to_string();
    if !is_token(&rule.header) {
        return Err("the header is a name such as Authorization".into());
    }
    if REFUSED_HEADERS
        .iter()
        .any(|refused| refused.eq_ignore_ascii_case(&rule.header))
    {
        return Err(format!(
            "{} is a hop-by-hop or framing header; a rule may not set it",
            rule.header
        ));
    }
    if !rule.format.contains(PLACEHOLDER) {
        return Err("the format must contain ${SECRET}".into());
    }
    if rule.format.chars().any(char::is_control) {
        return Err("the format has no control characters".into());
    }
    rule.secret = rule.secret.trim().to_string();
    if rule.secret.is_empty() {
        return Err("a rule names a vault secret".into());
    }
    if !bound(&rule.secret)? {
        return Err(format!(
            "no vault secret named {} is bound to the egress consumer",
            rule.secret
        ));
    }
    Ok(())
}

/// A host the rules compare: lower-case, without a trailing dot.
fn normalize_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
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
                .into(),
        )
    }
}

/// An RFC 7230 token: a header name or a method.
fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// Whether a rule's host (normalized) covers the request's host (normalized). `*.suffix` covers the
/// subdomains of `suffix`, not `suffix` itself.
fn host_matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => host
            .strip_suffix(suffix)
            .and_then(|rest| rest.strip_suffix('.'))
            .is_some_and(|label| !label.is_empty()),
        None => pattern == host,
    }
}

/// Whether the rule's prefix covers the path, whole segments only: `/api` covers `/api` and
/// `/api/x`, not `/apix`. A path with a `.` or `..` segment (also percent-encoded) covers nothing, so
/// the credential is never sent to a path the upstream would resolve elsewhere.
fn path_matches(prefix: &str, path: &str) -> bool {
    let path = path.split('?').next().unwrap_or("");
    let decoded = path.replace("%2e", ".").replace("%2E", ".");
    if decoded
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return false;
    }
    if prefix.ends_with('/') {
        return path.starts_with(prefix);
    }
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn covers_role(rule: &Rule, role: &str) -> bool {
    rule.roles.is_empty() || rule.roles.iter().any(|named| named == role)
}

/// The owner's rules in memory. Matching is first-match over the enabled rules, in the order they
/// were added.
#[derive(Debug, Default, Clone)]
pub(crate) struct RuleSet {
    pub rules: Vec<Rule>,
}

impl RuleSet {
    /// The first enabled rule for an agent of `role` at `host:port`, a `method` to `path`.
    pub fn find(
        &self,
        role: &str,
        host: &str,
        port: u16,
        method: &str,
        path: &str,
    ) -> Option<&Rule> {
        let host = normalize_host(host);
        self.rules.iter().find(|rule| {
            rule.enabled
                && covers_role(rule, role)
                && rule.port == port
                && host_matches(&rule.host, &host)
                && (rule.methods.is_empty()
                    || rule.methods.iter().any(|m| m.eq_ignore_ascii_case(method)))
                && path_matches(&rule.path_prefix, path)
        })
    }

    /// Whether a rule could inject for an agent of `role` at `host` (and `port`, when given). Method and
    /// path are not asked: the proxy intercepts the whole host and asks per request.
    pub fn could_match(&self, role: &str, host: &str, port: Option<u16>) -> bool {
        let host = normalize_host(host);
        self.rules.iter().any(|rule| {
            rule.enabled
                && covers_role(rule, role)
                && port.is_none_or(|port| port == rule.port)
                && host_matches(&rule.host, &host)
        })
    }

    /// Whether any enabled rule names `role` (or every role).
    pub fn covers_role(&self, role: &str) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.enabled && covers_role(rule, role))
    }
}

/// The owner's rules, held in memory and saved to `<state-dir>/egress-inject.json` on every change.
pub(crate) struct Store {
    path: PathBuf,
    /// Why the file could not be read. While it is set no rule applies and nothing is written, so a
    /// damaged file is never replaced by a shorter list.
    unreadable: Option<String>,
    set: Mutex<RuleSet>,
}

impl Store {
    pub fn load(state_dir: &Path) -> Store {
        let path = state_dir.join(RULES_FILE);
        let (rules, unreadable) = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Vec<Rule>>(&bytes) {
                Ok(rules) => (rules, None),
                Err(e) => (Vec::new(), Some(format!("{}: {e}", path.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None),
            Err(e) => (Vec::new(), Some(format!("{}: {e}", path.display()))),
        };
        if let Some(why) = &unreadable {
            eprintln!("apid: the egress rules are unreadable, so none applies ({why})");
        }
        Store {
            path,
            unreadable,
            set: Mutex::new(RuleSet { rules }),
        }
    }

    fn set(&self) -> MutexGuard<'_, RuleSet> {
        self.set.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn list(&self) -> Vec<Rule> {
        self.set().rules.clone()
    }

    fn create(
        &self,
        mut rule: Rule,
        now: u64,
        bound: impl Fn(&str) -> Result<bool, String>,
    ) -> Result<Rule, Refusal> {
        rule.id = new_id();
        rule.created_ms = now;
        validate(&mut rule, bound).map_err(bad_request)?;
        let mut set = self.set();
        let mut next = set.rules.clone();
        next.push(rule.clone());
        self.save(&next)?;
        set.rules = next;
        Ok(rule)
    }

    /// Merges `patch` into the rule `id`; its id and creation time stay.
    fn update(
        &self,
        id: &str,
        patch: Map<String, Value>,
        bound: impl Fn(&str) -> Result<bool, String>,
    ) -> Result<Rule, Refusal> {
        let mut set = self.set();
        let index = set
            .rules
            .iter()
            .position(|rule| rule.id == id)
            .ok_or_else(|| not_found(id))?;
        let mut merged = serde_json::to_value(&set.rules[index]).map_err(internal)?;
        if let Value::Object(base) = &mut merged {
            for (key, value) in patch {
                if key != "id" && key != "created_ms" {
                    base.insert(key, value);
                }
            }
        }
        let mut rule: Rule = serde_json::from_value(merged).map_err(bad_request)?;
        rule.id = id.to_string();
        rule.created_ms = set.rules[index].created_ms;
        validate(&mut rule, bound).map_err(bad_request)?;
        let mut next = set.rules.clone();
        next[index] = rule.clone();
        self.save(&next)?;
        set.rules = next;
        Ok(rule)
    }

    fn remove(&self, id: &str) -> Result<(), Refusal> {
        let mut set = self.set();
        let index = set
            .rules
            .iter()
            .position(|rule| rule.id == id)
            .ok_or_else(|| not_found(id))?;
        let mut next = set.rules.clone();
        next.remove(index);
        self.save(&next)?;
        set.rules = next;
        Ok(())
    }

    fn save(&self, rules: &[Rule]) -> Result<(), Refusal> {
        if let Some(why) = &self.unreadable {
            return Err((
                409,
                format!("the egress rules file is unreadable; fix or remove it first ({why})"),
            ));
        }
        let bytes = serde_json::to_vec_pretty(rules).map_err(internal)?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| (500, format!("{}: {e}", dir.display())))?;
        }
        apz_os::private::write(&self.path, &bytes)
            .map_err(|e| (500, format!("{}: {e}", self.path.display())))
    }
}

fn bad_request(why: impl ToString) -> Refusal {
    (400, why.to_string())
}

fn internal(why: impl ToString) -> Refusal {
    (500, why.to_string())
}

fn not_found(id: &str) -> Refusal {
    (404, format!("no egress rule `{id}`"))
}

/// `egr-` and twelve hex digits from the OS random source.
fn new_id() -> String {
    let bytes = apz_trust::random::<6>();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("egr-{hex}")
}

/// The hub's rules. There is one hub per process, so the store is loaded from its state dir on first
/// use and kept for the process.
static STORE: OnceLock<Store> = OnceLock::new();

fn store(app: &App) -> &'static Store {
    STORE.get_or_init(|| {
        let store = Store::load(&app.config.state_dir);
        register_consumers(app, &store);
        store
    })
}

/// Tells the vault which secrets the egress rules read, so its screen shows each as wanted by
/// "Egress credential for <host>" (and which node lacks it). Called whenever the rules change.
fn register_consumers(app: &App, store: &Store) {
    app.vault.unregister_consumer(BINDING);
    for rule in store.list() {
        app.vault.register_consumer(
            BINDING,
            &format!("Egress credential for {}", rule.host),
            &rule.secret,
        );
    }
}

/// Whether the vault has an entry of `name` bound to the egress consumer.
fn bound_in_vault(app: &App) -> impl Fn(&str) -> Result<bool, String> + '_ {
    move |name| {
        app.vault
            .entries_named(name)
            .map(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.bindings.iter().any(|b| b == BINDING))
            })
            .map_err(|e| e.message())
    }
}

/// The egress CA and the leaf certificates it has signed, one per host.
struct Authority {
    cert_pem: String,
    issuer: rcgen::Certificate,
    issuer_key: KeyPair,
    leaves: Mutex<HashMap<String, Leaf>>,
}

struct Leaf {
    cert_pem: String,
    key_pem: String,
    /// When the leaf's validity ends, in ms since the epoch.
    ends_ms: u64,
}

/// The CA, made or loaded on first need. A failure is kept: the same error answers every later call.
static AUTHORITY: OnceLock<Result<Authority, String>> = OnceLock::new();

fn authority(app: &App) -> Result<&'static Authority, String> {
    AUTHORITY
        .get_or_init(|| Authority::load_or_create(&app.config.state_dir))
        .as_ref()
        .map_err(String::clone)
}

impl Authority {
    /// Loads the CA from the state dir, or makes one there (ECDSA P-256, ten years) when either file is
    /// missing. The key is written 0600 and never logged.
    fn load_or_create(dir: &Path) -> Result<Authority, String> {
        let cert_path = dir.join(CA_CERT_FILE);
        let key_path = dir.join(CA_KEY_FILE);
        let (cert_pem, key_pem) = match (read_optional(&cert_path)?, read_optional(&key_path)?) {
            (Some(cert), Some(key)) => (cert, key),
            _ => {
                let (cert, key) = generate_ca()?;
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                apz_os::private::write(&key_path, key.as_bytes())
                    .map_err(|e| format!("{}: {e}", key_path.display()))?;
                apz_os::private::write(&cert_path, cert.as_bytes())
                    .map_err(|e| format!("{}: {e}", cert_path.display()))?;
                (cert, key)
            }
        };
        let issuer_key = KeyPair::from_pem(&key_pem)
            .map_err(|e| format!("the egress CA key does not load: {e}"))?;
        // Leaves take the CA's name and key from the issuer; the stored certificate (served as the CA)
        // is never re-signed, so this rebuilt copy's validity is never seen.
        let issuer = ca_params()
            .self_signed(&issuer_key)
            .map_err(|e| format!("the egress CA does not rebuild: {e}"))?;
        Ok(Authority {
            cert_pem,
            issuer,
            issuer_key,
            leaves: Mutex::new(HashMap::new()),
        })
    }

    /// A leaf certificate for `host`, signed by this CA, with its key; cached per host until a day
    /// before it ends.
    fn leaf_for(&self, host: &str, now: u64) -> Result<(String, String), String> {
        let mut leaves = self.leaves.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(leaf) = leaves.get(host) {
            if now + DAY_MS < leaf.ends_ms {
                return Ok((leaf.cert_pem.clone(), leaf.key_pem.clone()));
            }
        }
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|e| e.to_string())?;
        let mut params =
            CertificateParams::new(vec![host.to_string()]).map_err(|e| e.to_string())?;
        params.distinguished_name.push(DnType::CommonName, host);
        params.is_ca = IsCa::NoCa;
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let today = days_of(now);
        let (y, m, d) = ymd_of(today - 1);
        params.not_before = date_time_ymd(y, m, d);
        let (y, m, d) = ymd_of(today + LEAF_DAYS);
        params.not_after = date_time_ymd(y, m, d);
        let cert = params
            .signed_by(&key, &self.issuer, &self.issuer_key)
            .map_err(|e| e.to_string())?;
        let leaf = Leaf {
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
            ends_ms: (today + LEAF_DAYS) as u64 * DAY_MS,
        };
        let answer = (leaf.cert_pem.clone(), leaf.key_pem.clone());
        leaves.insert(host.to_string(), leaf);
        Ok(answer)
    }
}

fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The CA's name and constraints (its validity is set by whoever signs it).
fn ca_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    params.distinguished_name.push(DnType::CommonName, CA_NAME);
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params
}

/// A new self-signed CA: (certificate PEM, key PEM).
fn generate_ca() -> Result<(String, String), String> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|e| e.to_string())?;
    let mut params = ca_params();
    let today = days_of(apz_plaza::now_ms());
    let (y, m, d) = ymd_of(today - 1);
    params.not_before = date_time_ymd(y, m, d);
    let (y, m, d) = ymd_of(today + CA_DAYS);
    params.not_after = date_time_ymd(y, m, d);
    let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// Whole days since the epoch of `ms`.
fn days_of(ms: u64) -> i64 {
    (ms / DAY_MS) as i64
}

/// The UTC calendar date of `day` days since the epoch (proleptic Gregorian; Howard Hinnant's
/// civil_from_days).
fn ymd_of(day: i64) -> (i32, u8, u8) {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y as i32, m as u8, d as u8)
}

/// The role of agent `cid`; `None` when the roster does not know it.
fn role_of(app: &App, cid: u64) -> Option<String> {
    app.agents.get(cid).map(|agent| agent.role)
}

/// Whether agent `cid` may have a rule for `host` (and `port`, when given).
fn covers(app: &App, cid: u64, host: &str, port: Option<u16>) -> bool {
    role_of(app, cid).is_some_and(|role| store(app).set().could_match(&role, host, port))
}

/// Whether the proxy should open agent `cid`'s connection to `host:port` (a rule could match).
pub fn intercepts(app: &App, cid: u64, host: &str, port: u16) -> bool {
    covers(app, cid, host, Some(port))
}

/// The egress CA certificate (PEM) agents of `role` must trust: `Some` only when a rule could inject
/// for that role (see the module docs).
pub fn ca_for_role(app: &App, role: &str) -> Option<String> {
    if !store(app).set().covers_role(role) {
        return None;
    }
    match authority(app) {
        Ok(ca) => Some(ca.cert_pem.clone()),
        Err(why) => {
            eprintln!("apid: the egress CA is unavailable, so no agent gets it ({why})");
            None
        }
    }
}

/// `POST /internal/egress/cert`: the leaf certificate and key for `host`, when agent `cid` is
/// intercepted there. The key is the answer's only copy; nothing logs it.
pub(crate) fn cert_reply(app: &App, cid: u64, host: &str) -> Response {
    if !covers(app, cid, host, None) {
        return Response::error(403, "that host is not intercepted for this agent");
    }
    let ca = match authority(app) {
        Ok(ca) => ca,
        Err(why) => return Response::error(503, &why),
    };
    match ca.leaf_for(&normalize_host(host), apz_plaza::now_ms()) {
        Ok((cert_pem, key_pem)) => {
            Response::json(200, &json!({ "cert_pem": cert_pem, "key_pem": key_pem }))
        }
        Err(why) => Response::error(500, &why),
    }
}

/// A request the egress proxy is about to send for an intercepted host.
#[derive(Debug, Deserialize)]
pub struct Ask {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub method: String,
    pub path: String,
}

/// `POST /internal/egress/inject`: the header to add to this request, or `{}` when no rule matches.
/// A matched rule is a transcript entry (host, method, path, rule; never the value). A secret the
/// hub cannot read answers 503, so the proxy can refuse the request rather than send it bare.
pub(crate) fn inject_reply(app: &App, cid: u64, ask: &Ask) -> Response {
    let Some(role) = role_of(app, cid) else {
        return Response::json(200, &json!({}));
    };
    let found = store(app)
        .set()
        .find(&role, &ask.host, ask.port, &ask.method, &ask.path)
        .cloned();
    let Some(rule) = found else {
        return Response::json(200, &json!({}));
    };
    let Some(secret) = app.vault.value_for(&rule.secret, BINDING, &OnBehalf::Hub) else {
        return Response::json(
            503,
            &json!({ "error": format!("the hub cannot read the secret {} of rule {}", rule.secret, rule.id) }),
        );
    };
    let value = rule.format.replace(PLACEHOLDER, &secret);
    if value.chars().any(char::is_control) {
        return Response::json(
            503,
            &json!({ "error": format!("the secret {} of rule {} has a control character", rule.secret, rule.id) }),
        );
    }
    let path = ask.path.split('?').next().unwrap_or("");
    app.transcripts.record(
        &app.bus,
        cid,
        0,
        "egress_inject",
        json!({
            "host": normalize_host(&ask.host),
            "method": ask.method.to_ascii_uppercase(),
            "path": path,
            "rule": rule.id,
        }),
    );
    Response::json(
        200,
        &json!({ "header": rule.header, "value": value, "rule": rule }),
    )
}

/// The egress rules' routes, listed in the module docs. Owner-only: `hub_auth::gate` asks for the
/// owner token of every `/v1/` request that does not come from loopback.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/egress/inject", list_route)
        .post("/v1/egress/inject", create_route)
        .route("PATCH", "/v1/egress/inject/{id}", update_route)
        .delete("/v1/egress/inject/{id}", delete_route)
}

fn list_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    Response::json(200, &json!({ "rules": store(app).list() }))
}

fn create_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let rule: Rule = match req.json() {
        Ok(rule) => rule,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    match store(app).create(rule, apz_plaza::now_ms(), bound_in_vault(app)) {
        Ok(rule) => {
            register_consumers(app, store(app));
            Response::json(200, &rule)
        }
        Err((status, why)) => Response::error(status, &why),
    }
}

fn update_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let Ok(Value::Object(patch)) = req.json::<Value>() else {
        return Response::error(400, "the body must be a JSON object");
    };
    match store(app).update(p.get("id"), patch, bound_in_vault(app)) {
        Ok(rule) => {
            register_consumers(app, store(app));
            Response::json(200, &rule)
        }
        Err((status, why)) => Response::error(status, &why),
    }
}

fn delete_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match store(app).remove(p.get("id")) {
        Ok(()) => {
            register_consumers(app, store(app));
            Response::json(200, &json!({ "ok": true }))
        }
        Err((status, why)) => Response::error(status, &why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(host: &str) -> Rule {
        Rule {
            host: host.into(),
            header: "Authorization".into(),
            secret: "GITHUB_TOKEN".into(),
            ..Rule::default()
        }
    }

    fn known(name: &str) -> Result<bool, String> {
        Ok(name == "GITHUB_TOKEN")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apid-egress-inject-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn set_of(rules: Vec<Rule>) -> RuleSet {
        RuleSet { rules }
    }

    #[test]
    fn a_valid_rule_is_normalized_and_its_secret_must_be_known() {
        let mut ok = Rule {
            host: "API.Example.com.".into(),
            methods: vec!["get".into(), " post ".into()],
            roles: vec![" builder ".into(), "".into()],
            ..rule("x")
        };
        validate(&mut ok, known).unwrap();
        assert_eq!(ok.host, "api.example.com");
        assert_eq!(ok.methods, ["GET", "POST"]);
        assert_eq!(ok.roles, ["builder"]);

        let mut unknown = rule("example.com");
        unknown.secret = "NOT_IN_VAULT".into();
        let err = validate(&mut unknown, known).unwrap_err();
        assert!(err.contains("NOT_IN_VAULT") && !err.contains("GITHUB_TOKEN"));
        let mut vault_sealed = rule("example.com");
        assert!(validate(&mut vault_sealed, |_| Err("sealed".into())).is_err());
    }

    #[test]
    fn a_rule_that_could_leak_or_misroute_a_credential_is_refused() {
        let refused: Vec<(&str, Rule)> = vec![
            ("empty host", rule("")),
            ("scheme", rule("https://example.com")),
            ("path", rule("example.com/x")),
            ("port in host", rule("example.com:8443")),
            ("bare star", rule("*")),
            ("star in the middle", rule("a*.example.com")),
            (
                "zero port",
                Rule {
                    port: 0,
                    ..rule("example.com")
                },
            ),
            (
                "header not a token",
                Rule {
                    header: "Auth orization".into(),
                    ..rule("example.com")
                },
            ),
            (
                "hop-by-hop header",
                Rule {
                    header: "Connection".into(),
                    ..rule("example.com")
                },
            ),
            (
                "host header",
                Rule {
                    header: "host".into(),
                    ..rule("example.com")
                },
            ),
            (
                "format without the placeholder",
                Rule {
                    format: "Bearer x".into(),
                    ..rule("example.com")
                },
            ),
            (
                "format with a line break",
                Rule {
                    format: "Bearer ${SECRET}\r\nX: y".into(),
                    ..rule("example.com")
                },
            ),
            (
                "method not a token",
                Rule {
                    methods: vec!["GE T".into()],
                    ..rule("example.com")
                },
            ),
            (
                "path without a slash",
                Rule {
                    path_prefix: "api".into(),
                    ..rule("example.com")
                },
            ),
            (
                "no secret",
                Rule {
                    secret: " ".into(),
                    ..rule("example.com")
                },
            ),
        ];
        for (why, mut bad) in refused {
            assert!(validate(&mut bad, known).is_err(), "accepted: {why}");
        }
    }

    #[test]
    fn a_wildcard_covers_subdomains_and_never_the_apex() {
        let set = set_of(vec![rule("*.example.com")]);
        assert!(set.find("", "api.example.com", 443, "GET", "/").is_some());
        assert!(set.find("", "A.B.Example.COM.", 443, "GET", "/").is_some());
        assert!(set.find("", "example.com", 443, "GET", "/").is_none());
        assert!(set.find("", "evilexample.com", 443, "GET", "/").is_none());
        assert!(
            set.find("", "example.com.evil.net", 443, "GET", "/")
                .is_none()
        );
        let exact = set_of(vec![rule("api.example.com")]);
        assert!(exact.find("", "api.example.com", 443, "GET", "/").is_some());
        assert!(
            exact
                .find("", "x.api.example.com", 443, "GET", "/")
                .is_none()
        );
    }

    #[test]
    fn the_port_the_methods_and_the_path_narrow_a_match() {
        let mut narrow = rule("example.com");
        narrow.port = 8443;
        narrow.methods = vec!["POST".into()];
        narrow.path_prefix = "/v1".into();
        let set = set_of(vec![narrow]);
        assert!(set.find("", "example.com", 8443, "post", "/v1/x").is_some());
        assert!(set.find("", "example.com", 443, "POST", "/v1/x").is_none());
        assert!(set.find("", "example.com", 8443, "GET", "/v1/x").is_none());
        assert!(set.find("", "example.com", 8443, "POST", "/v10").is_none());
        assert!(set.find("", "example.com", 8443, "POST", "/v1").is_some());
        assert!(
            set.find("", "example.com", 8443, "POST", "/v1?q=1")
                .is_some()
        );
        assert!(
            set.find("", "example.com", 8443, "POST", "/v1/../admin")
                .is_none()
        );
        assert!(
            set.find("", "example.com", 8443, "POST", "/v1/%2e%2e/admin")
                .is_none()
        );
        let any_method = set_of(vec![rule("example.com")]);
        assert!(
            any_method
                .find("", "example.com", 443, "DELETE", "/")
                .is_some()
        );
    }

    #[test]
    fn the_roles_decide_who_a_rule_is_for() {
        let mut builders = rule("example.com");
        builders.roles = vec!["builder".into()];
        let everyone = rule("example.com");
        let set = set_of(vec![builders.clone()]);
        assert!(
            set.find("builder", "example.com", 443, "GET", "/")
                .is_some()
        );
        assert!(
            set.find("researcher", "example.com", 443, "GET", "/")
                .is_none()
        );
        assert!(set.could_match("builder", "example.com", None));
        assert!(!set.could_match("researcher", "example.com", None));
        assert!(set.covers_role("builder"));
        assert!(!set.covers_role("researcher"));
        let open = set_of(vec![everyone]);
        assert!(open.covers_role("anyone"));
        assert!(open.could_match("anyone", "example.com", Some(443)));
        assert!(!open.could_match("anyone", "example.com", Some(8443)));
    }

    #[test]
    fn a_disabled_rule_is_skipped_and_the_first_match_wins() {
        let mut off = rule("example.com");
        off.enabled = false;
        off.header = "X-Off".into();
        let mut first = rule("example.com");
        first.path_prefix = "/a".into();
        first.header = "X-First".into();
        let second = rule("example.com");
        let set = set_of(vec![off.clone(), first, second]);
        let hit = set.find("", "example.com", 443, "GET", "/a/b").unwrap();
        assert_eq!(hit.header, "X-First");
        let other = set.find("", "example.com", 443, "GET", "/b").unwrap();
        assert_eq!(other.header, "Authorization");
        assert!(
            set_of(vec![off])
                .find("", "example.com", 443, "GET", "/")
                .is_none()
        );
    }

    #[test]
    fn the_rules_survive_a_restart_and_a_damaged_file_is_never_replaced() {
        let dir = scratch("rules");
        let store = Store::load(&dir);
        let created = store
            .create(rule("example.com"), 1_000, known)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(created.id.starts_with("egr-") && created.created_ms == 1_000);
        let patched = store
            .update(
                &created.id,
                json!({"enabled": false, "id": "ignored"})
                    .as_object()
                    .unwrap()
                    .clone(),
                known,
            )
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!patched.enabled && patched.id == created.id);

        let reloaded = Store::load(&dir);
        assert_eq!(reloaded.list(), vec![patched.clone()]);
        reloaded.remove(&created.id).unwrap();
        assert!(Store::load(&dir).list().is_empty());

        std::fs::write(dir.join(RULES_FILE), b"{not json").unwrap();
        let damaged = Store::load(&dir);
        assert!(damaged.list().is_empty());
        let refused = damaged.create(rule("example.com"), 1, known).unwrap_err();
        assert_eq!(refused.0, 409);
        assert_eq!(std::fs::read(dir.join(RULES_FILE)).unwrap(), b"{not json");
    }

    #[test]
    fn an_unknown_rule_id_is_a_404() {
        let dir = scratch("unknown");
        let store = Store::load(&dir);
        assert_eq!(store.remove("egr-none").unwrap_err().0, 404);
        let patch = Map::new();
        assert_eq!(store.update("egr-none", patch, known).unwrap_err().0, 404);
    }

    #[test]
    fn the_ca_is_made_once_and_its_key_is_private() {
        let dir = scratch("ca");
        let first = Authority::load_or_create(&dir).unwrap();
        let again = Authority::load_or_create(&dir).unwrap();
        assert_eq!(first.cert_pem, again.cert_pem);
        assert!(first.cert_pem.starts_with("-----BEGIN CERTIFICATE-----"));
        let key_path = dir.join(CA_KEY_FILE);
        assert!(KeyPair::from_pem(&std::fs::read_to_string(&key_path).unwrap()).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_leaf_is_signed_for_its_host_cached_and_renewed_a_day_before_it_ends() {
        let dir = scratch("leaf");
        let ca = Authority::load_or_create(&dir).unwrap();
        let now = 1_800_000_000_000_u64;
        let (cert, key) = ca.leaf_for("api.example.com", now).unwrap();
        assert!(cert.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(KeyPair::from_pem(&key).is_ok());
        assert_ne!(key, ca.issuer_key.serialize_pem());
        let (cached, _) = ca.leaf_for("api.example.com", now + DAY_MS).unwrap();
        assert_eq!(cached, cert);
        let (renewed, _) = ca
            .leaf_for("api.example.com", now + 6 * DAY_MS + DAY_MS / 2)
            .unwrap();
        assert_ne!(renewed, cert);
    }
}
