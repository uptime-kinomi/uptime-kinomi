//! uptime-kinomi: the SBC-friendly cousin of uptime-kuma.
//!
//! See SPEC.md for the specification and AGENTS.md for the code rules.

#![forbid(unsafe_code)]

/// Like `println!`, but a closed stdout (e.g. `uptime-kinomi ... | head`)
/// is ignored instead of panicking with "Broken pipe".
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

mod clock;
mod config;
mod encoding;
mod heartbeat;
mod history;
mod monitors;
mod notifiers;
mod paths;
mod render;
mod run;
mod scheduler;
mod server;
mod state;
mod storage;
#[cfg(test)]
mod testutil;
mod update;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Exit codes (SPEC.md §10.4).
const EXIT_RUNTIME_ERROR: u8 = 1;
const EXIT_CONFIG_ERROR: u8 = 2;
const EXIT_UPDATE_AVAILABLE: u8 = 3;

const USAGE: &str = "\
Usage: uptime-kinomi [COMMAND] [--config-dir DIR]

Commands:
  run                  Run the monitor (default)
  check-config         Validate the configuration and exit
  rebuild-state        Regenerate state files from the logs
  gen-token            Print a new random push token
  hash-password USER   Print an htpasswd line (bcrypt) for USER
  self-update          Install the latest signed release
  version              Print the version
  help                 Print this help

Options:
  --config-dir DIR     Configuration directory. Default: $KINOMI_CONFIG_DIR,
                       else ~/.config/uptime-kinomi if it exists,
                       else /etc/uptime-kinomi
  -h, --help           Print this help
  -V, --version        Print the version

Options for self-update:
  --check              Only report whether a newer release exists (exit code 3)
  --release X.Y.Z      Install this release instead of the latest
  --allow-downgrade    Allow --release to install an older or equal version
  --rollback           Restore the binary replaced by the last update
";

#[derive(Debug, PartialEq)]
enum Command {
    Run,
    CheckConfig,
    RebuildState,
    GenToken,
    HashPassword(String),
    SelfUpdate(update::Options),
    Version,
    Help,
}

#[derive(Debug, PartialEq)]
struct Args {
    command: Command,
    config_dir: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("error: {msg}\n\n{USAGE}");
            return ExitCode::from(EXIT_CONFIG_ERROR);
        }
    };
    match args.command {
        Command::Help => {
            say!("{}", USAGE.trim_end());
            ExitCode::SUCCESS
        }
        Command::Version => {
            say!("{}", version_string());
            ExitCode::SUCCESS
        }
        Command::CheckConfig => check_config(args.config_dir.as_deref()),
        Command::SelfUpdate(options) => self_update(&options),
        Command::Run => run(args.config_dir.as_deref()),
        Command::RebuildState => rebuild_state(args.config_dir.as_deref()),
        Command::GenToken => gen_token(),
        Command::HashPassword(user) => hash_password(&user),
    }
}

fn parse_args(mut args: impl Iterator<Item = OsString>) -> Result<Args, String> {
    let mut command = None;
    let mut config_dir = None;
    let mut positional = Vec::new();
    let mut update = update::Options::default();
    let mut update_option_seen = false;

    while let Some(arg) = args.next() {
        let Some(text) = arg.to_str() else {
            // Non-UTF-8 is only allowed in the value after `--config-dir`,
            // which is consumed together with the option below.
            return Err(format!("invalid argument {arg:?}"));
        };
        if text == "--config-dir" {
            let dir = args.next().ok_or("--config-dir needs a value")?;
            config_dir = Some(PathBuf::from(dir));
        } else if let Some(dir) = text.strip_prefix("--config-dir=") {
            config_dir = Some(PathBuf::from(dir));
        } else if text == "--release" {
            let value = args.next().ok_or("--release needs a value")?;
            update.release = Some(parse_release(value.to_str().unwrap_or_default())?);
            update_option_seen = true;
        } else if let Some(value) = text.strip_prefix("--release=") {
            update.release = Some(parse_release(value)?);
            update_option_seen = true;
        } else if text == "--check" {
            update.check = true;
            update_option_seen = true;
        } else if text == "--allow-downgrade" {
            update.allow_downgrade = true;
            update_option_seen = true;
        } else if text == "--rollback" {
            update.rollback = true;
            update_option_seen = true;
        } else if text == "-h" || text == "--help" {
            command = Some(Command::Help);
        } else if text == "-V" || text == "--version" {
            command = Some(Command::Version);
        } else if text.starts_with('-') {
            return Err(format!("unknown option '{text}'"));
        } else {
            positional.push(text.to_string());
        }
    }

    let command = match command {
        Some(command) => command,
        None => parse_command(&positional)?,
    };
    let command = match command {
        Command::SelfUpdate(_) => {
            validate_update_options(&update)?;
            Command::SelfUpdate(update)
        }
        Command::Help | Command::Version => command,
        _ if update_option_seen => {
            return Err("--check, --release, --allow-downgrade and --rollback \
                        are only valid for self-update"
                .into());
        }
        _ => command,
    };
    Ok(Args {
        command,
        config_dir,
    })
}

