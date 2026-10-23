//! Loading and validating the configuration directory (SPEC.md §3).
//!
//! All problems are collected (not just the first), so `check-config` can
//! report everything at once.

pub mod duration;
pub mod global;
pub mod incidents;
pub mod maintenance;
pub mod pages;
pub mod secret;

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::monitors::{self, Kind, Monitor};
use crate::notifiers::{self, Notifier};
use crate::paths;
use global::Global;
use incidents::Incident;
use maintenance::Maintenance;
use pages::Page;

/// Largest accepted config file (SPEC.md §3.2).
const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Debug)]
pub struct Config {
    pub dir: PathBuf,
    pub global: Global,
    pub monitors: Vec<Monitor>,
    pub pages: Vec<Page>,
    pub notifiers: Vec<Notifier>,
    pub incidents: Vec<Incident>,
    pub maintenance: Vec<Maintenance>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug)]
pub struct Problem {
    pub file: PathBuf,
    pub severity: Severity,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{label}: {}: {}", self.file.display(), self.message)
    }
}

#[derive(Default)]
pub struct Problems(Vec<Problem>);

impl fmt::Debug for Problems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for p in &self.0 {
            writeln!(f, "{p}")?;
        }
        Ok(())
    }
}

impl Problems {
    pub fn error(&mut self, file: &Path, message: impl Into<String>) {
        self.push(file, Severity::Error, message.into());
    }

    pub fn warning(&mut self, file: &Path, message: impl Into<String>) {
        self.push(file, Severity::Warning, message.into());
    }

    fn push(&mut self, file: &Path, severity: Severity, message: String) {
        self.0.push(Problem {
            file: file.to_path_buf(),
            severity,
            message,
        });
    }

    pub fn for_file(&mut self, file: &Path) -> FileProblems<'_> {
        FileProblems {
            problems: self,
            file: file.to_path_buf(),
            errors: 0,
        }
    }

    pub fn has_errors(&self) -> bool {
        self.0.iter().any(|p| p.severity == Severity::Error)
    }

    pub fn all(&self) -> impl Iterator<Item = &Problem> {
        self.0.iter()
    }

    #[cfg(test)]
    pub fn warnings(&self) -> impl Iterator<Item = &Problem> {
        self.0.iter().filter(|p| p.severity == Severity::Warning)
    }
}

/// Problems of a single file.
pub struct FileProblems<'a> {
    problems: &'a mut Problems,
    file: PathBuf,
    errors: usize,
}

impl FileProblems<'_> {
    pub fn error(&mut self, message: impl Into<String>) {
        self.errors += 1;
        self.problems.error(&self.file, message);
    }

    pub fn warning(&mut self, message: impl Into<String>) {
        self.problems.warning(&self.file, message);
    }

    /// Errors reported for this file so far.
    pub fn has_errors(&self) -> bool {
        self.errors > 0
    }
}

/// Object IDs: file names without `.toml` (SPEC.md §3.2).
pub fn is_valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-')
}

/// Relative paths in config files are relative to the config directory.
pub fn resolve_path(config_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        config_dir.join(path)
    }
}

