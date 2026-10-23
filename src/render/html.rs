//! HTML of the status pages (SPEC.md §8.2). Plain Rust, no template engine.
//! Every piece of text goes through [`esc`]; there are no inline styles or
//! scripts (the CSP forbids both).

use std::fmt::Write as _;

use jiff::tz::TimeZone;

use super::{MonitorView, SelfStatus, Snapshot};
use crate::config::incidents::Incident;
use crate::config::maintenance::Maintenance;
use crate::config::pages::Page;
use crate::history::{Beat, BeatRecord};
use crate::monitors::Monitor;
use crate::state::State;

/// Static inputs of every render.
pub struct Context<'a> {
    pub pages: &'a [Page],
    pub monitors: &'a [Monitor],
    pub tz: &'a TimeZone,
    pub auto_refresh_s: u64,
    pub recent_checks: usize,
    pub incidents: &'a [Incident],
    pub maintenance: &'a [Maintenance],
    pub now: i64,
}

/// Shown incidents: ongoing, scheduled, or resolved within this time.
const RESOLVED_SHOWN: i64 = 7 * 86400;
/// Maintenance windows starting within this time are announced.
const MAINTENANCE_AHEAD: i64 = 7 * 86400;

/// Escape text for HTML element content and attribute values.
pub fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

pub fn format_time(tz: &TimeZone, unix: i64, pattern: &str) -> String {
    jiff::Timestamp::from_second(unix)
        .map(|t| t.to_zoned(tz.clone()).strftime(pattern).to_string())
        .unwrap_or_else(|_| "?".into())
}

fn time(tz: &TimeZone, unix: i64) -> String {
    format_time(tz, unix, "%Y-%m-%d %H:%M %Z")
}

/// "45 s", "12 min", "3 h", "5 d"
pub fn human_duration(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..=119 => format!("{secs} s"),
        120..=7199 => format!("{} min", secs / 60),
        7200..=172_799 => format!("{} h", secs / 3600),
        _ => format!("{} d", secs / 86400),
    }
}

/// Rounded down, so 99.999 % never shows as 100 % and hides an outage.
fn percent(value: Option<f64>) -> String {
    value.map_or("–".to_string(), |p| {
        if p >= 100.0 {
            "100 %".to_string()
        } else {
            format!("{:.2} %", (p * 100.0).floor() / 100.0)
        }
    })
}

fn state_label(m: &Monitor, v: &MonitorView) -> (&'static str, &'static str) {
    if !m.enabled {
        return ("paused", "PAUSED");
    }
    match v.state {
        State::Up => ("up", "UP"),
        State::Warn => ("warn", "WARN"),
        State::Pending => ("pending", "PENDING"),
        State::Down => ("down", "DOWN"),
        State::Unknown => ("unknown", "NO DATA"),
        State::Maintenance => ("maint", "MAINTENANCE"),
    }
}

/// Overall banner: (css class, text).
pub fn banner(views: &[&MonitorView]) -> (&'static str, &'static str) {
    let measured: Vec<_> = views
        .iter()
        .filter(|v| !matches!(v.state, State::Unknown | State::Maintenance))
        .collect();
    let down = measured.iter().filter(|v| v.state == State::Down).count();
    if measured.is_empty() && views.iter().any(|v| v.state == State::Maintenance) {
        ("maint", "Maintenance")
    } else if measured.is_empty() {
        ("unknown", "No data yet")
    } else if down == 0 {
        ("up", "All systems operational")
    } else if down == measured.len() {
        ("down", "Major outage")
    } else {
        ("partial", "Partial outage")
    }
}

fn beat_title(tz: &TimeZone, b: &BeatRecord) -> String {
    let when = time(tz, b.time);
    match (b.beat, b.latency_ms) {
        (Beat::Skipped, _) => format!("{when}: skipped (queue full)"),
        (beat, Some(ms)) => format!("{when}: {}, {ms} ms", beat.name()),
        (beat, None) => format!("{when}: {}", beat.name()),
    }
}

