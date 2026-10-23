//! Maintenance windows `maintenance/*.toml` (SPEC.md §14.2).

use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use serde::Deserialize;

use super::FileProblems;
use super::duration::Dur;
use crate::monitors::Monitor;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Schedule {
    Once,
    Daily,
    Weekly,
}

#[derive(Debug, Clone)]
pub struct Maintenance {
    pub title: String,
    /// Indices into the configured monitors.
    pub monitors: Vec<usize>,
    pub schedule: Schedule,
    /// Local wall-clock start (in `pages.timezone`).
    pub start: DateTime,
    pub duration_s: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MaintenanceFile {
    config_version: u32,
    title: String,
    #[serde(default)]
    monitors: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    schedule: Schedule,
    start: String,
    duration: Dur,
}

pub fn parse(
    _id: &str,
    text: &str,
    monitors: &[Monitor],
    p: &mut FileProblems,
) -> Option<Maintenance> {
    let raw: MaintenanceFile = match toml::from_str(text) {
        Ok(raw) => raw,
        Err(e) => {
            p.error(e.to_string());
            return None;
        }
    };
    if raw.config_version != 1 {
        p.error(format!(
            "config_version must be 1, found {}",
            raw.config_version
        ));
    }
    let title = raw.title.trim().to_string();
    if title.is_empty() || title.chars().count() > 200 || title.chars().any(char::is_control) {
        p.error("title must be 1 to 200 characters without control characters");
    }
    // jiff would accept and silently drop an offset; refuse it, since the
    // time is meant in pages.timezone.
    let has_offset = raw
        .start
        .split_once('T')
        .is_some_and(|(_, time)| time.contains(['+', '-', 'Z', 'z', '[']));
    let start = match raw.start.parse::<DateTime>() {
        Ok(dt) if !has_offset => dt,
        _ => {
            p.error("start must be a local time without offset, e.g. \"2026-10-25T03:00:00\"");
            DateTime::default()
        }
    };
    let duration_s = i64::try_from(raw.duration.0.as_secs()).unwrap_or(i64::MAX);
    let max = if raw.schedule == Schedule::Daily {
        86400
    } else {
        7 * 86400
    };
    if !(60..=max).contains(&duration_s) {
        p.error(format!(
            "duration must be between 1m and {}",
            if max == 86400 { "1d" } else { "7d" }
        ));
    }
    if raw.monitors.is_empty() && raw.tags.is_empty() {
        p.error("a maintenance window needs monitors or tags");
    }
    let mut selected = Vec::new();
    for m in &raw.monitors {
        if m == "*" {
            selected.extend(0..monitors.len());
        } else if let Some(i) = monitors.iter().position(|x| &x.id == m) {
            selected.push(i);
        } else {
            p.error(format!("monitor '{m}' does not exist"));
        }
    }
    for tag in &raw.tags {
        let tagged: Vec<usize> = (0..monitors.len())
            .filter(|&i| monitors[i].tags.contains(tag))
            .collect();
        if tagged.is_empty() {
            p.warning(format!("no monitor has tag '{tag}'"));
        }
        selected.extend(tagged);
    }
    selected.sort_unstable();
    selected.dedup();
    if p.has_errors() {
        return None;
    }
    Some(Maintenance {
        title,
        monitors: selected,
        schedule: raw.schedule,
        start,
        duration_s,
    })
}

impl Maintenance {
    /// Start times (Unix seconds) of the occurrences that could overlap
    /// `[from, to)`. Recurring windows are computed in local time, so they
    /// follow daylight saving time changes.
    fn starts(&self, tz: &TimeZone, from: i64, to: i64) -> Vec<i64> {
        let to_unix = |dt: DateTime| {
            dt.to_zoned(tz.clone())
                .ok()
                .map(|z| z.timestamp().as_second())
        };
        let Some(first) = to_unix(self.start) else {
            return Vec::new();
        };
        let step_days = match self.schedule {
            Schedule::Once => return vec![first],
            Schedule::Daily => 1,
            Schedule::Weekly => 7,
        };
        // Begin with the occurrence before `from - duration` and walk forward.
        let span = (from - self.duration_s - first).max(0);
        let skip = span / 86400 / step_days;
        let mut out = Vec::new();
        let mut n = skip.max(1) - 1;
        let day = |n: i64| {
            self.start
                .checked_add(jiff::Span::new().days(n * step_days))
                .ok()
        };
        while let Some(start) = day(n).and_then(to_unix) {
            if start >= to || out.len() > 400 {
                break;
            }
            out.push(start);
            n += 1;
        }
        out
    }

    /// Is `now` inside a window?
    pub fn active(&self, tz: &TimeZone, now: i64) -> bool {
        self.starts(tz, now, now + 1)
            .iter()
            .any(|&s| s <= now && now < s + self.duration_s)
    }

