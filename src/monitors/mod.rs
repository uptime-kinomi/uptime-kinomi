//! Monitors: one file per monitor type (SPEC.md §4).
//!
//! A monitor file has the common keys at the top level and one table named
//! after its type (`[http]`, `[tcp]`, ...). To add a type: create
//! `<type>.rs` with a `<Type>File` config struct and an implementation of
//! [`Check`], then add it to [`Kind`], [`MonitorFile`] and [`build_check`].

pub mod http;
pub mod tcp;
mod tls;

use std::fs::Metadata;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::config::duration::Dur;
use crate::config::global::{Defaults, MAX_RETRIES, MIN_INTERVAL};
use crate::config::{FileProblems, is_valid_id};

/// Outcome of one check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Up,
    /// Succeeded, but a warning condition holds (slow, certificate expiring).
    Warn,
    Fail,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CheckResult {
    pub status: Status,
    pub latency: Option<Duration>,
    /// kinomi's own summary, e.g. "HTTP 503" or "timeout". Never content
    /// received from the target (AGENTS.md invariant 5).
    pub msg: String,
    /// `notAfter` of the server certificate (Unix seconds), if checked.
    pub cert_expires: Option<i64>,
}

impl CheckResult {
    pub fn up(latency: Duration, msg: impl Into<String>) -> Self {
        CheckResult {
            status: Status::Up,
            latency: Some(latency),
            msg: msg.into(),
            cert_expires: None,
        }
    }

    pub fn warn(latency: Duration, msg: impl Into<String>) -> Self {
        CheckResult {
            status: Status::Warn,
            latency: Some(latency),
            msg: msg.into(),
            cert_expires: None,
        }
    }

    pub fn fail(latency: Option<Duration>, msg: impl Into<String>) -> Self {
        CheckResult {
            status: Status::Fail,
            latency,
            msg: msg.into(),
            cert_expires: None,
        }
    }
}

/// One monitor type. `check` must return within the monitor's timeout (plus
/// a small margin) and must never panic.
pub trait Check: Send + Sync {
    fn check(&self) -> CheckResult;

    /// Short description of the target for private pages, e.g. the URL.
    fn target(&self) -> String;
}

// Parsed and validated now; used by the server, pages and notifications
// (milestones M2 to M4).
#[allow(dead_code)]
pub struct Monitor {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub description: Option<String>,
    pub interval: Duration,
    pub timeout: Duration,
    pub retries: u32,
    pub notifications: Vec<String>,
    pub resend_interval: Duration,
    pub enabled: bool,
    pub tags: Vec<String>,
    pub check: Box<dyn Check>,
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("target", &self.check.target())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Http,
    Tcp,
    Dns,
    Push,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Http => "http",
            Kind::Tcp => "tcp",
            Kind::Dns => "dns",
            Kind::Push => "push",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MonitorFile {
    config_version: u32,
    #[serde(rename = "type")]
    kind: Kind,
    name: String,
    description: Option<String>,
    interval: Option<Dur>,
    timeout: Option<Dur>,
    retries: Option<u32>,
    notifications: Option<Vec<String>>,
    resend_interval: Option<Dur>,
    enabled: Option<bool>,
    #[serde(default)]
    tags: Vec<String>,
    http: Option<http::HttpFile>,
    tcp: Option<tcp::TcpFile>,
    // Planned types (SPEC.md §4.5, §4.6): accepted by the parser so the user
    // gets a clear "not implemented yet" instead of "unknown field".
    dns: Option<toml::Table>,
    push: Option<toml::Table>,
    grace: Option<Dur>,
}

/// Context for building a monitor from its file.
pub struct Context<'a> {
    pub config_dir: &'a Path,
    pub defaults: &'a Defaults,
    /// IDs of the files in `notifications/`.
    pub notifiers: &'a [String],
    pub file_meta: &'a Metadata,
}

