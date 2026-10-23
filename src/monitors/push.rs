//! `push` monitor: another system calls `/push/<token>` (SPEC.md §4.6).
//!
//! Nothing is polled. The built-in server accepts pushes and the event loop
//! fails the monitor if no push arrived within `interval + grace`.

use std::path::PathBuf;

use serde::Deserialize;

use super::{Check, CheckResult};
use crate::config::FileProblems;
use crate::config::secret::{self, Secret, is_world_readable};

pub const MIN_TOKEN_LEN: usize = 32;
pub const MAX_TOKEN_LEN: usize = 128;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PushFile {
    token: Option<String>,
    token_file: Option<PathBuf>,
}

pub fn is_valid_token(token: &str) -> bool {
    (MIN_TOKEN_LEN..=MAX_TOKEN_LEN).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn build(file: PushFile, ctx: &super::Context, p: &mut FileProblems) -> Option<Secret> {
    let mut warnings = Vec::new();
    let token_file = file
        .token_file
        .map(|f| crate::config::resolve_path(ctx.config_dir, &f));
    let token = secret::resolve(
        "push.token",
        file.token.as_deref(),
        token_file.as_deref(),
        &mut warnings,
    )
    .unwrap_or_else(|e| {
        p.error(e);
        None
    });
    if file.token.is_some() && is_world_readable(ctx.file_meta) {
        warnings.push(
            "contains push.token but is readable by all users (chmod o-r, or use token_file)"
                .into(),
        );
    }
    for w in warnings {
        p.warning(w);
    }
    let Some(token) = token else {
        p.error("push needs token or token_file (create one with: uptime-kinomi gen-token)");
        return None;
    };
    if !is_valid_token(token.expose()) {
        p.error(format!(
            "push.token must be {MIN_TOKEN_LEN} to {MAX_TOKEN_LEN} characters from A-Z, a-z, 0-9, _ and - (use: uptime-kinomi gen-token)"
        ));
    }
    Some(token)
}

/// Placeholder: push monitors are never polled by the scheduler.
pub struct PushCheck;

impl Check for PushCheck {
    fn check(&self) -> CheckResult {
        CheckResult::fail(None, "push monitors are not polled")
    }

    fn target(&self) -> String {
        "push".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        assert!(is_valid_token(&"a".repeat(32)));
        assert!(is_valid_token(
            "JszWA5OXxvVKjAna6eBy9RwPAX2bP0vyX_LI_dGNn2w"
        ));
        assert!(!is_valid_token(&"a".repeat(31)));
        assert!(!is_valid_token(&"a".repeat(129)));
        assert!(!is_valid_token(&format!("{}/", "a".repeat(32))));
        assert!(!is_valid_token(&format!("{}=", "a".repeat(32))));
    }
}