fn parse_command(positional: &[String]) -> Result<Command, String> {
    let (name, rest) = match positional.split_first() {
        None => return Ok(Command::Run),
        Some((name, rest)) => (name.as_str(), rest),
    };
    let command = match (name, rest) {
        ("run", []) => Command::Run,
        ("check-config", []) => Command::CheckConfig,
        ("rebuild-state", []) => Command::RebuildState,
        ("gen-token", []) => Command::GenToken,
        // Options are filled in by parse_args.
        ("self-update", []) => Command::SelfUpdate(update::Options::default()),
        ("hash-password", [user]) => Command::HashPassword(user.clone()),
        ("hash-password", _) => return Err("hash-password needs exactly one USER".into()),
        ("version", []) => Command::Version,
        ("help", _) => Command::Help,
        (
            "run" | "check-config" | "rebuild-state" | "gen-token" | "self-update" | "version",
            [extra, ..],
        ) => {
            return Err(format!("unexpected argument '{extra}'"));
        }
        (other, _) => return Err(format!("unknown command '{other}'")),
    };
    Ok(command)
}

fn parse_release(text: &str) -> Result<update::Version, String> {
    update::Version::parse(text).ok_or_else(|| format!("invalid release '{text}', expected X.Y.Z"))
}

fn validate_update_options(update: &update::Options) -> Result<(), String> {
    if update.rollback && (update.check || update.release.is_some() || update.allow_downgrade) {
        return Err("--rollback cannot be combined with other self-update options".into());
    }
    if update.allow_downgrade && update.release.is_none() {
        return Err("--allow-downgrade needs --release X.Y.Z".into());
    }
    Ok(())
}

fn self_update(options: &update::Options) -> ExitCode {
    match update::run(options) {
        Ok(update::Outcome::UpdateAvailable) => ExitCode::from(EXIT_UPDATE_AVAILABLE),
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(EXIT_RUNTIME_ERROR)
        }
    }
}

fn version_string() -> String {
    // Format relied upon by self-update's test run: "uptime-kinomi X.Y.Z (target)".
    format!(
        "uptime-kinomi {} ({})",
        env!("CARGO_PKG_VERSION"),
        env!("KINOMI_TARGET")
    )
}

fn run(cli_dir: Option<&Path>) -> ExitCode {
    let env = |key: &str| std::env::var_os(key);
    let dir = paths::config_dir(cli_dir, &env, &|p| p.is_dir());
    let (config, problems) = config::load(&dir, &env);
    for problem in problems.all() {
        eprintln!("{problem}");
    }
    let Some(config) = config else {
        eprintln!(
            "configuration in {} is invalid; run 'uptime-kinomi check-config'",
            dir.display()
        );
        return ExitCode::from(EXIT_CONFIG_ERROR);
    };
    match run::run(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(EXIT_RUNTIME_ERROR)
        }
    }
}

fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; N];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "no random numbers available from the operating system".to_string())?;
    Ok(bytes)
}

/// `gen-token`: 32 random bytes as base64url (43 characters, SPEC.md §4.6).
fn gen_token() -> ExitCode {
    match random_bytes::<32>() {
        Ok(bytes) => {
            say!("{}", encoding::base64url(&bytes));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(EXIT_RUNTIME_ERROR)
        }
    }
}

/// `hash-password USER`: print an htpasswd line (SPEC.md §9.3). The password
/// is read from stdin if it is not a terminal; otherwise a random password is
/// generated and shown once on stderr.
fn hash_password(user: &str) -> ExitCode {
    use std::io::{BufRead, IsTerminal};
    let valid_user = !user.is_empty()
        && user.len() <= 64
        && !user.contains(':')
        && !user.chars().any(char::is_control);
    if !valid_user {
        eprintln!("error: USER must be 1 to 64 characters without ':'");
        return ExitCode::from(EXIT_CONFIG_ERROR);
    }
    let password = if std::io::stdin().is_terminal() {
        let Ok(bytes) = random_bytes::<15>() else {
            eprintln!("error: no random numbers available");
            return ExitCode::from(EXIT_RUNTIME_ERROR);
        };
        let password = encoding::base64url(&bytes);
        eprintln!("password for {user}: {password}");
        password
    } else {
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line).is_err() {
            eprintln!("error: cannot read the password from stdin");
            return ExitCode::from(EXIT_RUNTIME_ERROR);
        }
        let password = line.trim_end_matches(['\r', '\n']).to_string();
        if password.is_empty() || password.len() > 72 {
            eprintln!("error: the password must be 1 to 72 bytes (bcrypt limit)");
            return ExitCode::from(EXIT_CONFIG_ERROR);
        }
        password
    };
    match random_bytes::<16>() {
        Ok(salt) => {
            say!(
                "{}",
                server::auth::htpasswd_line(user, password.as_bytes(), salt)
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(EXIT_RUNTIME_ERROR)
        }
    }
}

