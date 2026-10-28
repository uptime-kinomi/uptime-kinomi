//! `uptime-kinomi run`: the long-running service, and `rebuild-state`.

use crate::health::Health;
use std::collections::HashMap;
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::clock::{self, JumpDetector};
use crate::config::Config;
use crate::config::incidents::Incident;
use crate::config::maintenance::Maintenance;
use crate::history::History;
use crate::monitors::{CheckResult, Kind, Monitor};
use crate::notifiers::{self, Job, Message, Notifier};
use crate::render::{self, MonitorView, Renderer, SelfStatus, Snapshot};
use crate::scheduler::{Event, Scheduler};
use crate::server::{self, auth::Auth, push::PushRegistry};
use crate::state::{MonitorState, State};
use crate::storage::log::LogWriter;
use crate::storage::{replay, statefile};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Pages are re-rendered at least this often even if nothing changed, so
/// "last updated" and durations stay current.
const RENDER_AT_LEAST_EVERY: Duration = Duration::from_secs(600);

/// A WARN notification is sent at most this often per monitor.
const WARN_EVERY: i64 = 86400;

/// Everything the event loop owns.
struct Service {
    monitors: Arc<Vec<Monitor>>,
    states: Vec<MonitorState>,
    histories: HashMap<String, History>,
    log: LogWriter,
    data_dir: PathBuf,
    tz: jiff::tz::TimeZone,
    notifiers: Arc<Vec<Notifier>>,
    notify_tx: Option<Sender<Job>>,
    notify: Vec<NotifyState>,
    /// Push monitors: when the monitor fails if no push arrives.
    push_deadline: Vec<Option<Instant>>,
    /// Push monitors whose deadline still comes from the state file (until
    /// the first push or missed deadline): recomputed after a clock jump.
    push_restored: Vec<Option<RestoredPush>>,
    started: Instant,
    maintenance: Arc<Vec<Maintenance>>,
    /// The server's Basic Auth, for `event=AUTH_FAIL` (SPEC.md §9.3).
    auth: Option<Arc<Auth>>,
    /// Last successful log flush (Unix seconds), for the self status.
    last_flush: Option<i64>,
}

/// What the first deadline of a push monitor after a start is based on.
#[derive(Debug, Clone, Copy)]
struct RestoredPush {
    /// Last push or check before the start (Unix seconds).
    last: Option<i64>,
    /// That was a missed push (`no push received`).
    missed: bool,
}

