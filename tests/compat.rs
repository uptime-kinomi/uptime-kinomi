//! Upgrades: the current version must read the log and state files that
//! earlier releases wrote (SPEC.md §16). testdata/compat/<version>/ holds
//! them, made by testdata/compat/generate.sh with the real release.

use std::collections::HashMap;
use std::path::Path;

use uptime_kinomi::history::History;
use uptime_kinomi::state::State;
use uptime_kinomi::storage::{replay, statefile};

const MONITORS: [&str; 5] = ["web", "port", "closed", "old", "job"];

fn histories() -> HashMap<String, History> {
    MONITORS
        .iter()
        .map(|id| (id.to_string(), History::new(50, 400)))
        .collect()
}

#[test]
fn data_of_earlier_releases_is_read() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/compat");
    let mut versions: Vec<_> = std::fs::read_dir(&root)
        .expect("testdata/compat")
        .map(|e| e.expect("directory entry").path())
        .filter(|p| p.is_dir())
        .collect();
    versions.sort();
    assert!(versions.len() >= 5, "testdata/compat has too few releases");

    for dir in versions {
        let name = dir.display();
        // As at startup: state files, then the log after them.
        let mut restored = histories();
        for (id, history) in restored.iter_mut() {
            let warning = statefile::load(&dir, id, history);
            assert_eq!(warning, None, "{name}: state file of {id}");
        }
        replay::replay(&dir.join("log"), &mut restored).expect("replay");
        // As rebuild-state: the log alone.
        let mut rebuilt = histories();
        replay::replay(&dir.join("log"), &mut rebuilt).expect("replay");

        let has_push = dir.join("state/job.toml").exists();
        for id in MONITORS {
            let (a, b) = (&restored[id], &rebuilt[id]);
            let expected = match id {
                "web" | "port" => State::Up,
                "closed" => State::Down,
                "job" if has_push => State::Down,
                _ => State::Unknown,
            };
            assert_eq!(a.state, expected, "{name}: {id}");
            assert_eq!(b.state, expected, "{name}: {id} rebuilt");
            assert_eq!(a.last_check, b.last_check, "{name}: {id}");
            assert_eq!(a.last_msg, b.last_msg, "{name}: {id}");
            assert_eq!(a.recent.len(), b.recent.len(), "{name}: {id}");
            // State files before 0.3.0 had no `changes`: the feed starts
            // empty after such an upgrade, nothing else is lost.
            let state_file =
                std::fs::read_to_string(dir.join(format!("state/{id}.toml"))).unwrap_or_default();
            if state_file.contains("\nchanges = [") {
                assert_eq!(a.changes, b.changes, "{name}: {id}");
            }
            if expected != State::Unknown {
                assert!(a.recent.len() >= 2, "{name}: {id} has too few results");
                assert!(!a.daily.is_empty(), "{name}: {id} has no daily totals");
            }
        }
    }
}
