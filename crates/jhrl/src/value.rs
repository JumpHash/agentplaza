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

    /// Canonical encoding: equal values give equal bytes (sets and maps are ordered). Compact and
    /// self-delimiting: each value starts with a tag byte, lengths and counts are LEB128 varints,
    /// and an integer with no dimension is one zigzag varint (one or two bytes when small).
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Value::Bool(b) => out.push(u8::from(*b)),
            Value::Unknown(r) => {
                out.push(2);
                put_str(out, r);
            }
            Value::None => out.push(3),
            Value::Num(Num::Rat(a, 1), d) if d.is_none() => {
                out.push(4);
                put_ivar(out, *a);
            }
            Value::Num(n, d) => {
                out.push(5);
                for e in d.0 {
                    out.push(e as u8);
                }
                match n {
                    Num::Rat(a, b) => {
                        out.push(0);
                        put_ivar(out, *a);
                        put_uvar(out, *b as u128);
                    }
                    Num::Real(x) => {
                        out.push(1);
                        out.extend(x.to_bits().to_le_bytes());
                    }
                }
            }
            Value::Str(x) => {
                out.push(6);
                put_str(out, x);
            }
            Value::Model(x) => {
                out.push(7);
                put_str(out, x);
            }
            Value::List(v) => put_values(out, 8, v.iter()),
            Value::Set(v) => put_values(out, 9, v.iter()),
            Value::Map(m) => {
                out.push(10);
                put_uvar(out, m.len() as u128);
                for (k, v) in m.iter() {
                    k.encode(out);
                    v.encode(out);
                }
            }
            Value::Record(m) => {
                out.push(11);
                put_uvar(out, m.len() as u128);
                for (k, v) in m.iter() {
                    put_str(out, k);
                    v.encode(out);
                }
            }
            Value::Tuple(v) => put_values(out, 12, v.iter()),
        }
    }

    /// Reads one value written by [`Value::encode`] from the front of `b` and advances `b` past it.
    /// The bytes must come from `encode`: anything else is a bug, and it panics.
    pub fn decode(b: &mut &[u8]) -> Value {
        match take(b, 1)[0] {
            0 => Value::Bool(false),
            1 => Value::Bool(true),
            2 => Value::Unknown(take_str(b).to_owned()),
            3 => Value::None,
            4 => Value::Num(Num::int(take_ivar(b)), Dim::NONE),
            5 => {
                let d = Dim(std::array::from_fn(|_| take(b, 1)[0] as i8));
                let n = match take(b, 1)[0] {
                    0 => {
                        let a = take_ivar(b);
                        Num::Rat(a, take_uvar(b) as i128)
                    }
                    1 => Num::Real(f64::from_bits(u64::from_le_bytes(take_array(b)))),
                    k => unreachable!("bad number kind {k}"),
                };
                Value::Num(n, d)
            }
            6 => Value::Str(Arc::from(take_str(b))),
            7 => Value::Model(Arc::from(take_str(b))),
            8 => Value::List(Arc::new(take_values(b))),
            9 => Value::Set(Arc::new(
                (0..take_len(b)).map(|_| Value::decode(b)).collect(),
            )),
            10 => Value::Map(Arc::new(
                (0..take_len(b))
                    .map(|_| (Value::decode(b), Value::decode(b)))
                    .collect(),
            )),
            11 => Value::Record(Arc::new(
                (0..take_len(b))
                    .map(|_| (take_str(b).to_owned(), Value::decode(b)))
                    .collect(),
            )),
            12 => Value::Tuple(Arc::new(take_values(b))),
            t => unreachable!("bad value tag {t}"),
        }
    }
}

/// Splits `n` bytes off the front of `b`.
fn take<'a>(b: &mut &'a [u8], n: usize) -> &'a [u8] {
    let (head, rest) = std::mem::take(b).split_at(n);
    *b = rest;
    head
}

/// Splits `N` bytes off the front of `b`, as an array.
fn take_array<const N: usize>(b: &mut &[u8]) -> [u8; N] {
    take(b, N).try_into().unwrap()
}

/// Writes `x` as an unsigned LEB128 varint: seven bits per byte, the high bit set on every byte
/// but the last.
fn put_uvar(out: &mut Vec<u8>, mut x: u128) {
    while x >= 0x80 {
        out.push((x as u8) | 0x80);
        x >>= 7;
    }
    out.push(x as u8);
}

/// Writes a signed integer as a zigzag varint, so small negative numbers stay short.
fn put_ivar(out: &mut Vec<u8>, a: i128) {
    put_uvar(out, ((a << 1) ^ (a >> 127)) as u128);
}

/// Writes a length-prefixed UTF-8 string.
fn put_str(out: &mut Vec<u8>, s: &str) {
    put_uvar(out, s.len() as u128);
    out.extend_from_slice(s.as_bytes());
}

