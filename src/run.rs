//! `uptime-kinomi run`: the long-running service, and `rebuild-state`.

use std::collections::HashMap;
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::clock::{self, JumpDetector};
use crate::config::Config;
use crate::history::History;
use crate::monitors::Monitor;
use crate::render::{self, MonitorView, Renderer, SelfStatus, Snapshot};
use crate::scheduler::{Event, Scheduler};
use crate::server::{self, auth::Auth};
use crate::state::MonitorState;
use crate::storage::log::LogWriter;
use crate::storage::{replay, statefile};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Pages are re-rendered at least this often even if nothing changed, so
/// "last updated" and durations stay current.
const RENDER_AT_LEAST_EVERY: Duration = Duration::from_secs(600);

/// Everything the event loop owns.
struct Service {
    monitors: Arc<Vec<Monitor>>,
    states: Vec<MonitorState>,
    histories: HashMap<String, History>,
    log: LogWriter,
    data_dir: PathBuf,
}

pub fn run(mut config: Config) -> Result<(), String> {
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(signal, stop.clone())
            .map_err(|e| format!("cannot handle signals: {e}"))?;
    }
    if running_as_root() {
        eprintln!("warning: running as root is not needed; use a dedicated user (SPEC.md §10.6)");
    }
    wait_for_plausible_clock(&stop);
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }

    let g = &config.global;
    let flush_interval = g.storage.flush_interval.0;
    let state_interval = g.storage.state_interval.0;
    let (workers, max_queue) = (g.scheduler.workers as usize, g.scheduler.max_queue as usize);
    let log_dir = g.storage.log_dir.clone();
    let render_interval = g.pages.render_interval.0;
    let pages = Arc::new(std::mem::take(&mut config.pages));
    let tz = timezone(&config.global.pages.timezone);
    let css = Renderer::css(config.global.pages.custom_css.as_deref())?;
    let (auto_refresh_s, recent_checks) = (
        config.global.pages.auto_refresh.0.as_secs(),
        config.global.pages.recent_checks as usize,
    );
    let output_dir = config.global.pages.output_dir.clone();
    let server_options = server_options(&config, &pages)?;
    let mut service = Service::open(config)?;
    let enabled = service
        .monitors
        .iter()
        .filter(|m| m.enabled)
        .count()
        .to_string();
    service.log.record(
        SystemTime::now(),
        &[
            ("event", "START"),
            ("version", VERSION),
            ("monitors", &enabled),
        ],
    );
    service.flush();
    eprintln!(
        "uptime-kinomi {VERSION}: {enabled} monitors, log in {}",
        log_dir.display()
    );

    let published: render::Published = Arc::default();
    let render_ms = Arc::new(AtomicU64::new(0));
    let (snapshot_tx, snapshot_rx) = mpsc::channel();
    let renderer = Renderer {
        pages,
        monitors: service.monitors.clone(),
        tz,
        auto_refresh_s,
        recent_checks,
        css,
        output_dir: output_dir.clone(),
    };
    render::spawn(renderer, snapshot_rx, published.clone(), render_ms.clone());
    if let Some(options) = server_options {
        let listen = options.listen;
        server::start(options, published)?;
        eprintln!(
            "serving status pages on http://{listen}/ (pages also in {})",
            output_dir.display()
        );
    }

    let (tx, rx) = mpsc::channel();
    let scheduler = Scheduler::start(service.monitors.clone(), workers, max_queue, tx);
    let started = clock::unix_seconds(SystemTime::now());
    let mut next_render = Instant::now();
    let mut last_render: Option<(Vec<MonitorView>, Instant)> = None;
    let mut jumps = JumpDetector::new(SystemTime::now(), Instant::now());
    let mut next_flush = Instant::now() + flush_interval;
    let mut next_state_save = Instant::now() + state_interval;

    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(event) => service.handle(event),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let now = Instant::now();
        if now >= next_flush {
            if let Some(jump) = jumps.check(SystemTime::now(), now) {
                service.log.record(
                    SystemTime::now(),
                    &[("event", "CLOCK_JUMP"), ("delta_s", &jump.to_string())],
                );
            }
            service.flush();
            next_flush = now + flush_interval;
        }
        if now >= next_state_save {
            service.save_states();
            next_state_save = now + state_interval;
        }
        // Render when the checks are idle, but never more than one interval late.
        if now >= next_render
            && (scheduler.queue_len() == 0 || now >= next_render + render_interval)
        {
            let mut snap = service.snapshot();
            let unchanged = last_render.as_ref().is_some_and(|(views, at)| {
                *views == snap.monitors && now.duration_since(*at) < RENDER_AT_LEAST_EVERY
            });
            if !unchanged {
                snap.status = SelfStatus {
                    started,
                    queue_len: scheduler.queue_len(),
                    workers: workers as u32,
                    skipped_24h: snap.monitors.iter().map(|v| v.skipped_24h).sum(),
                    last_render_ms: render_ms.load(Ordering::Relaxed),
                };
                last_render = Some((snap.monitors.clone(), now));
                let _ = snapshot_tx.send(snap);
            }
            next_render = now + render_interval;
        }
    }

    scheduler.stop();
    while let Ok(event) = rx.try_recv() {
        service.handle(event);
    }
    service.log.record(SystemTime::now(), &[("event", "STOP")]);
    service.save_states();
    eprintln!("uptime-kinomi stopped");
    Ok(())
}

