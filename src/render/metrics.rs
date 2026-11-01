//! `/metrics` in the Prometheus text format (SPEC.md §9.5).

use std::fmt::Write as _;

use super::Snapshot;
use crate::monitors::Monitor;
use crate::state::State;

/// Escape a label value (backslash, quote, newline).
fn label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Milliseconds as seconds: Prometheus uses base units.
fn seconds(ms: u32) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

pub fn metrics(monitors: &[Monitor], snap: &Snapshot) -> String {
    let mut out = String::with_capacity(4096);
    let mut metric = |name: &str, help: &str, lines: Vec<String>| {
        if lines.is_empty() {
            return;
        }
        let _ = writeln!(out, "# HELP {name} {help}");
        let _ = writeln!(out, "# TYPE {name} gauge");
        for line in lines {
            let _ = writeln!(out, "{name}{line}");
        }
    };
    let s = &snap.status;
    metric(
        "kinomi_build_info",
        "Version of uptime-kinomi.",
        vec![format!("{{version=\"{}\"}} 1", env!("CARGO_PKG_VERSION"))],
    );
    metric(
        "kinomi_start_time_seconds",
        "Start time of the service (Unix seconds).",
        vec![format!(" {}", s.started)],
    );
    metric(
        "kinomi_queue_length",
        "Checks waiting for a worker.",
        vec![format!(" {}", s.queue_len)],
    );
    metric(
        "kinomi_skipped_checks_24h",
        "Checks skipped in the last 24 hours.",
        vec![format!(" {}", s.skipped_24h)],
    );
    if let Some(ms) = s.avg_check_ms_24h {
        metric(
            "kinomi_check_response_24h_seconds",
            "Average response time of all successful checks in the last 24 hours.",
            vec![format!(" {}", seconds(ms))],
        );
    }
    if let Some(t) = s.last_flush {
        metric(
            "kinomi_last_log_flush_timestamp_seconds",
            "Last successful write of the check log (Unix seconds).",
            vec![format!(" {t}")],
        );
    }

    let measured: Vec<(&Monitor, &super::MonitorView)> = monitors
        .iter()
        .zip(&snap.monitors)
        .filter(|(m, v)| m.enabled && !matches!(v.state, State::Unknown | State::Maintenance))
        .collect();
    let up_lines = measured
        .iter()
        .map(|(m, v)| {
            let up = u8::from(matches!(v.state, State::Up | State::Warn));
            format!(
                "{{monitor=\"{}\",type=\"{}\"}} {up}",
                label(&m.id),
                m.kind.name()
            )
        })
        .collect();
    metric(
        "kinomi_monitor_up",
        "1 if the monitor is UP or WARN, 0 if PENDING or DOWN.",
        up_lines,
    );

    let mut uptime = Vec::new();
    for (m, v) in monitors
        .iter()
        .zip(&snap.monitors)
        .filter(|(m, _)| m.enabled)
    {
        for (window, value) in [("24h", v.uptime_24h), ("30d", v.uptime_30d)] {
            if let Some(p) = value {
                uptime.push(format!(
                    "{{monitor=\"{}\",window=\"{window}\"}} {:.5}",
                    label(&m.id),
                    p / 100.0
                ));
            }
        }
    }
    metric(
        "kinomi_monitor_uptime_ratio",
        "Share of checks that were up (UP, WARN, PENDING).",
        uptime,
    );

    let response = monitors
        .iter()
        .zip(&snap.monitors)
        .filter(|(m, _)| m.enabled)
        .filter_map(|(m, v)| {
            v.avg_ms_24h
                .map(|ms| format!("{{monitor=\"{}\"}} {}", label(&m.id), seconds(ms)))
        })
        .collect();
    metric(
        "kinomi_monitor_response_24h_seconds",
        "Average response time of successful checks in the last 24 hours.",
        response,
    );

    let certs = monitors
        .iter()
        .zip(&snap.monitors)
        .filter(|(m, _)| m.enabled)
        .filter_map(|(m, v)| {
            v.cert_expires
                .map(|t| format!("{{monitor=\"{}\"}} {t}", label(&m.id)))
        })
        .collect();
    metric(
        "kinomi_monitor_cert_expiry_timestamp_seconds",
        "Expiry of the TLS certificate (Unix seconds).",
        certs,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_escaping() {
        assert_eq!(label("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }

    #[test]
    fn response_times_in_seconds() {
        assert_eq!(seconds(0), "0.000");
        assert_eq!(seconds(5), "0.005");
        assert_eq!(seconds(120), "0.120");
        assert_eq!(seconds(10_000), "10.000");
    }
}
