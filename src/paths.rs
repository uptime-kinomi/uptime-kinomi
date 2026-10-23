//! Configuration directory resolution (SPEC.md §3.1) and default data
//! directories (SPEC.md §10.6).
//!
//! The defaults depend only on the user (HOME, XDG_STATE_HOME, UID), never
//! on how kinomi was started: the systemd service and a manual
//! `uptime-kinomi run` of the same user use the same directories.
//!
//! The functions take the environment as a parameter, so they can be tested
//! without touching the real process environment.

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

/// Default `data_dir`: `$XDG_STATE_HOME/uptime-kinomi`
/// (`~/.local/state/uptime-kinomi`). `None` without a usable `$HOME`.
pub fn default_data_dir(env: EnvFn) -> Option<PathBuf> {
    app_state_dir(env)
}

/// Default `log_dir`: `<data_dir>/log`.
pub fn default_log_dir(env: EnvFn) -> Option<PathBuf> {
    app_state_dir(env).map(|d| d.join("log"))
}

/// Default `output_dir`: `/run/user/<uid>/uptime-kinomi/www` (a tmpfs) if the
/// user's runtime directory exists, else `<data_dir>/www`. `runtime` is the
/// result of [`user_runtime_dir`].
pub fn default_output_dir(env: EnvFn, runtime: Option<&Path>) -> Option<PathBuf> {
    match runtime {
        Some(dir) => Some(dir.join(APP_DIR).join("www")),
        None => app_state_dir(env).map(|d| d.join("www")),
    }
}

/// `/run/user/<uid>` of the current user, if it exists and belongs to us.
/// systemd-logind keeps it from boot to shutdown when lingering is enabled
/// (`loginctl enable-linger`), otherwise while the user is logged in. The
/// UID comes from /proc, not from `$XDG_RUNTIME_DIR`, so it is the same for
/// the service and a login shell.
pub fn user_runtime_dir() -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let uid = current_uid()?;
    let dir = PathBuf::from(format!("/run/user/{uid}"));
    let meta = std::fs::metadata(&dir).ok()?;
    (meta.is_dir() && meta.uid() == uid).then_some(dir)
}

/// Real UID of this process, from /proc/self/status (no libc call).
pub fn current_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
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
    fn service_and_shell_get_the_same_directories() {
        // What systemd sets for a service, and what a login shell has.
        let service = env_of(&[
            ("STATE_DIRECTORY", "/var/lib/uptime-kinomi"),
            ("LOGS_DIRECTORY", "/var/log/uptime-kinomi"),
            ("RUNTIME_DIRECTORY", "/run/user/1000/other"),
            ("HOME", "/home/u"),
        ]);
        let shell = env_of(&[("HOME", "/home/u"), ("XDG_RUNTIME_DIR", "/run/user/1000")]);
        let runtime = Some(Path::new("/run/user/1000"));
        for env in [&service as EnvFn, &shell] {
            assert_eq!(
                default_data_dir(env),
                Some(PathBuf::from("/home/u/.local/state/uptime-kinomi"))
            );
            assert_eq!(
                default_log_dir(env),
                Some(PathBuf::from("/home/u/.local/state/uptime-kinomi/log"))
            );
            assert_eq!(
                default_output_dir(env, runtime),
                Some(PathBuf::from("/run/user/1000/uptime-kinomi/www"))
            );
        }
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
        // Without a runtime directory the pages go below the data directory.
        assert_eq!(
            default_output_dir(&env, None),
            Some(PathBuf::from("/state/uptime-kinomi/www"))
        );
    }

    #[test]
    fn uid_and_runtime_dir() {
        assert!(current_uid().is_some());
        if let Some(dir) = user_runtime_dir() {
            assert!(dir.starts_with("/run/user/"));
        }
    }

    #[test]
    fn default_dirs_unknown_without_home() {
        let env = env_of(&[("STATE_DIRECTORY", "/var/lib/x")]);
        assert_eq!(default_data_dir(&env), None);
        assert_eq!(default_log_dir(&env), None);
        assert_eq!(default_output_dir(&env, None), None);
    }
}
