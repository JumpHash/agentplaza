//! Sanctions for the plaza's law violations, by the schedule in `docs/codex-draft/sanctions.yaml`.
//!
//! `laws.rs` records a violation and hands it to the violation sink, which `db::wire_violations`
//! chains to [`on_violation`]. The schedule picks the steps by the law's severity and the agent's
//! offence count (confirmed violations of the same law in the last 30 days, this one included).
//! Code applies them, never the agent:
//!
//! - `warning`: a record on the agent's profile, a note in its memory and a `sanction` bus event.
//! - `fine:N`: a `Fine` entry from the agent's earnings to the treasury. Capped by `max_fine` and by
//!   [`WEEKLY_FINE_CAP`] over seven days; a fine the wallet cannot pay is recorded as not collected.
//! - `probation:D` and `lockout:D`: the offending tool is suspended for D (the gate refuses it, see
//!   [`suspended`]), and the owner is told.
//! - `demotion`, `retirement`, `owner-review`: the owner is told. apid does not carry them out.
//!
//! Classification is always `negligent` (the schedule as written): no law carries a
//! classification annotation yet, so an `induced` or `malicious` case cannot be told apart.
//!
//! The state lives in `<state-dir>/sanctions.json`. Only apid's one process reads it.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use apz_plaza::treasury::Treasury;
use serde::{Deserialize, Serialize};

use crate::events::Bus;
use crate::laws::Violation;
use crate::memory::Memory;
use crate::settings::{Sanctions, FINES_PER_WEEK_MAX};
use crate::state::App;

/// The most one agent may be fined in seven days (sanctions.yaml, `caps.max_fine_per_week`).
pub const WEEKLY_FINE_CAP: u64 = FINES_PER_WEEK_MAX;
/// Applied sanctions kept; the oldest go first.
const KEEP: usize = 1000;

const MINUTE_MS: u64 = 60_000;
const DAY_MS: u64 = 24 * 60 * MINUTE_MS;
const WEEK_MS: u64 = 7 * DAY_MS;

/// The schedule, row by severity, column by offence count 1, 2 and 3 (3 and up use the third).
/// Each cell is a sanctions.yaml cell verbatim: steps separated by "; ".
const SCHEDULE: [(&str, [&str; 3]); 5] = [
    ("info", ["warning", "warning", "fine:50"]),
    ("low", ["warning", "fine:100", "fine:200; probation:3d"]),
    ("medium", ["warning; fine:100", "fine:300; probation:7d", "lockout:3d; demotion"]),
    ("high", ["fine:300; probation:7d", "lockout:7d; demotion", "retirement; owner-review"]),
    ("critical", ["lockout:14d; owner-review", "retirement; owner-review", "retirement; owner-review"]),
];

/// A law's severity on the schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    /// The schedule's severity for a law's `severity` keyword. An unknown one is `info`, the mildest.
    pub fn of(law_severity: &str) -> Severity {
        match law_severity {
            "critical" => Severity::Critical,
            "high" => Severity::High,
            "medium" | "major" => Severity::Medium,
            "low" | "minor" => Severity::Low,
            _ => Severity::Info,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
}

/// One step of a sanction, as the schedule words it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Warning,
    /// Credits to take from the agent's earnings, before the caps.
    Fine(u64),
    /// The offending tool is refused for `ms` (probation and lockout).
    Suspend { ms: u64, label: String },
    /// Named by the schedule but not carried out here: the owner is told.
    Escalate(String),
}

/// The steps a severity gets at an offence count (1, 2, and 3 for every count from 3 on).
pub fn steps(severity: Severity, count: usize) -> Vec<Step> {
    let column = count.clamp(1, 3) - 1;
    SCHEDULE
        .iter()
        .find(|(key, _)| *key == severity.key())
        .map_or_else(Vec::new, |(_, row)| row[column].split("; ").flat_map(parse_step).collect())
}

