//! Durations in config files: `"500ms"`, `"30s"`, `"5m"`, `"2h"`, `"1d"`
//! (SPEC.md §3.2).

use std::fmt;
use std::time::Duration;

use serde::de::{self, Deserialize, Deserializer, Visitor};

/// Longest accepted duration (10 years), to keep arithmetic far from overflow.
const MAX_SECONDS: u64 = 3650 * 86400;

/// A duration from a config file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Dur(pub Duration);

impl Dur {
    pub const fn secs(s: u64) -> Self {
        Dur(Duration::from_secs(s))
    }

    pub fn is_zero(self) -> bool {
        self.0.is_zero()
    }
}

impl fmt::Display for Dur {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = self.0.as_millis();
        let (value, unit) = [
            (86_400_000, "d"),
            (3_600_000, "h"),
            (60_000, "m"),
            (1000, "s"),
        ]
        .into_iter()
        .find(|(factor, _)| ms % factor == 0 && ms >= *factor)
        .map(|(factor, unit)| (ms / factor, unit))
        .unwrap_or((ms, "ms"));
        write!(f, "{value}{unit}")
    }
}

pub fn parse(text: &str) -> Result<Dur, String> {
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    if number.is_empty() || number.len() > 12 {
        return Err(format!(
            "invalid duration '{text}', expected e.g. \"30s\" or \"5m\""
        ));
    }
    let n: u64 = number
        .parse()
        .map_err(|_| format!("invalid duration '{text}'"))?;
    let millis_per_unit: u64 = match unit {
        "ms" => 1,
        "s" => 1000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => {
            return Err(format!(
                "invalid duration unit in '{text}', use ms, s, m, h or d"
            ));
        }
    };
    let millis = n
        .checked_mul(millis_per_unit)
        .filter(|ms| *ms <= MAX_SECONDS * 1000)
        .ok_or_else(|| format!("duration '{text}' is too long (max 3650d)"))?;
    Ok(Dur(Duration::from_millis(millis)))
}

impl<'de> Deserialize<'de> for Dur {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DurVisitor;

        impl Visitor<'_> for DurVisitor {
            type Value = Dur;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a duration string such as \"30s\" or \"5m\"")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Dur, E> {
                parse(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(DurVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid() {
        assert_eq!(parse("500ms").unwrap(), Dur(Duration::from_millis(500)));
        assert_eq!(parse("30s").unwrap(), Dur::secs(30));
        assert_eq!(parse("5m").unwrap(), Dur::secs(300));
        assert_eq!(parse("2h").unwrap(), Dur::secs(7200));
        assert_eq!(parse("1d").unwrap(), Dur::secs(86400));
        assert_eq!(parse("0s").unwrap(), Dur::secs(0));
        assert_eq!(parse("3650d").unwrap(), Dur::secs(MAX_SECONDS));
    }

    #[test]
    fn invalid() {
        for bad in [
            "",
            "30",
            "s",
            "30 s",
            " 30s",
            "30S",
            "1.5h",
            "-1s",
            "+1s",
            "1w",
            "3651d",
            "99999999999999d",
            "1m30s",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn display() {
        assert_eq!(Dur::secs(30).to_string(), "30s");
        assert_eq!(Dur::secs(300).to_string(), "5m");
        assert_eq!(Dur::secs(86400 * 14).to_string(), "14d");
        assert_eq!(Dur(Duration::from_millis(1500)).to_string(), "1500ms");
        assert_eq!(Dur::secs(0).to_string(), "0ms");
    }
}
