//! `/push/<token>` for push monitors (SPEC.md §4.6).
//!
//! Only acknowledged: the request body is never read, `msg` and `ping`
//! (uptime-kuma compatibility) and unknown parameters are ignored, and only
//! `status=up|down` is evaluated. Responses have no body.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime};

use crate::encoding::constant_time_eq;
use crate::monitors::{Kind, Monitor};
use crate::scheduler::Event;

const MAX_QUERY: usize = 256;
const MIN_PUSH_GAP: Duration = Duration::from_secs(1);

pub struct PushRegistry {
    /// (token, monitor index) of enabled push monitors.
    tokens: Vec<(Vec<u8>, usize)>,
    last: Mutex<HashMap<usize, Instant>>,
    events: Mutex<Sender<Event>>,
}

impl PushRegistry {
    pub fn new(monitors: &[Monitor], events: Sender<Event>) -> Self {
        let tokens = monitors
            .iter()
            .enumerate()
            .filter(|(_, m)| m.enabled && m.kind == Kind::Push)
            .filter_map(|(i, m)| {
                m.push_token
                    .as_ref()
                    .map(|t| (t.expose().as_bytes().to_vec(), i))
            })
            .collect();
        PushRegistry {
            tokens,
            last: Mutex::new(HashMap::new()),
            events: Mutex::new(events),
        }
    }

    /// Handle a push; returns the HTTP status.
    pub fn accept(&self, token: &str, query: Option<&str>) -> u16 {
        // Compare with every token, so the time does not reveal which
        // prefix matched.
        let mut found = None;
        for (candidate, monitor) in &self.tokens {
            if constant_time_eq(candidate, token.as_bytes()) {
                found = Some(*monitor);
            }
        }
        let Some(monitor) = found else {
            return 404;
        };
        let Some(down) = parse_status(query) else {
            return 400;
        };
        let now = Instant::now();
        {
            let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
            if last
                .get(&monitor)
                .is_some_and(|t| now.duration_since(*t) < MIN_PUSH_GAP)
            {
                return 429;
            }
            last.insert(monitor, now);
        }
        let events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        let _ = events.send(Event::Push {
            monitor,
            down,
            time: SystemTime::now(),
        });
        204
    }
}

/// `status=up|down` from the query (default up). `None` for a malformed
/// query or status value.
fn parse_status(query: Option<&str>) -> Option<bool> {
    let Some(query) = query else {
        return Some(false);
    };
    if query.len() > MAX_QUERY {
        return None;
    }
    let mut down = false;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == "status" {
            down = match value {
                "up" => false,
                "down" => true,
                _ => return None,
            };
        }
    }
    Some(down)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitors::test_monitor;
    use std::sync::mpsc;

    const TOKEN: &str = "JszWA5OXxvVKjAna6eBy9RwPAX2bP0vyX_LI_dGNn2w";

    fn registry() -> (PushRegistry, mpsc::Receiver<Event>) {
        let mut m = test_monitor("backup");
        m.kind = Kind::Push;
        let mut warnings = Vec::new();
        m.push_token =
            crate::config::secret::resolve("t", Some(TOKEN), None, &mut warnings).unwrap();
        let (tx, rx) = mpsc::channel();
        (PushRegistry::new(&[test_monitor("web"), m], tx), rx)
    }

    #[test]
    fn statuses() {
        assert_eq!(parse_status(None), Some(false));
        assert_eq!(parse_status(Some("status=down")), Some(true));
        // uptime-kuma style parameters are accepted and ignored.
        assert_eq!(parse_status(Some("status=up&msg=OK&ping=12")), Some(false));
        assert_eq!(parse_status(Some("x=1&&status=down")), Some(true));
        assert_eq!(parse_status(Some("status=maybe")), None);
        assert_eq!(parse_status(Some(&"a".repeat(300))), None);
    }

    #[test]
    fn accept_and_rate_limit() {
        let (reg, rx) = registry();
        assert_eq!(reg.accept(TOKEN, Some("status=down&msg=disk%20full")), 204);
        match rx.try_recv() {
            Ok(Event::Push { monitor, down, .. }) => assert_eq!((monitor, down), (1, true)),
            _ => panic!("no push event"),
        }
        assert_eq!(reg.accept(TOKEN, None), 429);
        assert!(rx.try_recv().is_err());
        assert_eq!(reg.accept(&TOKEN.replace('J', "K"), None), 404);
        assert_eq!(reg.accept("short", None), 404);
        assert_eq!(reg.accept(TOKEN, Some("status=weird")), 400);
    }
}
