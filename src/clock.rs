//! Wall-clock helpers: UTC formatting and clock sanity (SPEC.md §7, §10.1).
//!
//! Scheduling uses `Instant` (monotonic). Wall-clock time is only used for
//! timestamps, and SBCs without a real-time clock may start with a wrong
//! time until NTP has synced.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 2026-01-01T00:00:00Z. A clock before this has not been set yet.
const MIN_PLAUSIBLE_UNIX: u64 = 1_767_225_600;

/// Jumps larger than this between wall clock and monotonic clock are logged.
const JUMP_THRESHOLD: Duration = Duration::from_secs(60);

pub fn unix_seconds(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

pub fn from_unix(secs: i64) -> SystemTime {
    match u64::try_from(secs) {
        Ok(s) => UNIX_EPOCH + Duration::from_secs(s),
        Err(_) => UNIX_EPOCH - Duration::from_secs(secs.unsigned_abs()),
    }
}

pub fn is_plausible(t: SystemTime) -> bool {
    unix_seconds(t) >= MIN_PLAUSIBLE_UNIX as i64
}

/// `2026-10-22T14:03:11Z`
pub fn format_rfc3339(t: SystemTime) -> String {
    let secs = unix_seconds(t);
    let (y, m, d) = civil_from_days(secs.div_euclid(86400));
    let rem = secs.rem_euclid(86400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// `2026-10-22` (UTC)
pub fn format_date(t: SystemTime) -> String {
    let (y, m, d) = civil_from_days(unix_seconds(t).div_euclid(86400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 of a `YYYY-MM-DD` string.
pub fn parse_date(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    let digits = |r: std::ops::Range<usize>| -> Option<i64> {
        let s = text.get(r)?;
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, m, d) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let days = days_from_civil(y, m as u32, d as u32);
    // Reject dates like 2026-02-31 that do not round-trip.
    (civil_from_days(days) == (y, m as u32, d as u32)).then_some(days)
}

pub fn days_since_epoch(t: SystemTime) -> i64 {
    unix_seconds(t).div_euclid(86400)
}

/// Howard Hinnant's algorithm: days since 1970-01-01 → (year, month, day).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Detects wall-clock jumps (NTP sync after boot, manual changes) by
/// comparing wall-clock and monotonic progress.
pub struct JumpDetector {
    wall: SystemTime,
    mono: Instant,
}

impl JumpDetector {
    pub fn new(wall: SystemTime, mono: Instant) -> Self {
        JumpDetector { wall, mono }
    }

    /// Returns the jump in seconds (positive: clock moved forward) if the
    /// wall clock moved more than the threshold differently from the
    /// monotonic clock since the last call.
    pub fn check(&mut self, wall: SystemTime, mono: Instant) -> Option<i64> {
        let wall_delta = unix_seconds(wall) - unix_seconds(self.wall);
        let mono_delta =
            i64::try_from(mono.duration_since(self.mono).as_secs()).unwrap_or(i64::MAX);
        self.wall = wall;
        self.mono = mono;
        let jump = wall_delta - mono_delta;
        (jump.unsigned_abs() > JUMP_THRESHOLD.as_secs()).then_some(jump)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn formatting() {
        assert_eq!(format_rfc3339(at(0)), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(at(1_792_678_991)), "2026-10-22T14:23:11Z");
        assert_eq!(format_rfc3339(at(951_782_400)), "2000-02-29T00:00:00Z");
        assert_eq!(format_date(at(1_792_678_991)), "2026-10-22");
    }

    #[test]
    fn date_round_trip() {
        for days in [-1, 0, 59, 60, 365, 10_957, 20_748, 100_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(parse_date(&format!("{y:04}-{m:02}-{d:02}")), Some(days));
        }
        assert_eq!(
            parse_date("2026-10-22"),
            Some(days_since_epoch(at(1_792_678_991)))
        );
        for bad in [
            "",
            "2026-1-22",
            "2026-13-01",
            "2026-02-30",
            "2026-02-29",
            "20x6-01-01",
            "2026-01-01.log",
        ] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
        assert!(parse_date("2028-02-29").is_some());
    }

    #[test]
    fn plausibility() {
        assert!(!is_plausible(at(0)));
        assert!(!is_plausible(at(MIN_PLAUSIBLE_UNIX - 1)));
        assert!(is_plausible(at(MIN_PLAUSIBLE_UNIX)));
    }

    #[test]
    fn jumps() {
        let mono = Instant::now();
        let mut detector = JumpDetector::new(at(MIN_PLAUSIBLE_UNIX), mono);
        // Normal progress.
        assert_eq!(
            detector.check(at(MIN_PLAUSIBLE_UNIX + 30), mono + Duration::from_secs(30)),
            None
        );
        // NTP sync moved the clock one day forward.
        assert_eq!(
            detector.check(
                at(MIN_PLAUSIBLE_UNIX + 30 + 86400 + 10),
                mono + Duration::from_secs(40)
            ),
            Some(86400)
        );
        // Clock set back by 5 minutes.
        let wall = MIN_PLAUSIBLE_UNIX + 30 + 86400 + 10 - 300 + 10;
        assert_eq!(
            detector.check(at(wall), mono + Duration::from_secs(50)),
            Some(-300)
        );
    }
}
