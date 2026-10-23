//! `webhook`: HTTP POST with a JSON body (SPEC.md §5.3). Formats: kinomi's
//! own JSON, Discord (`content`) and Slack/Mattermost/Rocket.Chat (`text`).

use serde::Deserialize;

use super::{Json, Message, Target, check_url, json, post_json};
use crate::config::FileProblems;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Format {
    #[default]
    Json,
    Discord,
    Slack,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookFile {
    url: String,
    #[serde(default)]
    format: Format,
}

pub struct Webhook {
    url: String,
    format: Format,
}

pub fn build(file: WebhookFile, p: &mut FileProblems) -> Option<Webhook> {
    check_url("webhook.url", &file.url, p);
    Some(Webhook {
        url: file.url,
        format: file.format,
    })
}

/// The body for `message` in `format`.
fn body(format: Format, m: &Message) -> String {
    let text = m.text();
    match format {
        Format::Json => {
            let time = m.utc_time();
            let mut fields = vec![
                ("event", Json::Str(m.kind.name())),
                ("monitor", Json::Str(&m.monitor_id)),
                ("name", Json::Str(&m.monitor_name)),
                ("detail", Json::Str(&m.detail)),
                ("time", Json::Str(&time)),
            ];
            // Only for "up" (how long it was down) and "reminder".
            if let Some(d) = m.down_for {
                fields.push(("down_for_s", Json::Num(d)));
            }
            fields.push(("text", Json::Str(&text)));
            json(&fields)
        }
        Format::Discord => json(&[("content", Json::Str(&text))]),
        Format::Slack => json(&[("text", Json::Str(&text))]),
    }
}

impl Target for Webhook {
    fn send(&self, agent: &ureq::Agent, message: &Message) -> Result<(), String> {
        post_json(agent, &self.url, &[], &body(self.format, message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifiers::Kind;
    use crate::notifiers::tests::{agent, capture_server, message};

    #[test]
    fn bodies() {
        let m = message(Kind::Down);
        assert_eq!(
            body(Format::Json, &m),
            r#"{"event":"down","monitor":"blog","name":"My \"blog\"","detail":"HTTP 503","time":"2026-10-22T14:23:11Z","text":"DOWN: My \"blog\" (HTTP 503) at 2026-10-22 16:23 CEST"}"#
        );
        assert!(body(Format::Discord, &m).starts_with(r#"{"content":"DOWN: "#));
        assert!(body(Format::Slack, &m).starts_with(r#"{"text":"DOWN: "#));
    }

    #[test]
    fn sends_and_reports_errors() {
        let (url, rx) = capture_server("204 No Content");
        let hook = Webhook {
            url: format!("{url}/hook"),
            format: Format::Slack,
        };
        hook.send(&agent(), &message(Kind::Up)).unwrap();
        let got = rx.recv().unwrap();
        assert_eq!(got.request_line, "POST /hook HTTP/1.1");
        assert!(
            got.headers
                .iter()
                .any(|h| h.eq_ignore_ascii_case("content-type: application/json"))
        );
        assert!(got.body.contains("UP: My"));

        let (url, _rx) = capture_server("500 Internal Server Error");
        let hook = Webhook {
            url,
            format: Format::Json,
        };
        assert_eq!(
            hook.send(&agent(), &message(Kind::Up)),
            Err("HTTP 500".into())
        );
    }
}
