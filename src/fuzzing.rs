//! Entry points for the fuzz targets in fuzz/ (SPEC.md §11). Each takes
//! arbitrary bytes and must never panic; some also check properties
//! (round trips) and panic on purpose when one fails.
//!
//! They are compiled in every build, and `tests::corpus` runs the seed
//! corpus through all of them, so they keep working without nightly.

use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, mpsc};

use crate::config::global::Defaults;
use crate::config::pages::Page;
use crate::config::{Problems, duration, global, incidents, maintenance, pages};
use crate::encoding::{base64, base64_decode, bcrypt_decode, bcrypt_encode};
use crate::history::History;
use crate::monitors::dns::{RecordType, parse_response};
use crate::monitors::{self, Monitor};
use crate::render::{File, Site};
use crate::server::auth::{Auth, htpasswd_line};
use crate::server::push::PushRegistry;
use crate::server::{Options, parse_request, route};
use crate::storage::{log, replay, statefile};
use crate::{clock, notifiers, paths, update};

/// A push token known to the HTTP target, so `/push/` gets past the lookup.
pub const PUSH_TOKEN: &str = "fuzzfuzzfuzzfuzzfuzzfuzzfuzzfuzz";

fn text(data: &[u8]) -> Option<&str> {
    std::str::from_utf8(data).ok()
}

/// A config directory that does not exist: `*_file` secrets are not found.
fn config_dir() -> &'static Path {
    Path::new("/nonexistent/uptime-kinomi-fuzz")
}

/// Metadata for the permission checks of secrets (any existing directory).
fn file_meta() -> Option<&'static Metadata> {
    static META: OnceLock<Option<Metadata>> = OnceLock::new();
    META.get_or_init(|| std::fs::metadata("/").ok()).as_ref()
}

fn defaults() -> Defaults {
    Defaults {
        interval: duration::Dur::secs(60),
        timeout: duration::Dur::secs(10),
        retries: 1,
        notifications: vec![],
    }
}

fn monitor(id: &str, text: &str) -> Option<Monitor> {
    let defaults = defaults();
    let notifiers = vec!["phone".to_string()];
    let ctx = monitors::Context {
        config_dir: config_dir(),
        defaults: &defaults,
        notifiers: &notifiers,
        file_meta: file_meta()?,
    };
    let mut problems = Problems::default();
    monitors::parse(
        id,
        text,
        &ctx,
        &mut problems.for_file(Path::new("monitors/m.toml")),
    )
}

/// Monitors the page, incident and maintenance targets can refer to.
fn known_monitors() -> &'static [Monitor] {
    static MONITORS: OnceLock<Vec<Monitor>> = OnceLock::new();
    MONITORS.get_or_init(|| {
        [
            ("blog", "config_version = 1\ntype = \"http\"\nname = \"Blog\"\n[http]\nurl = \"https://example.org/\"\n"),
            ("backup", &format!(
                "config_version = 1\ntype = \"push\"\nname = \"Backup\"\ninterval = \"1d\"\n[push]\ntoken = \"{PUSH_TOKEN}\"\n"
            )),
        ]
        .iter()
        .filter_map(|(id, text)| monitor(id, text))
        .collect()
    })
}

fn known_pages() -> &'static [Page] {
    static PAGES: OnceLock<Vec<Page>> = OnceLock::new();
    PAGES.get_or_init(|| {
        let mut problems = Problems::default();
        let text = "config_version = 1\ntitle = \"Home\"\npublic = true\n[[groups]]\nname = \"Web\"\nmonitors = [\"blog\", \"backup\"]\n";
        pages::parse(
            "home",
            text,
            known_monitors(),
            &mut problems.for_file(Path::new("pages/home.toml")),
        )
        .into_iter()
        .collect()
    })
}

