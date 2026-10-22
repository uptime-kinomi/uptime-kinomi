//! Configuration directory resolution (SPEC.md §3.1) and default data
//! directories (SPEC.md §10.6).
//!
//! The functions take the environment and the file system check as
//! parameters, so they can be tested without touching the real process
//! environment.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Name of the application directory below XDG base directories.
const APP_DIR: &str = "uptime-kinomi";

/// Configuration directory of a system-wide installation.
const SYSTEM_CONFIG_DIR: &str = "/etc/uptime-kinomi";

/// Environment variable that overrides the configuration directory.
const KINOMI_CONFIG_DIR: &str = "KINOMI_CONFIG_DIR";

/// Environment lookup, e.g. `std::env::var_os`.
pub type EnvFn<'a> = &'a dyn Fn(&str) -> Option<OsString>;

/// Find the configuration directory. First match wins:
/// `--config-dir`, `$KINOMI_CONFIG_DIR`, `$XDG_CONFIG_HOME/uptime-kinomi`
/// (only if it exists), `/etc/uptime-kinomi`.
pub fn config_dir(cli: Option<&Path>, env: EnvFn, is_dir: &dyn Fn(&Path) -> bool) -> PathBuf {
    if let Some(dir) = cli {
        return dir.to_path_buf();
    }
    if let Some(dir) = env(KINOMI_CONFIG_DIR).filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(dir) = xdg_config_home(env).map(|d| d.join(APP_DIR)) {
        if is_dir(&dir) {
            return dir;
        }
    }
    PathBuf::from(SYSTEM_CONFIG_DIR)
}

/// Default `data_dir`: `$STATE_DIRECTORY` (systemd), else
/// `$XDG_STATE_HOME/uptime-kinomi`. `None` if neither can be determined.
pub fn default_data_dir(env: EnvFn) -> Option<PathBuf> {
    systemd_dir(env, "STATE_DIRECTORY").or_else(|| app_state_dir(env))
}

/// Default `log_dir`: `$LOGS_DIRECTORY` (systemd), else
/// `$XDG_STATE_HOME/uptime-kinomi/log`.
pub fn default_log_dir(env: EnvFn) -> Option<PathBuf> {
    systemd_dir(env, "LOGS_DIRECTORY").or_else(|| app_state_dir(env).map(|d| d.join("log")))
}

/// Default `output_dir`: `$RUNTIME_DIRECTORY/www` (systemd), else
/// `$XDG_STATE_HOME/uptime-kinomi/www`.
pub fn default_output_dir(env: EnvFn) -> Option<PathBuf> {
    systemd_dir(env, "RUNTIME_DIRECTORY")
        .map(|d| d.join("www"))
        .or_else(|| app_state_dir(env).map(|d| d.join("www")))
}

/// systemd passes `StateDirectory=a b` as `/var/lib/a:/var/lib/b`; we use the first.
fn systemd_dir(env: EnvFn, key: &str) -> Option<PathBuf> {
    let value = env(key)?;
    let first = value.to_str()?.split(':').next()?;
    absolute(PathBuf::from(first))
}

fn app_state_dir(env: EnvFn) -> Option<PathBuf> {
    xdg_dir(env, "XDG_STATE_HOME", ".local/state").map(|d| d.join(APP_DIR))
}

fn xdg_config_home(env: EnvFn) -> Option<PathBuf> {
    xdg_dir(env, "XDG_CONFIG_HOME", ".config")
}

/// XDG base directory with fallback below `$HOME`. The XDG spec says that
/// empty and relative values must be ignored.
fn xdg_dir(env: EnvFn, key: &str, home_fallback: &str) -> Option<PathBuf> {
    env(key)
        .and_then(|v| absolute(PathBuf::from(v)))
        .or_else(|| {
            env("HOME")
                .and_then(|h| absolute(PathBuf::from(h)))
                .map(|h| h.join(home_fallback))
        })
}

