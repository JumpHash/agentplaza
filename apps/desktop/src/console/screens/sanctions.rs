//! The sanctions the hub records against agents: the shapes `GET /v1/sanctions` returns and the
//! lift call. The agent page's Conduct section, the society's suspended mark and the approvals'
//! escalations all read them, so the calls live here once.

use serde::Deserialize;
use serde_json::json;

use super::tools::encode;
use crate::console::client;

/// One sanction on record, as the list gives it (newest first).
#[derive(Debug, Clone, Deserialize)]
pub struct Sanction {
    pub id: u64,
    #[serde(default)]
    pub law: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub at_ms: u64,
    #[serde(default)]
    pub sanction: String,
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub fine: u64,
    #[serde(default)]
    pub lifted: Option<Lifted>,
}

/// The owner's lift of a sanction: when, and the reason on the record.
#[derive(Debug, Clone, Deserialize)]
pub struct Lifted {
    #[serde(default)]
    pub why: String,
}

/// A suspension: a sanction that keeps a tool closed to the agent until `until_ms`.
#[derive(Debug, Clone, Deserialize)]
pub struct Suspension {
    pub id: u64,
    pub cid: u64,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub law: String,
    #[serde(default)]
    pub until_ms: u64,
    #[serde(default)]
    pub active: bool,
}

/// `GET /v1/sanctions`: the sanctions, and the suspensions among them.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Register {
    #[serde(default)]
    pub sanctions: Vec<Sanction>,
    #[serde(default)]
    pub suspensions: Vec<Suspension>,
}

/// The list path with its filters: `cid`, `law` and `limit` are each optional.
fn list_path(cid: Option<u64>, law: Option<&str>, limit: Option<u64>) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(cid) = cid {
        query.push(format!("cid={cid}"));
    }
    if let Some(law) = law {
        query.push(format!("law={}", encode(law)));
    }
    if let Some(limit) = limit {
        query.push(format!("limit={limit}"));
    }
    if query.is_empty() {
        "/v1/sanctions".to_string()
    } else {
        format!("/v1/sanctions?{}", query.join("&"))
    }
}

/// The sanctions and suspensions, narrowed by agent, law and count when given.
pub fn list(client: &client::Client, cid: Option<u64>, law: Option<&str>, limit: Option<u64>) -> Result<Register, String> {
    client.get(&list_path(cid, law, limit))
}

/// Lifts an active suspension with the owner's reason; answers with the updated sanction.
pub fn lift(client: &client::Client, id: u64, reason: &str) -> Result<Sanction, String> {
    client.post(&format!("/v1/sanctions/{id}/lift"), &json!({ "reason": reason }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_path_takes_the_filters_that_are_given() {
        assert_eq!(list_path(None, None, None), "/v1/sanctions");
        assert_eq!(list_path(Some(7), None, Some(50)), "/v1/sanctions?cid=7&limit=50");
        assert_eq!(list_path(None, Some("a b"), None), "/v1/sanctions?law=a%20b");
    }

    #[test]
    fn a_register_without_suspensions_still_loads() {
        let reg: Register = serde_json::from_value(json!({ "sanctions": [] })).unwrap();
        assert!(reg.sanctions.is_empty() && reg.suspensions.is_empty());
    }
}
