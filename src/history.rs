//! What a status page shows about a monitor (SPEC.md §8.2), kept in memory
//! and persisted in state files (§7.2): current state, the heartbeat bar,
//! and hourly/daily totals for uptime percentages.
//!
//! All times are Unix seconds (UTC).

use std::collections::{BTreeMap, VecDeque};

use crate::state::{LogResult, State};

/// Hourly buckets are kept for this long (enough for "last 24 h").
const HOURLY_KEEP_HOURS: i64 = 48;

/// One entry of the heartbeat bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Beat {
    Up,
    Warn,
    /// Failed with retries left (PENDING).
    Fail,
    Down,
    Skipped,
}

impl Beat {
    pub fn name(self) -> &'static str {
        match self {
            Beat::Up => "UP",
            Beat::Warn => "WARN",
            Beat::Fail => "FAIL",
            Beat::Down => "DOWN",
            Beat::Skipped => "SKIPPED",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "UP" => Beat::Up,
            "WARN" => Beat::Warn,
            "FAIL" => Beat::Fail,
            "DOWN" => Beat::Down,
            "SKIPPED" => Beat::Skipped,
            _ => return None,
        })
    }

    fn from_result(result: LogResult) -> Self {
        match result {
            LogResult::Up => Beat::Up,
            LogResult::Warn => Beat::Warn,
            LogResult::Fail => Beat::Fail,
            LogResult::Down => Beat::Down,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeatRecord {
    pub time: i64,
    pub beat: Beat,
    pub latency_ms: Option<u32>,
}

/// Counts for one hour or one day. UP, WARN and PENDING count as up
/// (SPEC.md §4.2), DOWN as down; skipped checks are excluded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bucket {
    pub up: u32,
    pub down: u32,
    pub skipped: u32,
    pub maint: u32,
    pub latency_sum_ms: u64,
    /// Checks with a measured latency.
    pub timed: u32,
}

impl Bucket {
    fn add(&mut self, other: &Bucket) {
        self.up += other.up;
        self.down += other.down;
        self.skipped += other.skipped;
        self.maint += other.maint;
        self.latency_sum_ms += other.latency_sum_ms;
        self.timed += other.timed;
    }

    pub fn avg_ms(&self) -> Option<u32> {
        (self.timed > 0)
            .then(|| u32::try_from(self.latency_sum_ms / u64::from(self.timed)).unwrap_or(u32::MAX))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct History {
    pub state: State,
    pub state_since: Option<i64>,
    pub last_check: Option<i64>,
    pub last_msg: String,
    pub cert_expires: Option<i64>,
    /// Time of the last event included (log replay continues after it).
    pub covered_until: Option<i64>,
    pub recent: VecDeque<BeatRecord>,
    pub hourly: BTreeMap<i64, Bucket>,
    pub daily: BTreeMap<i64, Bucket>,
    recent_max: usize,
    history_days: i64,
}

impl History {
    pub fn new(recent_max: usize, history_days: i64) -> Self {
        History {
            state: State::Unknown,
            state_since: None,
            last_check: None,
            last_msg: String::new(),
            cert_expires: None,
            covered_until: None,
            recent: VecDeque::new(),
            hourly: BTreeMap::new(),
            daily: BTreeMap::new(),
            recent_max: recent_max.max(1),
            history_days,
        }
    }

    pub fn record_check(
        &mut self,
        time: i64,
        result: LogResult,
        latency_ms: Option<u32>,
        msg: &str,
        cert_expires: Option<i64>,
    ) {
        let state = match result {
            LogResult::Up => State::Up,
            LogResult::Warn => State::Warn,
            LogResult::Fail => State::Pending,
            LogResult::Down => State::Down,
        };
        if state != self.state {
            self.state = state;
            self.state_since = Some(time);
        }
        self.last_check = Some(time);
        self.last_msg = msg.to_string();
        if cert_expires.is_some() {
            self.cert_expires = cert_expires;
        }
        let mut bucket = Bucket::default();
        if result == LogResult::Down {
            bucket.down = 1;
        } else {
            bucket.up = 1;
        }
        if let Some(ms) = latency_ms {
            bucket.latency_sum_ms = u64::from(ms);
            bucket.timed = 1;
        }
        self.add(time, &bucket);
        self.push_beat(BeatRecord {
            time,
            beat: Beat::from_result(result),
            latency_ms,
        });
    }

    pub fn record_skip(&mut self, time: i64, count: u32) {
        let bucket = Bucket {
            skipped: count,
            ..Bucket::default()
        };
        self.add(time, &bucket);
        self.push_beat(BeatRecord {
            time,
            beat: Beat::Skipped,
            latency_ms: None,
        });
    }

    fn add(&mut self, time: i64, bucket: &Bucket) {
        self.covered_until = Some(self.covered_until.map_or(time, |c| c.max(time)));
        self.hourly
            .entry(time.div_euclid(3600))
            .or_default()
            .add(bucket);
        self.daily
            .entry(time.div_euclid(86400))
            .or_default()
            .add(bucket);
        self.prune(time);
    }

    fn push_beat(&mut self, beat: BeatRecord) {
        self.recent.push_back(beat);
        while self.recent.len() > self.recent_max {
            self.recent.pop_front();
        }
    }

    /// Drop buckets that are too old relative to `now`.
    pub fn prune(&mut self, now: i64) {
        let oldest_hour = now.div_euclid(3600) - HOURLY_KEEP_HOURS;
        self.hourly = self.hourly.split_off(&oldest_hour);
        let oldest_day = now.div_euclid(86400) - self.history_days;
        self.daily = self.daily.split_off(&oldest_day);
    }

    /// Totals over the last `hours` hours, including the current one.
    pub fn last_hours(&self, now: i64, hours: i64) -> Bucket {
        let first = now.div_euclid(3600) - hours + 1;
        sum(self.hourly.range(first..))
    }

    /// Totals over the last `days` days, including today.
    pub fn last_days(&self, now: i64, days: i64) -> Bucket {
        let first = now.div_euclid(86400) - days + 1;
        sum(self.daily.range(first..))
    }

    /// Restore a parsed state file (see storage::statefile).
    pub fn restore(
        &mut self,
        state: State,
        state_since: Option<i64>,
        last_check: Option<i64>,
        last_msg: String,
        cert_expires: Option<i64>,
        covered_until: Option<i64>,
    ) {
        self.state = state;
        self.state_since = state_since;
        self.last_check = last_check;
        self.last_msg = last_msg;
        self.cert_expires = cert_expires;
        self.covered_until = covered_until;
    }

    pub fn recent_max(&self) -> usize {
        self.recent_max
    }
}

fn sum<'a>(buckets: impl Iterator<Item = (&'a i64, &'a Bucket)>) -> Bucket {
    let mut total = Bucket::default();
    for (_, b) in buckets {
        total.add(b);
    }
    total
}

/// Uptime in percent (`None` if nothing was measured).
pub fn uptime_percent(bucket: &Bucket) -> Option<f64> {
    let total = bucket.up + bucket.down;
    (total > 0).then(|| f64::from(bucket.up) * 100.0 / f64::from(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_792_678_991; // 2026-10-22T14:23:11Z

    #[test]
    fn records_and_states() {
        let mut h = History::new(3, 400);
        h.record_check(T, LogResult::Up, Some(100), "HTTP 200", None);
        h.record_check(T + 60, LogResult::Fail, Some(5000), "timeout", None);
        h.record_check(T + 120, LogResult::Down, None, "timeout", None);
        h.record_skip(T + 180, 2);
        assert_eq!(h.state, State::Down);
        assert_eq!(h.state_since, Some(T + 120));
        assert_eq!(h.last_msg, "timeout");
        assert_eq!(h.covered_until, Some(T + 180));
        // The bar keeps the last 3 entries.
        let beats: Vec<_> = h.recent.iter().map(|b| b.beat).collect();
        assert_eq!(beats, [Beat::Fail, Beat::Down, Beat::Skipped]);

        let day = h.last_hours(T + 180, 24);
        assert_eq!((day.up, day.down, day.skipped, day.timed), (2, 1, 2, 2));
        assert_eq!(day.avg_ms(), Some(2550));
        // PENDING counts as up.
        assert_eq!(uptime_percent(&day).map(|p| p.round()), Some(67.0));
    }

    #[test]
    fn windows_and_pruning() {
        let mut h = History::new(50, 30);
        h.record_check(T - 40 * 86400, LogResult::Down, None, "x", None);
        h.record_check(T - 25 * 3600, LogResult::Down, None, "x", None);
        h.record_check(T - 3600, LogResult::Up, None, "x", None);
        h.record_check(T, LogResult::Up, None, "x", None);
        // 40 days ago is beyond the 30 day history; yesterday and today remain.
        assert_eq!(h.daily.len(), 2);
        assert_eq!(h.last_days(T, 30).down, 1);
        // 25 h ago is outside the last 24 h, but kept for 48 h.
        assert_eq!(h.last_hours(T, 24).down, 0);
        assert_eq!(h.last_hours(T, 24).up, 2);
        assert_eq!(h.hourly.len(), 3);
        h.prune(T + 30 * 3600);
        assert_eq!(h.hourly.len(), 2);
    }

    #[test]
    fn cert_expiry_is_kept_until_replaced() {
        let mut h = History::new(5, 30);
        h.record_check(T, LogResult::Up, None, "HTTP 200", Some(T + 86400));
        h.record_check(T + 60, LogResult::Up, None, "HTTP 200", None);
        assert_eq!(h.cert_expires, Some(T + 86400));
    }

    #[test]
    fn empty_uptime() {
        assert_eq!(uptime_percent(&Bucket::default()), None);
        assert_eq!(Bucket::default().avg_ms(), None);
    }
}
