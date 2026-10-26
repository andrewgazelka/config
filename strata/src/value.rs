//! Values, their total order, and their canonical encoding.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

/// A fully evaluated value. Records keep keys in byte order, which is also
/// the order every renderer emits.
#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Arc<str>),
    List(Arc<[Value]>),
    Record(Arc<BTreeMap<String, Value>>),
}

impl Value {
    pub fn str(s: impl Into<Arc<str>>) -> Self {
        Value::Str(s.into())
    }

    /// Integral floats become ints so `1.0` and `1` are one value.
    pub fn float(f: f64) -> Self {
        if f.fract() == 0.0 && f.abs() < 9.0e15 { Value::Int(f as i64) } else { Value::Float(f) }
    }

    pub fn record(map: BTreeMap<String, Value>) -> Self {
        Value::Record(Arc::new(map))
    }

    pub fn list(items: Vec<Value>) -> Self {
        Value::List(items.into())
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "number",
            Value::Str(_) => "string",
            Value::List(_) => "list",
            Value::Record(_) => "record",
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_record(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Record(r) => Some(r),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    fn rank(&self) -> u8 {
        match self {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::Int(_) | Value::Float(_) => 2,
            Value::Str(_) => 3,
            Value::List(_) => 4,
            Value::Record(_) => 5,
        }
    }

    /// blake3 of the canonical encoding: the value's identity.
    pub fn hash(&self) -> blake3::Hash {
        blake3::hash(&self.canonical_cbor())
    }

    /// Deterministic CBOR (RFC 8949 section 4.2.1): shortest heads, definite
    /// lengths, map keys sorted by their encoded bytes, shortest exact floats.
    pub fn canonical_cbor(&self) -> Vec<u8> {
        let mut out = Vec::new();
        encode(self, &mut out);
        out
    }

    /// The text a key=value format shows for a scalar.
    pub fn display_scalar(&self) -> Option<String> {
        Some(match self {
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => serde_json::to_string(f).ok()?,
            Value::Str(s) => s.to_string(),
            Value::List(_) | Value::Record(_) => return None,
        })
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Value {}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Total order: by kind, then naturally (numbers numerically, strings by bytes).
impl Ord for Value {
    fn cmp(&self, other: &Self) -> Ordering {
        use Value::*;
        match (self, other) {
            (Null, Null) => Ordering::Equal,
            (Bool(a), Bool(b)) => a.cmp(b),
            (Int(a), Int(b)) => a.cmp(b),
            (Int(a), Float(b)) => (*a as f64).total_cmp(b),
            (Float(a), Int(b)) => a.total_cmp(&(*b as f64)),
            (Float(a), Float(b)) => a.total_cmp(b),
            (Str(a), Str(b)) => a.as_bytes().cmp(b.as_bytes()),
            (List(a), List(b)) => a.iter().cmp(b.iter()),
            (Record(a), Record(b)) => a.iter().cmp(b.iter()),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = serde_json::to_string(self).map_err(|_| fmt::Error)?;
        const MAX: usize = 120;
        if text.chars().count() > MAX {
            let cut: String = text.chars().take(MAX).collect();
            write!(f, "{cut}... ({} bytes)", text.len())
        } else {
            f.write_str(&text)
        }
    }
}

impl serde::Serialize for Value {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self {
            Value::Null => s.serialize_none(),
            Value::Bool(b) => s.serialize_bool(*b),
            Value::Int(i) => s.serialize_i64(*i),
            Value::Float(f) => s.serialize_f64(*f),
            Value::Str(v) => s.serialize_str(v),
            Value::List(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items.iter() {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Value::Record(map) => {
                let mut m = s.serialize_map(Some(map.len()))?;
                for (k, v) in map.iter() {
                    m.serialize_entry(k, v)?;
                }
                m.end()
            }
        }
    }
}

impl From<&serde_json::Value> for Value {
    fn from(j: &serde_json::Value) -> Self {
        match j {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(b) => Value::Bool(*b),
            serde_json::Value::Number(n) => match n.as_i64() {
                Some(i) => Value::Int(i),
                None => Value::float(n.as_f64().unwrap_or(f64::NAN)),
            },
            serde_json::Value::String(s) => Value::str(s.as_str()),
            serde_json::Value::Array(a) => Value::list(a.iter().map(Value::from).collect()),
            serde_json::Value::Object(o) => Value::record(o.iter().map(|(k, v)| (k.clone(), Value::from(v))).collect()),
        }
    }
}

fn head(major: u8, n: u64, out: &mut Vec<u8>) {
    let m = major << 5;
    match n {
        0..=23 => out.push(m | n as u8),
        24..=0xff => out.extend([m | 24, n as u8]),
        0x100..=0xffff => {
            out.push(m | 25);
            out.extend((n as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(m | 26);
            out.extend((n as u32).to_be_bytes());
        }
        _ => {
            out.push(m | 27);
            out.extend(n.to_be_bytes());
        }
    }
}

fn encode_float(f: f64, out: &mut Vec<u8>) {
    // Preferred serialization: the shortest IEEE width that round-trips.
    if let Some(h) = f16_exact(f) {
        out.push(0xf9);
        out.extend(h.to_be_bytes());
    } else if (f as f32) as f64 == f {
        out.push(0xfa);
        out.extend((f as f32).to_be_bytes());
    } else {
        out.push(0xfb);
        out.extend(f.to_be_bytes());
    }
}

/// Half-precision bits for `f` when the conversion is exact.
fn f16_exact(f: f64) -> Option<u16> {
    let single = f as f32;
    if single as f64 != f {
        return None;
    }
    let bits = single.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7f_ffff;
    if exp == 0xff {
        return Some(sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 });
    }
    if exp == 0 && mant == 0 {
        return Some(sign);
    }
    let e = exp - 127;
    if (-14..=15).contains(&e) {
        // Normal half: 10 mantissa bits must hold all set bits.
        if mant & 0x1fff != 0 {
            return None;
        }
        return Some(sign | (((e + 15) as u16) << 10) | (mant >> 13) as u16);
    }
    if (-24..-14).contains(&e) {
        // Subnormal half.
        let full = mant | 0x80_0000;
        let shift = 13 + (-14 - e);
        if full & ((1 << shift) - 1) != 0 {
            return None;
        }
        return Some(sign | (full >> shift) as u16);
    }
    None
}

fn encode(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Null => out.push(0xf6),
        Value::Bool(false) => out.push(0xf4),
        Value::Bool(true) => out.push(0xf5),
        Value::Int(i) if *i >= 0 => head(0, *i as u64, out),
        Value::Int(i) => head(1, (-1 - *i) as u64, out),
        Value::Float(f) => encode_float(*f, out),
        Value::Str(s) => {
            head(3, s.len() as u64, out);
            out.extend(s.as_bytes());
        }
        Value::List(items) => {
            head(4, items.len() as u64, out);
            for item in items.iter() {
                encode(item, out);
            }
        }
        Value::Record(map) => {
            let mut entries: Vec<(Vec<u8>, &Value)> = map
                .iter()
                .map(|(k, v)| {
                    let mut key = Vec::new();
                    encode(&Value::str(k.as_str()), &mut key);
                    (key, v)
                })
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            head(5, entries.len() as u64, out);
            for (key, value) in entries {
                out.extend(key);
                encode(value, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(v: &Value) -> String {
        v.canonical_cbor().iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn rfc8949_appendix_a_vectors() {
        assert_eq!(hex(&Value::Int(0)), "00");
        assert_eq!(hex(&Value::Int(23)), "17");
        assert_eq!(hex(&Value::Int(24)), "1818");
        assert_eq!(hex(&Value::Int(1000)), "1903e8");
        assert_eq!(hex(&Value::Int(1_000_000)), "1a000f4240");
        assert_eq!(hex(&Value::Int(-1)), "20");
        assert_eq!(hex(&Value::Int(-1000)), "3903e7");
        assert_eq!(hex(&Value::Float(1.1)), "fb3ff199999999999a");
        assert_eq!(hex(&Value::Float(1.5)), "f93e00");
        assert_eq!(hex(&Value::Float(100000.5)), "fa47c35040");
        assert_eq!(hex(&Value::Float(5.960464477539063e-8)), "f90001");
        assert_eq!(hex(&Value::Float(-4.1)), "fbc010666666666666");
        assert_eq!(hex(&Value::str("IETF")), "6449455446");
        assert_eq!(hex(&Value::Null), "f6");
        assert_eq!(hex(&Value::Bool(true)), "f5");
        assert_eq!(hex(&Value::list(vec![Value::Int(1), Value::Int(2)])), "820102");
    }

    #[test]
    fn map_keys_sort_by_encoded_bytes_shorter_first() {
        // Byte order would put "aa" before "b"; canonical CBOR puts "b" first.
        let v = Value::record([("aa".into(), Value::Int(1)), ("b".into(), Value::Int(2))].into());
        assert_eq!(hex(&v), "a261620262616101");
    }

    #[test]
    fn integral_floats_are_ints() {
        assert_eq!(Value::float(12.0), Value::Int(12));
        assert_eq!(Value::float(12.0).hash(), Value::Int(12).hash());
        assert_ne!(Value::float(1.1).hash(), Value::Int(1).hash());
    }
}