    /// The next window starting in `[now, now + horizon)`, as (start, end).
    pub fn next(&self, tz: &TimeZone, now: i64, horizon: i64) -> Option<(i64, i64)> {
        self.starts(tz, now, now + horizon)
            .into_iter()
            .map(|s| (s, s + self.duration_s))
            .find(|&(s, e)| e > now && s < now + horizon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Problems;
    use crate::monitors::test_monitor;
    use std::path::Path;

    fn window(schedule: Schedule, start: &str, duration_s: i64) -> Maintenance {
        Maintenance {
            title: "M".into(),
            monitors: vec![0],
            schedule,
            start: start.parse().unwrap(),
            duration_s,
        }
    }

    fn at(text: &str, tz: &TimeZone) -> i64 {
        text.parse::<DateTime>()
            .unwrap()
            .to_zoned(tz.clone())
            .unwrap()
            .timestamp()
            .as_second()
    }

    #[test]
    fn once() {
        let tz = TimeZone::get("Europe/Berlin").unwrap_or(TimeZone::UTC);
        let w = window(Schedule::Once, "2026-10-25T03:00:00", 7200);
        assert!(!w.active(&tz, at("2026-10-25T02:59:59", &tz)));
        assert!(w.active(&tz, at("2026-10-25T03:00:00", &tz)));
        assert!(!w.active(&tz, at("2026-10-25T03:00:00", &tz) + 7200));
        assert_eq!(
            w.next(&tz, at("2026-10-24T12:00:00", &tz), 7 * 86400)
                .map(|(s, _)| s),
            Some(at("2026-10-25T03:00:00", &tz))
        );
    }

    #[test]
    fn daily_across_midnight() {
        let tz = TimeZone::UTC;
        let w = window(Schedule::Daily, "2026-10-01T23:30:00", 3600);
        assert!(w.active(&tz, at("2026-10-20T23:45:00", &tz)));
        assert!(w.active(&tz, at("2026-10-21T00:15:00", &tz)));
        assert!(!w.active(&tz, at("2026-10-21T00:31:00", &tz)));
        // Not before the first date.
        assert!(!w.active(&tz, at("2026-09-30T23:45:00", &tz)));
    }

    #[test]
    fn weekly() {
        let tz = TimeZone::UTC;
        // 2026-10-25 is a Sunday.
        let w = window(Schedule::Weekly, "2026-10-25T03:00:00", 7200);
        assert!(w.active(&tz, at("2026-11-08T04:00:00", &tz)));
        assert!(!w.active(&tz, at("2026-11-07T04:00:00", &tz)));
        assert_eq!(
            w.next(&tz, at("2026-11-09T00:00:00", &tz), 7 * 86400)
                .map(|(s, _)| s),
            Some(at("2026-11-15T03:00:00", &tz))
        );
        assert_eq!(w.next(&tz, at("2026-11-09T00:00:00", &tz), 86400), None);
    }

    #[test]
    fn daily_follows_local_time_over_dst() {
        let Ok(tz) = TimeZone::get("Europe/Berlin") else {
            return; // no tz database on this system
        };
        let w = window(Schedule::Daily, "2026-10-20T03:00:00", 3600);
        // DST ends in Berlin on 2026-10-25; the window stays at 03:00 local time.
        assert!(w.active(&tz, at("2026-10-24T03:30:00", &tz)));
        assert!(w.active(&tz, at("2026-10-26T03:30:00", &tz)));
    }

    #[test]
    fn config() {
        let mut a = test_monitor("nas");
        a.tags = vec!["backup".into()];
        let monitors = vec![a, test_monitor("web")];
        let mut problems = Problems::default();
        let m = parse(
            "w",
            "config_version = 1\ntitle = \"Backup\"\nmonitors = [\"web\"]\ntags = [\"backup\"]\nschedule = \"weekly\"\nstart = \"2026-10-25T03:00:00\"\nduration = \"2h\"\n",
            &monitors,
            &mut problems.for_file(Path::new("w.toml")),
        )
        .unwrap();
        assert_eq!(m.monitors, [0, 1]);
        for (text, expected) in [
            (
                "config_version = 1\ntitle = \"B\"\nmonitors = [\"x\"]\nschedule = \"once\"\nstart = \"2026-10-25T03:00:00\"\nduration = \"2h\"\n",
                "'x'",
            ),
            (
                "config_version = 1\ntitle = \"B\"\nmonitors = [\"web\"]\nschedule = \"daily\"\nstart = \"2026-10-25T03:00:00\"\nduration = \"2d\"\n",
                "duration",
            ),
            (
                "config_version = 1\ntitle = \"B\"\nmonitors = [\"web\"]\nschedule = \"once\"\nstart = \"2026-10-25T03:00:00+01:00\"\nduration = \"2h\"\n",
                "without offset",
            ),
            (
                "config_version = 1\ntitle = \"B\"\nschedule = \"once\"\nstart = \"2026-10-25T03:00:00\"\nduration = \"2h\"\n",
                "monitors or tags",
            ),
        ] {
            let mut problems = Problems::default();
            let r = parse(
                "w",
                text,
                &monitors,
                &mut problems.for_file(Path::new("w.toml")),
            );
            let all = format!("{problems:?}");
            assert!(r.is_none(), "accepted: {text}");
            assert!(all.contains(expected), "{text}\n=> {all}");
        }
    }
}