pub fn load(dir: &Path, env: paths::EnvFn) -> (Option<Config>, Problems) {
    let mut problems = Problems::default();
    let global_file = dir.join("kinomi.toml");
    let global = match read_config_file(&global_file) {
        Ok((text, _)) => global::parse(&text, &global_file, dir, env, &mut problems),
        Err(e) => {
            problems.error(&global_file, e);
            None
        }
    };

    let notifiers: Vec<String> = list_toml_files(&dir.join("notifications"), &mut problems)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    if let Some(g) = &global {
        for n in &g.defaults.notifications {
            if !notifiers.contains(n) {
                problems.error(
                    &global_file,
                    format!("defaults.notifications: '{n}' does not exist (expected notifications/{n}.toml)"),
                );
            }
        }
    }

    let mut monitor_list = Vec::new();
    if let Some(g) = &global {
        for (id, path) in list_toml_files(&dir.join("monitors"), &mut problems) {
            let (text, meta) = match read_config_file(&path) {
                Ok(read) => read,
                Err(e) => {
                    problems.error(&path, e);
                    continue;
                }
            };
            let ctx = monitors::Context {
                config_dir: dir,
                defaults: &g.defaults,
                notifiers: &notifiers,
                file_meta: &meta,
            };
            if let Some(m) = monitors::parse(&id, &text, &ctx, &mut problems.for_file(&path)) {
                monitor_list.push(m);
            }
        }
        if monitor_list.is_empty() && !problems.has_errors() {
            problems.warning(&dir.join("monitors"), "no monitors configured");
        }
        check_push(g, &monitor_list, &dir.join("monitors"), &mut problems);
    }

    let mut notifier_list = Vec::new();
    for (id, path) in list_toml_files(&dir.join("notifications"), &mut problems) {
        match read_config_file(&path) {
            Ok((text, meta)) => {
                let ctx = notifiers::Context {
                    config_dir: dir,
                    file_meta: &meta,
                };
                let mut p = problems.for_file(&path);
                if let Some(n) = notifiers::parse(&id, &text, &ctx, &mut p) {
                    notifier_list.push(n);
                }
            }
            Err(e) => problems.error(&path, e),
        }
    }

    let mut page_list = Vec::new();
    if global.is_some() {
        for (id, path) in list_toml_files(&dir.join("pages"), &mut problems) {
            match read_config_file(&path) {
                Ok((text, _)) => {
                    let mut p = problems.for_file(&path);
                    if let Some(page) = pages::parse(&id, &text, &monitor_list, &mut p) {
                        page_list.push(page);
                    }
                }
                Err(e) => problems.error(&path, e),
            }
        }
    }
    if let Some(g) = &global {
        check_pages(
            g,
            &page_list,
            &global_file,
            &dir.join("pages"),
            &mut problems,
        );
    }

    let mut incident_list = Vec::new();
    for (id, path) in list_toml_files(&dir.join("incidents"), &mut problems) {
        match read_config_file(&path) {
            Ok((text, _)) => {
                if let Some(i) =
                    incidents::parse(&id, &text, &page_list, &mut problems.for_file(&path))
                {
                    incident_list.push(i);
                }
            }
            Err(e) => problems.error(&path, e),
        }
    }
    let mut maintenance_list = Vec::new();
    for (id, path) in list_toml_files(&dir.join("maintenance"), &mut problems) {
        match read_config_file(&path) {
            Ok((text, _)) => {
                let mut p = problems.for_file(&path);
                if let Some(m) = maintenance::parse(&id, &text, &monitor_list, &mut p) {
                    maintenance_list.push(m);
                }
            }
            Err(e) => problems.error(&path, e),
        }
    }

    let config = match global {
        Some(global) if !problems.has_errors() => Some(Config {
            dir: dir.to_path_buf(),
            global,
            monitors: monitor_list,
            pages: page_list,
            notifiers: notifier_list,
            incidents: incident_list,
            maintenance: maintenance_list,
        }),
        _ => None,
    };
    (config, problems)
}

/// Push tokens must be unique, and pushes need the built-in server.
fn check_push(g: &Global, monitors: &[Monitor], monitors_dir: &Path, problems: &mut Problems) {
    let push: Vec<&Monitor> = monitors.iter().filter(|m| m.kind == Kind::Push).collect();
    for (i, a) in push.iter().enumerate() {
        for b in &push[i + 1..] {
            if a.push_token == b.push_token {
                problems.error(
                    &monitors_dir.join(format!("{}.toml", b.id)),
                    format!(
                        "push token is the same as in {}.toml; every push monitor needs its own",
                        a.id
                    ),
                );
            }
        }
    }
    if !push.is_empty() && !g.server.enabled {
        problems.warning(
            monitors_dir,
            "push monitors need the built-in server (server.enabled = true), or they will always be DOWN",
        );
    }
}

/// Checks across pages and the global configuration.
fn check_pages(
    g: &Global,
    pages: &[Page],
    global_file: &Path,
    pages_dir: &Path,
    problems: &mut Problems,
) {
    if pages.is_empty() && !problems.has_errors() {
        problems.warning(
            pages_dir,
            "no pages configured: no status pages will be generated",
        );
    }
    if let Some(default) = &g.pages.default_page {
        if !pages.iter().any(|p| &p.id == default) {
            problems.error(
                global_file,
                format!(
                    "pages.default_page '{default}' does not exist (expected pages/{default}.toml)"
                ),
            );
        }
    }
    if g.server.enabled && g.server.metrics && g.server.htpasswd_file.is_none() {
        problems.error(
            global_file,
            "server.metrics needs server.htpasswd_file (/metrics is behind Basic Auth)",
        );
    }
    let private = pages.iter().filter(|p| !p.public).count();
    if private > 0 && g.server.enabled && g.server.htpasswd_file.is_none() {
        problems.error(
            global_file,
            "private pages need server.htpasswd_file (or server.enabled = false if your proxy does the authentication)",
        );
    }
    if private > 0 && g.server.enabled && !g.server.listen.ip().is_loopback() {
        problems.warning(
            global_file,
            "private pages on a non-loopback address: without a TLS proxy, passwords travel unencrypted",
        );
    }
}

