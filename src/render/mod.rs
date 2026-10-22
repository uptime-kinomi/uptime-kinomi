//! Static status pages (SPEC.md §8): built from a snapshot of all monitor
//! histories, written to `output_dir` (only files whose content changed) and
//! handed to the built-in server.
//!
//! Output layout (the URL paths of the built-in server are the same):
//!
//! ```text
//! public/assets/kinomi.css     /assets/kinomi.css
//! public/<page>/index.html     /<page>/
//! private/assets/kinomi.css    /private/assets/kinomi.css
//! private/<page>/index.html    /private/<page>/
//! ```

pub mod html;

use std::collections::HashMap;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Instant;

use jiff::tz::TimeZone;

use crate::config::is_valid_id;
use crate::config::pages::Page;
use crate::history::{self, BeatRecord, History};
use crate::monitors::Monitor;
use crate::state::State;
use crate::storage::statefile::write_atomic;

const DEFAULT_CSS: &str = include_str!("../../assets/kinomi.css");

/// What a page shows about one monitor (computed by the event loop).
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorView {
    pub state: State,
    pub since: Option<i64>,
    pub last_check: Option<i64>,
    pub last_msg: String,
    pub cert_expires: Option<i64>,
    pub beats: Vec<BeatRecord>,
    pub uptime_24h: Option<f64>,
    pub uptime_30d: Option<f64>,
    pub avg_ms_24h: Option<u32>,
    pub skipped_24h: u32,
}

impl MonitorView {
    pub fn new(h: &History, now: i64) -> Self {
        let day = h.last_hours(now, 24);
        MonitorView {
            state: h.state,
            since: h.state_since,
            last_check: h.last_check,
            last_msg: h.last_msg.clone(),
            cert_expires: h.cert_expires,
            beats: h.recent.iter().copied().collect(),
            uptime_24h: history::uptime_percent(&day),
            uptime_30d: history::uptime_percent(&h.last_days(now, 30)),
            avg_ms_24h: day.avg_ms(),
            skipped_24h: day.skipped,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelfStatus {
    pub started: i64,
    pub queue_len: usize,
    pub workers: u32,
    pub skipped_24h: u32,
    pub last_render_ms: u64,
}

/// Input of one render. `monitors` is index-aligned with the configuration.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub now: i64,
    pub monitors: Vec<MonitorView>,
    pub status: SelfStatus,
}

/// A generated file.
#[derive(Debug, PartialEq)]
pub struct File {
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub private: bool,
}

/// All generated files by URL path (see the module documentation).
#[derive(Debug, Default)]
pub struct Site {
    pub files: HashMap<String, Arc<File>>,
}

/// The site currently served; replaced after every render.
pub type Published = Arc<RwLock<Arc<Site>>>;

/// Static inputs of the renderer.
pub struct Renderer {
    pub pages: Arc<Vec<Page>>,
    pub monitors: Arc<Vec<Monitor>>,
    pub tz: TimeZone,
    pub auto_refresh_s: u64,
    pub recent_checks: usize,
    pub css: String,
    pub output_dir: PathBuf,
}

impl Renderer {
    pub fn css(custom: Option<&Path>) -> Result<String, String> {
        let mut css = DEFAULT_CSS.to_string();
        if let Some(path) = custom {
            let extra = fs::read_to_string(path)
                .map_err(|e| format!("pages.custom_css {}: {e}", path.display()))?;
            css.push_str("\n/* custom.css */\n");
            css.push_str(&extra);
        }
        Ok(css)
    }

    pub fn build(&self, snap: &Snapshot) -> Site {
        let ctx = html::Context {
            pages: &self.pages,
            monitors: &self.monitors,
            tz: &self.tz,
            auto_refresh_s: self.auto_refresh_s,
            recent_checks: self.recent_checks,
        };
        let mut files = HashMap::new();
        let css = Arc::new(File {
            content_type: "text/css; charset=utf-8",
            body: self.css.clone().into_bytes(),
            private: false,
        });
        files.insert("/assets/kinomi.css".to_string(), css.clone());
        if self.pages.iter().any(|p| !p.public) {
            let private_css = File {
                content_type: css.content_type,
                body: css.body.clone(),
                private: true,
            };
            files.insert(
                "/private/assets/kinomi.css".to_string(),
                Arc::new(private_css),
            );
        }
        for page in self.pages.iter() {
            let body = html::page(&ctx, page, snap).into_bytes();
            let path = if page.public {
                format!("/{}/", page.id)
            } else {
                format!("/private/{}/", page.id)
            };
            files.insert(
                path,
                Arc::new(File {
                    content_type: "text/html; charset=utf-8",
                    body,
                    private: !page.public,
                }),
            );
        }
        Site { files }
    }
}

/// File system path of a URL path below `output_dir`.
fn disk_path(output_dir: &Path, url: &str) -> PathBuf {
    let (top, rest) = match url.strip_prefix("/private/") {
        Some(rest) => ("private", rest),
        None => ("public", url.trim_start_matches('/')),
    };
    let mut path = output_dir.join(top).join(rest);
    if url.ends_with('/') {
        path.push("index.html");
    }
    path
}

/// Write changed files and remove page directories that are no longer
/// configured. Directories are 0750 and files 0640, so a web server group
/// can serve them directly (SPEC.md §9.1, mode B).
pub fn write(output_dir: &Path, site: &Site, previous: &Site, first: bool) -> io::Result<()> {
    for (url, file) in &site.files {
        if previous
            .files
            .get(url)
            .is_some_and(|old| old.body == file.body)
        {
            continue;
        }
        let path = disk_path(output_dir, url);
        if let Some(parent) = path.parent() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o750)
                .create(parent)?;
        }
        write_atomic(&path, &file.body, 0o640)?;
    }
    if first {
        remove_stale(output_dir, site)?;
    }
    Ok(())
}

