//! Verification: a subject's signed records, each walked from its signature to its key, the key's certificate,
//! and on to the owner's key that the log's first record names.
//!
//! A subject is a change (`w-12`), a revision (`w-12.r1`), a release (`agentplaza@v0.9.0`) or an artifact id.

use serde::Serialize;
use serde_json::Value;

use crate::record::{Actor, LogRecord, Statement};
use crate::state::State;
use crate::store::{Store, VerifyReport};

#[derive(Debug, Serialize)]
pub struct RecordView {
    pub seq: u64,
    pub kind: String,
    pub actor: String,
    pub at_ms: u64,
    pub key: String,
    /// Who holds the key, as its certificate says (`Priya, builder`).
    pub signer: String,
    /// The key's certificate chain: the key, its issuer, … up to the owner key.
    pub chain: Vec<String>,
    pub via_console: bool,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    pub body: Value,
}

#[derive(Debug, Serialize)]
pub struct SubjectReport {
    pub subject: String,
    pub ok: bool,
    pub records: Vec<RecordView>,
    pub owner_key: Option<String>,
}

/// Whether a statement is about `subject`.
fn about(stmt: &Statement, subject: &str) -> bool {
    let b = &stmt.body;
    let s = |k: &str| b[k].as_str().unwrap_or("");
    if let Some((repo, version)) = subject.split_once('@') {
        return s("repo") == repo && (s("version") == version || s("release") == version);
    }
    if let Some((change, rest)) = subject.split_once('.') {
        // A revision: the statement that made it, and the records about it.
        if s("revision") == subject {
            return true;
        }
        if s("change") != change {
            return false;
        }
        let n = b["n"].as_u64();
        return match (stmt.kind.as_str(), rest.chars().next()) {
            ("revision.draft", Some('d')) => n == rest[1..].parse().ok(),
            ("revision.seal" | "queue.add", Some('r')) => n == rest[1..].parse().ok(),
            ("land", Some('l')) => true,
            _ => false,
        };
    }
    s("change") == subject
        || s("revision").split('.').next() == Some(subject)
        || s("id") == subject
        || s("artifact") == subject
}

pub fn records_of(store: &Store, subject: &str) -> Result<Vec<RecordView>, String> {
    let st = store.state();
    let mut out = Vec::new();
    for rec in store.records()? {
        let stmt = rec.signed.statement()?;
        if about(&stmt, subject) {
            out.push(view(&st, &rec, &stmt));
        }
    }
    if out.is_empty() {
        return Err(format!("nothing is recorded about {subject}"));
    }
    Ok(out)
}

fn view(st: &State, rec: &LogRecord, stmt: &Statement) -> RecordView {
    let mut chain = vec![rec.signed.key_id.clone()];
    let mut problem = None;
    let mut signer = String::new();
    match st.key_at(&rec.signed.key_id, rec.seq) {
        Ok(k) => {
            if let Err(e) = k.key.verify(&rec.signed.stmt, &rec.signed.sig) {
                problem = Some(e);
            }
            signer = [k.holder.name.clone(), k.holder.role.clone()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ");
            if signer.is_empty() {
                signer = k.holder.actor.label();
            }
            // Up the certificates to the owner key.
            let mut issuer = k.issuer.clone();
            for _ in 0..8 {
                chain.push(issuer.clone());
                if st.owner.as_ref().is_some_and(|o| o.id() == issuer) {
                    break;
                }
                match st.keys.get(&issuer) {
                    Some(i) => issuer = i.issuer.clone(),
                    None => {
                        problem.get_or_insert_with(|| format!("key {issuer} is not certified"));
                        break;
                    }
                }
            }
            if !st
                .owner
                .as_ref()
                .is_some_and(|o| chain.last() == Some(&o.id()))
            {
                problem.get_or_insert_with(|| "the chain does not reach the owner key".into());
            }
        }
        Err(e) => {
            if stmt.kind == "genesis" {
                signer = "owner".into();
            } else {
                problem = Some(e);
            }
        }
    }
    let via_console = stmt.actor == Actor::Owner
        && st
            .owner
            .as_ref()
            .is_some_and(|o| o.id() != rec.signed.key_id);
    RecordView {
        seq: rec.seq,
        kind: stmt.kind.clone(),
        actor: stmt.actor.label(),
        at_ms: stmt.at_ms,
        key: rec.signed.key_id.clone(),
        signer,
        chain,
        via_console,
        ok: problem.is_none(),
        problem,
        body: stmt.body.clone(),
    }
}

pub fn verify_subject(store: &Store, subject: &str) -> Result<SubjectReport, String> {
    let records = records_of(store, subject)?;
    let ok = records.iter().all(|r| r.ok);
    Ok(SubjectReport {
        subject: subject.to_string(),
        ok,
        records,
        owner_key: store.state().owner.as_ref().map(|k| k.id()),
    })
}

/// The whole log: every chain link and signature, and a replay that gives the same state.
pub fn verify_chain(store: &Store) -> Result<VerifyReport, String> {
    store.verify_deep()
}
