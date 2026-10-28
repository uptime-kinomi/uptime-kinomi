//! Notifications: one file per notifier type (SPEC.md §5).
//!
//! A notifier file has the common keys at the top level and one table named
//! after its type (`[ntfy]`, `[telegram]`, ...). To add a type: create
//! `<type>.rs` with a `<Type>File` config struct and an implementation of
//! [`Target`], then add it to [`Type`], [`NotifierFile`] and [`build_target`].
//!
//! Messages are written by kinomi only: the monitor's name (from the
//! configuration) and kinomi's own check summary, never data received from a
//! monitored target.

pub mod gotify;
pub mod ntfy;
pub mod telegram;
pub mod webhook;

use std::fs::Metadata;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::clock;
use crate::config::FileProblems;
use crate::config::secret::{self, Secret, is_world_readable};
use crate::encoding::json_string;
use crate::render::html::human_duration;
use crate::scheduler::Event;

/// Attempts per message, and the pause before each retry.
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(5), Duration::from_secs(30)];
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Down,
    Up,
    Warn,
    Reminder,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Down, Kind::Up, Kind::Warn, Kind::Reminder];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Down => "down",
            Kind::Up => "up",
            Kind::Warn => "warn",
            Kind::Reminder => "reminder",
        }
    }
}

/// One notification, independent of the channel it is sent through.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub kind: Kind,
    pub monitor_id: String,
    pub monitor_name: String,
    /// kinomi's summary of the check, e.g. "HTTP 503".
    pub detail: String,
    pub time: i64,
    /// Formatted in the configured time zone.
    pub local_time: String,
    /// For `Up`: how long it was down. For `Reminder`: how long it is down.
    pub down_for: Option<i64>,
}

impl Message {
    pub fn title(&self) -> String {
        let what = match self.kind {
            Kind::Down => "is DOWN",
            Kind::Up => "is UP again",
            Kind::Warn => "has a warning",
            Kind::Reminder => "is still DOWN",
        };
        format!("{} {what}", self.monitor_name)
    }

    /// One line, e.g. `DOWN: My blog (HTTP 503) at 2026-10-22 16:03 CEST`.
    pub fn text(&self) -> String {
        let label = match self.kind {
            Kind::Down => "DOWN",
            Kind::Up => "UP",
            Kind::Warn => "WARN",
            Kind::Reminder => "STILL DOWN",
        };
        let duration = match (self.kind, self.down_for) {
            (Kind::Up, Some(d)) => format!(" after {} down", human_duration(d)),
            (Kind::Reminder, Some(d)) => format!(" for {}", human_duration(d)),
            _ => String::new(),
        };
        format!(
            "{label}: {} ({}){duration} at {}",
            self.monitor_name, self.detail, self.local_time
        )
    }

    pub fn utc_time(&self) -> String {
        clock::format_rfc3339(clock::from_unix(self.time))
    }
}

/// One notifier type.
pub trait Target: Send + Sync {
    /// Send one message. Errors are coarse classes, never remote content.
    fn send(&self, agent: &ureq::Agent, message: &Message) -> Result<(), String>;
}

pub struct Notifier {
    pub id: String,
    pub kind: Type,
    pub events: Vec<Kind>,
    pub target: Box<dyn Target>,
}

impl std::fmt::Debug for Notifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notifier")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Type {
    Webhook,
    Ntfy,
    Gotify,
    Telegram,
}

impl Type {
    pub fn name(self) -> &'static str {
        match self {
            Type::Webhook => "webhook",
            Type::Ntfy => "ntfy",
            Type::Gotify => "gotify",
            Type::Telegram => "telegram",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NotifierFile {
    config_version: u32,
    #[serde(rename = "type")]
    kind: Type,
    events: Option<Vec<Kind>>,
    webhook: Option<webhook::WebhookFile>,
    ntfy: Option<ntfy::NtfyFile>,
    gotify: Option<gotify::GotifyFile>,
    telegram: Option<telegram::TelegramFile>,
}

/// Context for building a notifier from its file.
pub struct Context<'a> {
    pub config_dir: &'a Path,
    pub file_meta: &'a Metadata,
}

impl Context<'_> {
    /// Resolve an inline or `*_file` secret, with permission warnings.
    pub fn secret(
        &self,
        key: &str,
        inline: Option<&str>,
        file: Option<&Path>,
        p: &mut FileProblems,
    ) -> Option<Secret> {
        let mut warnings = Vec::new();
        let file = file.map(|f| crate::config::resolve_path(self.config_dir, f));
        let secret =
            secret::resolve(key, inline, file.as_deref(), &mut warnings).unwrap_or_else(|e| {
                p.error(e);
                None
            });
        if inline.is_some() && is_world_readable(self.file_meta) {
            warnings.push(format!(
                "contains {key} but is readable by all users (chmod o-r, or use {key}_file)"
            ));
        }
        for w in warnings {
            p.warning(w);
        }
        secret
    }
}