/// `uptime-kinomi rebuild-state`: regenerate all state files from the log.
pub fn rebuild_state(config: &Config) -> Result<(), String> {
    let g = &config.global;
    let mut histories = new_histories(config);
    let lines = replay::replay(&g.storage.log_dir, &mut histories).map_err(|e| {
        format!(
            "cannot read the log in {}: {e}",
            g.storage.log_dir.display()
        )
    })?;
    create_private_dir(&g.storage.data_dir)?;
    for (id, history) in &histories {
        statefile::save(&g.storage.data_dir, id, history)
            .map_err(|e| format!("cannot write the state of {id}: {e}"))?;
    }
    println!(
        "rebuilt {} state files from {lines} log lines in {}",
        histories.len(),
        g.storage.log_dir.display()
    );
    Ok(())
}

/// Options for the built-in server, or `None` if it is disabled.
fn server_options(
    config: &Config,
    pages: &[crate::config::pages::Page],
) -> Result<Option<server::Options>, String> {
    let s = &config.global.server;
    if !s.enabled {
        return Ok(None);
    }
    let auth = s.htpasswd_file.as_deref().map(Auth::load).transpose()?;
    let default_page = config
        .global
        .pages
        .default_page
        .as_deref()
        .and_then(|id| pages.iter().find(|p| p.id == id))
        .or_else(|| pages.iter().find(|p| p.public))
        .map(|p| {
            if p.public {
                format!("/{}/", p.id)
            } else {
                format!("/private/{}/", p.id)
            }
        });
    Ok(Some(server::Options {
        listen: s.listen,
        hostnames: s.hostnames.iter().map(|h| h.to_ascii_lowercase()).collect(),
        default_page,
        auth,
    }))
}

fn timezone(name: &str) -> jiff::tz::TimeZone {
    if name == "UTC" {
        return jiff::tz::TimeZone::UTC;
    }
    jiff::tz::TimeZone::get(name).unwrap_or_else(|_| {
        eprintln!("warning: time zone '{name}' not found; using UTC");
        jiff::tz::TimeZone::UTC
    })
}

fn new_histories(config: &Config) -> HashMap<String, History> {
    let recent = config.global.pages.recent_checks as usize;
    let days = i64::try_from(config.global.storage.history_retention.0.as_secs() / 86400)
        .unwrap_or(i64::MAX);
    config
        .monitors
        .iter()
        .map(|m| (m.id.clone(), History::new(recent, days)))
        .collect()
}

fn create_private_dir(dir: &Path) -> Result<(), String> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))
}

