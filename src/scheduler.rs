//! Check scheduling with a bounded queue and fair skipping (SPEC.md §6).
//!
//! Every monitor has a fixed time grid: due at its start offset, then every
//! `interval`. A dispatcher thread moves due checks into a bounded queue;
//! worker threads run them. A due check that does not fit into the queue
//! waits; when room becomes free, the waiting check that was started least
//! recently goes first (round-robin). When a check starts late, every grid
//! point it missed is logged as SKIPPED. Under overload all monitors are
//! therefore checked equally often, and runs + skips always add up to the
//! elapsed intervals.

use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use crate::monitors::{CheckResult, Monitor};

/// First checks after startup are spread over this time.
const STARTUP_SPREAD: Duration = Duration::from_secs(10);

/// The dispatcher wakes at least this often.
const MAX_SLEEP: Duration = Duration::from_secs(1);

pub enum Event {
    Done {
        monitor: usize,
        result: CheckResult,
        time: SystemTime,
    },
    /// `count` grid points passed without a check (queue full).
    Skipped {
        monitor: usize,
        count: u32,
        time: SystemTime,
    },
}

#[derive(Debug)]
struct Slot {
    monitor: usize,
    interval: Duration,
    /// Next point on the monitor's time grid.
    next_due: Instant,
    /// Dispatch order of the last start (round-robin key); None = never.
    last_ticket: Option<u64>,
    running: bool,
}

/// Move waiting slots into the queue. `capacity` is the free space in the
/// queue. Returns the slots to run and, for those, the number of grid points
/// they missed while waiting.
///
/// The order is by last start, not by last completion or due time: those
/// depend on how fast a check is and when it happens to become due, and must
/// not decide who gets skipped.
fn plan(
    slots: &mut [Slot],
    now: Instant,
    capacity: usize,
    tickets: &mut u64,
) -> (Vec<usize>, Vec<(usize, u32)>) {
    let mut waiting: Vec<usize> = (0..slots.len())
        .filter(|&i| !slots[i].running && slots[i].next_due <= now)
        .collect();
    // Never started first (None < Some), then the least recently started.
    waiting.sort_by_key(|&i| (slots[i].last_ticket, i));
    waiting.truncate(capacity);
    let mut missed = Vec::new();
    for &i in &waiting {
        let slot = &mut slots[i];
        let passed = grid_points_passed(slot.next_due, slot.interval, now);
        slot.next_due += slot.interval * passed;
        if passed > 1 {
            missed.push((i, passed - 1));
        }
        slot.running = true;
        slot.last_ticket = Some(*tickets);
        *tickets += 1;
    }
    (waiting, missed)
}

/// Grid points `due, due + interval, ...` that are `<= now` (at least 1).
fn grid_points_passed(due: Instant, interval: Duration, now: Instant) -> u32 {
    let late = now.saturating_duration_since(due).as_nanos();
    let n = late / interval.as_nanos().max(1) + 1;
    u32::try_from(n).unwrap_or(u32::MAX)
}

struct Inner {
    slots: Vec<Slot>,
    queue: VecDeque<usize>,
    tickets: u64,
    shutdown: bool,
}

struct Shared {
    inner: Mutex<Inner>,
    work: Condvar,
    wake: Condvar,
    max_queue: usize,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A poisoned lock means a thread panicked while holding it; the data
        // (plain timestamps and flags) is still usable.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct Scheduler {
    shared: Arc<Shared>,
    dispatcher: Option<JoinHandle<()>>,
}

impl Scheduler {
    /// Start the dispatcher and `workers` worker threads for all enabled
    /// monitors. Results and skips are sent to `events`.
    pub fn start(
        monitors: Arc<Vec<Monitor>>,
        workers: usize,
        max_queue: usize,
        events: Sender<Event>,
    ) -> Self {
        let now = Instant::now();
        let enabled: Vec<usize> = (0..monitors.len())
            .filter(|&i| monitors[i].enabled)
            .collect();
        let count = enabled.len().max(1) as u32;
        let slots = enabled
            .iter()
            .enumerate()
            .map(|(n, &monitor)| Slot {
                monitor,
                interval: monitors[monitor].interval,
                next_due: now + STARTUP_SPREAD * n as u32 / count,
                last_ticket: None,
                running: false,
            })
            .collect();
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                slots,
                queue: VecDeque::new(),
                tickets: 0,
                shutdown: false,
            }),
            work: Condvar::new(),
            wake: Condvar::new(),
            max_queue,
        });

        for n in 0..workers {
            let (shared, monitors, events) = (shared.clone(), monitors.clone(), events.clone());
            // Workers are detached: on shutdown we do not wait for running
            // checks, whose timeouts can be long.
            let _ = thread::Builder::new()
                .name(format!("worker-{n}"))
                .spawn(move || worker(&shared, &monitors, &events));
        }
        let dispatcher = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("dispatcher".into())
                .spawn(move || dispatcher(&shared, &events))
                .ok()
        };
        Scheduler { shared, dispatcher }
    }

    pub fn stop(mut self) {
        self.shared.lock().shutdown = true;
        self.shared.work.notify_all();
        self.shared.wake.notify_all();
        if let Some(handle) = self.dispatcher.take() {
            let _ = handle.join();
        }
    }

    /// Checks waiting for a worker (for self-monitoring).
    #[allow(dead_code)] // shown on private pages (M2)
    pub fn queue_len(&self) -> usize {
        self.shared.lock().queue.len()
    }
}