pub fn parse(id: &str, text: &str, ctx: &Context, p: &mut FileProblems) -> Option<Notifier> {
    let raw: NotifierFile = match toml::from_str(text) {
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
    let events = raw.events.unwrap_or_else(|| Kind::ALL.to_vec());
    if events.is_empty() {
        p.error("events must not be empty (remove the key to get all events)");
    }
    let tables = [
        (Type::Webhook, raw.webhook.is_some()),
        (Type::Ntfy, raw.ntfy.is_some()),
        (Type::Gotify, raw.gotify.is_some()),
        (Type::Telegram, raw.telegram.is_some()),
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
    let target = build_target(
        raw.kind,
        raw.webhook,
        raw.ntfy,
        raw.gotify,
        raw.telegram,
        ctx,
        p,
    )?;
    if p.has_errors() {
        return None;
    }
    Some(Notifier {
        id: id.to_string(),
        kind: raw.kind,
        events,
        target,
    })
}

fn build_target(
    kind: Type,
    webhook: Option<webhook::WebhookFile>,
    ntfy: Option<ntfy::NtfyFile>,
    gotify: Option<gotify::GotifyFile>,
    telegram: Option<telegram::TelegramFile>,
    ctx: &Context,
    p: &mut FileProblems,
) -> Option<Box<dyn Target>> {
    fn boxed<T: Target + 'static>(t: Option<T>) -> Option<Box<dyn Target>> {
        t.map(|t| Box::new(t) as Box<dyn Target>)
    }
    let missing = |p: &mut FileProblems| {
        p.error(format!("type = \"{0}\" needs a [{0}] table", kind.name()));
        None
    };
    match (kind, webhook, ntfy, gotify, telegram) {
        (Type::Webhook, Some(f), ..) => boxed(webhook::build(f, ctx, p)),
        (Type::Ntfy, _, Some(f), ..) => boxed(ntfy::build(f, ctx, p)),
        (Type::Gotify, _, _, Some(f), _) => boxed(gotify::build(f, ctx, p)),
        (Type::Telegram, .., Some(f)) => boxed(telegram::build(f, ctx, p)),
        _ => missing(p),
    }
}

/// `http://` or `https://` URL without credentials, for notifier endpoints.
pub fn check_url(key: &str, url: &str, p: &mut FileProblems) {
    let scheme_ok = url.starts_with("https://") || url.starts_with("http://");
    let after_scheme = url.split_once("://").map_or("", |(_, rest)| rest);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if !scheme_ok
        || authority.is_empty()
        || authority.contains('@')
        || url.len() > 2048
        || ureq::http::Uri::try_from(url).is_err()
    {
        p.error(format!(
            "{key} must be an http:// or https:// URL without user name or password"
        ));
    }
}

/// Minimal JSON object writer: `json(&[("key", Json::Str("v")), ...])`.
pub enum Json<'a> {
    Str(&'a str),
    StrList(&'a [&'a str]),
    Num(i64),
    Bool(bool),
}

pub fn json(fields: &[(&str, Json)]) -> String {
    let mut out = String::from("{");
    for (i, (key, value)) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&json_string(key));
        out.push(':');
        match value {
            Json::Str(s) => out.push_str(&json_string(s)),
            Json::StrList(items) => {
                out.push('[');
                let quoted: Vec<String> = items.iter().map(|i| json_string(i)).collect();
                out.push_str(&quoted.join(","));
                out.push(']');
            }
            Json::Num(n) => out.push_str(&n.to_string()),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        }
    }
    out.push('}');
    out
}

