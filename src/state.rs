//! Monitor state machine (SPEC.md §4.2).
//!
//! UNKNOWN → UP/WARN on success; on failure PENDING until `retries`
//! consecutive failures are exceeded, then DOWN.

use crate::monitors::Status;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Unknown,
    Up,
    Warn,
    Pending,
    Down,
    /// Inside a maintenance window (shown only; the state machine keeps
    /// its own state, SPEC.md §14.2).
    Maintenance,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Unknown => "UNKNOWN",
            State::Up => "UP",
            State::Warn => "WARN",
            State::Pending => "PENDING",
            State::Down => "DOWN",
            State::Maintenance => "MAINTENANCE",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [
            State::Unknown,
            State::Up,
            State::Warn,
            State::Pending,
            State::Down,
            State::Maintenance,
        ]
        .into_iter()
        .find(|s| s.name() == text)
    }
}

/// Value of `result=` in the check log (SPEC.md §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogResult {
    Up,
    Warn,
    /// Failed, retries left (state PENDING).
    Fail,
    Down,
    /// Any result inside a maintenance window.
    Maint,
}

impl LogResult {
    pub fn name(self) -> &'static str {
        match self {
            LogResult::Up => "UP",
            LogResult::Warn => "WARN",
            LogResult::Fail => "FAIL",
            LogResult::Down => "DOWN",
            LogResult::Maint => "MAINT",
        }
    }
}

#[derive(Debug)]
pub struct MonitorState {
    pub state: State,
    failures: u32,
    retries: u32,
}

impl MonitorState {
    #[cfg(test)]
    pub fn new(retries: u32) -> Self {
        MonitorState {
            state: State::Unknown,
            failures: 0,
            retries,
        }
    }

    /// Continue from a state restored from a state file or the log.
    /// `failures`: the FAIL results in a row at the end of the history.
    pub fn restore(retries: u32, state: State, failures: u32) -> Self {
        let failures = match state {
            // The next failure keeps it DOWN instead of starting over.
            State::Down => retries.saturating_add(1),
            // PENDING continues counting towards DOWN.
            State::Pending => failures.clamp(1, retries.max(1)),
            _ => 0,
        };
        // MAINTENANCE is only shown; the state machine starts over.
        let state = if state == State::Maintenance {
            State::Unknown
        } else {
            state
        };
        MonitorState {
            state,
            failures,
            retries,
        }
    }

    /// Apply a check outcome. Returns the log result and, if the state
    /// changed, the previous state.
    pub fn apply(&mut self, status: Status) -> (LogResult, Option<State>) {
        let previous = self.state;
        let result = match status {
            Status::Up => {
                self.failures = 0;
                self.state = State::Up;
                LogResult::Up
            }
            Status::Warn => {
                self.failures = 0;
                self.state = State::Warn;
                LogResult::Warn
            }
            Status::Fail => {
                self.failures = self.failures.saturating_add(1);
                if self.failures > self.retries || previous == State::Down {
                    self.state = State::Down;
                    LogResult::Down
                } else {
                    self.state = State::Pending;
                    LogResult::Fail
                }
            }
        };
        (result, (previous != self.state).then_some(previous))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_continues_counting_after_a_restart() {
        // retries = 3: DOWN on the 4th failure. Two failures happened before
        // the restart, so the 4th is two checks away, not four.
        let mut m = MonitorState::restore(3, State::Pending, 2);
        assert_eq!(m.apply(Status::Fail).0, LogResult::Fail);
        assert_eq!(m.apply(Status::Fail).0, LogResult::Down);
        // Unknown count (old history): at least the one failure PENDING means.
        let mut m = MonitorState::restore(1, State::Pending, 0);
        assert_eq!(m.apply(Status::Fail).0, LogResult::Down);
    }

    #[test]
    fn retries_then_down() {
        let mut s = MonitorState::new(2);
        assert_eq!(s.apply(Status::Up), (LogResult::Up, Some(State::Unknown)));
        assert_eq!(s.apply(Status::Fail), (LogResult::Fail, Some(State::Up)));
        assert_eq!(s.apply(Status::Fail), (LogResult::Fail, None));
        assert_eq!(
            s.apply(Status::Fail),
            (LogResult::Down, Some(State::Pending))
        );
        assert_eq!(s.apply(Status::Fail), (LogResult::Down, None));
        assert_eq!(s.apply(Status::Warn), (LogResult::Warn, Some(State::Down)));
        assert_eq!(s.apply(Status::Up), (LogResult::Up, Some(State::Warn)));
    }

    #[test]
    fn no_retries_is_down_at_once() {
        let mut s = MonitorState::new(0);
        assert_eq!(
            s.apply(Status::Fail),
            (LogResult::Down, Some(State::Unknown))
        );
    }

    #[test]
    fn success_resets_failures() {
        let mut s = MonitorState::new(1);
        s.apply(Status::Fail);
        s.apply(Status::Up);
        assert_eq!(s.apply(Status::Fail), (LogResult::Fail, Some(State::Up)));
        assert_eq!(s.state, State::Pending);
    }
}
