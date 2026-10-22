//! Disk (PLAN 10.5, W14): what a project's disk holds, and the quota that asks the owner when it is passed.
//!
//! A project's disk is its Weft repo's bytes (the blobs its revisions name: `GET /v1/repos/{repo}/stats` in Weft) plus
//! the `/workspace` use its live sessions' VMs report (`record_stats` in `internal.rs`, in MiB). Its quota is its
//! budget's `disk_quota_bytes` (`PUT /v1/projects/{id}/budget`). Past the quota one `disk` card asks the owner in the
//! project's chat: raise the quota to an amount, or keep it. Enforcement (pausing the sessions) is phase 3's; until then
//! the card only asks.
//!
//! The quotas are checked every [`CHECK_MS`]. A project past its quota has one card, and is asked again only about a
//! quota it was not asked about; its card is withdrawn once its disk is back under the quota, or the quota is gone.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};

use crate::budgets::OwnerKind;
use crate::cards::{Card, Choice, Kind, NewCard};
use crate::state::App;

/// How often the quotas are checked (ms): a Weft stat per project with a quota is too much for every 2 s pass.
pub const CHECK_MS: u64 = 30_000;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// What the disk remembers between checks: the VMs' reports, the quota each project was asked about, and when the
/// quotas were last checked.
#[derive(Default)]
pub struct Disk {
    /// The `/workspace` use each VM reported, by cid, in MiB.
    workspace_mib: Mutex<HashMap<u64, u64>>,
    /// The quota a project's card asked about, by project.
    asked: Mutex<HashMap<String, u64>>,
    /// When the quotas were last checked (ms); none before the first check.
    checked_ms: Mutex<Option<u64>>,
}

impl Disk {
    /// Keeps the `/workspace` use a VM reported (`record_stats`).
    pub fn report(&self, cid: u64, used_mib: u64) {
        self.workspace_mib
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(cid, used_mib);
    }

    /// Drops the report of a VM that was retired.
    pub fn forget(&self, cid: u64) {
        self.workspace_mib
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&cid);
    }

    fn workspace(&self, cid: u64) -> u64 {
        self.workspace_mib
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&cid)
            .copied()
            .unwrap_or(0)
    }

    fn asked(&self, project: &str) -> Option<u64> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(project)
            .copied()
    }

    fn set_asked(&self, project: &str, quota: Option<u64>) {
        let mut asked = self.asked.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(quota) = quota {
            asked.insert(project.to_string(), quota);
        } else {
            asked.remove(project);
        }
    }

    /// Whether the quotas are due a check at `now`; takes the check when they are.
    fn due(&self, now: u64) -> bool {
        let mut checked = self
            .checked_ms
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if checked.is_some_and(|at| now.saturating_sub(at) < CHECK_MS) {
            return false;
        }
        *checked = Some(now);
        true
    }
}

/// What a project's disk holds: its repo's bytes and its workspaces' use (MiB, as the VMs reported them).
pub fn used_bytes(repo_bytes: u64, workspaces_mib: impl IntoIterator<Item = u64>) -> u64 {
    repo_bytes + workspaces_mib.into_iter().map(|mib| mib * MIB).sum::<u64>()
}

/// What a quota check does for one project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing: it is under its quota, or past one its card already asked about.
    Keep,
    /// Its card is withdrawn: its disk is back under its quota, or it has none.
    Withdraw,
    /// A card asks: it is past a quota it has not been asked about.
    Ask,
}

/// The verdict for a project whose disk holds `used` bytes, under `quota`, after being asked about `asked`.
pub fn verdict(used: u64, quota: Option<u64>, asked: Option<u64>) -> Verdict {
    match quota {
        Some(quota) if used > quota && asked == Some(quota) => Verdict::Keep,
        Some(quota) if used > quota => Verdict::Ask,
        _ if asked.is_some() => Verdict::Withdraw,
        _ => Verdict::Keep,
    }
}

/// What `project`'s disk holds now: its repo's bytes (none while it has no repo, or Weft does not answer) and its live
/// sessions' workspaces.
pub fn measure(app: &App, project: &str) -> u64 {
    let repo = app
        .projects
        .get(project)
        .and_then(|p| p.repo)
        .map_or(0, |repo| repo_bytes(app, &repo));
    let workspaces = app
        .sessions
        .list(&crate::sessions::Filter {
            project: Some(project.to_string()),
            live: true,
            ..Default::default()
        })
        .into_iter()
        .filter_map(|s| s.cid)
        .map(|cid| app.disk.workspace(cid));
    used_bytes(repo, workspaces)
}

