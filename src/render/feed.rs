//! Atom feed of a page (SPEC.md §8.7): incidents and changes to and from
//! DOWN of its monitors during the last 30 days.

use std::fmt::Write as _;

use super::Snapshot;
use super::html::{Context, esc, human_duration};
use crate::clock;
use crate::config::pages::Page;
use crate::state::State;

const MAX_ENTRIES: usize = 50;
const MAX_AGE: i64 = 30 * 86400;

struct Entry {
    id: String,
    updated: i64,
    title: String,
    content: String,
}

fn rfc3339(t: i64) -> String {
    clock::format_rfc3339(clock::from_unix(t))
}

pub fn feed(ctx: &Context, page: &Page, snap: &Snapshot, base_url: Option<&str>) -> String {
    let tag = |rest: &str| format!("tag:uptime-kinomi,2026:{}/{rest}", page.id);
    let mut entries = Vec::new();
    let oldest = snap.now - MAX_AGE;

    for incident in ctx.incidents.iter().filter(|i| i.shown_on(page)) {
        let updated = incident
            .end
            .unwrap_or(incident.start)
            .min(snap.now.max(incident.start));
        if incident.end.unwrap_or(snap.now) < oldest {
            continue;
        }
        let status = match incident.end {
            Some(end) if end <= snap.now => "resolved",
            _ if incident.start > snap.now => "scheduled",
            _ => "ongoing",
        };
        entries.push(Entry {
            id: tag(&format!("incident/{}", incident.id)),
            updated,
            title: format!(
                "[{}] {} ({status})",
                incident.severity.name(),
                incident.title
            ),
            content: incident.paragraphs.join("\n\n"),
        });
    }

    let mut seen = Vec::new();
    for group in &page.groups {
        for &i in &group.monitors {
            if seen.contains(&i) {
                continue;
            }
            seen.push(i);
            let (m, v) = (&ctx.monitors[i], &snap.monitors[i]);
            let mut previous: Option<(i64, State)> = None;
            for &(t, state) in &v.changes {
                let was_down = previous.is_some_and(|(_, s)| s == State::Down);
                if t >= oldest && (state == State::Down || was_down) {
                    let title = if state == State::Down {
                        format!("DOWN: {}", m.name)
                    } else {
                        let down_for = previous
                            .map(|(since, _)| human_duration(t - since))
                            .unwrap_or_default();
                        format!("{}: {} (after {down_for} down)", state.name(), m.name)
                    };
                    entries.push(Entry {
                        id: tag(&format!("{}/{t}", m.id)),
                        updated: t,
                        title,
                        content: String::new(),
                    });
                }
                previous = Some((t, state));
            }
        }
    }

    entries.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
    entries.truncate(MAX_ENTRIES);
    let updated = entries.first().map_or(snap.now, |e| e.updated);

    let mut out = String::with_capacity(4096);
    let _ = writeln!(out, "<?xml version=\"1.0\" encoding=\"utf-8\"?>");
    let _ = writeln!(out, "<feed xmlns=\"http://www.w3.org/2005/Atom\">");
    let _ = writeln!(out, "<title>{}</title>", esc(&page.title));
    let _ = writeln!(
        out,
        "<id>{}</id>",
        esc(&format!("tag:uptime-kinomi,2026:{}", page.id))
    );
    let _ = writeln!(out, "<updated>{}</updated>", rfc3339(updated));
    let _ = writeln!(out, "<author><name>uptime-kinomi</name></author>");
    let _ = writeln!(
        out,
        "<generator uri=\"https://github.com/uptime-kinomi/uptime-kinomi\">uptime-kinomi</generator>"
    );
    let page_url = base_url.map(|b| {
        let prefix = if page.public { "" } else { "private/" };
        format!("{}/{prefix}{}/", b.trim_end_matches('/'), page.id)
    });
    if let Some(url) = &page_url {
        let _ = writeln!(out, "<link rel=\"alternate\" href=\"{}\"/>", esc(url));
        let _ = writeln!(out, "<link rel=\"self\" href=\"{}feed.atom\"/>", esc(url));
    }
    for e in entries {
        let _ = writeln!(out, "<entry>");
        let _ = writeln!(out, "<title>{}</title>", esc(&e.title));
        let _ = writeln!(out, "<id>{}</id>", esc(&e.id));
        let _ = writeln!(out, "<updated>{}</updated>", rfc3339(e.updated));
        if let Some(url) = &page_url {
            let _ = writeln!(out, "<link href=\"{}\"/>", esc(url));
        }
        // RFC 4287 §4.1.1: an entry without a link (no base_url) must have
        // content; without text of its own, the title is the content.
        let content = if e.content.is_empty() {
            &e.title
        } else {
            &e.content
        };
        let _ = writeln!(out, "<content type=\"text\">{}</content>", esc(content));
        let _ = writeln!(out, "</entry>");
    }
    let _ = writeln!(out, "</feed>");
    out
}