/// POST a JSON body and map errors to coarse classes.
pub fn post_json(
    agent: &ureq::Agent,
    url: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Result<(), String> {
    let mut request = agent.post(url).header("Content-Type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    match request.send(body) {
        // Redirects are not followed, so a 3xx means nothing was delivered.
        Ok(response) if !response.status().is_success() => {
            Err(format!("HTTP {}", response.status().as_u16()))
        }
        Ok(_) => Ok(()),
        Err(ureq::Error::StatusCode(code)) => Err(format!("HTTP {code}")),
        Err(e) => Err(crate::monitors::http::error_message(&e)),
    }
}

/// A queued notification.
pub struct Job {
    pub notifier: usize,
    pub message: Arc<Message>,
}

/// The HTTP client for all notifiers: no redirects (SPEC.md §5.3).
fn new_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .max_redirects(0)
        .user_agent(format!("uptime-kinomi/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

/// Send queued notifications one after the other, with retries, and report
/// each result back to the event loop.
pub fn spawn(notifiers: Arc<Vec<Notifier>>, jobs: Receiver<Job>, events: Sender<Event>) {
    let agent = new_agent();
    let spawned = thread::Builder::new()
        .name("notifier".into())
        .spawn(move || {
            for job in jobs {
                let notifier = &notifiers[job.notifier];
                let mut result = notifier.target.send(&agent, &job.message);
                for delay in RETRY_DELAYS {
                    if result.is_ok() {
                        break;
                    }
                    thread::sleep(delay);
                    result = notifier.target.send(&agent, &job.message);
                }
                let event = Event::Notified {
                    monitor_id: job.message.monitor_id.clone(),
                    notifier: notifier.id.clone(),
                    kind: job.message.kind,
                    error: result.err(),
                    time: SystemTime::now(),
                };
                if events.send(event).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        eprintln!("error: cannot start the notifier thread: {e}");
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    pub fn message(kind: Kind) -> Message {
        Message {
            kind,
            monitor_id: "blog".into(),
            monitor_name: "My \"blog\"".into(),
            detail: "HTTP 503".into(),
            time: 1_792_678_991,
            local_time: "2026-10-22 16:23 CEST".into(),
            down_for: matches!(kind, Kind::Up | Kind::Reminder).then_some(720),
        }
    }

    /// A request received by [`capture_server`].
    #[derive(Debug)]
    pub struct Captured {
        pub request_line: String,
        pub headers: Vec<String>,
        pub body: String,
    }

    /// An HTTP server answering `status` to one request and sending what it
    /// received to the returned channel.
    pub fn capture_server(status: &'static str) -> (String, mpsc::Receiver<Captured>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming().take(1) {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut headers = Vec::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim_end().to_string();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length: ") {
                        length = v.parse().unwrap();
                    }
                    headers.push(line);
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let _ = stream.write_all(
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                );
                let _ = tx.send(Captured {
                    request_line: request_line.trim_end().to_string(),
                    headers,
                    body: String::from_utf8(body).unwrap(),
                });
            }
        });
        (format!("http://{addr}"), rx)
    }

    /// The same client as in production.
    pub fn agent() -> ureq::Agent {
        new_agent()
    }

    #[test]
    fn texts() {
        assert_eq!(
            message(Kind::Down).text(),
            "DOWN: My \"blog\" (HTTP 503) at 2026-10-22 16:23 CEST"
        );
        assert_eq!(
            message(Kind::Up).text(),
            "UP: My \"blog\" (HTTP 503) after 12 min down at 2026-10-22 16:23 CEST"
        );
        assert_eq!(
            message(Kind::Reminder).text(),
            "STILL DOWN: My \"blog\" (HTTP 503) for 12 min at 2026-10-22 16:23 CEST"
        );
        assert_eq!(message(Kind::Warn).title(), "My \"blog\" has a warning");
    }

    #[test]
    fn json_escaping() {
        let text = json(&[
            ("a", Json::Str("q\"\\\n\u{1}ü")),
            ("n", Json::Num(-3)),
            ("b", Json::Bool(true)),
        ]);
        assert_eq!(text, r#"{"a":"q\"\\\n\u0001ü","n":-3,"b":true}"#);
    }

    fn parse_text(text: &str) -> (Option<Notifier>, crate::config::Problems) {
        let dir = crate::testutil::TestDir::new("notifier");
        let file = dir.write("n.toml", text);
        let meta = std::fs::metadata(&file).unwrap();
        let ctx = Context {
            config_dir: dir.path(),
            file_meta: &meta,
        };
        let mut problems = crate::config::Problems::default();
        let n = parse("n", text, &ctx, &mut problems.for_file(&file));
        (n, problems)
    }

    #[test]
    fn common_keys() {
        let (n, problems) = parse_text(
            "config_version = 1\ntype = \"webhook\"\nevents = [\"down\", \"up\"]\n[webhook]\nurl = \"https://example.org/hook\"\n",
        );
        let n = n.unwrap_or_else(|| panic!("{problems:?}"));
        assert_eq!(n.events, [Kind::Down, Kind::Up]);
        for (text, expected) in [
            (
                "config_version = 1\ntype = \"webhook\"\n",
                "needs a [webhook] table",
            ),
            ("config_version = 1\ntype = \"sms\"\n", "sms"),
            (
                "config_version = 1\ntype = \"webhook\"\nevents = []\n[webhook]\nurl = \"https://x/\"\n",
                "events",
            ),
            (
                "config_version = 1\ntype = \"webhook\"\nevents = [\"sometimes\"]\n[webhook]\nurl = \"https://x/\"\n",
                "sometimes",
            ),
            (
                "config_version = 1\ntype = \"webhook\"\n[ntfy]\ntopic = \"x\"\n",
                "[ntfy] table",
            ),
            (
                "config_version = 1\ntype = \"webhook\"\n[webhook]\nurl = \"https://user:pw@x/\"\n",
                "without user name",
            ),
        ] {
            let (n, problems) = parse_text(text);
            let all = format!("{problems:?}");
            assert!(n.is_none(), "accepted: {text}");
            assert!(all.contains(expected), "{text}\n=> {all}");
        }
    }
}
