//! Statements, signatures and log records.
//!
//! A **statement** says what happened: its kind (`lane.open`, `revision.seal`, `attest`, …), who acted, when,
//! and a JSON body. The actor signs the statement's exact bytes. The **log record** wraps a signed statement
//! with its place in the log: a sequence number and the hash of the record before it. Signatures cover the
//! statement only, so an agent's statement can be signed (by apid, inside the agent's own call) before the
//! server knows where it will land; the chain covers everything.
//!
//! A record on disk is binary:
//!
//! ```text
//! seq u64 | prev 32 bytes | stmt_len u32 | stmt (JSON) | key_id_len u8 | key id (ASCII hex) | sig 64 bytes
//! ```
//!
//! and its hash is the BLAKE3 of those bytes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hash::Hash;
use crate::keys::KeyPair;

/// Who acted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Actor {
    /// The owner (any of the owner's devices).
    Owner,
    /// The hub: apid and `weft serve`.
    Hub,
    /// An agent, by its plaza cid.
    Agent { cid: u64 },
    /// A node, by its id (node caches).
    Node { id: String },
}

impl Actor {
    pub fn label(&self) -> String {
        match self {
            Actor::Owner => "owner".into(),
            Actor::Hub => "hub".into(),
            Actor::Agent { cid } => format!("agent:{cid}"),
            Actor::Node { id } => format!("node:{id}"),
        }
    }

    /// Parses `owner`, `hub`, `agent:<cid>`, `node:<id>`.
    pub fn parse(s: &str) -> Result<Actor, String> {
        match s {
            "owner" => Ok(Actor::Owner),
            "hub" => Ok(Actor::Hub),
            _ => {
                if let Some(cid) = s.strip_prefix("agent:") {
                    cid.parse()
                        .map(|cid| Actor::Agent { cid })
                        .map_err(|_| format!("bad agent cid in {s:?}"))
                } else if let Some(id) = s.strip_prefix("node:") {
                    Ok(Actor::Node { id: id.to_string() })
                } else {
                    Err(format!(
                        "not an actor: {s:?} (owner, hub, agent:<cid> or node:<id>)"
                    ))
                }
            }
        }
    }
}

/// What happened, signed by whoever did it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Statement {
    pub kind: String,
    pub actor: Actor,
    pub at_ms: u64,
    pub body: Value,
}

impl Statement {
    pub fn new(kind: &str, actor: Actor, body: Value) -> Statement {
        Statement {
            kind: kind.to_string(),
            actor,
            at_ms: now_ms(),
            body,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("a statement always serializes")
    }

    /// Signs the statement's bytes.
    pub fn sign(&self, key: &KeyPair) -> Signed {
        let stmt = self.to_bytes();
        let sig = key.sign(&stmt).to_vec();
        Signed {
            stmt,
            key_id: key.id(),
            sig,
        }
    }
}

/// A statement's exact bytes with the signature over them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    pub stmt: Vec<u8>,
    pub key_id: String,
    pub sig: Vec<u8>,
}

impl Signed {
    pub fn statement(&self) -> Result<Statement, String> {
        serde_json::from_slice(&self.stmt).map_err(|e| format!("not a statement: {e}"))
    }

    /// The form the wire carries: `{stmt, key, sig}` with the statement as a JSON string and the signature
    /// base64.
    pub fn to_json(&self) -> Value {
        use base64::Engine as _;
        serde_json::json!({
            "stmt": String::from_utf8_lossy(&self.stmt),
            "key": self.key_id,
            "sig": base64::engine::general_purpose::STANDARD.encode(&self.sig),
        })
    }

    pub fn from_json(v: &Value) -> Result<Signed, String> {
        use base64::Engine as _;
        let stmt = v["stmt"]
            .as_str()
            .ok_or("missing stmt")?
            .as_bytes()
            .to_vec();
        let key_id = v["key"].as_str().ok_or("missing key")?.to_string();
        let sig = base64::engine::general_purpose::STANDARD
            .decode(v["sig"].as_str().ok_or("missing sig")?)
            .map_err(|e| format!("bad sig: {e}"))?;
        Ok(Signed { stmt, key_id, sig })
    }
}

/// A signed statement at its place in the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRecord {
    pub seq: u64,
    pub prev: Hash,
    pub signed: Signed,
}

impl LogRecord {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + 32 + 4 + self.signed.stmt.len() + 1 + 32 + 64);
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.prev.0);
        out.extend_from_slice(&(self.signed.stmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.signed.stmt);
        out.push(self.signed.key_id.len() as u8);
        out.extend_from_slice(self.signed.key_id.as_bytes());
        out.extend_from_slice(&self.signed.sig);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<LogRecord, String> {
        let mut at = 0usize;
        let mut take = |n: usize| -> Result<&[u8], String> {
            let part = bytes.get(at..at + n).ok_or("record is cut short")?;
            at += n;
            Ok(part)
        };
        let seq = u64::from_le_bytes(take(8)?.try_into().unwrap());
        let prev = Hash(take(32)?.try_into().unwrap());
        let stmt_len = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
        let stmt = take(stmt_len)?.to_vec();
        let key_len = take(1)?[0] as usize;
        let key_id =
            String::from_utf8(take(key_len)?.to_vec()).map_err(|_| "key id is not ASCII")?;
        let sig = take(64)?.to_vec();
        if at != bytes.len() {
            return Err("record has trailing bytes".into());
        }
        Ok(LogRecord {
            seq,
            prev,
            signed: Signed { stmt, key_id, sig },
        })
    }

    /// The record's hash: what the next record names as `prev`.
    pub fn hash(&self) -> Hash {
        Hash::of(&self.encode())
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_round_trips_and_its_signature_verifies() {
        let key = KeyPair::generate();
        let stmt = Statement::new("lane.open", Actor::Hub, serde_json::json!({"task": "t51"}));
        let signed = stmt.sign(&key);
        let rec = LogRecord {
            seq: 7,
            prev: Hash::of(b"before"),
            signed,
        };
        let back = LogRecord::decode(&rec.encode()).unwrap();
        assert_eq!(back, rec);
        assert!(
            key.public()
                .verify(&back.signed.stmt, &back.signed.sig)
                .is_ok()
        );
        assert_eq!(back.signed.statement().unwrap(), stmt);
        let wire = Signed::from_json(&rec.signed.to_json()).unwrap();
        assert_eq!(wire, rec.signed);
    }

    #[test]
    fn actors_parse_and_print() {
        for s in ["owner", "hub", "agent:107", "node:harbor"] {
            assert_eq!(Actor::parse(s).unwrap().label(), s);
        }
        assert!(Actor::parse("agent:x").is_err());
    }
}