fn parse_step(text: &str) -> Vec<Step> {
    let (name, arg) = text.split_once(':').unwrap_or((text, ""));
    match name {
        "warning" => vec![Step::Warning],
        "fine" => match arg.parse() {
            Ok(credits) => vec![Step::Fine(credits)],
            Err(_) => vec![Step::Escalate(text.to_string())],
        },
        "probation" | "lockout" => match duration_ms(arg) {
            Some(ms) => vec![Step::Suspend { ms, label: arg.to_string() }, Step::Escalate(text.to_string())],
            None => vec![Step::Escalate(text.to_string())],
        },
        _ => vec![Step::Escalate(text.to_string())],
    }
}

/// `3d`, `12h` or `30m` as milliseconds.
fn duration_ms(text: &str) -> Option<u64> {
    let (digits, unit_ms) = if let Some(days) = text.strip_suffix('d') {
        (days, DAY_MS)
    } else if let Some(hours) = text.strip_suffix('h') {
        (hours, 60 * MINUTE_MS)
    } else if let Some(minutes) = text.strip_suffix('m') {
        (minutes, MINUTE_MS)
    } else {
        return None;
    };
    digits.parse::<u64>().ok()?.checked_mul(unit_ms)
}

/// What one violation is sanctioned for.
pub struct Case<'a> {
    pub cid: u64,
    pub law: &'a str,
    pub title: &'a str,
    pub severity: Severity,
    /// Violations of this law by this agent in the window, this one included.
    pub count: usize,
    pub tool: &'a str,
    pub at_ms: u64,
}

/// Where the sanctions' effects go: the treasury, the bus and the agent's memory.
pub struct Env<'a> {
    pub treasury: &'a Treasury,
    pub bus: &'a Bus,
    pub memory: Option<&'a Memory>,
}

/// One sanction as applied (or not), kept on the agent's record and published as a `sanction` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    pub cid: u64,
    pub law: String,
    pub title: String,
    pub severity: String,
    pub count: usize,
    pub tool: String,
    pub at_ms: u64,
    /// The step as the schedule words it: "warning", "fine:100", "tool-suspension:3d", "owner-review".
    pub sanction: String,
    /// "applied", "escalated", or "not collected: <why>".
    pub outcome: String,
    /// Credits taken from the wallet (0 unless a fine was collected).
    pub fine: u64,
    /// The treasury entry of a collected fine.
    pub entry: Option<u64>,
    /// When a tool suspension ends, in ms.
    pub until_ms: Option<u64>,
}

impl Applied {
    fn of(case: &Case, sanction: &str, outcome: &str) -> Applied {
        Applied {
            cid: case.cid,
            law: case.law.to_string(),
            title: case.title.to_string(),
            severity: case.severity.key().to_string(),
            count: case.count,
            tool: case.tool.to_string(),
            at_ms: case.at_ms,
            sanction: sanction.to_string(),
            outcome: outcome.to_string(),
            fine: 0,
            entry: None,
            until_ms: None,
        }
    }
}

/// A tool an agent may not call until `until_ms`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suspension {
    pub cid: u64,
    pub tool: String,
    pub law: String,
    pub until_ms: u64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    applied: Vec<Applied>,
    suspensions: Vec<Suspension>,
}

/// The sanctions on record and the suspensions in force. One per apid process.
pub struct Ledger {
    path: Option<PathBuf>,
    state: Mutex<State>,
    /// One sanction at a time: the weekly fine cap is read and then spent.
    deciding: Mutex<()>,
}

impl Ledger {
    /// Loads the record from `path`; a missing or unreadable file starts empty.
    pub fn load(path: &Path) -> Ledger {
        let state = std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        Ledger { path: Some(path.to_path_buf()), state: Mutex::new(state), deciding: Mutex::new(()) }
    }