/// Writes a tag, a count and the elements: the body of a list, set or tuple.
fn put_values<'a>(out: &mut Vec<u8>, tag: u8, items: impl ExactSizeIterator<Item = &'a Value>) {
    out.push(tag);
    put_uvar(out, items.len() as u128);
    for x in items {
        x.encode(out);
    }
}

/// Reads an unsigned varint, the inverse of `put_uvar`. One too long for a u128 panics.
fn take_uvar(b: &mut &[u8]) -> u128 {
    let (mut x, mut shift) = (0u128, 0u32);
    loop {
        let byte = take(b, 1)[0];
        x |= u128::from(byte & 0x7f)
            .checked_shl(shift)
            .expect("varint is too long");
        if byte < 0x80 {
            return x;
        }
        shift += 7;
    }
}

/// Reads a zigzag varint, the inverse of `put_ivar`.
fn take_ivar(b: &mut &[u8]) -> i128 {
    let z = take_uvar(b);
    ((z >> 1) as i128) ^ -((z & 1) as i128)
}

/// Reads a varint length or count.
fn take_len(b: &mut &[u8]) -> usize {
    take_uvar(b) as usize
}

/// Reads a length-prefixed UTF-8 string.
fn take_str<'a>(b: &mut &'a [u8]) -> &'a str {
    let n = take_len(b);
    std::str::from_utf8(take(b, n)).expect("encoded strings are UTF-8")
}

/// Reads a count and that many values: the body of a list or a tuple.
fn take_values(b: &mut &[u8]) -> Vec<Value> {
    let n = take_len(b);
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(Value::decode(b));
    }
    v
}

/// A 128-bit fingerprint of a canonical encoding: two std SipHash streams with different
/// seeds. As with any fingerprinting model checker, a collision would merge two states; for n states its
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

fn join<T>(
    f: &mut fmt::Formatter<'_>,
    it: impl Iterator<Item = T>,
    each: impl Fn(&mut fmt::Formatter<'_>, T) -> fmt::Result,
) -> fmt::Result {
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
    fn decode_reverses_encode() {
        let values = vec![
            Value::Bool(true),
            Value::Unknown("x is missing".into()),
            Value::None,
            Value::int(-42),
            Value::Num(Num::Real(0.5), Dim::TIME),
            Value::str("héllo"),
            Value::Model("ready".into()),
            Value::list(vec![Value::int(1), Value::str("two")]),
            Value::set([Value::str("x"), Value::str("y")]),
            Value::map(BTreeMap::from([
                (Value::str("k1"), Value::int(1)),
                (Value::str("k2"), Value::int(2)),
            ])),
            Value::record(BTreeMap::from([
                ("a".to_string(), Value::Bool(false)),
                ("b".to_string(), Value::None),
            ])),
            Value::tuple(vec![Value::int(3), Value::Bool(true)]),
        ];
        let mut bytes = Vec::new();
        for v in &values {
            v.encode(&mut bytes);
        }
        // Equality is the total order, which cannot tell Rat from Real, so also compare bytes.
        let mut rest = bytes.as_slice();
        let mut again = Vec::new();
        for v in &values {
            let d = Value::decode(&mut rest);
            assert_eq!(&d, v);
            d.encode(&mut again);
        }
        assert!(rest.is_empty());
        assert_eq!(again, bytes);
    }

    #[test]
    fn compact_integers_and_lengths() {
        let bytes = |v: &Value| {
            let mut out = Vec::new();
            v.encode(&mut out);
            out
        };
        assert_eq!(bytes(&Value::int(0)), [4u8, 0]);
        assert_eq!(bytes(&Value::int(-1)), [4u8, 1]);
        assert_eq!(bytes(&Value::int(300)).len(), 3);
        assert_eq!(bytes(&Value::Bool(true)), [1u8]);
        // Each value decodes back to an equal value, consumes exactly its bytes, and re-encodes
        // to the same bytes.
        let values = [
            Value::int(i128::MIN),
            Value::int(i128::MAX),
            Value::Num(Num::rat(1, 3).unwrap(), Dim::NONE),
            Value::Num(Num::int(5), Dim::MONEY),
            Value::Num(Num::rat(-7, 2).unwrap(), Dim::TIME.inv().unwrap()),
            Value::str(""),
            Value::list(vec![Value::int(-300), Value::Unknown("u".repeat(200))]),
        ];
        for v in &values {
            let e = bytes(v);
            let mut rest = e.as_slice();
            let d = Value::decode(&mut rest);
            assert!(rest.is_empty());
            assert_eq!(&d, v);
            assert_eq!(bytes(&d), e);
        }
    }
}
