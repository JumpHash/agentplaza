//! Runtime values, their total order, display and the canonical byte encoding used for state
//! hashing in the model checker.

use crate::num::{Dim, Num};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

/// Collections and strings are shared (`Arc`), so cloning a value is cheap and states can be
/// handed between worker threads; updates copy on write.
#[derive(Clone, Debug)]
pub enum Value {
    Bool(bool),
    /// Strong-Kleene unknown, with the missing path or the reason.
    Unknown(String),
    /// The `none` literal: an optional value that is absent.
    None,
    /// A number with its dimension; the magnitude is in base units.
    Num(Num, Dim),
    Str(Arc<str>),
    /// A model value from `values T = {a, b}`.
    Model(Arc<str>),
    List(Arc<Vec<Value>>),
    Set(Arc<BTreeSet<Value>>),
    Map(Arc<BTreeMap<Value, Value>>),
    Record(Arc<BTreeMap<String, Value>>),
    Tuple(Arc<Vec<Value>>),
}

impl Value {
    pub fn int(i: i128) -> Value {
        Value::Num(Num::int(i), Dim::NONE)
    }
    pub fn str(s: &str) -> Value {
        Value::Str(Arc::from(s))
    }
    pub fn list(v: Vec<Value>) -> Value {
        Value::List(Arc::new(v))
    }
    pub fn tuple(v: Vec<Value>) -> Value {
        Value::Tuple(Arc::new(v))
    }
    pub fn set(v: impl IntoIterator<Item = Value>) -> Value {
        Value::Set(Arc::new(v.into_iter().collect()))
    }
    pub fn map(m: BTreeMap<Value, Value>) -> Value {
        Value::Map(Arc::new(m))
    }
    pub fn record(m: BTreeMap<String, Value>) -> Value {
        Value::Record(Arc::new(m))
    }
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "Bool",
            Value::Unknown(_) => "unknown",
            Value::None => "none",
            Value::Num(..) => "number",
            Value::Str(_) => "String",
            Value::Model(_) => "model value",
            Value::List(_) => "List",
            Value::Set(_) => "Set",
            Value::Map(_) => "Map",
            Value::Record(_) => "record",
            Value::Tuple(_) => "tuple",
        }
    }

    fn tag(&self) -> u8 {
        match self {
            Value::Bool(_) => 0,
            Value::Unknown(_) => 1,
            Value::None => 2,
            Value::Num(..) => 3,
            Value::Str(_) => 4,
            Value::Model(_) => 5,
            Value::List(_) => 6,
            Value::Set(_) => 7,
            Value::Map(_) => 8,
            Value::Record(_) => 9,
            Value::Tuple(_) => 10,
        }
    }

    /// Canonical encoding: equal values give equal bytes (sets and maps are ordered).
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.tag());
        let s = |out: &mut Vec<u8>, s: &str| {
            out.extend((s.len() as u32).to_le_bytes());
            out.extend(s.as_bytes());
        };
        match self {
            Value::Bool(b) => out.push(*b as u8),
            Value::Unknown(r) => s(out, r),
            Value::None => {}
            Value::Num(n, d) => {
                for e in d.0 {
                    out.push(e as u8);
                }
                match n {
                    Num::Rat(a, b) => {
                        out.push(0);
                        out.extend(a.to_le_bytes());
                        out.extend(b.to_le_bytes());
                    }
                    Num::Real(x) => {
                        out.push(1);
                        out.extend(x.to_bits().to_le_bytes());
                    }
                }
            }
            Value::Str(x) | Value::Model(x) => s(out, x),
            Value::List(v) | Value::Tuple(v) => {
                out.extend((v.len() as u32).to_le_bytes());
                v.iter().for_each(|x| x.encode(out));
            }
            Value::Set(v) => {
                out.extend((v.len() as u32).to_le_bytes());
                v.iter().for_each(|x| x.encode(out));
            }
            Value::Map(m) => {
                out.extend((m.len() as u32).to_le_bytes());
                for (k, v) in m.iter() {
                    k.encode(out);
                    v.encode(out);
                }
            }
            Value::Record(m) => {
                out.extend((m.len() as u32).to_le_bytes());
                for (k, v) in m.iter() {
                    s(out, k);
                    v.encode(out);
                }
            }
        }
    }
}

