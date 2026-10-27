//! `status.json` of a page (SPEC.md §8.6).

use std::fmt::Write as _;

use super::html::{Context, banner, format_time};
use super::{MonitorView, Snapshot};
use crate::clock;
use crate::config::pages::Page;
use crate::encoding::json_string as q;
use crate::monitors::Monitor;
use crate::state::State;

/// Incremented on incompatible changes.
const SCHEMA: u32 = 1;

fn time(t: Option<i64>) -> String {
    t.map_or("null".into(), |t| {
        q(&clock::format_rfc3339(clock::from_unix(t)))
    })
}

/// Uptime with 3 decimals, rounded down like on the page.
fn percent(p: Option<f64>) -> String {
    p.map_or("null".into(), |p| {
        format!("{:.3}", (p * 1000.0).floor() / 1000.0)
    })
}

pub fn state_name(m: &Monitor, v: &MonitorView) -> &'static str {
    if m.enabled { v.state.name() } else { "PAUSED" }
}

pub fn status_json(ctx: &Context, page: &Page, snap: &Snapshot) -> String {
    let on_page: Vec<&MonitorView> = page
        .groups
        .iter()
        .flat_map(|g| g.monitors.iter())
        .filter(|&&i| ctx.monitors[i].enabled)
        .map(|&i| &snap.monitors[i])
        .collect();
    let overall = match banner(&on_page).0 {
        "up" => "up",
        "partial" => "partial",
        "down" => "down",
        "maint" => "maintenance",
        _ => "unknown",
    };
    let mut out = String::with_capacity(4096);
    let _ = write!(
        out,
        "{{\n  \"schema\": {SCHEMA},\n  \"kinomi_version\": {},\n  \"page\": {},\n  \"title\": {},\n  \"generated\": {},\n  \"overall\": {},\n  \"monitors\": [",
        q(env!("CARGO_PKG_VERSION")),
        q(&page.id),
        q(&page.title),
        time(Some(snap.now)),
        q(overall)
    );
    let mut first = true;
    for group in &page.groups {
        for &i in &group.monitors {
            let (m, v) = (&ctx.monitors[i], &snap.monitors[i]);
            out.push_str(if first { "\n" } else { ",\n" });
            first = false;
            let cert = v.cert_expires.map_or("null".into(), |t| {
                q(&format_time(&jiff::tz::TimeZone::UTC, t, "%Y-%m-%d"))
            });
            let avg = v.avg_ms_24h.map_or("null".into(), |ms| ms.to_string());
            let _ = write!(
                out,
                "    {{\"id\": {}, \"name\": {}, \"group\": {}, \"state\": {}, \"since\": {}, \"uptime_24h\": {}, \"uptime_30d\": {}, \"avg_ms_24h\": {avg}, \"cert_expires\": {cert}",
                q(&m.id),
                q(&m.name),
                q(&group.name),
                q(state_name(m, v)),
                time(v.since.filter(|_| v.state != State::Unknown)),
                percent(v.uptime_24h),
                percent(v.uptime_30d),
            );
            if !page.public {
                let _ = write!(
                    out,
                    ", \"type\": {}, \"target\": {}, \"last_check\": {}, \"last_msg\": {}, \"last_error_time\": {}, \"last_error\": {}",
                    q(m.kind.name()),
                    q(&m.check.target()),
                    time(v.last_check),
                    if v.last_check.is_some() {
                        q(&v.last_msg)
                    } else {
                        "null".into()
                    },
                    time(v.last_error.as_ref().map(|(t, _)| *t)),
                    v.last_error
                        .as_ref()
                        .map_or("null".into(), |(_, msg)| q(msg)),
                );
            }
            out.push('}');
        }
    }
    out.push_str("\n  ]\n}\n");
    out
}
