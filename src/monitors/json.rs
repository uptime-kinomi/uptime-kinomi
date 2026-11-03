//! JSON checks of the `http` monitor (SPEC.md §4.3, "JSON checks").
//!
//! The response body comes from an untrusted server. It is parsed in one
//! pass, without recursion and without building a tree: memory is the body
//! (already limited by `max_body_bytes`), a stack of at most [`MAX_DEPTH`]
//! levels and the one value the path points to. The whole document is
//! validated, so a broken response never passes. Only the fixed messages
//! below leave this module; nothing from the body does.

use std::time::Duration;

/// Deepest nesting of arrays and objects.
pub const MAX_DEPTH: usize = 64;
/// Longest value kept for a check (bytes after unescaping).
pub const MAX_VALUE: usize = 1024;
/// Longest number accepted by `json_min`/`json_max`.
const MAX_NUMBER: usize = 64;
/// Longest date string accepted by `json_max_age`.
const MAX_DATE: usize = 64;
/// A date this far in the future still counts as now (clock skew).
const FUTURE_TOLERANCE: i64 = 300;
const MAX_SEGMENTS: usize = 16;
const MAX_SEGMENT: usize = 64;
pub const MAX_PATH: usize = 256;

/// One step of a `json_path`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    key: String,
    /// The segment is all digits: also an index into an array.
    index: Option<usize>,
}

/// Parse `json_path`: `a.b.0.c`, see SPEC.md.
pub fn parse_path(text: &str) -> Result<Vec<Segment>, String> {
    let err = || {
        format!(
            "http.json_path must be 1 to {MAX_SEGMENTS} keys or indices separated by '.', \
             each 1 to {MAX_SEGMENT} characters from A-Z a-z 0-9 _ $ @ -"
        )
    };
    if text.is_empty() || text.len() > MAX_PATH {
        return Err(err());
    }
    let segments: Vec<Segment> = text
        .split('.')
        .map(|s| {
            let valid = !s.is_empty()
                && s.len() <= MAX_SEGMENT
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_$@-".contains(&b));
            valid.then(|| Segment {
                key: s.to_string(),
                index: if s.bytes().all(|b| b.is_ascii_digit()) {
                    s.parse().ok()
                } else {
                    None
                },
            })
        })
        .collect::<Option<_>>()
        .ok_or_else(err)?;
    if segments.len() > MAX_SEGMENTS {
        return Err(err());
    }
    Ok(segments)
}

/// What the configured check compares the value with.
#[derive(Clone, Debug, PartialEq)]
pub enum Expect {
    MaxAge(Duration),
    Str(String),
    Int(i64),
    Bool(bool),
    Range { min: Option<f64>, max: Option<f64> },
}