/// A 128-bit fingerprint of a canonical encoding: two std SipHash streams with different
/// seeds. Like TLC's fingerprints, a collision would merge two states; for n states its
/// probability is about n²/2¹²⁹ (≈ 10⁻²⁶ for the 4 million states of AgentSession).
pub fn fingerprint(bytes: &[u8]) -> u128 {
    use std::hash::{DefaultHasher, Hasher};
    let mut a = DefaultHasher::new();
    a.write(bytes);
    let mut b = DefaultHasher::new();
    b.write_u64(0x9e37_79b9_7f4a_7c15);
    b.write(bytes);
    ((a.finish() as u128) << 64) | b.finish() as u128
}

impl Ord for Value {
    fn cmp(&self, o: &Value) -> Ordering {
        match (self, o) {
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            (Value::Unknown(a), Value::Unknown(b)) => a.cmp(b),
            (Value::None, Value::None) => Ordering::Equal,
            (Value::Num(a, da), Value::Num(b, db)) => da.cmp(db).then_with(|| a.cmp_num(*b)),
            (Value::Str(a), Value::Str(b)) | (Value::Model(a), Value::Model(b)) => a.cmp(b),
            (Value::List(a), Value::List(b)) | (Value::Tuple(a), Value::Tuple(b)) => a.cmp(b),
            (Value::Set(a), Value::Set(b)) => a.cmp(b),
            (Value::Map(a), Value::Map(b)) => a.cmp(b),
            (Value::Record(a), Value::Record(b)) => a.cmp(b),
            _ => self.tag().cmp(&o.tag()),
        }
    }
}
impl PartialOrd for Value {
    fn partial_cmp(&self, o: &Value) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl PartialEq for Value {
    fn eq(&self, o: &Value) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Value {}

fn join<T>(f: &mut fmt::Formatter<'_>, it: impl Iterator<Item = T>, each: impl Fn(&mut fmt::Formatter<'_>, T) -> fmt::Result) -> fmt::Result {
    for (i, x) in it.enumerate() {
        if i > 0 {
            write!(f, ", ")?;
        }
        each(f, x)?;
    }
    Ok(())
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Bool(b) => write!(f, "{b}"),
            Value::Unknown(r) => write!(f, "unknown({r})"),
            Value::None => write!(f, "none"),
            Value::Num(n, d) => write!(f, "{n}{}", d.unit_suffix()),
            Value::Str(s) => write!(f, "{s:?}"),
            Value::Model(s) => write!(f, "{s}"),
            Value::List(v) => {
                write!(f, "[")?;
                join(f, v.iter(), |f, x| write!(f, "{x}"))?;
                write!(f, "]")
            }
            Value::Tuple(v) => {
                write!(f, "(")?;
                join(f, v.iter(), |f, x| write!(f, "{x}"))?;
                write!(f, ")")
            }
            Value::Set(v) => {
                write!(f, "{{")?;
                join(f, v.iter(), |f, x| write!(f, "{x}"))?;
                write!(f, "}}")
            }
            Value::Map(m) => {
                write!(f, "{{")?;
                join(f, m.iter(), |f, (k, v)| write!(f, "{k}: {v}"))?;
                write!(f, "}}")
            }
            Value::Record(m) => {
                write!(f, "{{")?;
                join(f, m.iter(), |f, (k, v)| write!(f, "{k}: {v}"))?;
                write!(f, "}}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_is_canonical_for_sets_and_maps() {
        let (mut ea, mut eb) = (vec![], vec![]);
        Value::set([Value::str("x"), Value::str("y")]).encode(&mut ea);
        Value::set([Value::str("y"), Value::str("x")]).encode(&mut eb);
        assert_eq!(ea, eb);
        let (mut e1, mut e2) = (vec![], vec![]);
        Value::list(vec![Value::str("ab")]).encode(&mut e1);
        Value::list(vec![Value::str("a"), Value::str("b")]).encode(&mut e2);
        assert_ne!(e1, e2);
        assert_ne!(fingerprint(&e1), fingerprint(&e2));
    }

    #[test]
    fn display_carries_units() {
        assert_eq!(Value::Num(Num::int(3), Dim::CREDITS).to_string(), "3 cr");
        assert_eq!(Value::list(vec![Value::str("owner")]).to_string(), "[\"owner\"]");
    }
}