fn server() -> &'static (Options, Site) {
    static SERVER: OnceLock<(Options, Site)> = OnceLock::new();
    SERVER.get_or_init(|| {
        // A real hash line with cost 4 instead of 10: same code path, fast.
        let line = htpasswd_line("admin", b"pw", [7; 16]).replacen("$10$", "$04$", 1);
        let (tx, rx) = mpsc::channel();
        drop(rx);
        let options = Options {
            listen: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            hostnames: vec!["status.example.org".into()],
            default_page: Some("/home/".into()),
            auth: Auth::parse(&line).ok(),
            push: Some(PushRegistry::new(known_monitors(), tx)),
            health: None,
        };
        let mut site = Site::default();
        for (path, private) in [
            ("/home/", false),
            ("/assets/kinomi.css", false),
            ("/private/all/", true),
        ] {
            site.files.insert(
                path.into(),
                std::sync::Arc::new(File {
                    content_type: "text/html; charset=utf-8",
                    body: b"page".to_vec(),
                    private,
                }),
            );
        }
        (options, site)
    })
}

/// An HTTP request head from the network: parsing and routing.
pub fn http_request(data: &[u8]) {
    let (options, site) = server();
    if let Some(req) = parse_request(data) {
        let response = route(&req, options, site);
        assert!((100..600).contains(&response.status));
    }
}

/// A DNS response from the network, followed by one byte that picks the
/// record type. The first two bytes double as the query ID, so the parser
/// gets past the ID check.
pub fn dns_response(data: &[u8]) {
    let Some((&kind, msg)) = data.split_last() else {
        return;
    };
    let id = match msg {
        [a, b, ..] => u16::from_be_bytes([*a, *b]),
        _ => 0,
    };
    let kind = match kind % 6 {
        0 => RecordType::A,
        1 => RecordType::Aaaa,
        2 => RecordType::Cname,
        3 => RecordType::Mx,
        4 => RecordType::Txt,
        _ => RecordType::Ns,
    };
    let _ = parse_response(msg, id, "example.org", kind);
}

/// A redirect `Location` header from a monitored server. The result must
/// stay on http(s) and never go from https to http; resolving a second
/// redirect from the result exercises odd base URLs.
pub fn http_redirect(data: &[u8]) {
    let Some(location) = text(data) else { return };
    for current in [
        "https://example.org/a/b?q=1",
        "http://example.org",
        "https://example.org:8443/",
    ] {
        let mut url = current.to_string();
        for _ in 0..2 {
            let Ok(next) = monitors::http::next_url(&url, location) else {
                break;
            };
            assert!(
                next.starts_with("https://")
                    || (next.starts_with("http://") && !url.starts_with("https://")),
                "redirect from {url:?} via {location:?} to {next:?}"
            );
            url = next;
        }
    }
}

/// A certificate from a TLS handshake: the notAfter reader.
pub fn x509(data: &[u8]) {
    let _ = monitors::tls::cert_not_after(data);
}

/// Release metadata from the network (self-update): the signed checksum
/// file, its signature and the version in it. Layout: two bytes length of
/// SHA256SUMS, then SHA256SUMS, then the signature.
pub fn release(data: &[u8]) {
    let (sums, sig) = match data {
        [a, b, rest @ ..] => {
            rest.split_at(usize::from(u16::from_be_bytes([*a, *b])).min(rest.len()))
        }
        _ => (data, &[][..]),
    };
    if let Ok(keys) = update::release_keys() {
        let _ = update::verify_signature(sums, sig, &keys);
    }
    for asset in ["uptime-kinomi-armv6", "uptime-kinomi-x86_64"] {
        let _ = update::checksum_for(sums, asset);
    }
    if let Some(t) = text(data) {
        let _ = update::Version::parse(t);
    }
}

/// An `Authorization` header (network) and an htpasswd file (disk).
pub fn auth(data: &[u8]) {
    let Some(t) = text(data) else { return };
    let (options, _) = server();
    if let Some(auth) = &options.auth {
        let _ = auth.check(Some(t));
    }
    let _ = Auth::parse(t);
}