fn rebuild_state(cli_dir: Option<&Path>) -> ExitCode {
    let env = |key: &str| std::env::var_os(key);
    let dir = paths::config_dir(cli_dir, &env, &|p| p.is_dir());
    let (config, problems) = config::load(&dir, &env);
    for problem in problems.all() {
        eprintln!("{problem}");
    }
    let Some(config) = config else {
        return ExitCode::from(EXIT_CONFIG_ERROR);
    };
    eprintln!("note: stop the service first, or it will overwrite the rebuilt files");
    match run::rebuild_state(&config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(EXIT_RUNTIME_ERROR)
        }
    }
}

fn check_config(cli_dir: Option<&Path>) -> ExitCode {
    let env = |key: &str| std::env::var_os(key);
    let dir = paths::config_dir(cli_dir, &env, &|p| p.is_dir());
    let (config, problems) = config::load(&dir, &env);
    for problem in problems.all() {
        eprintln!("{problem}");
    }
    let Some(config) = config else {
        eprintln!("configuration in {} is invalid", dir.display());
        return ExitCode::from(EXIT_CONFIG_ERROR);
    };
    let g = &config.global;
    say!("config dir: {}", config.dir.display());
    say!("data dir:   {}", g.storage.data_dir.display());
    say!("log dir:    {}", g.storage.log_dir.display());
    say!("output dir: {}", g.pages.output_dir.display());
    say!("monitors:   {}", config.monitors.len());
    for m in &config.monitors {
        let state = if m.enabled { "" } else { " (disabled)" };
        let every = config::duration::Dur(m.interval).to_string();
        say!(
            "  {:<24} {:<5} every {every:>5}  {}{state}",
            m.id,
            m.kind.name(),
            m.check.target()
        );
    }
    let names = |ids: Vec<&str>| {
        if ids.is_empty() {
            String::new()
        } else {
            format!(" ({})", ids.join(", "))
        }
    };
    say!(
        "pages:      {}{}",
        config.pages.len(),
        names(config.pages.iter().map(|p| p.id.as_str()).collect())
    );
    say!(
        "notifiers:  {}{}",
        config.notifiers.len(),
        names(config.notifiers.iter().map(|n| n.id.as_str()).collect())
    );
    say!("incidents:  {}", config.incidents.len());
    say!("maintenance windows: {}", config.maintenance.len());
    say!("configuration OK");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(OsString::from))
    }

    fn command(args: &[&str]) -> Command {
        parse(args).unwrap().command
    }

    #[test]
    fn no_arguments_means_run() {
        assert_eq!(command(&[]), Command::Run);
    }

    #[test]
    fn commands() {
        assert_eq!(command(&["run"]), Command::Run);
        assert_eq!(command(&["check-config"]), Command::CheckConfig);
        assert_eq!(command(&["rebuild-state"]), Command::RebuildState);
        assert_eq!(command(&["gen-token"]), Command::GenToken);
        assert_eq!(command(&["version"]), Command::Version);
        assert_eq!(command(&["help"]), Command::Help);
        assert_eq!(
            command(&["hash-password", "admin"]),
            Command::HashPassword("admin".into())
        );
    }

    #[test]
    fn flags_override_command() {
        assert_eq!(command(&["run", "--help"]), Command::Help);
        assert_eq!(command(&["-V"]), Command::Version);
    }

    #[test]
    fn config_dir_before_or_after_command() {
        let expected = Args {
            command: Command::CheckConfig,
            config_dir: Some(PathBuf::from("/cfg")),
        };
        assert_eq!(
            parse(&["--config-dir", "/cfg", "check-config"]).unwrap(),
            expected
        );
        assert_eq!(
            parse(&["check-config", "--config-dir=/cfg"]).unwrap(),
            expected
        );
    }

    #[test]
    fn self_update_options() {
        assert_eq!(
            command(&["self-update"]),
            Command::SelfUpdate(update::Options::default())
        );
        assert_eq!(
            command(&["self-update", "--check"]),
            Command::SelfUpdate(update::Options {
                check: true,
                ..Default::default()
            })
        );
        assert_eq!(
            command(&["self-update", "--release", "1.2.3", "--allow-downgrade"]),
            Command::SelfUpdate(update::Options {
                release: update::Version::parse("1.2.3"),
                allow_downgrade: true,
                ..Default::default()
            })
        );
        assert_eq!(
            command(&["self-update", "--rollback"]),
            Command::SelfUpdate(update::Options {
                rollback: true,
                ..Default::default()
            })
        );
    }

    #[test]
    fn self_update_errors() {
        assert!(parse(&["run", "--check"]).is_err());
        assert!(parse(&["self-update", "--allow-downgrade"]).is_err());
        assert!(parse(&["self-update", "--rollback", "--check"]).is_err());
        assert!(parse(&["self-update", "--release"]).is_err());
        for bad in ["1.2", "1.2.3.4", "v1.2.3", "1.2.x", "1..3", "1.2.3-rc1"] {
            assert!(parse(&["self-update", "--release", bad]).is_err(), "{bad}");
        }
    }

    #[test]
    fn errors() {
        assert!(parse(&["--config-dir"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["bogus"]).is_err());
        assert!(parse(&["run", "extra"]).is_err());
        assert!(parse(&["hash-password"]).is_err());
        assert!(parse(&["hash-password", "a", "b"]).is_err());
    }
}
