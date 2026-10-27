//! uptime-kinomi as a library: everything except the command line, which
//! lives in main.rs. The library exists so that the parsers can be fuzzed
//! (fuzz/, SPEC.md §11); it is not a stable API.

#![forbid(unsafe_code)]

/// Like `println!`, but a closed stdout (e.g. `uptime-kinomi ... | head`)
/// is ignored instead of panicking with "Broken pipe".
#[macro_export]
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

pub mod clock;
pub mod config;
pub mod encoding;
pub mod heartbeat;
pub mod history;
pub mod monitors;
pub mod notifiers;
pub mod paths;
pub mod render;
pub mod run;
pub mod scheduler;
pub mod server;
pub mod state;
pub mod storage;
#[cfg(test)]
mod testutil;
pub mod update;

#[doc(hidden)]
pub mod fuzzing;