fn absolute(path: PathBuf) -> Option<PathBuf> {
    path.is_absolute().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |key| map.get(key).cloned()
    }

    fn never_dir(_: &Path) -> bool {
        false
    }

    fn always_dir(_: &Path) -> bool {
        true
    }

    #[test]
    fn config_dir_cli_wins() {
        let env = env_of(&[("KINOMI_CONFIG_DIR", "/env"), ("HOME", "/home/u")]);
        let dir = config_dir(Some(Path::new("/cli")), &env, &always_dir);
        assert_eq!(dir, PathBuf::from("/cli"));
    }

    #[test]
    fn config_dir_env_before_xdg() {
        let env = env_of(&[("KINOMI_CONFIG_DIR", "/env"), ("HOME", "/home/u")]);
        assert_eq!(config_dir(None, &env, &always_dir), PathBuf::from("/env"));
    }

    #[test]
    fn config_dir_empty_env_is_ignored() {
        let env = env_of(&[("KINOMI_CONFIG_DIR", ""), ("HOME", "/home/u")]);
        assert_eq!(
            config_dir(None, &env, &always_dir),
            PathBuf::from("/home/u/.config/uptime-kinomi")
        );
    }

    #[test]
    fn config_dir_xdg_only_if_it_exists() {
        let env = env_of(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/u")]);
        assert_eq!(
            config_dir(None, &env, &always_dir),
            PathBuf::from("/xdg/uptime-kinomi")
        );
        assert_eq!(
            config_dir(None, &env, &never_dir),
            PathBuf::from("/etc/uptime-kinomi")
        );
    }

    #[test]
    fn config_dir_relative_xdg_is_ignored() {
        let env = env_of(&[("XDG_CONFIG_HOME", "relative"), ("HOME", "/home/u")]);
        assert_eq!(
            config_dir(None, &env, &always_dir),
            PathBuf::from("/home/u/.config/uptime-kinomi")
        );
    }

    #[test]
    fn config_dir_without_home_falls_back_to_etc() {
        let env = env_of(&[]);
        assert_eq!(
            config_dir(None, &env, &always_dir),
            PathBuf::from("/etc/uptime-kinomi")
        );
    }

    #[test]
    fn default_dirs_from_systemd() {
        let env = env_of(&[
            ("STATE_DIRECTORY", "/var/lib/uptime-kinomi:/var/lib/other"),
            ("LOGS_DIRECTORY", "/var/log/uptime-kinomi"),
            ("RUNTIME_DIRECTORY", "/run/uptime-kinomi"),
            ("HOME", "/home/u"),
        ]);
        assert_eq!(
            default_data_dir(&env),
            Some(PathBuf::from("/var/lib/uptime-kinomi"))
        );
        assert_eq!(
            default_log_dir(&env),
            Some(PathBuf::from("/var/log/uptime-kinomi"))
        );
        assert_eq!(
            default_output_dir(&env),
            Some(PathBuf::from("/run/uptime-kinomi/www"))
        );
    }

    #[test]
    fn default_dirs_from_xdg_state_home() {
        let env = env_of(&[("XDG_STATE_HOME", "/state"), ("HOME", "/home/u")]);
        assert_eq!(
            default_data_dir(&env),
            Some(PathBuf::from("/state/uptime-kinomi"))
        );
        assert_eq!(
            default_log_dir(&env),
            Some(PathBuf::from("/state/uptime-kinomi/log"))
        );
        assert_eq!(
            default_output_dir(&env),
            Some(PathBuf::from("/state/uptime-kinomi/www"))
        );
    }

    #[test]
    fn default_dirs_from_home() {
        let env = env_of(&[("HOME", "/home/u")]);
        assert_eq!(
            default_data_dir(&env),
            Some(PathBuf::from("/home/u/.local/state/uptime-kinomi"))
        );
    }

    #[test]
    fn default_dirs_unknown_without_home() {
        let env = env_of(&[("STATE_DIRECTORY", "relative")]);
        assert_eq!(default_data_dir(&env), None);
        assert_eq!(default_log_dir(&env), None);
        assert_eq!(default_output_dir(&env), None);
    }
}