fn read_config_file(path: &Path) -> Result<(String, fs::Metadata), String> {
    let meta = fs::metadata(path).map_err(|e| format!("cannot read: {e}"))?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > MAX_CONFIG_BYTES {
        return Err(format!("larger than {MAX_CONFIG_BYTES} bytes"));
    }
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read: {e}"))?;
    Ok((text, meta))
}

/// `(id, path)` of all `*.toml` files in `dir`, sorted. Hidden files and
/// other extensions are ignored; a missing directory is empty.
fn list_toml_files(dir: &Path, problems: &mut Problems) -> Vec<(String, PathBuf)> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            problems.error(dir, format!("cannot read directory: {e}"));
            return Vec::new();
        }
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let Some(id) = name.strip_suffix(".toml") else {
            continue;
        };
        if !is_valid_id(id) {
            problems.error(&path, "file name must match [a-z0-9][a-z0-9_-]{0,63}.toml");
            continue;
        }
        files.push((id.to_string(), path));
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestDir;
    use std::ffi::OsString;

    fn env(key: &str) -> Option<OsString> {
        (key == "HOME").then(|| OsString::from("/home/u"))
    }

    const TCP: &str = "config_version = 1\ntype = \"tcp\"\nname = \"x\"\n[tcp]\nhost = \"localhost\"\nport = 22\n";

    #[test]
    fn ids() {
        for ok in ["a", "blog", "my-blog_2", "0day", &"a".repeat(64)] {
            assert!(is_valid_id(ok), "{ok}");
        }
        for bad in [
            "",
            "-a",
            "_a",
            "Blog",
            "my blog",
            "a.b",
            "ä",
            &"a".repeat(65),
        ] {
            assert!(!is_valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn full_directory() {
        let dir = TestDir::new("config");
        dir.write(
            "kinomi.toml",
            "config_version = 1\n[server]\nhostnames = [\"localhost\"]\n",
        );
        dir.write("monitors/ssh.toml", TCP);
        dir.write("monitors/web.toml", "config_version = 1\ntype = \"http\"\nname = \"Web\"\n[http]\nurl = \"https://example.org/\"\n");
        dir.write("monitors/.hidden.toml", "garbage");
        dir.write("monitors/notes.txt", "garbage");
        dir.write("monitors/web.toml~", "garbage");
        dir.write(
            "pages/home.toml",
            "config_version = 1\ntitle = \"Home\"\npublic = true\n[[groups]]\nname = \"All\"\nmonitors = [\"*\"]\n",
        );
        let (config, problems) = load(dir.path(), &env);
        let config = config.unwrap_or_else(|| panic!("{problems:?}"));
        let ids: Vec<_> = config.monitors.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["ssh", "web"]);
        assert_eq!(problems.all().count(), 0, "{problems:?}");
    }

    #[test]
    fn problems_from_several_files() {
        let dir = TestDir::new("config-bad");
        dir.write(
            "kinomi.toml",
            "config_version = 1\n[defaults]\nnotifications = [\"nope\"]\n",
        );
        dir.write("monitors/ok.toml", TCP);
        dir.write("monitors/bad.toml", "config_version = 1\ntype = \"tcp\"\n");
        dir.write("monitors/Bad Name.toml", TCP);
        dir.write("pages/home.toml", "anything");
        let (config, problems) = load(dir.path(), &env);
        assert!(config.is_none());
        let all = format!("{problems:?}");
        assert!(all.contains("defaults.notifications"), "{all}");
        assert!(all.contains("bad.toml"), "{all}");
        assert!(all.contains("file name must match"), "{all}");
        assert!(all.contains("pages/home.toml"), "{all}");
    }

    #[test]
    fn missing_global_file() {
        let dir = TestDir::new("config-missing");
        let (config, problems) = load(dir.path(), &env);
        assert!(config.is_none());
        assert!(format!("{problems:?}").contains("kinomi.toml"));
    }

    #[test]
    fn oversized_file() {
        let dir = TestDir::new("config-big");
        dir.write(
            "kinomi.toml",
            &format!("config_version = 1\n#{}\n", "x".repeat(70_000)),
        );
        let (_, problems) = load(dir.path(), &env);
        assert!(format!("{problems:?}").contains("larger than"));
    }
}
