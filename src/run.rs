//! `uptime-kinomi run`: the long-running service.

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::clock::{self, JumpDetector};
use crate::config::Config;
use crate::monitors::Monitor;
use crate::scheduler::{Event, Scheduler};
use crate::state::MonitorState;
use crate::storage::log::LogWriter;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn run(config: Config) -> Result<(), String> {
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
    let retention_days =
        i64::try_from(g.storage.log_retention.0.as_secs() / 86400).unwrap_or(i64::MAX);
    let mut log = LogWriter::open(&g.storage.log_dir, retention_days).map_err(|e| {
        format!(
            "cannot open log directory {}: {e}",
            g.storage.log_dir.display()
        )
    })?;
    let flush_interval = g.storage.flush_interval.0;
    let (workers, max_queue) = (g.scheduler.workers as usize, g.scheduler.max_queue as usize);

    let monitors: Arc<Vec<Monitor>> = Arc::new(config.monitors);
    let mut states: Vec<MonitorState> = monitors
        .iter()
        .map(|m| MonitorState::new(m.retries))
        .collect();
    let enabled = monitors.iter().filter(|m| m.enabled).count().to_string();
    log.record(
        SystemTime::now(),
        &[
            ("event", "START"),
            ("version", VERSION),
            ("monitors", &enabled),
        ],
    );
    flush(&mut log);
    eprintln!(
        "uptime-kinomi {VERSION}: {enabled} monitors, log in {}",
        g.storage.log_dir.display()
    );

    let (tx, rx) = mpsc::channel();
    let scheduler = Scheduler::start(monitors.clone(), workers, max_queue, tx);
    let mut jumps = JumpDetector::new(SystemTime::now(), Instant::now());
    let mut next_flush = Instant::now() + flush_interval;

    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(event) => handle(event, &monitors, &mut states, &mut log),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if Instant::now() >= next_flush {
            if let Some(jump) = jumps.check(SystemTime::now(), Instant::now()) {
                log.record(
                    SystemTime::now(),
                    &[("event", "CLOCK_JUMP"), ("delta_s", &jump.to_string())],
                );
            }
            flush(&mut log);
            next_flush = Instant::now() + flush_interval;
        }
    }

    scheduler.stop();
    while let Ok(event) = rx.try_recv() {
        handle(event, &monitors, &mut states, &mut log);
    }
    log.record(SystemTime::now(), &[("event", "STOP")]);
    flush(&mut log);
    eprintln!("uptime-kinomi stopped");
    Ok(())
}

fn handle(event: Event, monitors: &[Monitor], states: &mut [MonitorState], log: &mut LogWriter) {
    match event {
        Event::Done {
            monitor,
            result,
            time,
        } => {
            let m = &monitors[monitor];
            let (logged, previous) = states[monitor].apply(result.status);
            let latency = result.latency.map(|l| l.as_millis().to_string());
            let mut fields = vec![("monitor", m.id.as_str()), ("result", logged.name())];
            if let Some(latency) = &latency {
                fields.push(("latency_ms", latency));
            }
            fields.push(("msg", &result.msg));
            log.record(time, &fields);
            if let Some(previous) = previous {
                // Also visible in the journal.
                eprintln!(
                    "{} ({}): {} -> {} ({})",
                    m.name,
                    m.id,
                    previous.name(),
                    states[monitor].state.name(),
                    result.msg
                );
            }
        }
        Event::Skipped {
            monitor,
            count,
            time,
        } => {
            let id = monitors[monitor].id.as_str();
            let count = count.to_string();
            let fields = [
                ("monitor", id),
                ("result", "SKIPPED"),
                ("count", count.as_str()),
                ("msg", "queue full"),
            ];
            log.record(time, &fields);
        }
    }
}

fn flush(log: &mut LogWriter) {
    if let Err(e) = log.flush(SystemTime::now()) {
        eprintln!("error: cannot write the check log: {e}");
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