pub fn parse(id: &str, text: &str, ctx: &Context, p: &mut FileProblems) -> Option<Monitor> {
    let raw: MonitorFile = match toml::from_str(text) {
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

    let name = raw.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        p.error("name must be 1 to 100 characters without control characters");
    }
    if let Some(d) = &raw.description {
        if d.chars().count() > 1000 {
            p.error("description must be at most 1000 characters");
        }
    }

    let interval = raw.interval.unwrap_or(ctx.defaults.interval);
    let timeout = raw.timeout.unwrap_or(ctx.defaults.timeout);
    let retries = raw.retries.unwrap_or(ctx.defaults.retries);
    if interval < MIN_INTERVAL {
        p.error(format!("interval must be at least {MIN_INTERVAL}"));
    }
    if timeout.is_zero() || timeout >= interval {
        p.error(format!(
            "timeout ({timeout}) must be greater than 0 and less than interval ({interval})"
        ));
    }
    if retries > MAX_RETRIES {
        p.error(format!("retries must be at most {MAX_RETRIES}"));
    }

    let notifications = raw
        .notifications
        .unwrap_or_else(|| ctx.defaults.notifications.clone());
    for n in &notifications {
        if !ctx.notifiers.contains(n) {
            p.error(format!(
                "notification '{n}' does not exist (expected notifications/{n}.toml)"
            ));
        }
    }
    for tag in &raw.tags {
        if !is_valid_id(tag) {
            p.error(format!("tag '{tag}' must match [a-z0-9][a-z0-9_-]*"));
        }
    }

    let tables = [
        (Kind::Http, raw.http.is_some()),
        (Kind::Tcp, raw.tcp.is_some()),
        (Kind::Dns, raw.dns.is_some()),
        (Kind::Push, raw.push.is_some()),
    ];
    for (kind, present) in tables {
        if present && kind != raw.kind {
            p.error(format!(
                "[{}] table does not match type = \"{}\"",
                kind.name(),
                raw.kind.name()
            ));
        }
    }
    if raw.grace.is_some() && raw.kind != Kind::Push {
        p.error("grace is only valid for push monitors");
    }

    let check = build_check(raw.kind, raw.http, raw.tcp, timeout, ctx, p)?;
    if p.has_errors() {
        return None;
    }
    Some(Monitor {
        id: id.to_string(),
        kind: raw.kind,
        name,
        description: raw.description,
        interval: interval.0,
        timeout: timeout.0,
        retries,
        notifications,
        resend_interval: raw.resend_interval.unwrap_or(Dur::secs(0)).0,
        enabled: raw.enabled.unwrap_or(true),
        tags: raw.tags,
        check,
    })
}

fn build_check(
    kind: Kind,
    http: Option<http::HttpFile>,
    tcp: Option<tcp::TcpFile>,
    timeout: Dur,
    ctx: &Context,
    p: &mut FileProblems,
) -> Option<Box<dyn Check>> {
    let missing = |p: &mut FileProblems| {
        p.error(format!("type = \"{0}\" needs a [{0}] table", kind.name()));
        None
    };
    match kind {
        Kind::Http => match http {
            Some(file) => {
                http::build(file, timeout.0, ctx, p).map(|c| Box::new(c) as Box<dyn Check>)
            }
            None => missing(p),
        },
        Kind::Tcp => match tcp {
            Some(file) => tcp::build(file, timeout.0, p).map(|c| Box::new(c) as Box<dyn Check>),
            None => missing(p),
        },
        Kind::Dns | Kind::Push => {
            p.error(format!(
                "monitor type '{}' is not implemented yet (planned for milestone M3)",
                kind.name()
            ));
            None
        }
    }
}

/// A monitor whose check always succeeds at once (for tests).
#[cfg(test)]
pub fn test_monitor(id: &str) -> Monitor {
    struct AlwaysUp;

    impl Check for AlwaysUp {
        fn check(&self) -> CheckResult {
            CheckResult::up(Duration::from_millis(1), "ok")
        }

        fn target(&self) -> String {
            "test".into()
        }
    }

    Monitor {
        id: id.into(),
        kind: Kind::Tcp,
        name: id.into(),
        description: None,
        interval: Duration::from_secs(10),
        timeout: Duration::from_secs(1),
        retries: 0,
        notifications: vec![],
        resend_interval: Duration::ZERO,
        enabled: true,
        tags: vec![],
        check: Box::new(AlwaysUp),
    }
}

/// Host name (RFC 1123) or IP address literal.
pub fn is_valid_host(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
        || (!host.is_empty()
            && host.len() <= 253
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            }))
}