#[derive(Debug, Default)]
struct NotifyState {
    down_since: Option<i64>,
    last_down_notice: Option<i64>,
    last_warn_notice: Option<i64>,
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
    let data_dir = g.storage.data_dir.clone();
    let config_dir = config.dir.clone();
    let render_interval = g.pages.render_interval.0;
    let pages = Arc::new(std::mem::take(&mut config.pages));
    let tz = timezone(&config.global.pages.timezone);
    let css = Renderer::css(config.global.pages.custom_css.as_deref())?;
    let (auto_refresh_s, recent_checks) = (
        config.global.pages.auto_refresh.0.as_secs(),
        config.global.pages.recent_checks as usize,
    );
    let output_dir = config.global.pages.output_dir.clone();
    let base_url = config.global.server.base_url.clone();
    let metrics = config.global.server.enabled && config.global.server.metrics;
    let heartbeat = config
        .global
        .heartbeat
        .as_ref()
        .map(|h| (h.url.clone(), h.interval.0));
    let notifier_list = Arc::new(std::mem::take(&mut config.notifiers));
    let maintenance = Arc::new(std::mem::take(&mut config.maintenance));
    let incidents: Arc<Vec<Incident>> = Arc::new(std::mem::take(&mut config.incidents));
    let (tx, rx) = mpsc::channel();
    let health = Arc::new(Health::default());
    let mut server_options = server_options(&config, &pages)?;
    if let Some(options) = server_options.as_mut() {
        options.health = Some(health.clone());
        if config
            .monitors
            .iter()
            .any(|m| m.enabled && m.kind == Kind::Push)
        {
            options.push = Some(PushRegistry::new(&config.monitors, tx.clone()));
        }
    }
    let mut service = Service::open(
        config,
        tz.clone(),
        notifier_list.clone(),
        maintenance.clone(),
    )?;
    service.auth = server_options.as_ref().and_then(|o| o.auth.clone());
    if !notifier_list.is_empty() {
        let (job_tx, job_rx) = mpsc::channel();
        notifiers::spawn(notifier_list, job_rx, tx.clone());
        service.notify_tx = Some(job_tx);
    }
    if let Some((url, interval)) = heartbeat {
        crate::heartbeat::spawn(url, interval, tx.clone());
    }
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
    eprintln!("uptime-kinomi {VERSION}: {enabled} monitors");
    eprintln!("  config: {}", config_dir.display());
    eprintln!("  data:   {}", data_dir.display());
    eprintln!("  log:    {}", log_dir.display());
    eprintln!("  pages:  {}", output_dir.display());

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
        incidents,
        maintenance,
        base_url,
        metrics,
    };
    render::spawn(renderer, snapshot_rx, published.clone(), render_ms.clone());
    if let Some(options) = server_options {
        let listen = options.listen;
        server::start(options, published)?;
        eprintln!("  server: http://{listen}/");
    }

    let scheduler = Scheduler::start(
        service.monitors.clone(),
        workers,
        max_queue,
        tx,
        health.clone(),
    );
    let started = clock::unix_seconds(SystemTime::now());
    let mut next_render = Instant::now();
    let mut last_render: Option<(Vec<MonitorView>, Instant)> = None;
    let mut jumps = JumpDetector::new(SystemTime::now(), Instant::now());
    let mut next_flush = Instant::now() + flush_interval;
    let mut next_state_save = Instant::now() + state_interval;

    while !stop.load(Ordering::Relaxed) {
        health.event_loop_alive();
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(event) => service.handle(event),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let now = Instant::now();
        service.check_push_deadlines(now);
        service.send_due_reminders(clock::unix_seconds(SystemTime::now()));
        if now >= next_flush {
            if let Some(jump) = jumps.check(SystemTime::now(), now) {
                service.log.record(
                    SystemTime::now(),
                    &[("event", "CLOCK_JUMP"), ("delta_s", &jump.to_string())],
                );
                service.correct_push_deadlines();
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
                    avg_check_ms_24h: service.avg_check_ms_24h(snap.now),
                    last_flush: service.last_flush,
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
    service.record_auth_failures();
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
    say!(
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
    let auth = s
        .htpasswd_file
        .as_deref()
        .map(Auth::load)
        .transpose()?
        .map(Arc::new);
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
        push: None,
        health: None,
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
    fn open(
        config: Config,
        tz: jiff::tz::TimeZone,
        notifiers: Arc<Vec<Notifier>>,
        maintenance: Arc<Vec<Maintenance>>,
    ) -> Result<Self, String> {
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
        // Pushes to a disabled monitor are not recorded: after it is enabled
        // again, its last check says nothing about the client.
        let push_restored: Vec<Option<RestoredPush>> = config
            .monitors
            .iter()
            .map(|m| {
                let h = histories.get_mut(&m.id)?;
                let was_paused = std::mem::replace(&mut h.paused, !m.enabled);
                (m.enabled && m.kind == Kind::Push).then(|| RestoredPush {
                    last: if was_paused { None } else { h.last_check },
                    missed: h.last_msg == NO_PUSH,
                })
            })
            .collect();
        let data_dir = storage.data_dir.clone();
        let monitors = Arc::new(config.monitors);
        let states = monitors
            .iter()
            .map(|m| {
                let h = &histories[&m.id];
                MonitorState::restore(m.retries, h.state, h.trailing_failures())
            })
            .collect();
        let now = clock::unix_seconds(SystemTime::now());
        // A monitor that was already DOWN before the restart gets no new
        // DOWN notification, and its reminders continue from now.
        let notify = monitors
            .iter()
            .map(|m| {
                let h = &histories[&m.id];
                if h.state == State::Down {
                    NotifyState {
                        down_since: h.state_since,
                        last_down_notice: Some(now),
                        last_warn_notice: None,
                    }
                } else {
                    NotifyState::default()
                }
            })
            .collect();
        let started = Instant::now();
        let push_deadline = monitors
            .iter()
            .zip(&push_restored)
            .map(|(m, r)| r.map(|r| started + first_push_wait(now, r, m.interval, m.grace)))
            .collect();
        Ok(Service {
            monitors,
            states,
            histories,
            log,
            data_dir,
            tz,
            notifiers,
            notify_tx: None,
            notify,
            push_deadline,
            push_restored,
            started,
            maintenance,
            auth: None,
            last_flush: None,
        })
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Done {
                monitor,
                result,
                time,
            } => self.process(monitor, &result, time),
            Event::Push {
                monitor,
                down,
                time,
            } => {
                let m = &self.monitors[monitor];
                self.push_deadline[monitor] = Some(Instant::now() + m.interval + m.grace);
                self.push_restored[monitor] = None;
                if down {
                    self.process(monitor, &CheckResult::fail(None, "push: status=down"), time);
                } else {
                    // A push is a report from the client, not a check: it
                    // counts also inside a maintenance window (SPEC.md §14.2).
                    let result = CheckResult {
                        status: crate::monitors::Status::Up,
                        latency: None,
                        msg: "push".into(),
                        cert_expires: None,
                    };
                    self.apply(monitor, &result, time);
                }
            }
            Event::Notified {
                monitor_id,
                notifier,
                kind,
                error,
                time,
            } => {
                let ok = if error.is_some() { "false" } else { "true" };
                let mut fields = vec![
                    ("event", "NOTIFY"),
                    ("monitor", monitor_id.as_str()),
                    ("notifier", notifier.as_str()),
                    ("kind", kind.name()),
                    ("ok", ok),
                ];
                if let Some(e) = &error {
                    fields.push(("msg", e.as_str()));
                    eprintln!("error: notification {notifier} for {monitor_id} failed: {e}");
                }
                self.log.record(time, &fields);
            }
            Event::Heartbeat { error, time } => {
                let mut fields = vec![
                    ("event", "HEARTBEAT"),
                    ("ok", if error.is_some() { "false" } else { "true" }),
                ];
                if let Some(e) = &error {
                    fields.push(("msg", e.as_str()));
                    eprintln!("warning: heartbeat failed: {e}");
                }
                self.log.record(time, &fields);
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

    /// Apply a check (or push) result: state, log, history, notifications.
    fn process(&mut self, monitor: usize, result: &CheckResult, time: SystemTime) {
        let t = clock::unix_seconds(time);
        if self.in_maintenance(monitor, t) {
            self.process_maintenance(monitor, result, time);
        } else {
            self.apply(monitor, result, time);
        }
    }

    /// Apply a result outside of maintenance: state, log, history,
    /// notifications.
    fn apply(&mut self, monitor: usize, result: &CheckResult, time: SystemTime) {
        let m = &self.monitors[monitor];
        let before = self.states[monitor].state;
        let (logged, previous) = self.states[monitor].apply(result.status);
        let after = self.states[monitor].state;
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
        let t = clock::unix_seconds(time);
        if let Some(h) = self.histories.get_mut(&m.id) {
            h.record_check(t, logged, latency, &result.msg, result.cert_expires);
        }
        if let Some(previous) = previous {
            // Also visible in the journal.
            eprintln!(
                "{} ({}): {} -> {} ({})",
                m.name,
                m.id,
                previous.name(),
                after.name(),
                result.msg
            );
        }
        self.notify(monitor, before, after, &result.msg, t);
    }

    fn in_maintenance(&self, monitor: usize, t: i64) -> bool {
        self.maintenance_start(monitor, t).is_some()
    }

    /// Start of the earliest maintenance window `monitor` is in at `t`.
    fn maintenance_start(&self, monitor: usize, t: i64) -> Option<i64> {
        self.maintenance
            .iter()
            .filter(|w| w.monitors.contains(&monitor))
            .filter_map(|w| w.current(&self.tz, t))
            .min()
    }

    /// Inside a maintenance window: log and count the result as MAINT, but
    /// leave the state machine and notifications alone (SPEC.md §14.2).
    fn process_maintenance(&mut self, monitor: usize, result: &CheckResult, time: SystemTime) {
        let m = &self.monitors[monitor];
        let latency = result
            .latency
            .map(|l| u32::try_from(l.as_millis()).unwrap_or(u32::MAX));
        let latency_text = latency.map(|l| l.to_string());
        let mut fields = vec![("monitor", m.id.as_str()), ("result", "MAINT")];
        if let Some(latency) = &latency_text {
            fields.push(("latency_ms", latency));
        }
        // The log is the source of truth: also for what a MAINT check found.
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
            h.record_check(
                t,
                crate::state::LogResult::Maint,
                latency,
                &result.msg,
                result.cert_expires,
            );
        }
    }

    /// Decide whether a state change is worth a notification (SPEC.md §5.1)
    /// and queue it for the monitor's notifiers.
    fn notify(&mut self, monitor: usize, before: State, after: State, detail: &str, t: i64) {
        let m = &self.monitors[monitor];
        let ns = &mut self.notify[monitor];
        let resend = i64::try_from(m.resend_interval.as_secs()).unwrap_or(i64::MAX);
        let events = notify_events(ns, before, after, t, resend);
        let Some(jobs) = &self.notify_tx else {
            return;
        };
        for (kind, down_for) in events {
            let message = Arc::new(Message {
                kind,
                monitor_id: m.id.clone(),
                monitor_name: m.name.clone(),
                detail: detail.to_string(),
                time: t,
                local_time: render::html::format_time(&self.tz, t, "%Y-%m-%d %H:%M %Z"),
                down_for,
            });
            for id in &m.notifications {
                let Some(index) = self.notifiers.iter().position(|n| &n.id == id) else {
                    continue;
                };
                if self.notifiers[index].events.contains(&kind) {
                    let _ = jobs.send(Job {
                        notifier: index,
                        message: message.clone(),
                    });
                }
            }
        }
    }

    /// Fail push monitors whose push is overdue; again every interval.
    fn check_push_deadlines(&mut self, now: Instant) {
        for i in 0..self.monitors.len() {
            if self.push_deadline[i].is_some_and(|d| now >= d) {
                self.push_deadline[i] = Some(now + self.monitors[i].interval);
                self.push_restored[i] = None;
                let result = CheckResult::fail(None, NO_PUSH);
                self.process(i, &result, SystemTime::now());
            }
        }
    }

    /// The first push deadlines were computed with the wall clock at the
    /// start. If it was wrong then (e.g. fake-hwclock before NTP), compute
    /// them again with the corrected clock.
    fn correct_push_deadlines(&mut self) {
        let elapsed = i64::try_from(self.started.elapsed().as_secs()).unwrap_or(i64::MAX);
        let start = clock::unix_seconds(SystemTime::now()).saturating_sub(elapsed);
        for (i, m) in self.monitors.iter().enumerate() {
            if let Some(r) = self.push_restored[i] {
                self.push_deadline[i] =
                    Some(self.started + first_push_wait(start, r, m.interval, m.grace));
            }
        }
    }

    /// Reminders are due by time, not only when a result comes in: a push
    /// monitor has one result per `interval`, which may be longer than
    /// `resend_interval`.
    fn send_due_reminders(&mut self, t: i64) {
        for i in 0..self.monitors.len() {
            let m = &self.monitors[i];
            if !m.enabled || self.states[i].state != State::Down || self.in_maintenance(i, t) {
                continue;
            }
            let detail = self
                .histories
                .get(&m.id)
                .map(|h| h.last_msg.clone())
                .unwrap_or_default();
            self.notify(i, State::Down, State::Down, &detail, t);
        }
    }

    /// Page data for the renderer (the self status is filled in by the caller).
    fn snapshot(&self) -> Snapshot {
        let now = clock::unix_seconds(SystemTime::now());
        let empty = History::new(1, 1);
        let monitors = self
            .monitors
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let h = self.histories.get(&m.id).unwrap_or(&empty);
                let mut view = MonitorView::new(h, now, self.maintenance_start(i, now));
                if !m.enabled {
                    // Shown as PAUSED: since when is not known.
                    view.since = None;
                }
                view
            })
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
                avg_check_ms_24h: None,
                last_flush: None,
            },
        }
    }

    fn flush(&mut self) {
        self.record_auth_failures();
        match self.log.flush(SystemTime::now()) {
            Ok(()) => self.last_flush = Some(clock::unix_seconds(SystemTime::now())),
            Err(e) => eprintln!("error: cannot write the check log: {e}"),
        }
    }

    /// `event=AUTH_FAIL` with the failed logins since the last call.
    fn record_auth_failures(&mut self) {
        if let Some((failed, throttled)) = self
            .auth
            .as_ref()
            .map(|a| a.take_failures())
            .filter(|&(f, t)| f > 0 || t > 0)
        {
            self.log.record(
                SystemTime::now(),
                &[
                    ("event", "AUTH_FAIL"),
                    ("count", &failed.to_string()),
                    ("throttled", &throttled.to_string()),
                ],
            );
        }
    }

    /// Average response time of all successful checks of the last 24 h.
    fn avg_check_ms_24h(&self, now: i64) -> Option<u32> {
        let mut total = crate::history::Bucket::default();
        for h in self.histories.values() {
            total.add(&h.last_hours(now, 24));
        }
        total.avg_ms()
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

/// The result message of a missed push.
const NO_PUSH: &str = "no push received";

/// How long after a start (at `now`) a push monitor waits before it fails
/// (SPEC.md §4.6): as if kinomi had kept running, i.e. `interval + grace`
/// after the last push, `interval` after the last missed push, so restarts
/// do not postpone it; but at least `grace`, for a push that was sent while
/// kinomi was down. `interval + grace` from the start if the monitor has no
/// result yet or was disabled.
fn first_push_wait(now: i64, r: RestoredPush, interval: Duration, grace: Duration) -> Duration {
    let Some(last) = r.last else {
        return interval + grace;
    };
    let due = if r.missed { interval } else { interval + grace };
    let since = Duration::from_secs(u64::try_from(now.saturating_sub(last)).unwrap_or(0));
    due.saturating_sub(since).max(grace)
}

/// The notifications a state change is worth (SPEC.md §5.1), updating
/// `ns`. `resend`: reminder interval in seconds, 0 = none.
fn notify_events(
    ns: &mut NotifyState,
    before: State,
    after: State,
    t: i64,
    resend: i64,
) -> Vec<(notifiers::Kind, Option<i64>)> {
    let event = if after == State::Down && before != State::Down {
        ns.down_since = Some(t);
        ns.last_down_notice = Some(t);
        Some((notifiers::Kind::Down, None))
    } else if before == State::Down && after != State::Down {
        let down_for = ns.down_since.map(|s| t - s);
        ns.down_since = None;
        Some((notifiers::Kind::Up, down_for))
    } else if after == State::Down
        && resend > 0
        && ns.last_down_notice.is_some_and(|l| t - l >= resend)
    {
        ns.last_down_notice = Some(t);
        Some((notifiers::Kind::Reminder, ns.down_since.map(|s| t - s)))
    } else if after == State::Warn
        && before != State::Warn
        && ns.last_warn_notice.is_none_or(|w| t - w >= WARN_EVERY)
    {
        ns.last_warn_notice = Some(t);
        Some((notifiers::Kind::Warn, None))
    } else {
        None
    };
    let mut events: Vec<(notifiers::Kind, Option<i64>)> = event.into_iter().collect();
    // Back from DOWN straight into WARN (e.g. up again, but the
    // certificate expires soon): the warning is news too.
    if before == State::Down
        && after == State::Warn
        && ns.last_warn_notice.is_none_or(|w| t - w >= WARN_EVERY)
    {
        ns.last_warn_notice = Some(t);
        events.push((notifiers::Kind::Warn, None));
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifiers::Kind;
    use crate::testutil::TestDir;

    #[test]
    fn first_push_wait_counts_from_the_last_push() {
        let h = |n: u64| Duration::from_secs(n * 3600);
        let (day, grace) = (h(24), h(2));
        let now = 1_000_000_000;
        let wait = |last: Option<i64>, missed| {
            first_push_wait(now, RestoredPush { last, missed }, day, grace)
        };
        // Never pushed, or disabled before: interval + grace from the start.
        assert_eq!(wait(None, false), day + grace);
        // Pushed 10 h before the restart: 16 h left, restarts do not reset it.
        assert_eq!(wait(Some(now - 36_000), false), h(16));
        // Overdue (last push 2 days ago): fail after grace.
        assert_eq!(wait(Some(now - 2 * 86_400), false), grace);
        // Less than grace left: still grace.
        assert_eq!(wait(Some(now - 25 * 3600), false), grace);
        // Clock went back: never longer than interval + grace.
        assert_eq!(wait(Some(now + 3600), false), day + grace);
        // Missed 10 h before the restart: the next failure 14 h later, as
        // without the restart (interval, no grace).
        assert_eq!(wait(Some(now - 36_000), true), h(14));
    }

    fn kinds(events: &[(Kind, Option<i64>)]) -> Vec<Kind> {
        events.iter().map(|(k, _)| *k).collect()
    }

    #[test]
    fn notification_events() {
        let mut ns = NotifyState::default();
        let t = 1_000_000;
        assert_eq!(
            kinds(&notify_events(&mut ns, State::Up, State::Down, t, 0)),
            [Kind::Down]
        );
        // No reminders without resend_interval, one per interval with it.
        assert!(notify_events(&mut ns, State::Down, State::Down, t + 600, 0).is_empty());
        assert_eq!(
            kinds(&notify_events(
                &mut ns,
                State::Down,
                State::Down,
                t + 600,
                300
            )),
            [Kind::Reminder]
        );
        // Back from DOWN straight into WARN: up, and the warning.
        let events = notify_events(&mut ns, State::Down, State::Warn, t + 900, 0);
        assert_eq!(kinds(&events), [Kind::Up, Kind::Warn]);
        assert_eq!(events[0].1, Some(900), "down for 15 min");
        // The warning at most once per 24 h.
        notify_events(&mut ns, State::Warn, State::Down, t + 1000, 0);
        assert_eq!(
            kinds(&notify_events(
                &mut ns,
                State::Down,
                State::Warn,
                t + 1100,
                0
            )),
            [Kind::Up]
        );
    }

    const TOKEN: &str = "abcdefghijklmnopqrstuvwxyz012345";

    /// A service with one push monitor `b` (and `extra` in its file).
    fn push_service(dir: &TestDir, extra: &str) -> Service {
        dir.write(
            "kinomi.toml",
            &format!(
                "config_version = 1\n[storage]\ndata_dir = \"{}/data\"\n\
                 [pages]\ntimezone = \"UTC\"\n",
                dir.path().display()
            ),
        );
        dir.write(
            "monitors/b.toml",
            &format!(
                "config_version = 1\nname = \"Backup\"\ntype = \"push\"\ninterval = \"1d\"\ngrace = \"2h\"\n\
                 retries = 0\n{extra}\n[push]\ntoken = \"{TOKEN}\"\n"
            ),
        );
        let env = |_: &str| None;
        let (config, problems) = crate::config::load(dir.path(), &env, None);
        let mut config = config.unwrap_or_else(|| panic!("{problems:?}"));
        let maintenance = Arc::new(std::mem::take(&mut config.maintenance));
        Service::open(config, timezone("UTC"), Arc::new(Vec::new()), maintenance).unwrap()
    }

    #[test]
    fn re_enabled_push_monitor_waits_a_full_interval() {
        let dir = TestDir::new("run-push-paused");
        let week_ago = clock::unix_seconds(SystemTime::now()) - 7 * 86_400;
        let mut service = push_service(&dir, "enabled = false");
        assert_eq!(service.push_deadline[0], None);
        let h = service.histories.get_mut("b").unwrap();
        assert!(h.paused);
        (h.last_check, h.last_msg) = (Some(week_ago), "push".into());
        service.save_states();
        drop(service);
        let service = push_service(&dir, "");
        let r = service.push_restored[0].unwrap();
        assert_eq!(r.last, None, "the old push says nothing after a pause");
        assert!(!service.histories["b"].paused);
        let wait = service.push_deadline[0].unwrap() - service.started;
        assert_eq!(wait, Duration::from_secs(26 * 3600));
        // Without the pause, the same old push is overdue: grace.
        let mut service = service;
        service.save_states();
        drop(service);
        let service = push_service(&dir, "");
        assert_eq!(service.push_restored[0].unwrap().last, Some(week_ago));
        let wait = service.push_deadline[0].unwrap() - service.started;
        assert_eq!(wait, Duration::from_secs(2 * 3600));
    }

    #[test]
    fn push_inside_a_maintenance_window_counts() {
        let dir = TestDir::new("run-push-maint");
        let mut service = push_service(&dir, "");
        let now = SystemTime::now();
        service.apply(0, &CheckResult::fail(None, NO_PUSH), now);
        assert_eq!(service.states[0].state, State::Down);
        service.save_states();
        drop(service);
        let start = clock::format_rfc3339(now - Duration::from_secs(3600));
        dir.write(
            "maintenance/w.toml",
            &format!(
                "config_version = 1\ntitle = \"w\"\nmonitors = [\"b\"]\n\
                 schedule = \"once\"\nstart = \"{}\"\nduration = \"2h\"\n",
                start.trim_end_matches('Z')
            ),
        );
        let mut service = push_service(&dir, "");
        let t = clock::unix_seconds(now);
        assert!(service.in_maintenance(0, t));
        assert_eq!(service.states[0].state, State::Down);
        // status=down inside the window: MAINT, the state stays.
        let push = |down| Event::Push {
            monitor: 0,
            down,
            time: now,
        };
        service.handle(push(true));
        assert_eq!(service.states[0].state, State::Down);
        // A push is a report from the client: UP even inside the window.
        service.handle(push(false));
        assert_eq!(service.states[0].state, State::Up);
    }

    #[test]
    fn reminders_are_due_by_time() {
        let dir = TestDir::new("run-reminder");
        let mut service = push_service(&dir, "resend_interval = \"1h\"");
        let now = SystemTime::now();
        let t = clock::unix_seconds(now);
        service.apply(0, &CheckResult::fail(None, NO_PUSH), now);
        assert_eq!(service.notify[0].last_down_notice, Some(t));
        service.send_due_reminders(t + 1800);
        assert_eq!(service.notify[0].last_down_notice, Some(t));
        // No result for a day, but a reminder every hour.
        service.send_due_reminders(t + 3600);
        assert_eq!(service.notify[0].last_down_notice, Some(t + 3600));
    }
}
