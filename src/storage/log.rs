//! The check log: one human-readable file per UTC day (SPEC.md §7.1).
//!
//! ```text
//! 2026-10-22T14:03:11Z monitor=blog result=UP latency_ms=123 msg="HTTP 200"
//! ```
//!
//! Lines are buffered and written every `flush_interval` to spare SD cards.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::clock;

const MAX_MSG_CHARS: usize = 120;
pub(crate) const MAX_VALUE_CHARS: usize = 200;
/// If writing keeps failing (e.g. disk full), keep at most this many lines.
const MAX_BUFFERED_LINES: usize = 10_000;

pub struct LogWriter {
    dir: PathBuf,
    retention_days: i64,
    /// (UTC date, line)
    buffer: Vec<(String, String)>,
    dropped: usize,
    last_cleanup_day: Option<i64>,
}

impl LogWriter {
    pub fn open(dir: &Path, retention_days: i64) -> io::Result<Self> {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        Ok(LogWriter {
            dir: dir.to_path_buf(),
            retention_days,
            buffer: Vec::new(),
            dropped: 0,
            last_cleanup_day: None,
        })
    }

    /// Add a line `<timestamp> key=value ...`. Values are escaped; `msg` is
    /// shortened to 120 characters.
    pub fn record(&mut self, time: SystemTime, fields: &[(&str, &str)]) {
        let mut line = clock::format_rfc3339(time);
        for (key, value) in fields {
            let limit = if *key == "msg" {
                MAX_MSG_CHARS
            } else {
                MAX_VALUE_CHARS
            };
            line.push(' ');
            line.push_str(key);
            line.push('=');
            push_value(&mut line, value, limit);
        }
        line.push('\n');
        if self.buffer.len() >= MAX_BUFFERED_LINES {
            self.buffer.remove(0);
            self.dropped += 1;
        }
        self.buffer.push((clock::format_date(time), line));
    }

    /// Write buffered lines to their day files and delete expired files
    /// once per day. On error the lines stay buffered for the next attempt.
    pub fn flush(&mut self, now: SystemTime) -> io::Result<()> {
        if self.dropped > 0 {
            let dropped = self.dropped.to_string();
            self.dropped = 0;
            self.record(now, &[("event", "LOG_LINES_DROPPED"), ("count", &dropped)]);
        }
        while let Some((day, _)) = self.buffer.first() {
            let day = day.clone();
            let count = self.buffer.iter().take_while(|(d, _)| *d == day).count();
            let text: String = self.buffer[..count]
                .iter()
                .map(|(_, line)| line.as_str())
                .collect();
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(self.dir.join(format!("{day}.log")))?;
            file.write_all(text.as_bytes())?;
            file.sync_data()?;
            self.buffer.drain(..count);
        }

        // Cleanup errors are not write errors: the lines are written. They
        // are reported and the cleanup is tried again at the next flush.
        let today = clock::days_since_epoch(now);
        if self.last_cleanup_day != Some(today) && clock::is_plausible(now) {
            match self.delete_expired(today) {
                Ok(()) => self.last_cleanup_day = Some(today),
                Err(e) => eprintln!(
                    "warning: cannot delete expired log files in {}: {e}",
                    self.dir.display()
                ),
            }
        }
        Ok(())
    }