pub fn page(ctx: &Context, page: &Page, snap: &Snapshot) -> String {
    let tz = ctx.tz;
    let mut out = String::with_capacity(16 * 1024);
    let _ = writeln!(
        out,
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">"
    );
    let _ = writeln!(
        out,
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">"
    );
    if ctx.auto_refresh_s > 0 {
        let _ = writeln!(
            out,
            "<meta http-equiv=\"refresh\" content=\"{}\">",
            ctx.auto_refresh_s
        );
    }
    if !page.public {
        let _ = writeln!(out, "<meta name=\"robots\" content=\"noindex, nofollow\">");
    }
    let _ = writeln!(out, "<meta name=\"generator\" content=\"uptime-kinomi\">");
    let _ = writeln!(out, "<title>{}</title>", esc(&page.title));
    let _ = writeln!(
        out,
        "<link rel=\"stylesheet\" href=\"../assets/kinomi.css\">\n</head>\n<body>\n<div class=\"wrap\">"
    );

    let _ = writeln!(out, "<header>\n<h1>{}</h1>", esc(&page.title));
    if let Some(d) = &page.description {
        let _ = writeln!(out, "<p>{}</p>", esc(d));
    }
    let siblings: Vec<&Page> = ctx
        .pages
        .iter()
        .filter(|p| p.public == page.public)
        .collect();
    if siblings.len() > 1 {
        let _ = writeln!(out, "<nav>");
        for p in siblings {
            let current = if p.id == page.id {
                " aria-current=\"page\""
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "<a href=\"../{}/\"{current}>{}</a>",
                esc(&p.id),
                esc(&p.title)
            );
        }
        let _ = writeln!(out, "</nav>");
    }
    let _ = writeln!(out, "</header>");

    let on_page: Vec<&MonitorView> = page
        .groups
        .iter()
        .flat_map(|g| g.monitors.iter())
        .filter(|&&i| ctx.monitors[i].enabled)
        .map(|&i| &snap.monitors[i])
        .collect();
    let (class, text) = banner(&on_page);
    let _ = writeln!(
        out,
        "<p class=\"banner banner-{class}\" role=\"status\">{text}</p>\n<main>"
    );
    incidents(&mut out, ctx, page);
    upcoming_maintenance(&mut out, ctx, page);

    for group in &page.groups {
        let _ = writeln!(
            out,
            "<section class=\"group\">\n<h2>{}</h2>\n<ul class=\"monitors\">",
            esc(&group.name)
        );
        for &i in &group.monitors {
            monitor(
                &mut out,
                ctx,
                &ctx.monitors[i],
                &snap.monitors[i],
                page.public,
                snap.now,
            );
        }
        let _ = writeln!(out, "</ul>\n</section>");
    }
    if !page.public {
        self_status(&mut out, tz, &snap.status, snap.now);
    }
    let _ = writeln!(out, "</main>");
    let _ = writeln!(
        out,
        "<footer>Last updated {} · <a href=\"feed.atom\">Feed</a> · <a href=\"status.json\">JSON</a> · <a href=\"https://github.com/uptime-kinomi/uptime-kinomi\">uptime-kinomi</a> {}</footer>",
        esc(&time(tz, snap.now)),
        esc(env!("CARGO_PKG_VERSION"))
    );
    let _ = writeln!(out, "</div>\n</body>\n</html>");
    out
}

fn incidents(out: &mut String, ctx: &Context, page: &Page) {
    let mut shown: Vec<&Incident> = ctx
        .incidents
        .iter()
        .filter(|i| i.shown_on(page) && i.end.is_none_or(|e| e > ctx.now - RESOLVED_SHOWN))
        .collect();
    if shown.is_empty() {
        return;
    }
    shown.sort_by_key(|i| std::cmp::Reverse(i.start));
    let _ = writeln!(out, "<section class=\"group\">\n<h2>Incidents</h2>");
    for i in shown {
        let status = match i.end {
            Some(end) if end <= ctx.now => "resolved",
            _ if i.start > ctx.now => "scheduled",
            _ => "ongoing",
        };
        let when = match i.end {
            Some(end) => format!("{} – {}", time(ctx.tz, i.start), time(ctx.tz, end)),
            None => format!("since {}", time(ctx.tz, i.start)),
        };
        let _ = writeln!(
            out,
            "<article class=\"incident incident-{} incident-{status}\">\n<h3>{} <span class=\"incident-status\">{status}</span></h3>\n<p class=\"when\">{}</p>",
            i.severity.name(),
            esc(&i.title),
            esc(&when)
        );
        for p in &i.paragraphs {
            let _ = writeln!(out, "<p>{}</p>", esc(p));
        }
        let _ = writeln!(out, "</article>");
    }
    let _ = writeln!(out, "</section>");
}