/// Remove `<page>/index.html` directories of pages that no longer exist.
/// Only directories that look exactly like ours are touched.
fn remove_stale(output_dir: &Path, site: &Site) -> io::Result<()> {
    for top in ["public", "private"] {
        let Ok(entries) = fs::read_dir(output_dir.join(top)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(id) = name.to_str() else { continue };
            let url = if top == "public" {
                format!("/{id}/")
            } else {
                format!("/private/{id}/")
            };
            if id == "assets" || !is_valid_id(id) || site.files.contains_key(&url) {
                continue;
            }
            let dir = entry.path();
            let only_index = fs::read_dir(&dir)
                .map(|e| e.flatten().map(|f| f.file_name()).collect::<Vec<_>>())
                .is_ok_and(|names| names.len() == 1 && names[0] == "index.html");
            if only_index {
                fs::remove_file(dir.join("index.html"))?;
                fs::remove_dir(&dir)?;
            }
        }
    }
    Ok(())
}

/// Render every snapshot received, write the files and publish the site.
pub fn spawn(
    renderer: Renderer,
    snapshots: Receiver<Snapshot>,
    published: Published,
    render_ms: Arc<AtomicU64>,
) {
    let spawned = thread::Builder::new()
        .name("renderer".into())
        .spawn(move || {
            let _ = DirBuilder::new()
                .recursive(true)
                .mode(0o750)
                .create(&renderer.output_dir);
            let _ = fs::set_permissions(&renderer.output_dir, fs::Permissions::from_mode(0o750));
            let mut previous = Arc::new(Site::default());
            let mut first = true;
            for snap in snapshots {
                let start = Instant::now();
                let site = Arc::new(renderer.build(&snap));
                if let Err(e) = write(&renderer.output_dir, &site, &previous, first) {
                    eprintln!(
                        "error: cannot write status pages to {}: {e}",
                        renderer.output_dir.display()
                    );
                }
                first = false;
                if let Ok(mut current) = published.write() {
                    *current = site.clone();
                }
                previous = site;
                render_ms.store(
                    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );
            }
        });
    if let Err(e) = spawned {
        eprintln!("error: cannot start the renderer: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::pages::Group;
    use crate::monitors::test_monitor;
    use crate::testutil::TestDir;

    fn renderer(output_dir: &Path) -> Renderer {
        let page = |id: &str, public| Page {
            id: id.into(),
            title: id.into(),
            public,
            description: None,
            groups: vec![Group {
                name: "All".into(),
                monitors: vec![0],
            }],
            show_incidents: true,
            show_badges: false,
        };
        Renderer {
            pages: Arc::new(vec![page("home", true), page("all", false)]),
            monitors: Arc::new(vec![test_monitor("web")]),
            tz: TimeZone::UTC,
            auto_refresh_s: 0,
            recent_checks: 10,
            css: Renderer::css(None).unwrap(),
            output_dir: output_dir.to_path_buf(),
        }
    }

    fn snapshot(now: i64) -> Snapshot {
        let h = History::new(10, 30);
        Snapshot {
            now,
            monitors: vec![MonitorView::new(&h, now)],
            status: SelfStatus {
                started: now,
                queue_len: 0,
                workers: 1,
                skipped_24h: 0,
                last_render_ms: 0,
            },
        }
    }

    #[test]
    fn site_layout() {
        let dir = TestDir::new("render");
        let site = renderer(dir.path()).build(&snapshot(1_792_678_991));
        let mut urls: Vec<_> = site.files.keys().cloned().collect();
        urls.sort();
        assert_eq!(
            urls,
            [
                "/assets/kinomi.css",
                "/home/",
                "/private/all/",
                "/private/assets/kinomi.css"
            ]
        );
        assert!(site.files["/private/all/"].private);
        assert!(!site.files["/home/"].private);
        assert_eq!(
            disk_path(dir.path(), "/home/"),
            dir.path().join("public/home/index.html")
        );
        assert_eq!(
            disk_path(dir.path(), "/private/assets/kinomi.css"),
            dir.path().join("private/assets/kinomi.css")
        );
    }

    #[test]
    fn writes_only_changes_and_removes_stale_pages() {
        let dir = TestDir::new("render-write");
        let r = renderer(dir.path());
        fs::create_dir_all(dir.path().join("public/old")).unwrap();
        fs::write(dir.path().join("public/old/index.html"), "x").unwrap();
        fs::create_dir_all(dir.path().join("public/keep")).unwrap();
        fs::write(dir.path().join("public/keep/notes.txt"), "not ours").unwrap();

        let first = r.build(&snapshot(1_792_678_991));
        write(dir.path(), &first, &Site::default(), true).unwrap();
        assert!(dir.path().join("public/home/index.html").is_file());
        assert!(dir.path().join("private/all/index.html").is_file());
        assert!(!dir.path().join("public/old").exists());
        assert!(dir.path().join("public/keep/notes.txt").exists());
        let mode = fs::metadata(dir.path().join("public/home/index.html"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o640);

        // Unchanged CSS is not rewritten.
        let css = dir.path().join("public/assets/kinomi.css");
        fs::write(&css, "marker").unwrap();
        let second = r.build(&snapshot(1_792_679_051));
        write(dir.path(), &second, &first, false).unwrap();
        assert_eq!(fs::read_to_string(&css).unwrap(), "marker");
    }

    #[test]
    fn custom_css_is_appended() {
        let dir = TestDir::new("render-css");
        let custom = dir.write("custom.css", "body { color: red; }");
        let css = Renderer::css(Some(&custom)).unwrap();
        assert!(css.starts_with("/* uptime-kinomi default style sheet."));
        assert!(css.ends_with("body { color: red; }"));
        assert!(Renderer::css(Some(&dir.path().join("missing.css"))).is_err());
    }
}
