//! The global configuration file `kinomi.toml` (SPEC.md §3.3).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::duration::Dur;
use super::{Problems, is_valid_id, resolve_path};
use crate::paths;

#[derive(Debug)]
// Parsed and validated now; used by the server, pages and notifications
// (milestones M2 to M4).
#[allow(dead_code)]
pub struct Global {
    pub server: Server,
    pub pages: Pages,
    pub storage: Storage,
    pub scheduler: Scheduler,
    pub heartbeat: Option<Heartbeat>,
    pub defaults: Defaults,
}

#[derive(Debug)]
#[allow(dead_code)] // used by the built-in server (M2)
pub struct Server {
    pub enabled: bool,
    pub listen: SocketAddr,
    pub hostnames: Vec<String>,
    pub base_url: Option<String>,
    pub htpasswd_file: Option<PathBuf>,
    pub metrics: bool,
}

#[derive(Debug)]
#[allow(dead_code)] // used by the page renderer (M2)
pub struct Pages {
    pub output_dir: PathBuf,
    pub render_interval: Dur,
    pub timezone: String,
    pub custom_css: Option<PathBuf>,
    pub auto_refresh: Dur,
    pub default_page: Option<String>,
    pub recent_checks: u32,
}

#[derive(Debug)]
pub struct Storage {
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub flush_interval: Dur,
    pub state_interval: Dur,
    pub log_retention: Dur,
    pub history_retention: Dur,
}

#[derive(Debug)]
pub struct Scheduler {
    pub workers: u32,
    pub max_queue: u32,
}

#[derive(Debug)]
#[allow(dead_code)] // outbound heartbeat (M3)
pub struct Heartbeat {
    pub url: String,
    pub interval: Dur,
}

#[derive(Debug)]
pub struct Defaults {
    pub interval: Dur,
    pub timeout: Dur,
    pub retries: u32,
    pub notifications: Vec<String>,
}

