//! The APZW1 payload codec (port of trainer/apz_train/wire.py; protocol in trainer/PROTOCOL.md).
//!
//! Layout: b"APZW1" | uint32 big-endian header length | UTF-8 JSON header | tensor bytes.
//! The header is {"meta": {...}, "tensors": [{"name", "shape", "dtype", "offset", "nbytes"}, ...]}.
//! Tensors are sorted by name, little-endian float32, and `offset` is relative to the first byte
//! after the JSON. Nothing is pickled.

use std::collections::BTreeMap;

use serde_json::{Value, json};

const MAGIC: &[u8] = b"APZW1";
const PREFIX: usize = MAGIC.len() + 4;

pub type Tensors = BTreeMap<String, Tensor>;

#[derive(Clone, Debug, PartialEq)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

/// Encodes `meta` and `tensors`. `tensors.data` must hold `shape` elements; `unpack` rejects anything else.
pub fn pack(meta: &Value, tensors: &Tensors) -> Vec<u8> {
    let mut index = Vec::with_capacity(tensors.len());
    let mut body = Vec::new();
    for (name, t) in tensors {
        index.push(json!({
            "name": name,
            "shape": t.shape,
            "dtype": "float32",
            "offset": body.len(),
            "nbytes": t.data.len() * 4,
        }));
        for x in &t.data {
            body.extend_from_slice(&x.to_le_bytes());
        }
    }
    let header = serde_json::to_vec(&json!({ "meta": meta, "tensors": index }))
        .expect("a JSON value always serialises");
    let header_len = u32::try_from(header.len()).expect("header under 4 GiB");
    let mut out = Vec::with_capacity(PREFIX + header.len() + body.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&header_len.to_be_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(&body);
    out
}

/// Decodes a payload into its meta object and tensors. Errors on a bad magic, a truncated input,
/// a non-float32 dtype, or a tensor whose byte length is not `shape * 4` or that runs past the body.
pub fn unpack(bytes: &[u8]) -> Result<(Value, Tensors), String> {
    if !bytes.starts_with(MAGIC) {
        return Err("not an APZW1 payload".into());
    }
    if bytes.len() < PREFIX {
        return Err("short payload: no header length".into());
    }
    let header_len = u32::from_be_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]) as usize;
    let body_start = PREFIX
        .checked_add(header_len)
        .filter(|&end| end <= bytes.len())
        .ok_or("short payload: header runs past the input")?;

    let header: Value = serde_json::from_slice(&bytes[PREFIX..body_start])
        .map_err(|e| format!("bad header JSON: {e}"))?;
    let meta = header.get("meta").cloned().ok_or("header has no meta")?;
    let index = header
        .get("tensors")
        .and_then(Value::as_array)
        .ok_or("header has no tensors list")?;

    let body = &bytes[body_start..];
    let mut tensors = Tensors::new();
    for entry in index {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .ok_or("tensor without a name")?;
        let dtype = entry
            .get("dtype")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if dtype != "float32" {
            return Err(format!("unsupported dtype {dtype:?} for tensor {name}"));
        }
        let shape = entry
            .get("shape")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("tensor {name}: no shape"))?
            .iter()
            .map(|d| {
                d.as_u64()
                    .and_then(|d| usize::try_from(d).ok())
                    .ok_or_else(|| format!("tensor {name}: bad shape"))
            })
            .collect::<Result<Vec<usize>, String>>()?;
        let elems = shape
            .iter()
            .try_fold(1u64, |acc, &d| acc.checked_mul(d as u64))
            .ok_or_else(|| format!("tensor {name}: shape overflows"))?;
        let nbytes = field_u64(entry, "nbytes", name)?;
        let offset = field_u64(entry, "offset", name)?;
        if elems.checked_mul(4) != Some(nbytes) {
            return Err(format!(
                "tensor {name}: {nbytes} bytes do not match shape {shape:?}"
            ));
        }
        let end = offset
            .checked_add(nbytes)
            .filter(|&end| end <= body.len() as u64)
            .ok_or_else(|| format!("tensor {name} runs past the body"))?;
        let raw = &body[offset as usize..end as usize];
        let data = raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        tensors.insert(name.to_string(), Tensor { shape, data });
    }
    Ok((meta, tensors))
}

fn field_u64(entry: &Value, key: &str, name: &str) -> Result<u64, String> {
    entry
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("tensor {name}: bad {key}"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Written by trainer/apz_train/wire.py via testdata/diloco_fixture.py.
    const FIXTURE: &[u8] = include_bytes!("testdata/wire_fixture.apzw");

    fn sample() -> Tensors {
        let mut t = Tensors::new();
        t.insert(
            "w".into(),
            Tensor {
                shape: vec![2, 3],
                data: vec![0.5, -1.0, 0.25, 2.0, -0.125, 0.75],
            },
        );
        t.insert(
            "b".into(),
            Tensor {
                shape: vec![4],
                data: vec![1.0, -2.0, 0.5, 4.0],
            },
        );
        t
    }

    /// A payload with a hand-written header and body, for the rejection cases.
    fn frame(header: &str, body: &[u8]) -> Vec<u8> {
        let mut v = b"APZW1".to_vec();
        v.extend_from_slice(&(header.len() as u32).to_be_bytes());
        v.extend_from_slice(header.as_bytes());
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn round_trip() {
        let meta = json!({"island": "a", "round": 3, "samples": 12.5});
        let bytes = pack(&meta, &sample());
        assert_eq!(&bytes[..5], b"APZW1");
        let (m, t) = unpack(&bytes).unwrap();
        assert_eq!(m, meta);
        assert_eq!(t, sample());
    }

    #[test]
    fn decodes_python_payload() {
        let (meta, t) = unpack(FIXTURE).unwrap();
        assert_eq!(meta, json!({"island": "a", "round": 3, "samples": 12.5}));
        assert_eq!(t, sample());
    }

    #[test]
    fn unpack_refuses_malformed_payloads() {
        // A payload that is not APZW1 is named as such.
        {
            let mut bytes = FIXTURE.to_vec();
            bytes[4] = b'2';
            assert!(unpack(&bytes).unwrap_err().contains("not an APZW1 payload"));
        }
        // Input too short for the magic, the header or the body is refused.
        {
            assert!(unpack(b"APZ").is_err());
            assert!(unpack(&FIXTURE[..7]).is_err());
            assert!(unpack(&FIXTURE[..FIXTURE.len() - 1]).is_err());
        }
        // A bad tensor is refused, and the error names its reason.
        {
            // (case, header, body, the text the error must name)
            let cases: [(&str, &str, Vec<u8>, &str); 3] = [
                (
                    "a dtype other than float32",
                    r#"{"meta":{},"tensors":[{"name":"x","shape":[1],"dtype":"int32","offset":0,"nbytes":4}]}"#,
                    vec![0; 4],
                    "int32",
                ),
                (
                    "a length that does not match the shape",
                    r#"{"meta":{},"tensors":[{"name":"x","shape":[2,3],"dtype":"float32","offset":0,"nbytes":20}]}"#,
                    vec![0; 20],
                    "do not match shape",
                ),
                (
                    "a tensor that runs past the body",
                    r#"{"meta":{},"tensors":[{"name":"x","shape":[4],"dtype":"float32","offset":8,"nbytes":16}]}"#,
                    vec![0; 16],
                    "past the body",
                ),
            ];
            for (case, header, body, want) in cases {
                let err = unpack(&frame(header, &body)).unwrap_err();
                assert!(err.contains(want), "{case}: {err}");
            }
        }
    }
}