/// The value at the path.
#[derive(Debug, PartialEq)]
pub enum Found {
    Str(String),
    /// The number as written (validated JSON number syntax).
    Num(String),
    Bool(bool),
    Null,
    Container,
    /// A string or number longer than [`MAX_VALUE`].
    TooLong,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Invalid {
    /// Not JSON (syntax, UTF-8, BOM, trailing data).
    Syntax,
    /// Nested deeper than [`MAX_DEPTH`].
    TooDeep,
}

/// The result of a check: `Ok(())` or kinomi's own message.
pub fn evaluate(body: &[u8], path: &[Segment], expect: &Expect, now: i64) -> Result<(), String> {
    let found = match find(body, path) {
        Err(_) => return Err("json: invalid".into()),
        Ok(None) => return Err("json: not found".into()),
        Ok(Some(found)) => found,
    };
    let wrong_type = || Err("json: wrong type".to_string());
    match (expect, found) {
        (Expect::MaxAge(max), Found::Str(s)) => {
            let ts = (s.len() <= MAX_DATE)
                .then(|| s.parse::<jiff::Timestamp>().ok())
                .flatten()
                .map(|t| t.as_second());
            match ts {
                Some(ts) => age(ts, *max, now),
                None => Err("json: not a date".into()),
            }
        }
        (Expect::MaxAge(max), Found::Num(n)) => match integer(&n) {
            Some(ts) => age(ts, *max, now),
            None => Err("json: not a date".into()),
        },
        (Expect::Str(want), Found::Str(s)) => equal(*want == s),
        (Expect::Bool(want), Found::Bool(b)) => equal(*want == b),
        (Expect::Int(want), Found::Num(n)) => equal(match integer(&n) {
            Some(i) => i == *want,
            // 42.0 or 4.2e1: equal if exactly that integer.
            None => n
                .parse::<f64>()
                .is_ok_and(|f| want.unsigned_abs() <= 1 << 53 && f == *want as f64),
        }),
        (Expect::Range { min, max }, Found::Num(n)) if n.len() <= MAX_NUMBER => {
            let Ok(v) = n.parse::<f64>() else {
                return wrong_type();
            };
            let inside = min.is_none_or(|m| v >= m) && max.is_none_or(|m| v <= m);
            if inside {
                Ok(())
            } else {
                Err("json: out of range".into())
            }
        }
        _ => wrong_type(),
    }
}

fn equal(same: bool) -> Result<(), String> {
    if same {
        Ok(())
    } else {
        Err("json: value differs".into())
    }
}

fn age(ts: i64, max: Duration, now: i64) -> Result<(), String> {
    if ts > now.saturating_add(FUTURE_TOLERANCE) {
        return Err("json: date in the future".into());
    }
    let age = now.saturating_sub(ts);
    if age > i64::try_from(max.as_secs()).unwrap_or(i64::MAX) {
        // Only a number kinomi computed goes into the message.
        return Err(format!("json: too old ({}d)", age / 86400));
    }
    Ok(())
}

/// A JSON number without fraction or exponent, as i64.
fn integer(n: &str) -> Option<i64> {
    let digits = n.strip_prefix('-').unwrap_or(n);
    if digits.bytes().all(|b| b.is_ascii_digit()) {
        n.parse().ok()
    } else {
        None
    }
}

/// The value at `path` in `body` (the first one if a key repeats), after
/// validating the whole document.
pub fn find(body: &[u8], path: &[Segment]) -> Result<Option<Found>, Invalid> {
    // UTF-8 for the whole document up front; a BOM is not whitespace below.
    let text = std::str::from_utf8(body).map_err(|_| Invalid::Syntax)?;
    let mut p = Parser {
        b: text.as_bytes(),
        pos: 0,
        stack: Vec::new(),
        path,
        found: None,
        buf: Vec::new(),
    };
    p.document()?;
    Ok(p.found)
}

struct Frame {
    object: bool,
    /// This container is the value at `path[..depth]`.
    on_path: bool,
    /// The current member's key or index is the next path segment.
    member_on_path: bool,
    /// An earlier key of this object was the path segment (first one wins).
    matched: bool,
    index: usize,
}

struct Parser<'a> {
    b: &'a [u8],
    pos: usize,
    stack: Vec<Frame>,
    path: &'a [Segment],
    found: Option<Found>,
    /// Scratch for unescaped strings that are compared or kept.
    buf: Vec<u8>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, c: u8) -> Result<(), Invalid> {
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(Invalid::Syntax)
        }
    }

    /// The value about to be parsed is the one at the path.
    fn at_target(&self) -> bool {
        self.found.is_none()
            && self.stack.len() == self.path.len()
            && self
                .stack
                .last()
                .is_some_and(|f| f.on_path && f.member_on_path)
    }

    /// A container about to be opened lies on the path (not at its end).
    fn container_on_path(&self) -> bool {
        self.stack.len() < self.path.len()
            && self
                .stack
                .last()
                .is_none_or(|f| f.on_path && f.member_on_path)
    }

    /// The whole document: one value, then only whitespace. Iterative: the
    /// stack holds the open containers, so no input can overflow the
    /// thread's stack.
    fn document(&mut self) -> Result<(), Invalid> {
        self.ws();
        self.value()?;
        while !self.stack.is_empty() {
            self.ws();
            let object = self.stack.last().is_some_and(|f| f.object);
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.ws();
                    self.member(object)?;
                }
                Some(b'}') if object => {
                    self.pos += 1;
                    self.stack.pop();
                }
                Some(b']') if !object => {
                    self.pos += 1;
                    self.stack.pop();
                }
                _ => return Err(Invalid::Syntax),
            }
        }
        self.ws();
        if self.pos == self.b.len() {
            Ok(())
        } else {
            Err(Invalid::Syntax)
        }
    }

    /// The next member of the open container: `"key": value` or `value`.
    fn member(&mut self, object: bool) -> Result<(), Invalid> {
        let depth = self.stack.len();
        if object {
            let on_path = self.stack.last().is_some_and(|f| f.on_path && !f.matched);
            if self.peek() != Some(b'"') {
                return Err(Invalid::Syntax);
            }
            let is_segment = if on_path {
                self.string(true)?;
                self.path
                    .get(depth - 1)
                    .is_some_and(|s| self.buf == s.key.as_bytes())
            } else {
                self.string(false)?;
                false
            };
            if let Some(f) = self.stack.last_mut() {
                f.member_on_path = is_segment;
                f.matched |= is_segment;
            }
            self.ws();
            self.expect(b':')?;
            self.ws();
        } else {
            let segment = self.path.get(depth - 1).and_then(|s| s.index);
            if let Some(f) = self.stack.last_mut() {
                f.member_on_path = f.on_path && segment == Some(f.index);
                f.index += 1;
            }
        }
        self.value()
    }

    /// One value. Arrays and objects are opened here and continued by
    /// [`Self::document`].
    fn value(&mut self) -> Result<(), Invalid> {
        let target = self.at_target();
        match self.peek() {
            Some(open @ (b'{' | b'[')) => {
                if target {
                    self.found = Some(Found::Container);
                }
                if self.stack.len() == MAX_DEPTH {
                    return Err(Invalid::TooDeep);
                }
                let object = open == b'{';
                self.stack.push(Frame {
                    object,
                    on_path: self.container_on_path(),
                    member_on_path: false,
                    matched: false,
                    index: 0,
                });
                self.pos += 1;
                self.ws();
                let close = if object { b'}' } else { b']' };
                if self.peek() == Some(close) {
                    self.pos += 1;
                    self.stack.pop();
                    Ok(())
                } else {
                    self.member(object)
                }
            }
            Some(b'"') => {
                self.string(target)?;
                if target {
                    self.found = Some(if self.buf.len() > MAX_VALUE {
                        Found::TooLong
                    } else {
                        // Valid UTF-8: the input is, and escapes produce chars.
                        Found::Str(String::from_utf8_lossy(&self.buf).into_owned())
                    });
                }
                Ok(())
            }
            Some(b't') => self.literal("true", target.then_some(Found::Bool(true))),
            Some(b'f') => self.literal("false", target.then_some(Found::Bool(false))),
            Some(b'n') => self.literal("null", target.then_some(Found::Null)),
            Some(b'-' | b'0'..=b'9') => {
                let start = self.pos;
                self.number()?;
                if target {
                    let n = &self.b[start..self.pos];
                    self.found = Some(if n.len() > MAX_VALUE {
                        Found::TooLong
                    } else {
                        Found::Num(String::from_utf8_lossy(n).into_owned())
                    });
                }
                Ok(())
            }
            _ => Err(Invalid::Syntax),
        }
    }

    fn literal(&mut self, word: &str, found: Option<Found>) -> Result<(), Invalid> {
        if self.b[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            if found.is_some() {
                self.found = found;
            }
            Ok(())
        } else {
            Err(Invalid::Syntax)
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        self.pos - start
    }

    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`
    fn number(&mut self) -> Result<(), Invalid> {
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(Invalid::Syntax),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if self.digits() == 0 {
                return Err(Invalid::Syntax);
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if self.digits() == 0 {
                return Err(Invalid::Syntax);
            }
        }
        Ok(())
    }

    /// A string at `pos` (which is `"`). With `keep`, its unescaped bytes
    /// end up in `buf`, at most [`MAX_VALUE`] + 1 of them (enough to tell
    /// that it is too long).
    fn string(&mut self, keep: bool) -> Result<(), Invalid> {
        self.buf.clear();
        self.pos += 1;
        loop {
            let c = self.peek().ok_or(Invalid::Syntax)?;
            self.pos += 1;
            match c {
                b'"' => return Ok(()),
                0..=0x1f => return Err(Invalid::Syntax),
                b'\\' => {
                    let e = self.peek().ok_or(Invalid::Syntax)?;
                    self.pos += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => self.unicode_escape()?,
                        _ => return Err(Invalid::Syntax),
                    };
                    if keep && self.buf.len() <= MAX_VALUE {
                        let mut utf8 = [0; 4];
                        self.buf
                            .extend_from_slice(ch.encode_utf8(&mut utf8).as_bytes());
                    }
                }
                _ => {
                    if keep && self.buf.len() <= MAX_VALUE {
                        self.buf.push(c);
                    }
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, Invalid> {
        let digits = self.b.get(self.pos..self.pos + 4).ok_or(Invalid::Syntax)?;
        let mut v = 0;
        for &d in digits {
            v = v * 16 + char::from(d).to_digit(16).ok_or(Invalid::Syntax)?;
        }
        self.pos += 4;
        Ok(v)
    }

    /// After `\u`: four hex digits, a surrogate pair as two escapes.
    fn unicode_escape(&mut self) -> Result<char, Invalid> {
        let first = self.hex4()?;
        let code = match first {
            0xd800..=0xdbff => {
                if !self.b[self.pos..].starts_with(b"\\u") {
                    return Err(Invalid::Syntax);
                }
                self.pos += 2;
                let low = self.hex4()?;
                if !(0xdc00..=0xdfff).contains(&low) {
                    return Err(Invalid::Syntax);
                }
                0x10000 + ((first - 0xd800) << 10) + (low - 0xdc00)
            }
            0xdc00..=0xdfff => return Err(Invalid::Syntax),
            c => c,
        };
        char::from_u32(code).ok_or(Invalid::Syntax)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(p: &str) -> Vec<Segment> {
        parse_path(p).unwrap()
    }

    fn get(body: &str, p: &str) -> Result<Option<Found>, Invalid> {
        find(body.as_bytes(), &path(p))
    }

    fn s(v: &str) -> Option<Found> {
        Some(Found::Str(v.into()))
    }

    #[test]
    fn paths() {
        assert_eq!(path("a.0.b-c").len(), 3);
        assert_eq!(path("0")[0].index, Some(0));
        assert_eq!(path("x1")[0].index, None);
        for bad in [
            "",
            ".",
            "a..b",
            "a.",
            "a b",
            "a[0]",
            "*",
            &["a"; 17].join("."),
        ] {
            assert!(parse_path(bad).is_err(), "{bad:?}");
        }
        assert!(parse_path(&["a"; 16].join(".")).is_ok());
        assert!(parse_path(&"x".repeat(65)).is_err());
    }

    #[test]
    fn finds_values() {
        let doc = r#"{"a": {"b": [10, {"c": "x"}, true]}, "n": null, "d": -1.5e3}"#;
        assert_eq!(get(doc, "a.b.1.c"), Ok(s("x")));
        assert_eq!(get(doc, "a.b.0"), Ok(Some(Found::Num("10".into()))));
        assert_eq!(get(doc, "a.b.2"), Ok(Some(Found::Bool(true))));
        assert_eq!(get(doc, "n"), Ok(Some(Found::Null)));
        assert_eq!(get(doc, "d"), Ok(Some(Found::Num("-1.5e3".into()))));
        assert_eq!(get(doc, "a"), Ok(Some(Found::Container)));
        assert_eq!(get(doc, "a.b.3"), Ok(None));
        assert_eq!(get(doc, "a.c"), Ok(None));
        assert_eq!(get(doc, "a.b.1.c.d"), Ok(None));
        // Digits are a key on objects, an index on arrays.
        assert_eq!(get(r#"{"0": "zero"}"#, "0"), Ok(s("zero")));
        assert_eq!(get(r#"["zero"]"#, "0"), Ok(s("zero")));
    }

    #[test]
    fn first_key_wins() {
        assert_eq!(
            get(r#"{"a": 1, "a": 2}"#, "a"),
            Ok(Some(Found::Num("1".into())))
        );
        assert_eq!(get(r#"{"a": {"x": 1}, "a": {"b": 2}}"#, "a.b"), Ok(None));
    }

    #[test]
    fn escapes() {
        assert_eq!(
            get(r#"{"key": "a\"\\\/\né😀"}"#, "key"),
            Ok(s("a\"\\/\né😀"))
        );
        for bad in [
            r#""\ud800""#,
            r#""\udc00""#,
            r#""\ud800A""#,
            r#""\x""#,
            r#""\u12""#,
        ] {
            assert_eq!(
                find(bad.as_bytes(), &path("a")),
                Err(Invalid::Syntax),
                "{bad}"
            );
        }
    }

    #[test]
    fn rejects_invalid_documents() {
        for bad in [
            "",
            " ",
            "{",
            "{\"a\":1,}",
            "[1,]",
            "[1 2]",
            "{\"a\" 1}",
            "{1:2}",
            "01",
            "1.",
            "-",
            "1e",
            "+1",
            ".5",
            "tru",
            "nul",
            "\"a",
            "\"\t\"",
            "{} {}",
            "{}x",
            "\u{feff}{}",
            "NaN",
            "[1]]",
            "{]",
        ] {
            assert_eq!(get(bad, "a"), Err(Invalid::Syntax), "{bad:?}");
        }
        assert_eq!(find(b"[\"\xff\"]", &path("0")), Err(Invalid::Syntax));
        // Valid, with surrounding whitespace.
        assert_eq!(
            get(" \n{\"a\" : [ ] }\r\n", "a"),
            Ok(Some(Found::Container))
        );
        assert_eq!(get("[]", "0"), Ok(None));
        assert_eq!(get("3", "a"), Ok(None));
    }

    #[test]
    fn whole_document_is_validated_after_the_value() {
        assert_eq!(get(r#"{"a": 1, "b": [}"#, "a"), Err(Invalid::Syntax));
    }

    #[test]
    fn depth_is_limited_without_recursion() {
        let ok = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert_eq!(get(&ok, "a"), Ok(None));
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
        assert_eq!(get(&deep, "a"), Err(Invalid::TooDeep));
        // A huge unterminated nesting fails fast, not with a stack overflow.
        assert!(get(&"[".repeat(1 << 20), "a").is_err());
    }

    #[test]
    fn long_values() {
        let long = format!(
            r#"{{"a": "{}", "n": {}}}"#,
            "x".repeat(MAX_VALUE + 1),
            "1".repeat(MAX_VALUE + 1)
        );
        assert_eq!(get(&long, "a"), Ok(Some(Found::TooLong)));
        assert_eq!(get(&long, "n"), Ok(Some(Found::TooLong)));
        let fits = format!(r#"{{"a": "{}"}}"#, "x".repeat(MAX_VALUE));
        assert_eq!(get(&fits, "a"), Ok(s(&"x".repeat(MAX_VALUE))));
    }

    const NOW: i64 = 1_793_000_000; // 2026-10-26

    fn eval(body: &str, p: &str, expect: Expect) -> Result<(), String> {
        evaluate(body.as_bytes(), &path(p), &expect, NOW)
    }

    fn max_age(days: u64) -> Expect {
        Expect::MaxAge(Duration::from_secs(days * 86400))
    }

    #[test]
    fn dates() {
        let doc = |v: &str| format!(r#"{{"published_at": {v}}}"#);
        let ten_days_ago = NOW - 10 * 86400;
        let rfc = |secs: i64| {
            format!(
                "\"{}\"",
                crate::clock::format_rfc3339(crate::clock::from_unix(secs))
            )
        };
        assert_eq!(
            eval(&doc(&rfc(ten_days_ago)), "published_at", max_age(80)),
            Ok(())
        );
        assert_eq!(
            eval(&doc(&rfc(ten_days_ago)), "published_at", max_age(5)),
            Err("json: too old (10d)".into())
        );
        assert_eq!(
            eval(
                &doc("\"2026-10-16T02:00:00+02:00\""),
                "published_at",
                max_age(80)
            ),
            Ok(())
        );
        assert_eq!(
            eval(&doc(&ten_days_ago.to_string()), "published_at", max_age(80)),
            Ok(())
        );
        assert_eq!(
            eval(&doc(&(NOW + 3600).to_string()), "published_at", max_age(80)),
            Err("json: date in the future".into())
        );
        assert_eq!(
            eval(&doc(&(NOW + 60).to_string()), "published_at", max_age(80)),
            Ok(())
        );
        for not_a_date in ["\"yesterday\"", "\"2026-10-16\"", "1.5", "1e9"] {
            assert_eq!(
                eval(&doc(not_a_date), "published_at", max_age(80)),
                Err("json: not a date".into()),
                "{not_a_date}"
            );
        }
        assert_eq!(
            eval(&doc("true"), "published_at", max_age(80)),
            Err("json: wrong type".into())
        );
        assert_eq!(
            eval(&doc("{}"), "published_at", max_age(80)),
            Err("json: wrong type".into())
        );
        assert_eq!(
            eval(&doc("1"), "other", max_age(80)),
            Err("json: not found".into())
        );
        assert_eq!(eval("{", "a", max_age(80)), Err("json: invalid".into()));
    }

    #[test]
    fn equals() {
        let doc = r#"{"s": "ok", "i": 42, "f": 42.0, "e": 4.2e1, "b": false}"#;
        assert_eq!(eval(doc, "s", Expect::Str("ok".into())), Ok(()));
        assert_eq!(
            eval(doc, "s", Expect::Str("OK".into())),
            Err("json: value differs".into())
        );
        assert_eq!(
            eval(doc, "i", Expect::Str("42".into())),
            Err("json: wrong type".into())
        );
        for p in ["i", "f", "e"] {
            assert_eq!(eval(doc, p, Expect::Int(42)), Ok(()), "{p}");
        }
        assert_eq!(
            eval(doc, "i", Expect::Int(43)),
            Err("json: value differs".into())
        );
        assert_eq!(
            eval(doc, "s", Expect::Int(42)),
            Err("json: wrong type".into())
        );
        assert_eq!(eval(doc, "b", Expect::Bool(false)), Ok(()));
        assert_eq!(
            eval(doc, "b", Expect::Bool(true)),
            Err("json: value differs".into())
        );
    }

    #[test]
    fn ranges() {
        let doc = format!(r#"{{"v": 7.5, "big": 1{}}}"#, "0".repeat(70));
        let range = |min, max| Expect::Range { min, max };
        assert_eq!(eval(&doc, "v", range(Some(0.0), Some(10.0))), Ok(()));
        assert_eq!(eval(&doc, "v", range(Some(7.5), Some(7.5))), Ok(()));
        assert_eq!(
            eval(&doc, "v", range(None, Some(7.0))),
            Err("json: out of range".into())
        );
        assert_eq!(
            eval(&doc, "v", range(Some(8.0), None)),
            Err("json: out of range".into())
        );
        assert_eq!(
            eval(&doc, "big", range(Some(0.0), None)),
            Err("json: wrong type".into())
        );
    }

    #[test]
    fn messages_never_contain_the_value() {
        let secret = "SECRET-VALUE";
        let doc = format!(r#"{{"a": "{secret}", "n": 99999}}"#);
        for expect in [
            Expect::Str("x".into()),
            Expect::Int(1),
            Expect::Bool(true),
            max_age(1),
            Expect::Range {
                min: Some(0.0),
                max: Some(1.0),
            },
        ] {
            for p in ["a", "n"] {
                if let Err(msg) = eval(&doc, p, expect.clone()) {
                    assert!(!msg.contains(secret) && !msg.contains("99999"), "{msg}");
                }
            }
        }
    }
}