fn dispatcher(shared: &Shared, events: &Sender<Event>) {
    let mut inner = shared.lock();
    while !inner.shutdown {
        let now = Instant::now();
        let capacity = shared.max_queue.saturating_sub(inner.queue.len());
        let Inner { slots, tickets, .. } = &mut *inner;
        let (run, missed) = plan(slots, now, capacity, tickets);
        if !run.is_empty() {
            inner.queue.extend(run);
            shared.work.notify_all();
        }
        let time = SystemTime::now();
        for (i, count) in missed {
            let _ = events.send(Event::Skipped {
                monitor: inner.slots[i].monitor,
                count,
                time,
            });
        }
        // Waiting slots need no timer: workers wake us when they take a check
        // from the queue.
        let next = inner
            .slots
            .iter()
            .filter(|s| !s.running && s.next_due > now)
            .map(|s| s.next_due)
            .min()
            .unwrap_or(now + MAX_SLEEP);
        let sleep = next
            .saturating_duration_since(Instant::now())
            .min(MAX_SLEEP);
        inner = shared
            .wake
            .wait_timeout(inner, sleep)
            .map(|(guard, _)| guard)
            .unwrap_or_else(|e| e.into_inner().0);
    }
}

fn worker(shared: &Shared, monitors: &[Monitor], events: &Sender<Event>) {
    loop {
        let slot_index = {
            let mut inner = shared.lock();
            loop {
                if inner.shutdown {
                    return;
                }
                if let Some(i) = inner.queue.pop_front() {
                    break i;
                }
                inner = shared.work.wait(inner).unwrap_or_else(|e| e.into_inner());
            }
        };
        // There is room in the queue again.
        shared.wake.notify_all();
        let monitor = shared.lock().slots[slot_index].monitor;
        let result = monitors[monitor].check.check();
        {
            let mut inner = shared.lock();
            inner.slots[slot_index].running = false;
        }
        shared.wake.notify_all();
        if events
            .send(Event::Done {
                monitor,
                result,
                time: SystemTime::now(),
            })
            .is_err()
        {
            return; // receiver gone: shutting down
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitors::{Check, Kind, Status};
    use std::sync::mpsc;

    fn slots(n: usize, interval: Duration, now: Instant) -> Vec<Slot> {
        (0..n)
            .map(|monitor| Slot {
                monitor,
                interval,
                next_due: now,
                last_ticket: None,
                running: false,
            })
            .collect()
    }

    /// Simulate `rounds` intervals where every check completes instantly.
    fn simulate(n: usize, capacity: usize, rounds: u32) -> Vec<u32> {
        let interval = Duration::from_secs(10);
        let start = Instant::now();
        let mut slots = slots(n, interval, start);
        let mut runs = vec![0u32; n];
        let mut tickets = 0;
        for round in 0..rounds {
            let now = start + interval * round;
            let (run, _) = plan(&mut slots, now, capacity, &mut tickets);
            for i in run {
                runs[i] += 1;
                slots[i].running = false;
            }
        }
        runs
    }

    /// Simulate real timing in 100 ms steps: `workers` workers, a queue of
    /// `max_queue`, checks that take `check_ms`. Returns (runs, skips) per
    /// monitor.
    fn simulate_timed(
        n: usize,
        check_ms: u64,
        workers: usize,
        max_queue: usize,
        seconds: u64,
    ) -> (Vec<u32>, Vec<u32>) {
        let start = Instant::now();
        let interval = Duration::from_secs(10);
        let mut slots: Vec<Slot> = (0..n)
            .map(|monitor| Slot {
                monitor,
                interval,
                // Startup spread as in Scheduler::start.
                next_due: start + STARTUP_SPREAD * monitor as u32 / n as u32,
                last_ticket: None,
                running: false,
            })
            .collect();
        let (mut runs, mut skips) = (vec![0u32; n], vec![0u32; n]);
        let mut queue = VecDeque::new();
        let mut busy: Vec<Option<(usize, Instant)>> = vec![None; workers];
        let mut tickets = 0;
        for step in 0..seconds * 10 {
            let now = start + Duration::from_millis(step * 100);
            for w in busy.iter_mut() {
                if let Some((slot, end)) = *w {
                    if end <= now {
                        slots[slot].running = false;
                        *w = None;
                    }
                }
            }
            for _ in 0..2 {
                for w in busy.iter_mut().filter(|w| w.is_none()) {
                    if let Some(slot) = queue.pop_front() {
                        runs[slot] += 1;
                        *w = Some((slot, now + Duration::from_millis(check_ms)));
                    }
                }
                let (run, skipped) = plan(&mut slots, now, max_queue - queue.len(), &mut tickets);
                queue.extend(run);
                for (i, count) in skipped {
                    skips[i] += count;
                }
            }
        }
        (runs, skips)
    }

    #[test]
    fn overload_is_shared_fairly_in_real_time() {
        // The case that showed the problem: 4 monitors with 5 s checks every
        // 10 s, 1 worker, queue of 1. Demand is twice the capacity.
        let (runs, skips) = simulate_timed(4, 5000, 1, 1, 300);
        let spread = |v: &[u32]| v.iter().max().unwrap() - v.iter().min().unwrap();
        assert!(spread(&runs) <= 1, "runs {runs:?}");
        assert!(spread(&skips) <= 1, "skips {skips:?}");
        // Every grid point (30 per monitor in 300 s) is either a run or a
        // skip; the last one or two may still be waiting.
        for i in 0..4 {
            let total = runs[i] + skips[i];
            assert!(
                (28..=30).contains(&total),
                "monitor {i}: {runs:?} {skips:?}"
            );
        }
        assert!(
            runs.iter().sum::<u32>() >= 55,
            "worker idle too often: {runs:?}"
        );
        // Other shapes.
        for (n, check_ms, workers, max_queue) in
            [(10, 3000, 2, 3), (7, 9000, 3, 1), (20, 2500, 4, 32)]
        {
            let (runs, _) = simulate_timed(n, check_ms, workers, max_queue, 600);
            assert!(
                spread(&runs) <= 1,
                "{n} monitors, {check_ms} ms, {workers} workers: {runs:?}"
            );
        }
    }

    #[test]
    fn no_skips_without_overload() {
        let (runs, skips) = simulate_timed(10, 1000, 2, 32, 100);
        assert!(skips.iter().all(|&s| s == 0), "{skips:?}");
        assert!(runs.iter().all(|&r| r == 10), "{runs:?}");
    }

    #[test]
    fn everything_runs_with_enough_capacity() {
        assert_eq!(simulate(5, 10, 4), vec![4; 5]);
    }

    #[test]
    fn skips_are_spread_evenly() {
        // 10 monitors, room for 3 per round: over 10 rounds each runs 3 times.
        assert_eq!(simulate(10, 3, 10), vec![3; 10]);
        // Uneven case: no monitor gets more than one run more than another.
        let runs = simulate(7, 2, 20);
        let (min, max) = (runs.iter().min().unwrap(), runs.iter().max().unwrap());
        assert!(max - min <= 1, "{runs:?}");
        assert_eq!(runs.iter().sum::<u32>(), 40);
    }

    #[test]
    fn running_checks_are_not_started_twice() {
        let now = Instant::now();
        let mut slots = slots(1, Duration::from_secs(10), now);
        let mut tickets = 0;
        let (run, _) = plan(&mut slots, now, 5, &mut tickets);
        assert_eq!(run, [0]);
        // Still running when due again.
        let (run, skipped) = plan(&mut slots, now + Duration::from_secs(20), 5, &mut tickets);
        assert!(run.is_empty() && skipped.is_empty());
    }

    #[test]
    fn grid_points() {
        let now = Instant::now();
        let ten = Duration::from_secs(10);
        assert_eq!(grid_points_passed(now, ten, now), 1);
        assert_eq!(
            grid_points_passed(now, ten, now + Duration::from_millis(9999)),
            1
        );
        assert_eq!(grid_points_passed(now, ten, now + ten), 2);
        assert_eq!(
            grid_points_passed(now, ten, now + Duration::from_secs(3600)),
            361
        );
    }

    #[test]
    fn late_start_reports_missed_grid_points_once() {
        let now = Instant::now();
        let mut slots = slots(1, Duration::from_secs(10), now);
        let mut tickets = 0;
        // Starts 35 s late: grid points 0, 10, 20, 30 passed, 3 were missed.
        let later = now + Duration::from_secs(35);
        let (run, missed) = plan(&mut slots, later, 1, &mut tickets);
        assert_eq!(run, [0]);
        assert_eq!(missed, [(0, 3)]);
        // The grid is kept: next due at 40 s, not 35 s + 10 s.
        assert_eq!(slots[0].next_due, now + Duration::from_secs(40));
    }

    fn monitor(id: &str, enabled: bool) -> Monitor {
        let mut m = crate::monitors::test_monitor(id);
        m.enabled = enabled;
        m
    }

    #[test]
    fn threads_run_enabled_monitors() {
        let monitors = Arc::new(vec![
            monitor("a", true),
            monitor("b", false),
            monitor("c", true),
        ]);
        let (tx, rx) = mpsc::channel();
        let scheduler = Scheduler::start(monitors, 2, 8, tx);
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(15);
        while seen.len() < 2 && Instant::now() < deadline {
            if let Ok(Event::Done {
                monitor, result, ..
            }) = rx.recv_timeout(Duration::from_millis(500))
            {
                assert_eq!(result.status, Status::Up);
                seen.push(monitor);
            }
        }
        scheduler.stop();
        seen.sort();
        assert_eq!(seen, [0, 2]);
    }
}