    fn delete_expired(&self, today: i64) -> io::Result<()> {
        for entry in fs::read_dir(&self.dir)?.flatten() {
            let name = entry.file_name();
            let Some(day) = name
                .to_str()
                .and_then(|n| n.strip_suffix(".log"))
                .and_then(clock::parse_date)
            else {
                continue; // not one of our files
            };
            if day < today - self.retention_days {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }
}

/// logfmt value: bare if it only has safe characters, else quoted.
pub(crate) fn push_value(line: &mut String, value: &str, limit: usize) {
    let value: String = value.chars().take(limit).collect();
    let safe = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:/@+-".contains(&b));
    if safe {
        line.push_str(&value);
        return;
    }
    line.push('"');
    for c in value.chars() {
        match c {
            '"' => line.push_str("\\\""),
            '\\' => line.push_str("\\\\"),
            c if c.is_control() => line.push('?'),
            c => line.push(c),
        }
    }
    line.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestDir;
    use std::time::{Duration, UNIX_EPOCH};

    /// 2026-10-22T14:23:11Z
    fn t(offset_secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_792_678_991 + offset_secs)
    }

    #[test]
    fn line_format() {
        let dir = TestDir::new("log");
        let mut log = LogWriter::open(&dir.path().join("log"), 90).unwrap();
        log.record(
            t(0),
            &[
                ("monitor", "blog"),
                ("result", "UP"),
                ("latency_ms", "123"),
                ("msg", "HTTP 200"),
            ],
        );
        log.record(
            t(1),
            &[
                ("monitor", "x"),
                ("msg", "a \"quoted\" \\ value\nwith newline"),
            ],
        );
        log.record(t(2), &[("event", "START"), ("empty", "")]);
        log.flush(t(3)).unwrap();
        let text = fs::read_to_string(dir.path().join("log/2026-10-22.log")).unwrap();
        assert_eq!(
            text,
            "2026-10-22T14:23:11Z monitor=blog result=UP latency_ms=123 msg=\"HTTP 200\"\n\
             2026-10-22T14:23:12Z monitor=x msg=\"a \\\"quoted\\\" \\\\ value?with newline\"\n\
             2026-10-22T14:23:13Z event=START empty=\"\"\n"
        );
    }

    #[test]
    fn msg_is_shortened() {
        let mut line = String::new();
        push_value(&mut line, &"x".repeat(500), MAX_MSG_CHARS);
        assert_eq!(line.len(), MAX_MSG_CHARS);
    }

    #[test]
    fn buffered_until_flush_and_split_by_day() {
        let dir = TestDir::new("log-days");
        let mut log = LogWriter::open(dir.path(), 90).unwrap();
        log.record(t(0), &[("event", "A")]);
        // 10 hours later it is the next UTC day.
        log.record(t(36_000), &[("event", "B")]);
        assert_eq!(log.buffered(), 2);
        assert!(!dir.path().join("2026-10-22.log").exists());
        log.flush(t(36_001)).unwrap();
        assert_eq!(log.buffered(), 0);
        assert!(
            fs::read_to_string(dir.path().join("2026-10-22.log"))
                .unwrap()
                .contains("event=A")
        );
        assert!(
            fs::read_to_string(dir.path().join("2026-10-23.log"))
                .unwrap()
                .contains("event=B")
        );
        // Appends on the next flush.
        log.record(t(36_002), &[("event", "C")]);
        log.flush(t(36_003)).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("2026-10-23.log"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn cleanup_errors_are_not_write_errors() {
        let dir = TestDir::new("log-cleanup-error");
        // An "expired log file" that cannot be removed with remove_file.
        fs::create_dir_all(dir.path().join("2026-07-01.log")).unwrap();
        let mut log = LogWriter::open(dir.path(), 90).unwrap();
        log.record(t(0), &[("event", "START")]);
        log.flush(t(0)).unwrap();
        assert!(
            fs::read_to_string(dir.path().join("2026-10-22.log"))
                .unwrap()
                .contains("event=START")
        );
        // Tried again at the next flush.
        assert_eq!(log.last_cleanup_day, None);
    }

    #[test]
    fn retention() {
        let dir = TestDir::new("log-retention");
        for name in [
            "2026-07-01.log",
            "2026-07-24.log",
            "2026-07-25.log",
            "2026-10-21.log",
            "notes.log",
            "2026-01-01.txt",
        ] {
            fs::write(dir.path().join(name), "x").unwrap();
        }
        let mut log = LogWriter::open(dir.path(), 90).unwrap();
        log.flush(t(0)).unwrap();
        let mut left: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        // 2026-10-22 minus 90 days is 2026-07-24.
        assert_eq!(
            left,
            [
                "2026-01-01.txt",
                "2026-07-24.log",
                "2026-07-25.log",
                "2026-10-21.log",
                "notes.log"
            ]
        );
    }

    #[test]
    fn files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new("log-perm");
        let log_dir = dir.path().join("sub");
        let mut log = LogWriter::open(&log_dir, 90).unwrap();
        log.record(t(0), &[("event", "A")]);
        log.flush(t(0)).unwrap();
        assert_eq!(
            fs::metadata(&log_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let file = fs::metadata(log_dir.join("2026-10-22.log")).unwrap();
        assert_eq!(file.permissions().mode() & 0o777, 0o600);
    }
}
