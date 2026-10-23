//! `gotify`: messages to a Gotify server (SPEC.md §5.3). The application
//! token is sent in the `X-Gotify-Key` header, not in the URL.

use std::path::PathBuf;

use serde::Deserialize;

use super::{Context, Json, Kind, Message, Target, check_url, json, post_json};
use crate::config::FileProblems;
use crate::config::secret::Secret;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GotifyFile {
    url: String,
    token: Option<String>,
    token_file: Option<PathBuf>,
    priority: Option<u8>,
}

pub struct Gotify {
    url: String,
    token: Secret,
    priority: Option<u8>,
}

pub fn build(file: GotifyFile, ctx: &Context, p: &mut FileProblems) -> Option<Gotify> {
    check_url("gotify.url", &file.url, p);
    if file.priority.is_some_and(|prio| prio > 10) {
        p.error("gotify.priority must be between 0 and 10");
    }
    let token = ctx.secret(
        "gotify.token",
        file.token.as_deref(),
        file.token_file.as_deref(),
        p,
    );
    let Some(token) = token else {
        p.error("gotify needs token or token_file");
        return None;
    };
    Some(Gotify {
        url: format!("{}/message", file.url.trim_end_matches('/')),
        token,
        priority: file.priority,
    })
}

impl Target for Gotify {
    fn send(&self, agent: &ureq::Agent, m: &Message) -> Result<(), String> {
        let priority = self.priority.unwrap_or(match m.kind {
            Kind::Down | Kind::Reminder => 8,
            Kind::Up | Kind::Warn => 5,
        });
        let (title, text) = (m.title(), m.text());
        let body = json(&[
            ("title", Json::Str(&title)),
            ("message", Json::Str(&text)),
            ("priority", Json::Num(i64::from(priority))),
        ]);
        post_json(
            agent,
            &self.url,
            &[("X-Gotify-Key", self.token.expose())],
            &body,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifiers::tests::{agent, capture_server, message};

    #[test]
    fn posts_message() {
        let (url, rx) = capture_server("200 OK");
        let mut warnings = Vec::new();
        let token = crate::config::secret::resolve("t", Some("AbCd"), None, &mut warnings)
            .unwrap()
            .unwrap();
        let gotify = Gotify {
            url: format!("{url}/message"),
            token,
            priority: None,
        };
        gotify.send(&agent(), &message(Kind::Up)).unwrap();
        let got = rx.recv().unwrap();
        assert_eq!(got.request_line, "POST /message HTTP/1.1");
        assert!(
            got.headers
                .iter()
                .any(|h| h.eq_ignore_ascii_case("x-gotify-key: AbCd")),
            "{:?}",
            got.headers
        );
        assert!(got.body.ends_with(r#""priority":5}"#), "{}", got.body);
    }
}
