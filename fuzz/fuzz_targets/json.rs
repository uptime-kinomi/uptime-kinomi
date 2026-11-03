#![no_main]

//! Besides the checks in `fuzzing::json`: kinomi's JSON parser agrees with
//! serde_json on whether a document is valid and on the value at the path.

use std::collections::HashSet;
use std::fmt;

use libfuzzer_sys::fuzz_target;
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use uptime_kinomi::fuzzing::{json, split_json_input};
use uptime_kinomi::monitors::json::{Found, Invalid, MAX_VALUE, find, parse_path};

fuzz_target!(|data: &[u8]| {
    json(data);
    let (path_text, body) = split_json_input(data);
    let Ok(path) = parse_path(path_text) else {
        return;
    };
    let ours = find(body, &path);
    let theirs = serde_json::from_slice::<Value>(body);
    match (&ours, &theirs) {
        (Err(Invalid::TooDeep), _) => return,
        // serde_json rejects numbers outside f64; they are valid JSON.
        (Ok(_), Err(e)) if e.to_string().contains("number out of range") => return,
        (Ok(_), Err(e)) => panic!("kinomi accepts what serde_json rejects: {e}"),
        (Err(_), Ok(_)) => panic!("kinomi rejects what serde_json accepts"),
        (Err(_), Err(_)) => return,
        (Ok(_), Ok(_)) => {}
    }
    // serde_json keeps the last of repeated keys, kinomi the first.
    if serde_json::from_slice::<NoDuplicates>(body).is_err() {
        return;
    }
    let (Ok(found), Ok(value)) = (ours, theirs) else {
        return;
    };
    let mut at = Some(&value);
    for segment in path_text.split('.') {
        at = at.and_then(|v| match v {
            Value::Object(map) => map.get(segment),
            Value::Array(items) => segment.parse::<usize>().ok().and_then(|i| items.get(i)),
            _ => None,
        });
    }
    match (found, at) {
        (None, None) => {}
        (Some(Found::Str(s)), Some(Value::String(t))) => assert_eq!(&s, t),
        (Some(Found::TooLong), Some(Value::String(t))) => assert!(t.len() > MAX_VALUE),
        (Some(Found::Num(n)), Some(Value::Number(m))) => {
            // serde_json's float parsing may be off by one unit in the last place.
            let (a, b) = (
                n.parse::<f64>().unwrap_or(f64::NAN),
                m.as_f64().unwrap_or(f64::NAN),
            );
            assert!(
                a == b || (a - b).abs() <= a.abs() * 1e-15,
                "number {n}: {a} vs {b}"
            );
        }
        (Some(Found::TooLong), Some(Value::Number(_))) => {}
        (Some(Found::Bool(a)), Some(Value::Bool(b))) => assert_eq!(a, *b),
        (Some(Found::Null), Some(Value::Null)) => {}
        (Some(Found::Container), Some(Value::Object(_) | Value::Array(_))) => {}
        (ours, theirs) => panic!("kinomi found {ours:?}, serde_json {theirs:?}"),
    }
});

/// Any JSON value; fails on an object with a repeated key.
struct NoDuplicates;

impl<'de> Deserialize<'de> for NoDuplicates {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(NoDuplicatesVisitor)
    }
}

struct NoDuplicatesVisitor;

impl<'de> Visitor<'de> for NoDuplicatesVisitor {
    type Value = NoDuplicates;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("JSON")
    }
    fn visit_bool<E>(self, _: bool) -> Result<NoDuplicates, E> {
        Ok(NoDuplicates)
    }
    fn visit_i64<E>(self, _: i64) -> Result<NoDuplicates, E> {
        Ok(NoDuplicates)
    }
    fn visit_u64<E>(self, _: u64) -> Result<NoDuplicates, E> {
        Ok(NoDuplicates)
    }
    fn visit_f64<E>(self, _: f64) -> Result<NoDuplicates, E> {
        Ok(NoDuplicates)
    }
    fn visit_str<E>(self, _: &str) -> Result<NoDuplicates, E> {
        Ok(NoDuplicates)
    }
    fn visit_unit<E>(self) -> Result<NoDuplicates, E> {
        Ok(NoDuplicates)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<NoDuplicates, A::Error> {
        while seq.next_element::<NoDuplicates>()?.is_some() {}
        Ok(NoDuplicates)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<NoDuplicates, A::Error> {
        let mut keys = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(de::Error::custom("repeated key"));
            }
            map.next_value::<NoDuplicates>()?;
        }
        Ok(NoDuplicates)
    }
}
