//! Replay the check log into monitor histories (SPEC.md §7.2): at startup
//! for the events after a state file's `covered_until`, and completely for
//! `rebuild-state`.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use crate::clock;
use crate::history::History;
use crate::state::LogResult;

/// Longer lines are not ours and are ignored.
const MAX_LINE_BYTES: usize = 4096;

/// A parsed log line: timestamp and key/value pairs.
#[derive(Debug, PartialEq)]
pub struct Line {
    pub time: i64,
    pub fields: Vec<(String, String)>,
}

impl Line {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Parse `<RFC 3339 time> key=value key="quoted value" ...` as written by
/// `LogWriter`. Returns `None` for anything else.
pub fn parse_line(line: &str) -> Option<Line> {
    if line.len() > MAX_LINE_BYTES {
        return None;
    }
    let (time, mut rest) = line.split_once(' ')?;
    let time = clock::parse_rfc3339(time)?;
    let mut fields = Vec::new();
    while !rest.is_empty() {
        let (key, after_key) = rest.split_once('=')?;
        if key.is_empty()
            || !key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return None;
        }
        let (value, after_value) = if let Some(quoted) = after_key.strip_prefix('"') {
            let mut value = String::new();
            let mut chars = quoted.char_indices();
            let end = loop {
                match chars.next()? {
                    (i, '"') => break i + 1,
                    (_, '\\') => value.push(chars.next()?.1),
                    (_, c) => value.push(c),
                }
            };
            (value, &quoted[end..])
        } else {
            let end = after_key.find(' ').unwrap_or(after_key.len());
            (after_key[..end].to_string(), &after_key[end..])
        };
        fields.push((key.to_string(), value));
        rest = match after_value.strip_prefix(' ') {
            Some(r) => r,
            None if after_value.is_empty() => "",
            None => return None,
        };
    }
    Some(Line { time, fields })
}

/// Apply one log line to the matching history, if it is newer than what the
/// history already covers.
fn apply(line: &Line, histories: &mut HashMap<String, History>) {
    let Some(monitor) = line.get("monitor") else {
        return;
    };
    let Some(history) = histories.get_mut(monitor) else {
        return; // monitor no longer configured
    };
    if history.covered_until.is_some_and(|c| line.time <= c) {
        return;
    }
    let latency = line.get("latency_ms").and_then(|v| v.parse().ok());
    let msg = line.get("msg").unwrap_or("");
    let cert = line
        .get("cert_expires")
        .and_then(clock::parse_date)
        .map(|day| day * 86400);
    let result = match line.get("result") {
        Some("UP") => LogResult::Up,
        Some("WARN") => LogResult::Warn,
        Some("FAIL") => LogResult::Fail,
        Some("DOWN") => LogResult::Down,
        Some("SKIPPED") => {
            let count = line.get("count").and_then(|c| c.parse().ok()).unwrap_or(1);
            history.record_skip(line.time, count);
            return;
        }
        _ => return,
    };
    history.record_check(line.time, result, latency, msg, cert);
}

