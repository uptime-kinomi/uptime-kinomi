//! `telegram`: messages via a Telegram bot (SPEC.md §5.3). The Bot API puts
//! the token in the URL path, so URLs of this notifier are never logged.

use std::path::PathBuf;

use serde::Deserialize;

use super::{Context, Json, Message, Target, json, post_json};
use crate::config::FileProblems;
use crate::config::secret::Secret;

const API: &str = "https://api.telegram.org";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramFile {
    bot_token: Option<String>,
    bot_token_file: Option<PathBuf>,
    /// A numeric chat ID (also negative for groups) or "@channelname".
    chat_id: String,
}

pub struct Telegram {
    api: String,
    bot_token: Secret,
    chat_id: String,
}

fn is_bot_token(token: &str) -> bool {
    match token.split_once(':') {
        Some((id, key)) => {
            !id.is_empty()
                && id.len() <= 20
                && id.bytes().all(|b| b.is_ascii_digit())
                && (30..=64).contains(&key.len())
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        }
        None => false,
    }
}

fn is_chat_id(id: &str) -> bool {
    let numeric = id.strip_prefix('-').unwrap_or(id);
    (!numeric.is_empty() && numeric.len() <= 20 && numeric.bytes().all(|b| b.is_ascii_digit()))
        || id.strip_prefix('@').is_some_and(|name| {
            (5..=32).contains(&name.len())
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
}

pub fn build(file: TelegramFile, ctx: &Context, p: &mut FileProblems) -> Option<Telegram> {
    let token = ctx.secret(
        "telegram.bot_token",
        file.bot_token.as_deref(),
        file.bot_token_file.as_deref(),
        p,
    );
    let Some(bot_token) = token else {
        p.error("telegram needs bot_token or bot_token_file");
        return None;
    };
    if !is_bot_token(bot_token.expose()) {
        p.error("telegram.bot_token does not look like a bot token (123456:ABC-...)");
    }
    if !is_chat_id(&file.chat_id) {
        p.error("telegram.chat_id must be a number or @channelname");
    }
    Some(Telegram {
        api: API.into(),
        bot_token,
        chat_id: file.chat_id,
    })
}

impl Target for Telegram {
    fn send(&self, agent: &ureq::Agent, m: &Message) -> Result<(), String> {
        let url = format!("{}/bot{}/sendMessage", self.api, self.bot_token.expose());
        let text = m.text();
        let body = json(&[
            ("chat_id", Json::Str(&self.chat_id)),
            ("text", Json::Str(&text)),
            ("disable_web_page_preview", Json::Bool(true)),
        ]);
        post_json(agent, &url, &[], &body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifiers::Kind;
    use crate::notifiers::tests::{agent, capture_server, message};

    const TOKEN: &str = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";

    #[test]
    fn validation() {
        assert!(is_bot_token(TOKEN));
        for bad in [
            "",
            "123456",
            "abc:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw",
            "123:short",
            "123:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDs/w",
        ] {
            assert!(!is_bot_token(bad), "{bad}");
        }
        for ok in ["12345", "-1001234567890", "@my_channel"] {
            assert!(is_chat_id(ok), "{ok}");
        }
        for bad in ["", "-", "abc", "@ab", "@bad-name", "1 2"] {
            assert!(!is_chat_id(bad), "{bad}");
        }
    }

    #[test]
    fn sends_message() {
        let (url, rx) = capture_server("200 OK");
        let mut warnings = Vec::new();
        let bot_token = crate::config::secret::resolve("t", Some(TOKEN), None, &mut warnings)
            .unwrap()
            .unwrap();
        let telegram = Telegram {
            api: url,
            bot_token,
            chat_id: "-100123".into(),
        };
        telegram.send(&agent(), &message(Kind::Warn)).unwrap();
        let got = rx.recv().unwrap();
        assert_eq!(
            got.request_line,
            format!("POST /bot{TOKEN}/sendMessage HTTP/1.1")
        );
        assert!(
            got.body
                .starts_with(r#"{"chat_id":"-100123","text":"WARN: "#),
            "{}",
            got.body
        );
    }
}