/// Configuration files of every kind. The first byte picks the kind.
pub fn config(data: &[u8]) {
    let Some((&kind, rest)) = data.split_first() else {
        return;
    };
    let Some(t) = text(rest) else { return };
    let mut problems = Problems::default();
    let file = PathBuf::from("fuzz.toml");
    match kind % 7 {
        0 => {
            let env = |key: &str| (key == "HOME").then(|| "/home/fuzz".into());
            let _ = global::parse(t, &file, config_dir(), &env, None, &mut problems);
        }
        1 => {
            let _ = monitor("m", t);
        }
        2 => {
            if let Some(file_meta) = file_meta() {
                let ctx = notifiers::Context {
                    config_dir: config_dir(),
                    file_meta,
                };
                let _ = notifiers::parse("n", t, &ctx, &mut problems.for_file(&file));
            }
        }
        3 => {
            let _ = pages::parse("p", t, known_monitors(), &mut problems.for_file(&file));
        }
        4 => {
            let _ = incidents::parse("i", t, known_pages(), &mut problems.for_file(&file));
        }
        5 => {
            let _ = maintenance::parse("w", t, known_monitors(), &mut problems.for_file(&file));
        }
        _ => {
            let _ = duration::parse(t);
        }
    }
    let _ = paths::default_data_dir(&|_| None);
}

/// Log lines and state files from disk (replayed at startup).
pub fn storage(data: &[u8]) {
    let Some(t) = text(data) else { return };
    for line in t.lines() {
        let _ = replay::parse_line(line);
    }
    let mut history = History::new(20, 30);
    let _ = statefile::from_toml(t, "m", &mut history);
    let _ = crate::state::State::parse(t);
    let _ = crate::history::Beat::parse(t);

    // Round trip: a value written by the log writer reads back unchanged
    // (control characters become '?').
    let mut line = String::from("2026-10-27T12:00:00Z k=");
    log::push_value(&mut line, t, log::MAX_VALUE_CHARS);
    let parsed = replay::parse_line(&line);
    let expected: String = t
        .chars()
        .take(log::MAX_VALUE_CHARS)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    let got = parsed
        .as_ref()
        .and_then(|l| l.fields.first())
        .map(|(_, v)| v.as_str());
    assert_eq!(
        got,
        Some(expected.as_str()),
        "log value round trip: {line:?}"
    );
}

/// Encodings and time formats.
pub fn encodings(data: &[u8]) {
    assert_eq!(
        base64_decode(&base64(data)).as_deref(),
        Some(data),
        "base64 round trip"
    );
    if data.len() == 16 || data.len() == 23 {
        assert_eq!(
            bcrypt_decode(&bcrypt_encode(data)).as_deref(),
            Some(data),
            "bcrypt base64 round trip"
        );
    }
    if let Some(t) = text(data) {
        let _ = base64_decode(t);
        let _ = bcrypt_decode(t);
        let _ = clock::parse_date(t);
        let _ = clock::parse_rfc3339(t);
        let _ = clock::parse_hour(t);
        let _ = incidents::parse_timestamp(t);
        let _ = duration::parse(t);
    }
    // Round trip of timestamps between 1970 and 9999.
    if let Some(bytes) = data.get(..8) {
        let mut b = [0u8; 8];
        b.copy_from_slice(bytes);
        let secs = (u64::from_le_bytes(b) % 253_402_300_800) as i64;
        let formatted = clock::format_rfc3339(clock::from_unix(secs));
        assert_eq!(
            clock::parse_rfc3339(&formatted),
            Some(secs),
            "RFC 3339 round trip: {formatted}"
        );
    }
}

/// A fuzz entry point.
pub type Target = fn(&[u8]);

/// All targets by name, as used by fuzz/ and the corpus test.
pub const TARGETS: &[(&str, Target)] = &[
    ("http_request", http_request),
    ("http_redirect", http_redirect),
    ("dns_response", dns_response),
    ("x509", x509),
    ("release", release),
    ("auth", auth),
    ("config", config),
    ("storage", storage),
    ("encodings", encodings),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every file of fuzz/seeds/<target>/ through its target, plus some
    /// fixed edge cases, on stable Rust.
    #[test]
    fn corpus() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz/seeds");
        for (name, target) in TARGETS {
            for input in [&b""[..], b"\0", b"\xff\xff\xff\xff", &[0x80; 64]] {
                target(input);
            }
            let Ok(dir) = std::fs::read_dir(root.join(name)) else {
                continue;
            };
            for entry in dir.flatten() {
                let data = std::fs::read(entry.path()).unwrap();
                target(&data);
            }
        }
    }

    #[test]
    fn known_fixtures_parse() {
        assert_eq!(known_monitors().len(), 2);
        assert_eq!(known_pages().len(), 1);
        assert!(server().0.auth.is_some());
    }
}