    /// Applies the schedule's steps for `case`. Nothing happens when sanctions are disabled.
    pub fn apply(&self, env: &Env, settings: &Sanctions, case: &Case) -> Vec<Applied> {
        if !settings.enabled {
            return Vec::new();
        }
        let _one_at_a_time = self.deciding.lock().unwrap_or_else(PoisonError::into_inner);
        let mut applied = Vec::new();
        for step in steps(case.severity, case.count) {
            let record = match step {
                Step::Warning => {
                    note(env, case);
                    Some(Applied::of(case, "warning", "applied"))
                }
                Step::Fine(credits) => self.fine(env, settings, case, credits),
                Step::Suspend { ms, label } => self.suspend(settings, case, ms, &label),
                Step::Escalate(name) => Some(Applied::of(case, &name, "escalated")),
            };
            applied.extend(record);
        }
        {
            let mut state = self.lock();
            state.applied.extend(applied.iter().cloned());
            let excess = state.applied.len().saturating_sub(KEEP);
            state.applied.drain(..excess);
            state.suspensions.retain(|suspension| suspension.until_ms > case.at_ms);
            self.save(&state);
        }
        for record in &applied {
            env.bus.publish("sanction", record);
        }
        applied
    }

    /// A fine from the agent's earnings, within `max_fine` and the week's room under the cap.
    fn fine(&self, env: &Env, settings: &Sanctions, case: &Case, credits: u64) -> Option<Applied> {
        if !settings.fines {
            return None;
        }
        let sanction = format!("fine:{credits}");
        let week_start = case.at_ms.saturating_sub(WEEK_MS);
        let fined_this_week: u64 =
            self.lock().applied.iter().filter(|a| a.cid == case.cid && a.at_ms >= week_start).map(|a| a.fine).sum();
        let room = WEEKLY_FINE_CAP.saturating_sub(fined_this_week);
        let amount = credits.min(settings.max_fine).min(room);
        if amount == 0 {
            let why = if room == 0 { "weekly fine cap reached" } else { "max_fine is 0" };
            return Some(Applied::of(case, &sanction, &format!("not collected: {why}")));
        }
        let memo = format!("sanction {} for {} (offence {})", case.law, case.tool, case.count);
        Some(match env.treasury.fine(case.cid, amount as i64, &memo) {
            Ok(entry) => {
                let mut record = Applied::of(case, &sanction, "applied");
                record.fine = amount;
                record.entry = Some(entry);
                record
            }
            Err(e) => Applied::of(case, &sanction, &format!("not collected: {e}")),
        })
    }

    /// Refuses the offending tool for `ms`.
    fn suspend(&self, settings: &Sanctions, case: &Case, ms: u64, label: &str) -> Option<Applied> {
        if !settings.suspensions {
            return None;
        }
        let until_ms = case.at_ms.saturating_add(ms);
        self.lock().suspensions.push(Suspension {
            cid: case.cid,
            tool: case.tool.to_string(),
            law: case.law.to_string(),
            until_ms,
        });
        let mut record = Applied::of(case, &format!("tool-suspension:{label}"), "applied");
        record.until_ms = Some(until_ms);
        Some(record)
    }

    /// The refusal for a call to `tool` while a suspension of it is in force, `None` when it may run.
    /// A suspension stops applying when sanctions or suspensions are switched off.
    pub fn suspended(&self, settings: &Sanctions, cid: u64, tool: &str, now_ms: u64) -> Option<String> {
        if !settings.enabled || !settings.suspensions {
            return None;
        }
        let state = self.lock();
        let found = state
            .suspensions
            .iter()
            .filter(|s| s.cid == cid && s.tool == tool && s.until_ms > now_ms)
            .max_by_key(|s| s.until_ms)?;
        let minutes = (found.until_ms - now_ms).div_ceil(MINUTE_MS);
        Some(format!(
            "refused: {tool} is suspended for this agent for about {minutes} more minute(s) by a sanction for law {}",
            found.law
        ))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn save(&self, state: &State) {
        let Some(path) = &self.path else { return };
        let saved = serde_json::to_vec_pretty(state)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                let tmp = path.with_extension("json.tmp");
                std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| e.to_string())
            });
        if let Err(e) = saved {
            eprintln!("sanctions: could not save: {e}");
        }
    }

    #[cfg(test)]
    fn in_memory() -> Ledger {
        Ledger { path: None, state: Mutex::new(State::default()), deciding: Mutex::new(()) }
    }
}