impl Service {
    /// Open the log, load state files and replay what they do not cover.
    fn open(config: Config) -> Result<Self, String> {
        let storage = &config.global.storage;
        let retention_days =
            i64::try_from(storage.log_retention.0.as_secs() / 86400).unwrap_or(i64::MAX);
        let log = LogWriter::open(&storage.log_dir, retention_days).map_err(|e| {
            format!(
                "cannot open log directory {}: {e}",
                storage.log_dir.display()
            )
        })?;
        create_private_dir(&storage.data_dir)?;

        let mut histories = new_histories(&config);
        for (id, history) in histories.iter_mut() {
            if let Some(warning) = statefile::load(&storage.data_dir, id, history) {
                eprintln!("warning: {warning}");
            }
        }
        if let Err(e) = replay::replay(&storage.log_dir, &mut histories) {
            eprintln!("warning: cannot replay the log: {e}");
        }
        let data_dir = storage.data_dir.clone();
        let monitors = Arc::new(config.monitors);
        let states = monitors
            .iter()
            .map(|m| MonitorState::restore(m.retries, histories[&m.id].state))
            .collect();
        Ok(Service {
            monitors,
            states,
            histories,
            log,
            data_dir,
        })
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Done {
                monitor,
                result,
                time,
            } => {
                let m = &self.monitors[monitor];
                let (logged, previous) = self.states[monitor].apply(result.status);
                let latency = result
                    .latency
                    .map(|l| u32::try_from(l.as_millis()).unwrap_or(u32::MAX));
                let latency_text = latency.map(|l| l.to_string());
                let mut fields = vec![("monitor", m.id.as_str()), ("result", logged.name())];
                if let Some(latency) = &latency_text {
                    fields.push(("latency_ms", latency));
                }
                let cert_expires = result
                    .cert_expires
                    .map(|t| clock::format_date(clock::from_unix(t)));
                if let Some(date) = &cert_expires {
                    fields.push(("cert_expires", date));
                }
                fields.push(("msg", &result.msg));
                self.log.record(time, &fields);
                if let Some(h) = self.histories.get_mut(&m.id) {
                    let t = clock::unix_seconds(time);
                    h.record_check(t, logged, latency, &result.msg, result.cert_expires);
                }
                if let Some(previous) = previous {
                    // Also visible in the journal.
                    eprintln!(
                        "{} ({}): {} -> {} ({})",
                        m.name,
                        m.id,
                        previous.name(),
                        self.states[monitor].state.name(),
                        result.msg
                    );
                }
            }
            Event::Skipped {
                monitor,
                count,
                paused,
                time,
            } => {
                let id = self.monitors[monitor].id.as_str();
                let count_text = count.to_string();
                let msg = if paused { "paused" } else { "queue full" };
                let fields = [
                    ("monitor", id),
                    ("result", "SKIPPED"),
                    ("count", count_text.as_str()),
                    ("msg", msg),
                ];
                self.log.record(time, &fields);
                if let Some(h) = self.histories.get_mut(id) {
                    h.record_skip(clock::unix_seconds(time), count);
                }
            }
            Event::Paused { seconds, time } => {
                let duration = seconds.to_string();
                self.log
                    .record(time, &[("event", "PAUSED"), ("duration_s", &duration)]);
                eprintln!(
                    "warning: uptime-kinomi did not run for {seconds} s (system paused or frozen)"
                );
            }
        }
    }

    /// Page data for the renderer (the self status is filled in by the caller).
    fn snapshot(&self) -> Snapshot {
        let now = clock::unix_seconds(SystemTime::now());
        let empty = History::new(1, 1);
        let monitors = self
            .monitors
            .iter()
            .map(|m| MonitorView::new(self.histories.get(&m.id).unwrap_or(&empty), now))
            .collect();
        Snapshot {
            now,
            monitors,
            status: SelfStatus {
                started: now,
                queue_len: 0,
                workers: 0,
                skipped_24h: 0,
                last_render_ms: 0,
            },
        }
    }

    fn flush(&mut self) {
        if let Err(e) = self.log.flush(SystemTime::now()) {
            eprintln!("error: cannot write the check log: {e}");
        }
    }

    /// Flush the log first: a state file must never cover events the log
    /// does not have.
    fn save_states(&mut self) {
        self.flush();
        for (id, history) in &self.histories {
            if let Err(e) = statefile::save(&self.data_dir, id, history) {
                eprintln!("error: cannot write the state of {id}: {e}");
            }
        }
    }
}

/// SBCs without a real-time clock start with a wrong time until NTP has
/// synced. Timestamps from that time would be wrong, so wait (SPEC.md §10.1).
fn wait_for_plausible_clock(stop: &AtomicBool) {
    let mut warned = false;
    while !clock::is_plausible(SystemTime::now()) && !stop.load(Ordering::Relaxed) {
        if !warned {
            eprintln!(
                "warning: the system clock ({}) is not set yet; waiting for NTP before checking",
                clock::format_rfc3339(SystemTime::now())
            );
            warned = true;
        }
        thread::sleep(Duration::from_secs(5));
    }
}

/// Effective UID 0, read from /proc (no libc call needed).
fn running_as_root() -> bool {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .and_then(|ids| ids.split_whitespace().nth(1).map(|euid| euid == "0"))
        })
        .unwrap_or(false)
}