/// The bytes of a repo's objects, as Weft counts them; 0 when Weft does not answer.
fn repo_bytes(app: &App, repo: &str) -> u64 {
    app.weft
        .get(&format!("/repos/{repo}/stats"))
        .ok()
        .and_then(|stats| stats["bytes"].as_u64())
        .unwrap_or(0)
}

/// The `disk` a project's budget view shows: what it holds, and its quota (none when it has none).
pub fn view(app: &App, project: &str, quota: Option<u64>) -> Value {
    json!({ "used_bytes": measure(app, project), "quota_bytes": quota })
}

/// Checks the quotas, every [`CHECK_MS`]: a project past its quota gets its card; a card whose project is under its
/// quota again, or has none, is withdrawn.
pub fn tick(app: &App, now: u64) {
    if !app.disk.due(now) {
        return;
    }
    for project in app.projects.list() {
        let quota = app
            .budgets
            .get(OwnerKind::Project, &project.id)
            .and_then(|b| b.disk_quota_bytes);
        let asked = app.disk.asked(&project.id);
        let used = if quota.is_some() {
            measure(app, &project.id)
        } else {
            0
        };
        match verdict(used, quota, asked) {
            Verdict::Keep => {}
            Verdict::Withdraw => {
                withdraw_card(app, &project.id, now);
                app.disk.set_asked(&project.id, None);
            }
            Verdict::Ask => {
                if let Some(quota) = quota {
                    // The card of an older quota goes first: a project has one open card.
                    withdraw_card(app, &project.id, now);
                    ask(app, &project.id, used, quota, now);
                    app.disk.set_asked(&project.id, Some(quota));
                }
            }
        }
    }
}

/// The owner set `project`'s quota (`PUT /v1/projects/{id}/budget`): a card that asked about the old one is settled. A
/// project still past the new quota is asked about it at the next check.
pub fn quota_changed(app: &App, project: &str, now: u64) {
    app.disk.set_asked(project, None);
    withdraw_card(app, project, now);
}

/// Does what an answered disk card decides: the quota becomes the amount the owner typed, or stays. Returns it in words.
pub fn card_answered(app: &App, card: &Card) -> String {
    let Some(project) = card.subject.strip_prefix("disk:") else {
        return "recorded".into();
    };
    if card.answer.as_deref() != Some("raise_to") {
        return "the quota stays as it is".into();
    }
    let Some(to) = card.amount.filter(|to| *to > 0) else {
        return "a quota is more than zero bytes: it stays as it is".into();
    };
    let Some(budget) = app.budgets.get(OwnerKind::Project, project) else {
        return "no such budget".into();
    };
    match app.budgets.set_project(
        project,
        budget.unit,
        budget.cap,
        budget.warn_at_pct,
        budget.monthly_cap,
        Some(to),
        apz_plaza::now_ms(),
    ) {
        Ok(_) => format!("the disk quota is now {}", bytes(to)),
        Err(e) => e,
    }
}

fn subject(project: &str) -> String {
    format!("disk:{project}")
}

fn withdraw_card(app: &App, project: &str, now: u64) {
    for card in app.cards.withdraw(Kind::Disk, &subject(project), now) {
        app.bus.publish("card", &card);
    }
}

/// Asks the owner, in the project's chat, to raise the project's quota or keep it.
fn ask(app: &App, project: &str, used: u64, quota: u64, now: u64) {
    let chat = crate::work_loop::project_chat(app, project, now);
    crate::work_loop::open_card(app, disk_card(project, used, quota, chat), now);
}