/// Find `needle` in `haystack` (bodies are bounded, so the naive search is fine).
pub fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Problems;
    use crate::testutil::TestDir;

    fn defaults() -> Defaults {
        Defaults {
            interval: Dur::secs(60),
            timeout: Dur::secs(10),
            retries: 1,
            notifications: vec![],
        }
    }

    fn parse_text(text: &str) -> (Option<Monitor>, Problems) {
        let dir = TestDir::new("monitor");
        let file = dir.write("monitors/m.toml", text);
        let meta = std::fs::metadata(&file).unwrap();
        let defaults = defaults();
        let notifiers = vec!["phone".to_string()];
        let ctx = Context {
            config_dir: dir.path(),
            defaults: &defaults,
            notifiers: &notifiers,
            file_meta: &meta,
        };
        let mut problems = Problems::default();
        let monitor = parse("m", text, &ctx, &mut problems.for_file(&file));
        (monitor, problems)
    }

    #[test]
    fn tcp_monitor_with_defaults() {
        let (m, problems) = parse_text(
            "config_version = 1\ntype = \"tcp\"\nname = \"SSH\"\n[tcp]\nhost = \"192.168.1.10\"\nport = 22\n",
        );
        let m = m.unwrap_or_else(|| panic!("{problems:?}"));
        assert_eq!(m.kind, Kind::Tcp);
        assert_eq!(m.interval, Duration::from_secs(60));
        assert_eq!(m.timeout, Duration::from_secs(10));
        assert_eq!(m.retries, 1);
        assert!(m.enabled);
        assert_eq!(m.check.target(), "192.168.1.10:22");
    }

    #[test]
    fn common_errors() {
        let tcp = "[tcp]\nhost = \"h\"\nport = 1\n";
        for (text, expected) in [
            (
                format!("config_version = 1\ntype = \"tcp\"\nname = \"\"\n{tcp}"),
                "name",
            ),
            (
                format!(
                    "config_version = 1\ntype = \"tcp\"\nname = \"a\"\ninterval = \"5s\"\n{tcp}"
                ),
                "interval",
            ),
            (
                format!(
                    "config_version = 1\ntype = \"tcp\"\nname = \"a\"\ntimeout = \"60s\"\n{tcp}"
                ),
                "timeout",
            ),
            (
                format!("config_version = 1\ntype = \"tcp\"\nname = \"a\"\nretries = 11\n{tcp}"),
                "retries",
            ),
            (
                format!(
                    "config_version = 1\ntype = \"tcp\"\nname = \"a\"\nnotifications = [\"x\"]\n{tcp}"
                ),
                "notification 'x'",
            ),
            (
                format!(
                    "config_version = 1\ntype = \"tcp\"\nname = \"a\"\ntags = [\"Bad Tag\"]\n{tcp}"
                ),
                "tag",
            ),
            (
                format!("config_version = 1\ntype = \"tcp\"\nname = \"a\"\nbogus = 1\n{tcp}"),
                "bogus",
            ),
            (
                format!("config_version = 1\ntype = \"ftp\"\nname = \"a\"\n{tcp}"),
                "ftp",
            ),
            (
                format!("config_version = 1\ntype = \"http\"\nname = \"a\"\n{tcp}"),
                "[tcp] table",
            ),
            (
                "config_version = 1\ntype = \"tcp\"\nname = \"a\"\n".to_string(),
                "needs a [tcp] table",
            ),
            (
                "config_version = 1\ntype = \"dns\"\nname = \"a\"\n[dns]\nname = \"x\"\n"
                    .to_string(),
                "not implemented",
            ),
            (
                "config_version = 1\ntype = \"push\"\nname = \"a\"\ngrace = \"1m\"\n[push]\n"
                    .to_string(),
                "not implemented",
            ),
            (
                format!("config_version = 1\ntype = \"tcp\"\nname = \"a\"\ngrace = \"1m\"\n{tcp}"),
                "grace",
            ),
        ] {
            let (m, problems) = parse_text(&text);
            let all = format!("{problems:?}");
            assert!(m.is_none() && problems.has_errors(), "accepted: {text}");
            assert!(all.contains(expected), "{text}\n=> {all}");
        }
    }

    #[test]
    fn notification_reference_ok() {
        let (m, problems) = parse_text(
            "config_version = 1\ntype = \"tcp\"\nname = \"a\"\nnotifications = [\"phone\"]\n[tcp]\nhost = \"h\"\nport = 1\n",
        );
        assert!(m.is_some(), "{problems:?}");
    }

    #[test]
    fn hosts() {
        for ok in ["example.org", "localhost", "192.168.1.1", "::1", "fe80::1"] {
            assert!(is_valid_host(ok), "{ok}");
        }
        for bad in ["", "a b", "a..b", "-x", "[::1]", "http://x"] {
            assert!(!is_valid_host(bad), "{bad}");
        }
    }

    #[test]
    fn byte_search() {
        assert!(contains_bytes(b"hello world", b"o w"));
        assert!(!contains_bytes(b"hello", b"world"));
        assert!(contains_bytes(b"", b""));
        assert!(!contains_bytes(b"ab", b"abc"));
    }
}