fn upcoming_maintenance(out: &mut String, ctx: &Context, page: &Page) {
    let on_page: Vec<usize> = page
        .groups
        .iter()
        .flat_map(|g| g.monitors.iter().copied())
        .collect();
    let mut windows: Vec<(i64, i64, &Maintenance)> = ctx
        .maintenance
        .iter()
        .filter(|w| w.monitors.iter().any(|m| on_page.contains(m)))
        .filter_map(|w| {
            w.next(ctx.tz, ctx.now, MAINTENANCE_AHEAD)
                .map(|(s, e)| (s, e, w))
        })
        .collect();
    if windows.is_empty() {
        return;
    }
    windows.sort_by_key(|(s, _, _)| *s);
    let _ = writeln!(
        out,
        "<section class=\"group\">\n<h2>Planned maintenance</h2>\n<ul class=\"maintenance\">"
    );
    for (start, end, w) in windows {
        let names: Vec<String> = w
            .monitors
            .iter()
            .filter(|m| on_page.contains(m))
            .map(|&m| esc(&ctx.monitors[m].name))
            .collect();
        let now_text = if start <= ctx.now { " (now)" } else { "" };
        let _ = writeln!(
            out,
            "<li><strong>{}</strong>{now_text}: {} – {}<br><span class=\"details\">{}</span></li>",
            esc(&w.title),
            esc(&time(ctx.tz, start)),
            esc(&time(ctx.tz, end)),
            names.join(", ")
        );
    }
    let _ = writeln!(out, "</ul>\n</section>");
}

fn monitor(out: &mut String, ctx: &Context, m: &Monitor, v: &MonitorView, public: bool, now: i64) {
    let tz = ctx.tz;
    let (class, label) = state_label(m, v);
    let _ = writeln!(out, "<li>\n<div class=\"row\">");
    let _ = writeln!(out, "<span class=\"name\">{}</span>", esc(&m.name));
    let _ = writeln!(
        out,
        "<span class=\"uptime\">{} (24 h) · {} (30 d)</span>",
        percent(v.uptime_24h),
        percent(v.uptime_30d)
    );
    let _ = writeln!(
        out,
        "<span class=\"state state-{class}\">{label}</span>\n</div>"
    );

    let _ = write!(
        out,
        "<div class=\"beats\" role=\"img\" aria-label=\"last {} checks\">",
        v.beats.len()
    );
    for _ in v.beats.len()..ctx.recent_checks {
        out.push_str("<span class=\"beat beat-empty\"></span>");
    }
    for b in &v.beats {
        let _ = write!(
            out,
            "<span class=\"beat beat-{}\" title=\"{}\"></span>",
            b.beat.name().to_ascii_lowercase(),
            esc(&beat_title(tz, b))
        );
    }
    let _ = writeln!(out, "</div>");

    let mut details = Vec::new();
    if let Some(ms) = v.avg_ms_24h {
        details.push(format!("avg {ms} ms (24 h)"));
    }
    if let Some(since) = v.since {
        details.push(format!("{label} for {}", human_duration(now - since)));
    }
    if let Some(expires) = v.cert_expires {
        details.push(format!(
            "certificate valid until {}",
            format_time(tz, expires, "%Y-%m-%d")
        ));
    }
    if !details.is_empty() {
        let _ = write!(out, "<div class=\"details\">");
        for d in details {
            let _ = write!(out, "<span>{}</span>", esc(&d));
        }
        let _ = writeln!(out, "</div>");
    }

    if !public {
        let _ = write!(
            out,
            "<div class=\"private\">{} <code>{}</code>",
            m.kind.name(),
            esc(&m.check.target())
        );
        if let Some(t) = v.last_check {
            let _ = write!(
                out,
                " · last check {}: {}",
                esc(&time(tz, t)),
                esc(&v.last_msg)
            );
        }
        if v.skipped_24h > 0 {
            let _ = write!(out, " · {} skipped (24 h)", v.skipped_24h);
        }
        if let Some(d) = &m.description {
            let _ = write!(out, "<br>{}", esc(d));
        }
        let _ = writeln!(out, "</div>");
    }
    let _ = writeln!(out, "</li>");
}

