//! Incidents `incidents/*.toml` (SPEC.md §14.1): announcements shown on pages.

use serde::Deserialize;

use super::FileProblems;
use super::pages::Page;

const MAX_TEXT: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Minor,
    Major,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Minor => "minor",
            Severity::Major => "major",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Incident {
    pub id: String,
    pub title: String,
    pub severity: Severity,
    /// Unix seconds.
    pub start: i64,
    pub end: Option<i64>,
    /// Page IDs; `None` = all pages with `show_incidents`.
    pub pages: Option<Vec<String>>,
    pub paragraphs: Vec<String>,
}

impl Incident {
    pub fn shown_on(&self, page: &Page) -> bool {
        page.show_incidents && self.pages.as_ref().is_none_or(|p| p.contains(&page.id))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IncidentFile {
    config_version: u32,
    title: String,
    severity: Severity,
    start: String,
    end: Option<String>,
    pages: Option<Vec<String>>,
    #[serde(default)]
    text: String,
}

/// RFC 3339 with an offset or `Z` → Unix seconds.
pub fn parse_timestamp(text: &str) -> Option<i64> {
    text.parse::<jiff::Timestamp>().ok().map(|t| t.as_second())
}

pub fn parse(id: &str, text: &str, pages: &[Page], p: &mut FileProblems) -> Option<Incident> {
    let raw: IncidentFile = match toml::from_str(text) {
        Ok(raw) => raw,
        Err(e) => {
            p.error(e.to_string());
            return None;
        }
    };
    if raw.config_version != 1 {
        p.error(format!(
            "config_version must be 1, found {}",
            raw.config_version
        ));
    }
    let title = raw.title.trim().to_string();
    if title.is_empty() || title.chars().count() > 200 || title.chars().any(char::is_control) {
        p.error("title must be 1 to 200 characters without control characters");
    }
    let start = parse_timestamp(&raw.start).unwrap_or_else(|| {
        p.error("start must be a time with offset, e.g. \"2026-10-22T18:00:00+02:00\"");
        0
    });
    let end = raw.end.as_deref().map(|e| {
        parse_timestamp(e).unwrap_or_else(|| {
            p.error("end must be a time with offset, e.g. \"2026-10-22T19:00:00+02:00\"");
            0
        })
    });
    if end.is_some_and(|e| e < start) {
        p.error("end is before start");
    }
    if let Some(list) = &raw.pages {
        for page in list {
            if !pages.iter().any(|pg| &pg.id == page) {
                p.error(format!("page '{page}' does not exist"));
            }
        }
    }
    if raw.text.len() > MAX_TEXT {
        p.error(format!("text must be at most {MAX_TEXT} bytes"));
    }
    let paragraphs = raw
        .text
        .split("\n\n")
        .map(|para| para.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|para| !para.is_empty())
        .collect();
    if p.has_errors() {
        return None;
    }
    Some(Incident {
        id: id.to_string(),
        title,
        severity: raw.severity,
        start,
        end,
        pages: raw.pages,
        paragraphs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Problems;
    use std::path::Path;

    fn parse_text(text: &str) -> (Option<Incident>, Problems) {
        let mut problems = Problems::default();
        let i = parse("i", text, &[], &mut problems.for_file(Path::new("i.toml")));
        (i, problems)
    }

    #[test]
    fn spec_example() {
        let (i, problems) = parse_text(
            "config_version = 1\ntitle = \"Router firmware update\"\nseverity = \"minor\"\nstart = \"2026-10-22T18:00:00+02:00\"\nend = \"2026-10-22T19:00:00+02:00\"\ntext = \"\"\"\nThe router is being\nupdated.\n\nBack soon.\n\"\"\"\n",
        );
        let i = i.unwrap_or_else(|| panic!("{problems:?}"));
        assert_eq!(i.start, 1_792_684_800); // 16:00 UTC
        assert_eq!(i.end, Some(1_792_688_400));
        assert_eq!(i.paragraphs, ["The router is being updated.", "Back soon."]);
    }

    #[test]
    fn errors() {
        for (text, expected) in [
            (
                "config_version = 1\ntitle = \"T\"\nseverity = \"huge\"\nstart = \"2026-10-22T18:00:00Z\"\n",
                "huge",
            ),
            (
                "config_version = 1\ntitle = \"T\"\nseverity = \"info\"\nstart = \"2026-10-22 18:00\"\n",
                "start",
            ),
            (
                "config_version = 1\ntitle = \"T\"\nseverity = \"info\"\nstart = \"2026-10-22T18:00:00\"\n",
                "offset",
            ),
            (
                "config_version = 1\ntitle = \"T\"\nseverity = \"info\"\nstart = \"2026-10-22T18:00:00Z\"\nend = \"2026-10-22T17:00:00Z\"\n",
                "before start",
            ),
            (
                "config_version = 1\ntitle = \"T\"\nseverity = \"info\"\nstart = \"2026-10-22T18:00:00Z\"\npages = [\"nope\"]\n",
                "'nope'",
            ),
            (
                "config_version = 1\ntitle = \"\"\nseverity = \"info\"\nstart = \"2026-10-22T18:00:00Z\"\n",
                "title",
            ),
        ] {
            let (i, problems) = parse_text(text);
            let all = format!("{problems:?}");
            assert!(i.is_none(), "accepted: {text}");
            assert!(all.contains(expected), "{text}\n=> {all}");
        }
    }
}