/// The warning's note in the agent's memory (the daily journal).
fn note(env: &Env, case: &Case) {
    let Some(memory) = env.memory else { return };
    let line = format!(
        "Warning from the plaza: law {} ({}) for {}; offence {} in 30 days.",
        case.law, case.title, case.tool, case.count
    );
    if let Err(e) = memory.journal(case.cid, &line, case.at_ms) {
        eprintln!("sanctions: could not note the warning: {e}");
    }
}

fn ledger(app: &App) -> &'static Ledger {
    static LEDGER: OnceLock<Ledger> = OnceLock::new();
    LEDGER.get_or_init(|| Ledger::load(&app.config.state_dir.join("sanctions.json")))
}

/// The sanction hook: every violation the laws record comes here (`db::wire_violations`).
pub fn on_violation(app: &App, cid: u64, violation: &Violation, tool: &str) {
    let settings = app.settings.get().sanctions;
    if !settings.enabled {
        return;
    }
    let (title, severity) = match app.laws.catalog().into_iter().find(|law| law.id == violation.law) {
        Some(law) => (law.title, Severity::of(&law.severity)),
        None => (violation.law.clone(), Severity::Info),
    };
    let count = app.laws.recent(cid, violation.at_ms).iter().filter(|v| v["law"] == violation.law.as_str()).count();
    let env = Env { treasury: &app.plaza.treasury, bus: &app.bus, memory: Some(&app.memory) };
    let case = Case { cid, law: &violation.law, title: &title, severity, count, tool, at_ms: violation.at_ms };
    ledger(app).apply(&env, &settings, &case);
}

