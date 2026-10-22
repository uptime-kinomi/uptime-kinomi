//! Status pages `pages/*.toml` (SPEC.md §8.1).

use serde::Deserialize;

use super::FileProblems;
use crate::monitors::Monitor;

/// Page IDs that would collide with routes of the built-in server.
const RESERVED_IDS: [&str; 5] = ["assets", "private", "push", "healthz", "metrics"];

#[derive(Debug)]
pub struct Page {
    pub id: String,
    pub title: String,
    pub public: bool,
    pub description: Option<String>,
    pub groups: Vec<Group>,
    #[allow(dead_code)] // incidents (M4)
    pub show_incidents: bool,
    #[allow(dead_code)] // badges (M4)
    pub show_badges: bool,
}

#[derive(Debug)]
pub struct Group {
    pub name: String,
    /// Indices into the configured monitors, in display order.
    pub monitors: Vec<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageFile {
    config_version: u32,
    title: String,
    public: bool,
    description: Option<String>,
    show_incidents: Option<bool>,
    show_badges: Option<bool>,
    groups: Vec<GroupFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupFile {
    name: String,
    #[serde(default)]
    monitors: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
}

pub fn parse(id: &str, text: &str, monitors: &[Monitor], p: &mut FileProblems) -> Option<Page> {
    let raw: PageFile = match toml::from_str(text) {
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
    if RESERVED_IDS.contains(&id) {
        p.error(format!(
            "'{id}' is reserved and cannot be a page ID (rename the file)"
        ));
    }
    let title = raw.title.trim().to_string();
    if title.is_empty() || title.chars().count() > 100 || title.chars().any(char::is_control) {
        p.error("title must be 1 to 100 characters without control characters");
    }
    if raw
        .description
        .as_ref()
        .is_some_and(|d| d.chars().count() > 1000)
    {
        p.error("description must be at most 1000 characters");
    }
    if raw.groups.is_empty() {
        p.error("a page needs at least one [[groups]] entry");
    }

    let mut groups = Vec::new();
    for g in raw.groups {
        let name = g.name.trim().to_string();
        if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
            p.error("group name must be 1 to 100 characters without control characters");
        }
        if g.monitors.is_empty() && g.tags.is_empty() {
            p.error(format!("group '{name}' needs monitors or tags"));
        }
        let mut selected: Vec<usize> = Vec::new();
        let mut add = |i: usize| {
            if !selected.contains(&i) {
                selected.push(i);
            }
        };
        for m in &g.monitors {
            if m == "*" {
                let mut all: Vec<usize> = (0..monitors.len()).collect();
                all.sort_by(|&a, &b| monitors[a].id.cmp(&monitors[b].id));
                all.into_iter().for_each(&mut add);
            } else if let Some(i) = monitors.iter().position(|x| &x.id == m) {
                add(i);
            } else {
                p.error(format!("group '{name}': monitor '{m}' does not exist"));
            }
        }
        for tag in &g.tags {
            let tagged: Vec<usize> = (0..monitors.len())
                .filter(|&i| monitors[i].tags.contains(tag))
                .collect();
            if tagged.is_empty() {
                p.warning(format!("group '{name}': no monitor has tag '{tag}'"));
            }
            tagged.into_iter().for_each(&mut add);
        }
        groups.push(Group {
            name,
            monitors: selected,
        });
    }

    if p.has_errors() {
        return None;
    }
    Some(Page {
        id: id.to_string(),
        title,
        public: raw.public,
        description: raw.description,
        groups,
        show_incidents: raw.show_incidents.unwrap_or(true),
        show_badges: raw.show_badges.unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Problems;
    use crate::monitors::test_monitor;
    use std::path::Path;

    fn monitors() -> Vec<Monitor> {
        let mut web = test_monitor("web");
        web.tags = vec!["family".into()];
        let mut nas = test_monitor("nas");
        nas.tags = vec!["family".into()];
        vec![web, nas, test_monitor("router")]
    }

    fn parse_text(id: &str, text: &str) -> (Option<Page>, Problems) {
        let mut problems = Problems::default();
        let page = parse(
            id,
            text,
            &monitors(),
            &mut problems.for_file(Path::new("p.toml")),
        );
        (page, problems)
    }

    #[test]
    fn spec_example() {
        let (page, problems) = parse_text(
            "home",
            r#"
config_version = 1
title = "Family services"
public = true
description = "Is the internet broken, or just our Wi-Fi?"

[[groups]]
name = "Internet"
monitors = ["router"]

[[groups]]
name = "Home"
tags = ["family"]

[[groups]]
name = "All"
monitors = ["*"]
"#,
        );
        let page = page.unwrap_or_else(|| panic!("{problems:?}"));
        assert!(page.public);
        let groups: Vec<_> = page
            .groups
            .iter()
            .map(|g| (g.name.as_str(), g.monitors.clone()))
            .collect();
        // "*" is sorted by ID: nas, router, web.
        assert_eq!(
            groups,
            [
                ("Internet", vec![2]),
                ("Home", vec![0, 1]),
                ("All", vec![1, 2, 0])
            ]
        );
    }

    #[test]
    fn errors() {
        let group = "[[groups]]\nname = \"G\"\nmonitors = [\"web\"]\n";
        for (id, text, expected) in [
            ("home", format!("config_version = 1\ntitle = \"T\"\n{group}"), "public"),
            ("home", "config_version = 1\ntitle = \"T\"\npublic = true\n".to_string(), "groups"),
            ("home", format!("config_version = 1\ntitle = \"\"\npublic = true\n{group}"), "title"),
            ("private", format!("config_version = 1\ntitle = \"T\"\npublic = true\n{group}"), "reserved"),
            ("home", "config_version = 1\ntitle = \"T\"\npublic = true\n[[groups]]\nname = \"G\"\n".to_string(), "needs monitors or tags"),
            ("home", "config_version = 1\ntitle = \"T\"\npublic = true\n[[groups]]\nname = \"G\"\nmonitors = [\"nope\"]\n".to_string(), "'nope' does not exist"),
            ("home", format!("config_version = 1\ntitle = \"T\"\npublic = true\nbogus = 1\n{group}"), "bogus"),
        ] {
            let (page, problems) = parse_text(id, &text);
            let all = format!("{problems:?}");
            assert!(page.is_none(), "accepted: {text}");
            assert!(all.contains(expected), "{text}\n=> {all}");
        }
    }

    #[test]
    fn unused_tag_is_a_warning() {
        let (page, problems) = parse_text(
            "home",
            "config_version = 1\ntitle = \"T\"\npublic = false\n[[groups]]\nname = \"G\"\ntags = [\"nothing\"]\n",
        );
        assert!(page.is_some());
        assert_eq!(problems.warnings().count(), 1);
    }
}