fn self_status(out: &mut String, tz: &TimeZone, s: &SelfStatus, now: i64) {
    let _ = writeln!(
        out,
        "<section class=\"group\">\n<h2>uptime-kinomi</h2>\n<table class=\"self\">"
    );
    let rows = [
        ("Version", env!("CARGO_PKG_VERSION").to_string()),
        (
            "Running since",
            format!(
                "{} ({})",
                time(tz, s.started),
                human_duration(now - s.started)
            ),
        ),
        (
            "Checks waiting for a worker",
            format!("{} (workers: {})", s.queue_len, s.workers),
        ),
        ("Skipped checks (24 h)", s.skipped_24h.to_string()),
        ("Last page render", format!("{} ms", s.last_render_ms)),
    ];
    for (name, value) in rows {
        let _ = writeln!(
            out,
            "<tr><th>{}</th><td>{}</td></tr>",
            esc(name),
            esc(&value)
        );
    }
    let _ = writeln!(out, "</table>\n</section>");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::pages::Group;
    use crate::monitors::test_monitor;

    const T: i64 = 1_792_678_991;

    fn view(state: State) -> MonitorView {
        MonitorView {
            state,
            since: Some(T - 7200),
            last_check: Some(T - 30),
            last_msg: "HTTP 200".into(),
            cert_expires: Some(T + 40 * 86400),
            beats: vec![
                BeatRecord {
                    time: T - 60,
                    beat: Beat::Up,
                    latency_ms: Some(120),
                },
                BeatRecord {
                    time: T - 30,
                    beat: Beat::Skipped,
                    latency_ms: None,
                },
            ],
            uptime_24h: Some(99.5),
            uptime_30d: Some(100.0),
            avg_ms_24h: Some(120),
            skipped_24h: 1,
            changes: vec![],
        }
    }

    fn render(public: bool, name: &str, views: Vec<MonitorView>) -> String {
        let mut m = test_monitor("web");
        m.name = name.into();
        m.description = Some("<b>desc</b>".into());
        let monitors: Vec<Monitor> = (0..views.len())
            .map(|_| test_monitor("x"))
            .collect::<Vec<_>>();
        let mut monitors = monitors;
        monitors[0] = m;
        let pages = vec![Page {
            id: "home".into(),
            title: "Home <&>".into(),
            public,
            description: Some("It's \"ours\"".into()),
            groups: vec![Group {
                name: "G<1>".into(),
                monitors: (0..views.len()).collect(),
            }],
            show_incidents: true,
            show_badges: false,
        }];
        let ctx = Context {
            pages: &pages,
            monitors: &monitors,
            tz: &TimeZone::UTC,
            auto_refresh_s: 60,
            recent_checks: 5,
            incidents: &[],
            maintenance: &[],
            now: T,
        };
        let snap = Snapshot {
            now: T,
            monitors: views,
            status: SelfStatus {
                started: T - 3600,
                queue_len: 0,
                workers: 4,
                skipped_24h: 1,
                last_render_ms: 3,
            },
        };
        page(&ctx, &pages[0], &snap)
    }

    #[test]
    fn hostile_text_is_escaped() {
        let html = render(false, "<script>alert(1)</script>", vec![view(State::Up)]);
        assert!(!html.contains("<script"), "{html}");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("<title>Home &lt;&amp;&gt;</title>"));
        assert!(html.contains("It&#39;s &quot;ours&quot;"));
        assert!(html.contains("&lt;b&gt;desc&lt;/b&gt;"));
        assert!(!html.contains("style="), "no inline styles (CSP)");
        let has_handler = html.split(" on").skip(1).any(|rest| {
            rest.split('=').next().is_some_and(|name| {
                !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase())
            })
        });
        assert!(!has_handler, "no event handler attributes");
    }

    #[test]
    fn public_and_private_content() {
        let public = render(true, "Web", vec![view(State::Up)]);
        assert!(public.contains("<meta http-equiv=\"refresh\" content=\"60\">"));
        assert!(public.contains("All systems operational"));
        assert!(public.contains("99.50 % (24 h) · 100 % (30 d)"));
        assert!(public.contains("<span class=\"state state-up\">UP</span>"));
        assert!(public.contains("certificate valid until 2026-12-01"));
        assert!(public.contains("UP for 2 h"));
        // 5 slots: 3 empty, then the 2 beats.
        assert_eq!(public.matches("beat beat-empty").count(), 3);
        assert!(public.contains("title=\"2026-10-22 14:22 UTC: UP, 120 ms\""));
        // Private details are not on public pages.
        assert!(!public.contains("last check"));
        assert!(!public.contains("noindex"));
        assert!(!public.contains("Running since"));

        let private = render(false, "Web", vec![view(State::Up)]);
        assert!(
            private.contains("tcp <code>test</code> · last check 2026-10-22 14:22 UTC: HTTP 200")
        );
        assert!(private.contains("noindex"));
        assert!(private.contains("Running since"));
    }

    #[test]
    fn banners() {
        let b = |states: &[State]| {
            let views: Vec<MonitorView> = states.iter().map(|&s| view(s)).collect();
            banner(&views.iter().collect::<Vec<_>>()).1
        };
        assert_eq!(
            b(&[State::Up, State::Warn, State::Pending]),
            "All systems operational"
        );
        assert_eq!(b(&[State::Up, State::Down]), "Partial outage");
        assert_eq!(b(&[State::Down, State::Unknown]), "Major outage");
        assert_eq!(b(&[State::Unknown]), "No data yet");
    }

    #[test]
    fn durations_and_percent() {
        assert_eq!(human_duration(45), "45 s");
        assert_eq!(human_duration(600), "10 min");
        assert_eq!(human_duration(3 * 3600), "3 h");
        assert_eq!(human_duration(5 * 86400), "5 d");
        assert_eq!(human_duration(-5), "0 s");
        assert_eq!(percent(Some(99.999)), "99.99 %");
        assert_eq!(percent(Some(100.0)), "100 %");
        assert_eq!(percent(None), "–");
    }
}