/// Replay all log files that may contain events after the oldest
/// `covered_until`. Returns the number of lines applied.
pub fn replay(log_dir: &Path, histories: &mut HashMap<String, History>) -> io::Result<usize> {
    let first_day = histories
        .values()
        .map(|h| h.covered_until.map_or(i64::MIN, |t| t.div_euclid(86400)))
        .min()
        .unwrap_or(i64::MAX);
    let mut days: Vec<(i64, std::path::PathBuf)> = match fs::read_dir(log_dir) {
        Ok(entries) => entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name();
                let day = name
                    .to_str()?
                    .strip_suffix(".log")
                    .and_then(clock::parse_date)?;
                (day >= first_day).then(|| (day, e.path()))
            })
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    days.sort();
    let mut applied = 0;
    for (_, path) in days {
        let reader = BufReader::new(fs::File::open(&path)?);
        for line in reader.split(b'\n') {
            let line = line?;
            // Non-UTF-8 or foreign lines are skipped, not fatal.
            let Some(parsed) = std::str::from_utf8(&line).ok().and_then(parse_line) else {
                continue;
            };
            apply(&parsed, histories);
            applied += 1;
        }
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;
    use crate::storage::log::LogWriter;
    use crate::testutil::TestDir;
    use std::time::{Duration, UNIX_EPOCH};

    const T: i64 = 1_792_678_991;

    #[test]
    fn parse_lines() {
        let line = parse_line(
            "2026-10-22T14:23:11Z monitor=blog result=UP latency_ms=123 msg=\"HTTP 200\"",
        )
        .unwrap();
        assert_eq!(line.time, T);
        assert_eq!(line.get("monitor"), Some("blog"));
        assert_eq!(line.get("msg"), Some("HTTP 200"));
        let escaped = parse_line(r#"2026-10-22T14:23:11Z msg="a \"q\" \\ b" x=1"#).unwrap();
        assert_eq!(escaped.get("msg"), Some(r#"a "q" \ b"#));
        assert_eq!(escaped.get("x"), Some("1"));
        assert_eq!(
            parse_line("2026-10-22T14:23:11Z event=START")
                .unwrap()
                .fields
                .len(),
            1
        );
        for bad in [
            "",
            "garbage",
            "2026-10-22 event=START",
            "2026-10-22T14:23:11Z event",
            "2026-10-22T14:23:11Z =x",
            "2026-10-22T14:23:11Z Event=x",
            "2026-10-22T14:23:11Z msg=\"unterminated",
            "2026-10-22T14:23:11Z msg=\"a\"b=1",
        ] {
            assert_eq!(parse_line(bad), None, "{bad}");
        }
        assert_eq!(
            parse_line(&format!("2026-10-22T14:23:11Z msg={}", "x".repeat(5000))),
            None
        );
    }

    #[test]
    fn writer_output_parses() {
        let dir = TestDir::new("replay-format");
        let mut log = LogWriter::open(dir.path(), 90).unwrap();
        let t = UNIX_EPOCH + Duration::from_secs(T as u64);
        let nasty = "quote \" backslash \\ newline\n tab\t end";
        log.record(t, &[("monitor", "m"), ("result", "DOWN"), ("msg", nasty)]);
        log.flush(t).unwrap();
        let text = fs::read_to_string(dir.path().join("2026-10-22.log")).unwrap();
        let line = parse_line(text.trim_end()).unwrap();
        // Control characters were replaced by '?' when writing.
        assert_eq!(
            line.get("msg"),
            Some("quote \" backslash \\ newline? tab? end")
        );
    }

    fn write_log(dir: &Path, name: &str, lines: &[&str]) {
        fs::write(dir.join(name), lines.join("\n") + "\n").unwrap();
    }

    #[test]
    fn replay_after_covered_until() {
        let dir = TestDir::new("replay");
        write_log(
            dir.path(),
            "2026-10-21.log",
            &["2026-10-21T10:00:00Z monitor=a result=UP latency_ms=10 msg=ok"],
        );
        write_log(
            dir.path(),
            "2026-10-22.log",
            &[
                "2026-10-22T10:00:00Z event=START version=0.2.0 monitors=2",
                "2026-10-22T10:00:01Z monitor=a result=DOWN msg=timeout",
                "2026-10-22T10:00:02Z monitor=b result=UP latency_ms=5 cert_expires=2026-12-01 msg=ok",
                "2026-10-22T10:00:03Z monitor=a result=SKIPPED count=2 msg=\"queue full\"",
                "2026-10-22T10:00:04Z monitor=gone result=UP msg=ok",
                "not a log line",
            ],
        );
        fs::write(dir.path().join("notes.txt"), "ignored").unwrap();

        let mut histories = HashMap::new();
        histories.insert("a".to_string(), History::new(50, 400));
        let mut b = History::new(50, 400);
        // b already covers everything up to 10:00:02 (inclusive).
        b.covered_until = clock::parse_rfc3339("2026-10-22T10:00:02Z");
        histories.insert("b".to_string(), b);

        replay(dir.path(), &mut histories).unwrap();
        let a = &histories["a"];
        assert_eq!(a.state, State::Down);
        assert_eq!(a.recent.len(), 3);
        assert_eq!(a.last_days(T, 30).skipped, 2);
        assert_eq!(
            a.covered_until,
            clock::parse_rfc3339("2026-10-22T10:00:03Z")
        );
        // Nothing new for b.
        assert_eq!(histories["b"].recent.len(), 0);
    }

    #[test]
    fn replay_skips_old_files() {
        let dir = TestDir::new("replay-days");
        write_log(
            dir.path(),
            "2026-10-20.log",
            &["2026-10-20T10:00:00Z monitor=a result=UP msg=ok"],
        );
        write_log(
            dir.path(),
            "2026-10-22.log",
            &["2026-10-22T10:00:00Z monitor=a result=UP msg=ok"],
        );
        let mut h = History::new(50, 400);
        h.covered_until = clock::parse_rfc3339("2026-10-21T23:00:00Z");
        let mut histories = HashMap::from([("a".to_string(), h)]);
        assert_eq!(replay(dir.path(), &mut histories).unwrap(), 1);
        assert_eq!(histories["a"].recent.len(), 1);
    }

    #[test]
    fn missing_log_dir_is_empty() {
        let dir = TestDir::new("replay-none");
        let mut histories = HashMap::from([("a".to_string(), History::new(5, 5))]);
        assert_eq!(replay(&dir.path().join("nope"), &mut histories).unwrap(), 0);
    }
}