/// The card a project past its quota gets.
fn disk_card(project: &str, used: u64, quota: u64, chat: String) -> NewCard {
    NewCard {
        kind: Kind::Disk,
        chat: Some(chat),
        project: Some(project.to_string()),
        task: None,
        session: None,
        subject: subject(project),
        title: format!("Project {project} is past its disk quota"),
        body: Some(format!(
            "It uses {} of its {} quota.",
            bytes(used),
            bytes(quota)
        )),
        choices: vec![
            Choice {
                id: "raise_to".into(),
                label: "Raise to…".into(),
                detail: Some("a new quota you type, in bytes".into()),
                takes_amount: true,
            },
            Choice {
                id: "keep".into(),
                label: "Keep the quota".into(),
                detail: Some(
                    "the project goes on past it; no new card until its disk is back under it"
                        .into(),
                ),
                takes_amount: false,
            },
        ],
    }
}

/// A byte count as the cards say it: GiB with one decimal, whole MiB, or bytes.
fn bytes(n: u64) -> String {
    if n >= GIB {
        format!("{:.1} GiB", n as f64 / GIB as f64)
    } else if n >= MIB {
        format!("{} MiB", n / MIB)
    } else {
        format!("{n} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::{Cards, Refused, State};

    #[test]
    fn the_disk_is_the_repo_plus_the_workspaces_in_mib() {
        assert_eq!(used_bytes(10, []), 10);
        assert_eq!(used_bytes(10, [1, 2]), 10 + 3 * MIB);
    }

    #[test]
    fn a_project_is_asked_once_per_quota_and_settled_under_it() {
        assert_eq!(verdict(5, Some(10), None), Verdict::Keep);
        assert_eq!(verdict(11, Some(10), None), Verdict::Ask);
        assert_eq!(
            verdict(11, Some(10), Some(10)),
            Verdict::Keep,
            "kept: not asked again"
        );
        assert_eq!(
            verdict(11, Some(20), Some(10)),
            Verdict::Withdraw,
            "under the raised quota: the card of the old one is settled"
        );
        assert_eq!(
            verdict(21, Some(20), Some(10)),
            Verdict::Ask,
            "a new quota is a new question"
        );
        assert_eq!(
            verdict(9, Some(10), Some(10)),
            Verdict::Withdraw,
            "back under it"
        );
        assert_eq!(
            verdict(50, None, Some(10)),
            Verdict::Withdraw,
            "the quota is gone"
        );
        assert_eq!(verdict(50, None, None), Verdict::Keep);
    }

    #[test]
    fn a_check_is_due_every_check_interval() {
        let disk = Disk::default();
        assert!(disk.due(0));
        assert!(!disk.due(CHECK_MS - 1));
        assert!(disk.due(CHECK_MS));
    }

    #[test]
    fn a_vm_report_is_kept_until_its_vm_is_retired() {
        let disk = Disk::default();
        disk.report(7, 40);
        disk.report(7, 55);
        assert_eq!(disk.workspace(7), 55, "the latest report counts");
        assert_eq!(disk.workspace(8), 0);
        disk.forget(7);
        assert_eq!(disk.workspace(7), 0);
    }

    #[test]
    fn a_quota_card_raises_to_an_amount_or_keeps_and_is_withdrawn_when_the_quota_changes() {
        let cards = Cards::default();
        let (card, created) = cards.open(disk_card("pr1", 3 * GIB, GIB, "ch1".into()), 1);
        assert!(created);
        assert_eq!(card.kind, Kind::Disk);
        assert_eq!(card.subject, "disk:pr1");
        assert!(matches!(
            cards.answer(&card.id, "raise_to", None, 2),
            Err(Refused::AmountNeeded(_))
        ));
        let raised = cards
            .answer(&card.id, "raise_to", Some(4 * GIB), 3)
            .unwrap();
        assert_eq!(
            (raised.state, raised.amount),
            (State::Answered, Some(4 * GIB))
        );
        assert_eq!(bytes(4 * GIB), "4.0 GiB");

        // Another quota asks again; a quota changed elsewhere withdraws the open card.
        let (again, created) = cards.open(disk_card("pr1", 3 * GIB, GIB, "ch1".into()), 4);
        assert!(created && again.id != card.id);
        assert_eq!(cards.withdraw(Kind::Disk, "disk:pr1", 5).len(), 1);
        assert!(cards.list(true, None).is_empty());
    }

    #[test]
    fn a_byte_count_reads_in_the_unit_it_is_in() {
        assert_eq!(bytes(3 * MIB), "3 MiB");
        assert_eq!(bytes(12), "12 bytes");
        assert_eq!(bytes(5 * GIB + GIB / 2), "5.5 GiB");
    }
}