// ---- file format ---------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GlobalFile {
    config_version: u32,
    #[serde(default)]
    server: ServerFile,
    #[serde(default)]
    pages: PagesFile,
    #[serde(default)]
    storage: StorageFile,
    #[serde(default)]
    scheduler: SchedulerFile,
    heartbeat: Option<HeartbeatFile>,
    #[serde(default)]
    defaults: DefaultsFile,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerFile {
    enabled: Option<bool>,
    listen: Option<String>,
    #[serde(default)]
    hostnames: Vec<String>,
    base_url: Option<String>,
    htpasswd_file: Option<PathBuf>,
    #[serde(default)]
    metrics: bool,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PagesFile {
    output_dir: Option<PathBuf>,
    render_interval: Option<Dur>,
    timezone: Option<String>,
    custom_css: Option<PathBuf>,
    auto_refresh: Option<Dur>,
    default_page: Option<String>,
    recent_checks: Option<u32>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StorageFile {
    data_dir: Option<PathBuf>,
    log_dir: Option<PathBuf>,
    flush_interval: Option<Dur>,
    state_interval: Option<Dur>,
    log_retention: Option<Dur>,
    history_retention: Option<Dur>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SchedulerFile {
    workers: Option<u32>,
    max_queue: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HeartbeatFile {
    url: String,
    interval: Option<Dur>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DefaultsFile {
    interval: Option<Dur>,
    timeout: Option<Dur>,
    retries: Option<u32>,
    notifications: Option<Vec<String>>,
}

// ---- validation ----------------------------------------------------------

pub const MIN_INTERVAL: Dur = Dur::secs(10);
pub const MAX_RETRIES: u32 = 10;

pub fn parse(
    text: &str,
    file: &Path,
    config_dir: &Path,
    env: paths::EnvFn,
    runtime: Option<&Path>,
    problems: &mut Problems,
) -> Option<Global> {
    let raw: GlobalFile = match toml::from_str(text) {
        Ok(raw) => raw,
        Err(e) => {
            problems.error(file, e.to_string());
            return None;
        }
    };
    let mut p = problems.for_file(file);
    if raw.config_version != 1 {
        p.error(format!(
            "config_version must be 1, found {}",
            raw.config_version
        ));
    }

    let server = {
        let s = raw.server;
        let listen_text = s.listen.as_deref().unwrap_or("127.0.0.1:8080");
        let listen = listen_text.parse().unwrap_or_else(|_| {
            p.error(format!(
                "server.listen '{listen_text}' is not an address like 127.0.0.1:8080"
            ));
            SocketAddr::from(([127, 0, 0, 1], 8080))
        });
        for host in &s.hostnames {
            if !is_valid_hostname(host) {
                p.error(format!(
                    "server.hostnames: '{host}' is not a valid host name"
                ));
            }
        }
        if let Some(url) = &s.base_url {
            if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() > 2048 {
                p.error("server.base_url must start with https:// or http://");
            }
        }
        let enabled = s.enabled.unwrap_or(true);
        if enabled && s.hostnames.is_empty() {
            p.warning("server.hostnames is empty: any Host header is accepted");
        }
        let htpasswd_file = s.htpasswd_file.map(|f| resolve_path(config_dir, &f));
        if let Some(f) = &htpasswd_file {
            if !f.is_file() {
                p.error(format!(
                    "server.htpasswd_file {} does not exist",
                    f.display()
                ));
            }
        }
        Server {
            enabled,
            listen,
            hostnames: s.hostnames,
            base_url: s.base_url,
            htpasswd_file,
            metrics: s.metrics,
        }
    };

    let storage = {
        let s = raw.storage;
        let dir = |configured: Option<PathBuf>,
                   default: Option<PathBuf>,
                   key: &str,
                   p: &mut super::FileProblems| {
            configured
                .map(|d| resolve_path(config_dir, &d))
                .or(default)
                .unwrap_or_else(|| {
                    p.error(format!(
                        "storage.{key} cannot be determined (no $HOME); set it explicitly"
                    ));
                    PathBuf::new()
                })
        };
        let data_dir = dir(s.data_dir, paths::default_data_dir(env), "data_dir", &mut p);
        // Both follow a configured data_dir (SPEC.md §10.6).
        let default_log =
            (!data_dir.as_os_str().is_empty()).then(|| paths::default_log_dir(&data_dir));
        let log_dir = dir(s.log_dir, default_log, "log_dir", &mut p);
        let storage = Storage {
            data_dir,
            log_dir,
            flush_interval: s.flush_interval.unwrap_or(Dur::secs(30)),
            state_interval: s.state_interval.unwrap_or(Dur::secs(600)),
            log_retention: s.log_retention.unwrap_or(Dur::secs(90 * 86400)),
            history_retention: s.history_retention.unwrap_or(Dur::secs(400 * 86400)),
        };
        if storage.flush_interval < Dur::secs(1) || storage.flush_interval > Dur::secs(600) {
            p.error("storage.flush_interval must be between 1s and 10m");
        }
        if storage.state_interval < Dur::secs(60) {
            p.error("storage.state_interval must be at least 1m");
        }
        if storage.log_retention < Dur::secs(86400) {
            p.error("storage.log_retention must be at least 1d");
        }
        if storage.history_retention < Dur::secs(86400) {
            p.error("storage.history_retention must be at least 1d");
        }
        storage
    };

    let pages = {
        let s = raw.pages;
        let output_dir = s
            .output_dir
            .map(|d| resolve_path(config_dir, &d))
            .or_else(|| {
                (!storage.data_dir.as_os_str().is_empty())
                    .then(|| paths::default_output_dir(&storage.data_dir, runtime))
            })
            .unwrap_or_else(|| {
                p.error("pages.output_dir cannot be determined (no $HOME); set it explicitly");
                PathBuf::new()
            });
        let timezone = s.timezone.unwrap_or_else(|| "UTC".to_string());
        if !is_plausible_timezone(&timezone) {
            p.error(format!(
                "pages.timezone '{timezone}' is not an IANA time zone like Europe/Berlin"
            ));
        } else if timezone != "UTC" && jiff::tz::TimeZone::get(&timezone).is_err() {
            p.warning(format!(
                "pages.timezone '{timezone}' not found in the system time zone database; UTC is used"
            ));
        }
        let custom_css = s.custom_css.map(|f| resolve_path(config_dir, &f));
        if let Some(f) = &custom_css {
            if !f.is_file() {
                p.error(format!("pages.custom_css {} does not exist", f.display()));
            }
        }
        if let Some(page) = &s.default_page {
            if !is_valid_id(page) {
                p.error(format!(
                    "pages.default_page '{page}' is not a valid page ID"
                ));
            }
        }
        let pages = Pages {
            output_dir,
            render_interval: s.render_interval.unwrap_or(Dur::secs(60)),
            timezone,
            custom_css,
            auto_refresh: s.auto_refresh.unwrap_or(Dur::secs(60)),
            default_page: s.default_page,
            recent_checks: s.recent_checks.unwrap_or(50),
        };
        if pages.render_interval < Dur::secs(10) {
            p.error("pages.render_interval must be at least 10s");
        }
        if !pages.auto_refresh.is_zero() && pages.auto_refresh < Dur::secs(10) {
            p.error("pages.auto_refresh must be 0s (off) or at least 10s");
        }
        if !(1..=200).contains(&pages.recent_checks) {
            p.error("pages.recent_checks must be between 1 and 200");
        }
        pages
    };

    let scheduler = Scheduler {
        workers: raw.scheduler.workers.unwrap_or(4),
        max_queue: raw.scheduler.max_queue.unwrap_or(32),
    };
    if !(1..=64).contains(&scheduler.workers) {
        p.error("scheduler.workers must be between 1 and 64");
    }
    if !(1..=10_000).contains(&scheduler.max_queue) {
        p.error("scheduler.max_queue must be between 1 and 10000");
    }

    let heartbeat = raw.heartbeat.map(|h| {
        if !h.url.starts_with("https://") && !h.url.starts_with("http://") {
            p.error("heartbeat.url must start with https:// or http://");
        }
        let interval = h.interval.unwrap_or(Dur::secs(60));
        if interval < MIN_INTERVAL {
            p.error("heartbeat.interval must be at least 10s");
        }
        Heartbeat {
            url: h.url,
            interval,
        }
    });

    let defaults = Defaults {
        interval: raw.defaults.interval.unwrap_or(Dur::secs(60)),
        timeout: raw.defaults.timeout.unwrap_or(Dur::secs(10)),
        retries: raw.defaults.retries.unwrap_or(1),
        notifications: raw.defaults.notifications.unwrap_or_default(),
    };
    if defaults.interval < MIN_INTERVAL {
        p.error("defaults.interval must be at least 10s");
    }
    if defaults.timeout.is_zero() {
        p.error("defaults.timeout must not be 0s");
    }
    if defaults.retries > MAX_RETRIES {
        p.error(format!("defaults.retries must be at most {MAX_RETRIES}"));
    }

    Some(Global {
        server,
        pages,
        storage,
        scheduler,
        heartbeat,
        defaults,
    })
}

/// A DNS name or an IP address (IPv6 without brackets, e.g. `::1`).
fn is_valid_hostname(host: &str) -> bool {
    host.parse::<std::net::Ipv6Addr>().is_ok()
        || !host.is_empty()
            && host.len() <= 253
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
}

/// "UTC" or "Area/Location[/More]" with the characters used by the tz database.
fn is_plausible_timezone(tz: &str) -> bool {
    tz == "UTC"
        || (tz.len() <= 64
            && tz.contains('/')
            && !tz.contains("..")
            && !tz.starts_with('/')
            && tz.split('/').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+'))
            }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn env(key: &str) -> Option<OsString> {
        (key == "HOME").then(|| OsString::from("/home/u"))
    }

    fn parse_text(text: &str) -> (Option<Global>, Problems) {
        let mut problems = Problems::default();
        let global = parse(
            text,
            Path::new("kinomi.toml"),
            Path::new("/cfg"),
            &env,
            None,
            &mut problems,
        );
        (global, problems)
    }

    #[test]
    fn minimal_file_gets_defaults() {
        let (global, problems) = parse_text("config_version = 1\n");
        let g = global.unwrap();
        assert!(!problems.has_errors(), "{problems:?}");
        assert_eq!(g.server.listen, SocketAddr::from(([127, 0, 0, 1], 8080)));
        assert_eq!(
            g.storage.data_dir,
            PathBuf::from("/home/u/.local/state/uptime-kinomi")
        );
        assert_eq!(
            g.storage.log_dir,
            PathBuf::from("/home/u/.local/state/uptime-kinomi/log")
        );
        assert_eq!(
            g.pages.output_dir,
            PathBuf::from("/home/u/.local/state/uptime-kinomi/www")
        );
        assert_eq!(g.scheduler.workers, 4);
        assert_eq!(g.defaults.interval, Dur::secs(60));
        // Empty hostnames is allowed but warned about.
        assert_eq!(problems.warnings().count(), 1);
    }

    #[test]
    fn spec_example_parses() {
        let text = r#"
config_version = 1

[server]
enabled = true
listen = "127.0.0.1:8080"
hostnames = ["status.example.org"]
base_url = "https://status.example.org"

[pages]
render_interval = "60s"
timezone = "UTC"
auto_refresh = "60s"
default_page = "home"
recent_checks = 50

[storage]
data_dir = "data"
flush_interval = "30s"
state_interval = "10m"
log_retention = "90d"
history_retention = "400d"

[scheduler]
workers = 4
max_queue = 32

[heartbeat]
url = "https://other-pi.example.org/push/abcdef"
interval = "60s"

[defaults]
interval = "60s"
timeout = "10s"
retries = 1
notifications = ["ntfy-home"]
"#;
        let (global, problems) = parse_text(text);
        assert!(!problems.has_errors(), "{problems:?}");
        let g = global.unwrap();
        assert_eq!(g.storage.data_dir, PathBuf::from("/cfg/data"));
        // Log and page directories follow the configured data_dir.
        assert_eq!(g.storage.log_dir, PathBuf::from("/cfg/data/log"));
        assert_eq!(g.pages.output_dir, PathBuf::from("/cfg/data/www"));
        assert_eq!(g.defaults.notifications, vec!["ntfy-home".to_string()]);
        assert!(g.heartbeat.is_some());
    }

    #[test]
    fn errors() {
        for (text, expected) in [
            ("config_version = 2\n", "config_version"),
            ("", "config_version"),
            ("config_version = 1\nbogus = 1\n", "bogus"),
            (
                "config_version = 1\n[server]\nlisten = \"localhost\"\n",
                "server.listen",
            ),
            (
                "config_version = 1\n[server]\nhostnames = [\"a b\"]\n",
                "hostnames",
            ),
            ("config_version = 1\n[scheduler]\nworkers = 0\n", "workers"),
            (
                "config_version = 1\n[defaults]\ninterval = \"5s\"\n",
                "interval",
            ),
            (
                "config_version = 1\n[defaults]\ninterval = \"5 s\"\n",
                "duration",
            ),
            (
                "config_version = 1\n[pages]\ntimezone = \"../etc/passwd\"\n",
                "timezone",
            ),
            (
                "config_version = 1\n[pages]\ndefault_page = \"Home Page\"\n",
                "default_page",
            ),
        ] {
            let (_, problems) = parse_text(text);
            let all = format!("{problems:?}");
            assert!(problems.has_errors(), "no error for {text:?}");
            assert!(all.contains(expected), "{text:?}: {all}");
        }
    }

    #[test]
    fn hostnames() {
        assert!(is_valid_hostname("status.example.org"));
        assert!(is_valid_hostname("localhost"));
        assert!(is_valid_hostname("::1"));
        assert!(is_valid_hostname("2001:db8::1"));
        for bad in [
            "", "a..b", "-a.org", "a-.org", "a_b.org", "a b", "ä.org", "[::1]", ":::",
        ] {
            assert!(!is_valid_hostname(bad), "{bad}");
        }
    }

    #[test]
    fn timezones() {
        assert!(is_plausible_timezone("UTC"));
        assert!(is_plausible_timezone("Europe/Berlin"));
        assert!(is_plausible_timezone("America/Argentina/Buenos_Aires"));
        assert!(is_plausible_timezone("Etc/GMT+1"));
        for bad in [
            "",
            "Berlin",
            "/etc/passwd",
            "Europe/../x",
            "Europe//Berlin",
            "Europe/Ber lin",
        ] {
            assert!(!is_plausible_timezone(bad), "{bad}");
        }
    }
}
