//! Outbound heartbeat ("dead man's switch", SPEC.md §10.3): kinomi calls a
//! URL every `interval`, e.g. the push URL of another kinomi or of a service
//! such as healthchecks.io, which alerts if the calls stop.

use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, SystemTime};

use crate::scheduler::Event;

const TIMEOUT: Duration = Duration::from_secs(15);

/// Call `url` every `interval`. Only changes (working → failing and back)
/// are reported, so a long outage does not flood the log.
pub fn spawn(url: String, interval: Duration, events: Sender<Event>) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .max_redirects(3)
        .user_agent(format!(
            "uptime-kinomi/{} (heartbeat)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .new_agent();
    let spawned = thread::Builder::new()
        .name("heartbeat".into())
        .spawn(move || {
            let mut last_error: Option<Option<String>> = None;
            loop {
                let error = match agent.get(&url).call() {
                    Ok(_) => None,
                    Err(ureq::Error::StatusCode(code)) => Some(format!("HTTP {code}")),
                    Err(e) => Some(crate::monitors::http::error_message(&e)),
                };
                if last_error.as_ref() != Some(&error) {
                    let event = Event::Heartbeat {
                        error: error.clone(),
                        time: SystemTime::now(),
                    };
                    if events.send(event).is_err() {
                        return;
                    }
                    last_error = Some(error);
                }
                thread::sleep(interval);
            }
        });
    if let Err(e) = spawned {
        eprintln!("error: cannot start the heartbeat: {e}");
    }
}
