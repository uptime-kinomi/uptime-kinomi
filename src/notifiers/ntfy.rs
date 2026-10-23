//! `ntfy`: push notifications via an ntfy server (SPEC.md §5.3). Uses the
//! JSON publishing API, so titles with non-ASCII characters work.

use std::path::PathBuf;

use serde::Deserialize;

use super::{Context, Json, Kind, Message, Target, check_url, json, post_json};
use crate::config::FileProblems;
use crate::config::secret::Secret;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtfyFile {
    url: Option<String>,
    topic: String,
    token: Option<String>,
    token_file: Option<PathBuf>,
    priority: Option<u8>,
}

pub struct Ntfy {
    url: String,
    topic: String,
    token: Option<Secret>,
    priority: Option<u8>,
}

pub fn build(file: NtfyFile, ctx: &Context, p: &mut FileProblems) -> Option<Ntfy> {
    let url = file.url.unwrap_or_else(|| "https://ntfy.sh".into());
    check_url("ntfy.url", &url, p);
    let topic_ok = !file.topic.is_empty()
        && file.topic.len() <= 64
        && file
            .topic
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !topic_ok {
        p.error("ntfy.topic must be 1 to 64 characters from A-Z, a-z, 0-9, _ and -");
    }
    if file.priority.is_some_and(|prio| !(1..=5).contains(&prio)) {
        p.error("ntfy.priority must be between 1 and 5");
    }
    let token = ctx.secret(
        "ntfy.token",
        file.token.as_deref(),
        file.token_file.as_deref(),
        p,
    );
    Some(Ntfy {
        url: url.trim_end_matches('/').to_string(),
        topic: file.topic,
        token,
        priority: file.priority,
    })
}

impl Target for Ntfy {
    fn send(&self, agent: &ureq::Agent, m: &Message) -> Result<(), String> {
        // Default priorities: high for problems, normal otherwise.
        let priority = self.priority.unwrap_or(match m.kind {
            Kind::Down | Kind::Reminder => 4,
            Kind::Up | Kind::Warn => 3,
        });
        let tag = match m.kind {
            Kind::Down | Kind::Reminder => "red_circle",
            Kind::Up => "green_circle",
            Kind::Warn => "yellow_circle",
        };
        let (title, text) = (m.title(), m.text());
        let body = json(&[
            ("topic", Json::Str(&self.topic)),
            ("title", Json::Str(&title)),
            ("message", Json::Str(&text)),
            ("priority", Json::Num(i64::from(priority))),
            ("tags", Json::StrList(&[tag])),
        ]);
        let auth = self
            .token
            .as_ref()
            .map(|t| format!("Bearer {}", t.expose()));
        let headers: Vec<(&str, &str)> =
            auth.iter().map(|a| ("Authorization", a.as_str())).collect();
        post_json(agent, &self.url, &headers, &body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifiers::tests::{agent, capture_server, message};

    #[test]
    fn publishes_json() {
        let (url, rx) = capture_server("200 OK");
        let ntfy = Ntfy {
            url,
            topic: "kinomi-test".into(),
            token: Some(secret("tk_abc")),
            priority: None,
        };
        ntfy.send(&agent(), &message(Kind::Down)).unwrap();
        let got = rx.recv().unwrap();
        assert_eq!(got.request_line, "POST / HTTP/1.1");
        assert!(got.headers.iter().any(|h| h == "authorization: Bearer tk_abc" || h == "Authorization: Bearer tk_abc"), "{:?}", got.headers);
        assert!(
            got.body.starts_with(
                r#"{"topic":"kinomi-test","title":"My \"blog\" is DOWN","message":"DOWN: "#
            ),
            "{}",
            got.body
        );
        assert!(
            got.body.ends_with(r#""priority":4,"tags":["red_circle"]}"#),
            "{}",
            got.body
        );
    }

    fn secret(value: &str) -> Secret {
        let mut warnings = Vec::new();
        crate::config::secret::resolve("t", Some(value), None, &mut warnings)
            .unwrap()
            .unwrap()
    }
}
