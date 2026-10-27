//! Liveness of the scheduler for `/healthz` (SPEC.md §9.2): the dispatcher
//! and the event loop report that they are running; `/healthz` is only OK
//! while both did so recently.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The dispatcher wakes at least once per second and the event loop every
/// 500 ms, so 10 s without a sign of life means one of them is stuck.
const MAX_SILENCE: Duration = Duration::from_secs(10);

pub struct Health {
    start: Instant,
    /// Milliseconds since `start` of the last round of each thread.
    dispatcher: AtomicU64,
    event_loop: AtomicU64,
}

impl Default for Health {
    fn default() -> Self {
        Health {
            start: Instant::now(),
            dispatcher: AtomicU64::new(0),
            event_loop: AtomicU64::new(0),
        }
    }
}

impl Health {
    pub fn dispatcher_alive(&self) {
        self.dispatcher.store(self.now_ms(), Ordering::Relaxed);
    }

    pub fn event_loop_alive(&self) {
        self.event_loop.store(self.now_ms(), Ordering::Relaxed);
    }

    pub fn ok(&self) -> bool {
        self.ok_at(self.start.elapsed())
    }

    fn ok_at(&self, now: Duration) -> bool {
        let limit = u64::try_from(MAX_SILENCE.as_millis()).unwrap_or(u64::MAX);
        let now = u64::try_from(now.as_millis()).unwrap_or(u64::MAX);
        [&self.dispatcher, &self.event_loop]
            .iter()
            .all(|t| now.saturating_sub(t.load(Ordering::Relaxed)) <= limit)
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_threads_must_be_alive() {
        let h = Health::default();
        h.dispatcher_alive();
        h.event_loop_alive();
        assert!(h.ok());
        // Only one of them reports later on: not OK.
        h.event_loop.store(60_000, Ordering::Relaxed);
        assert!(!h.ok_at(Duration::from_secs(61)));
        h.dispatcher.store(60_000, Ordering::Relaxed);
        assert!(h.ok_at(Duration::from_secs(61)));
        assert!(!h.ok_at(Duration::from_secs(71)));
    }
}