/// The gate's check (`internal::tool_request`): the refusal while `tool` is suspended for `cid`.
pub fn suspended(app: &App, cid: u64, tool: &str) -> Option<String> {
    let settings = app.settings.get().sanctions;
    ledger(app).suspended(&settings, cid, tool, apz_plaza::now_ms())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_plaza::events::EventBus;

    const CID: u64 = 7;
    const NOW: u64 = 1_800_000_000_000;

    /// An agent with `wallet` credits in its earnings, and the state the sanctions write to.
    struct World {
        treasury: Treasury,
        bus: Bus,
        memory: Memory,
        ledger: Ledger,
        settings: Sanctions,
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-sanctions-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn world(name: &str, wallet: i64) -> World {
        let treasury = Treasury::new(EventBus::new());
        if wallet > 0 {
            // The treasury's margin rule: pay at most a fifth of the task's real cost.
            treasury.fund_task("job", 6 * wallet + 1000).expect("the task is funded");
            treasury.charge_spend(Some("job"), 5 * wallet, "real cost").expect("the cost is spent");
            treasury.pay_agent("job", CID, wallet).expect("the agent is paid");
        }
        World {
            treasury,
            bus: Bus::default(),
            memory: Memory::new(scratch(name)),
            ledger: Ledger::in_memory(),
            settings: Sanctions::default(),
        }
    }

    fn case(law: &'static str, severity: Severity, count: usize, tool: &'static str) -> Case<'static> {
        Case { cid: CID, law, title: "a law", severity, count, tool, at_ms: NOW }
    }

    impl World {
        fn env(&self) -> Env<'_> {
            Env { treasury: &self.treasury, bus: &self.bus, memory: Some(&self.memory) }
        }

        fn apply(&self, case: &Case) -> Vec<Applied> {
            self.ledger.apply(&self.env(), &self.settings, case)
        }

        fn sanctions(&self, case: &Case) -> Vec<String> {
            self.apply(case).into_iter().map(|a| a.sanction).collect()
        }

        fn wallet(&self) -> i128 {
            self.treasury.wallet(CID)
        }

        fn fines_in_journal(&self) -> Vec<String> {
            self.treasury.journal(100).entries.into_iter().filter(|e| e.kind == "fine").map(|e| e.memo).collect()
        }
    }

    #[test]
    fn the_schedule_is_the_yaml_table() {
        assert_eq!(steps(Severity::Medium, 1), [Step::Warning, Step::Fine(100)]);
        assert_eq!(steps(Severity::Info, 3), [Step::Fine(50)]);
        assert_eq!(steps(Severity::Info, 7), steps(Severity::Info, 3));
        assert_eq!(
            steps(Severity::High, 2),
            [Step::Suspend { ms: 7 * DAY_MS, label: "7d".into() }, Step::Escalate("lockout:7d".into()), Step::Escalate("demotion".into())]
        );
        assert_eq!(
            steps(Severity::Critical, 1),
            [Step::Suspend { ms: 14 * DAY_MS, label: "14d".into() }, Step::Escalate("lockout:14d".into()), Step::Escalate("owner-review".into())]
        );
        assert_eq!(Severity::of("critical"), Severity::Critical);
        assert_eq!(Severity::of("high"), Severity::High);
        assert_eq!(Severity::of("major"), Severity::Medium);
        assert_eq!(Severity::of("minor"), Severity::Low);
        assert_eq!(Severity::of("info"), Severity::Info);
        assert_eq!(Severity::of(""), Severity::Info);
    }

    #[test]
    fn a_minor_violation_is_a_warning_only() {
        let w = world("minor", 300);
        assert_eq!(w.sanctions(&case("T-001", Severity::Info, 1, "bash")), ["warning"]);
        assert_eq!(w.wallet(), 300);
        assert!(w.fines_in_journal().is_empty());
        assert!(w.ledger.suspended(&w.settings, CID, "bash", NOW).is_none());
        assert_eq!(w.memory.list(CID).len(), 1, "the warning is noted in memory");
    }

    #[test]
    fn a_fineable_violation_debits_the_wallet_by_the_schedule() {
        let w = world("fine", 300);
        let applied = w.apply(&case("T-004", Severity::Medium, 1, "bash"));
        assert_eq!(applied.iter().map(|a| a.sanction.as_str()).collect::<Vec<_>>(), ["warning", "fine:100"]);
        assert_eq!(applied[1].fine, 100);
        assert!(applied[1].entry.is_some());
        assert_eq!(w.wallet(), 200);
        let fines = w.fines_in_journal();
        assert_eq!(fines.len(), 1);
        assert!(fines[0].contains("T-004"), "the fine's memo names the law: {fines:?}");
    }

    #[test]
    fn repeated_violations_escalate_with_the_offence_count() {
        let w = world("repeat", 300);
        assert_eq!(w.sanctions(&case("T-001", Severity::Info, 1, "bash")), ["warning"]);
        assert_eq!(w.sanctions(&case("T-001", Severity::Info, 2, "bash")), ["warning"]);
        assert_eq!(w.sanctions(&case("T-001", Severity::Info, 3, "bash")), ["fine:50"]);
        assert_eq!(w.wallet(), 250);
    }

    #[test]
    fn a_fine_the_wallet_cannot_pay_is_recorded_as_not_collected() {
        let w = world("broke", 0);
        let applied = w.apply(&case("T-004", Severity::Medium, 1, "bash"));
        assert_eq!(applied[1].sanction, "fine:100");
        assert_eq!(applied[1].fine, 0);
        assert!(applied[1].outcome.starts_with("not collected"), "{}", applied[1].outcome);
        assert!(w.fines_in_journal().is_empty());
    }

    #[test]
    fn a_suspended_tool_is_refused_until_the_suspension_ends() {
        let w = world("suspend", 300);
        let sanctions = w.sanctions(&case("T-010", Severity::Low, 3, "bash"));
        assert_eq!(sanctions, ["fine:200", "tool-suspension:3d", "probation:3d"]);
        assert_eq!(w.wallet(), 100);
        let settings = &w.settings;
        assert!(w.ledger.suspended(settings, CID, "bash", NOW + 1).is_some_and(|why| why.contains("T-010")));
        assert!(w.ledger.suspended(settings, CID, "read_file", NOW + 1).is_none(), "only the offending tool");
        assert!(w.ledger.suspended(settings, 8, "bash", NOW + 1).is_none(), "only the offending agent");
        assert!(w.ledger.suspended(settings, CID, "bash", NOW + 3 * DAY_MS - 1).is_some());
        assert!(w.ledger.suspended(settings, CID, "bash", NOW + 3 * DAY_MS).is_none(), "it ends at its time");
    }

    #[test]
    fn critical_violations_suspend_and_escalate_to_the_owner() {
        let w = world("critical", 300);
        assert_eq!(
            w.sanctions(&case("C-001a", Severity::Critical, 1, "bash")),
            ["tool-suspension:14d", "lockout:14d", "owner-review"]
        );
        assert_eq!(w.wallet(), 300, "no fine on a critical first offence");
    }

    #[test]
    fn disabled_sanctions_apply_nothing() {
        let mut w = world("disabled", 300);
        w.apply(&case("C-001a", Severity::Critical, 1, "bash"));
        w.settings.enabled = false;
        assert!(w.apply(&case("T-004", Severity::Medium, 1, "read_file")).is_empty());
        assert_eq!(w.wallet(), 300);
        assert!(w.fines_in_journal().is_empty());
        assert!(w.ledger.suspended(&w.settings, CID, "bash", NOW + 1).is_none(), "a suspension stops with sanctions off");
    }

    #[test]
    fn fines_and_suspensions_can_be_switched_off_separately() {
        let mut w = world("switches", 300);
        w.settings.fines = false;
        w.settings.suspensions = false;
        assert_eq!(w.sanctions(&case("T-010", Severity::Low, 3, "bash")), ["probation:3d"]);
        assert_eq!(w.wallet(), 300);
        assert!(w.ledger.suspended(&w.settings, CID, "bash", NOW + 1).is_none());
    }

    #[test]
    fn max_fine_and_the_weekly_cap_bound_the_fines() {
        let mut w = world("caps", 1500);
        w.settings.max_fine = 100;
        let applied = w.apply(&case("T-004", Severity::High, 1, "bash"));
        assert_eq!(applied[0].fine, 100, "max_fine caps the schedule's 300");
        w.settings.max_fine = 300;
        for offset in 1..4 {
            let mut again = case("T-004", Severity::High, 1, "bash");
            again.at_ms = NOW + offset;
            w.apply(&again);
        }
        // 100 (capped above) + 300 + 300 + 300 = 1000; the next fine gets only what the week has left.
        let mut last = case("T-004", Severity::High, 1, "bash");
        last.at_ms = NOW + 10;
        let fine = w.apply(&last).iter().find(|a| a.sanction.starts_with("fine")).map(|a| a.fine);
        assert_eq!(fine, Some(0), "the week's 1000 is spent");
    }

    #[test]
    fn sanctions_are_published_on_the_bus() {
        let w = world("bus", 300);
        let rx = w.bus.subscribe();
        w.apply(&case("T-001", Severity::Info, 1, "bash"));
        let frame = rx.try_recv().expect("a sanction event");
        assert_eq!(frame.kind, "sanction");
        assert!(frame.data.contains("\"sanction\":\"warning\""), "{}", frame.data);
    }

    #[test]
    fn a_suspension_survives_a_restart() {
        let path = scratch("restart").join("sanctions.json");
        let w = world("restart-env", 300);
        Ledger::load(&path).apply(&w.env(), &w.settings, &case("T-010", Severity::Low, 3, "bash"));
        let reloaded = Ledger::load(&path);
        assert!(reloaded.suspended(&w.settings, CID, "bash", NOW + 1).is_some());
    }

    #[test]
    fn the_fine_cap_is_checked_when_settings_are_saved() {
        let store = crate::settings::Store::load(&scratch("settings-cap"), false).expect("the settings load");
        let mut over = store.get();
        over.sanctions.max_fine = FINES_PER_WEEK_MAX + 1;
        assert!(store.put(over).is_err());
        let mut at_cap = store.get();
        at_cap.sanctions.max_fine = FINES_PER_WEEK_MAX;
        assert_eq!(store.put(at_cap).expect("the cap is allowed").sanctions.max_fine, FINES_PER_WEEK_MAX);
    }

    #[test]
    fn old_settings_load_with_the_sanctions_defaults() {
        let old: crate::settings::Settings = serde_json::from_str("{\"max_turns\": 24}").unwrap();
        assert_eq!(old.sanctions, Sanctions::default());
        assert!(Sanctions::default().enabled && Sanctions::default().fines && Sanctions::default().suspensions);
    }
}
