//! `command` monitor: run a local program and use its exit code, following
//! the Nagios plugin convention (SPEC.md §4.8). Only programs allowed in
//! `kinomi.toml` run, without a shell, with a cleared environment, in their
//! own process group. The output is not read unless `keep_output` is set;
//! then it is copied unparsed into a file that is only offered as a download.

use std::fs::{self, DirBuilder, File};
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use rustix::process::{Pid, Signal, WaitId, WaitIdOptions};
use serde::Deserialize;

use super::{Check, CheckResult, Context};
use crate::clock;
use crate::config::FileProblems;
use crate::config::global::Allowed;
use crate::paths;
use crate::storage::statefile::{Durability, write_atomic};

const MAX_ARGS: usize = 32;
const MAX_ARG_BYTES: usize = 1024;
const PATH: &str = "/usr/local/bin:/usr/bin:/bin";
/// How often a running program is looked at.
const POLL: Duration = Duration::from_millis(20);
/// How long the end of the output may take after the program ended.
const OUTPUT_GRACE: Duration = Duration::from_secs(1);
/// Below `output_dir`; not a valid page ID, so never a page.
pub const OUTPUT_DIR: &str = "private/_output";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandFile {
    program: PathBuf,
    #[serde(default)]
    args: Vec<String>,
    keep_output: Option<bool>,
}

pub struct CommandCheck {
    program: PathBuf,
    args: Vec<String>,
    home: Option<String>,
    timeout: Duration,
    output: Option<Output>,
}

/// `keep_output = true`: where the output of the last run goes.
struct Output {
    file: PathBuf,
    max_bytes: u64,
}

pub fn build(
    id: &str,
    file: CommandFile,
    timeout: Duration,
    ctx: &Context,
    p: &mut FileProblems,
) -> Option<CommandCheck> {
    let Some(commands) = ctx.commands.filter(|c| !c.allow.is_empty()) else {
        p.error(
            "command monitors need [commands] allow in kinomi.toml, listing the programs they may run",
        );
        return None;
    };
    if paths::running_as_root() {
        p.error("command monitors are not run as root; run kinomi as an ordinary user");
    }
    let path = crate::config::resolve_path(ctx.config_dir, &file.program);
    let program = match fs::canonicalize(&path) {
        Ok(real) => real,
        Err(_) => {
            p.error(format!("command.program {} does not exist", path.display()));
            return None;
        }
    };
    let allowed = commands.allow.iter().any(|a| match a {
        Allowed::Program(allowed) => *allowed == program,
        Allowed::Dir(dir) => program.parent() == Some(dir.as_path()),
    });
    if !allowed {
        p.error(format!(
            "command.program {} is not allowed by [commands] allow in kinomi.toml",
            program.display()
        ));
    }
    match fs::metadata(&program) {
        Ok(meta) if meta.is_file() && meta.permissions().mode() & 0o111 != 0 => {}
        _ => p.error(format!(
            "command.program {} is not an executable file",
            program.display()
        )),
    }
    if let Some(unsafe_path) = writable_by_others(&program) {
        p.error(format!(
            "command.program: {} can be changed by other users (owner must be root or you, \
             not writable by group or others)",
            unsafe_path.display()
        ));
    }
    if file.args.len() > MAX_ARGS {
        p.error(format!("command.args: at most {MAX_ARGS} arguments"));
    }
    if file
        .args
        .iter()
        .any(|a| a.len() > MAX_ARG_BYTES || a.contains('\0'))
    {
        p.error(format!(
            "command.args: each argument at most {MAX_ARG_BYTES} bytes, without NUL"
        ));
    }
    let output = file.keep_output.unwrap_or(false).then(|| Output {
        file: ctx
            .output_dir
            .unwrap_or(Path::new("/nonexistent"))
            .join(OUTPUT_DIR)
            .join(format!("{id}.txt")),
        max_bytes: commands.output_max_bytes,
    });
    Some(CommandCheck {
        program,
        args: file.args,
        home: std::env::var("HOME").ok(),
        timeout,
        output,
    })
}

/// The first of `path` and its parent directories that another user could
/// change: owned by someone other than root or us, or writable by group or
/// others (like OpenSSH's StrictModes).
fn writable_by_others(path: &Path) -> Option<PathBuf> {
    let uid = paths::current_uid();
    path.ancestors().find_map(|p| match fs::metadata(p) {
        Ok(meta) => {
            let owner_ok = meta.uid() == 0 || Some(meta.uid()) == uid;
            (!owner_ok || meta.mode() & 0o022 != 0).then(|| p.to_path_buf())
        }
        Err(_) => Some(p.to_path_buf()),
    })
}

impl Check for CommandCheck {
    fn check(&self) -> CheckResult {
        let start = Instant::now();
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .env_clear()
            .env("PATH", PATH)
            .env("LC_ALL", "C")
            .current_dir("/")
            .stdin(Stdio::null())
            .process_group(0);
        if let Some(home) = &self.home {
            command.env("HOME", home);
        }
        let reader = match &self.output {
            None => {
                command.stdout(Stdio::null()).stderr(Stdio::null());
                None
            }
            Some(output) => match pipe_into(&mut command, output.max_bytes) {
                Some(reader) => Some(reader),
                None => return CheckResult::fail(Some(start.elapsed()), "cannot start program"),
            },
        };
        let spawned = command.spawn();
        // Command holds the write end of the pipe: closing it lets the
        // reader see the end of the output once the program is done.
        drop(command);
        let mut child = match spawned {
            Ok(child) => child,
            Err(_) => return CheckResult::fail(Some(start.elapsed()), "cannot start program"),
        };
        let finished = wait(&mut child, self.timeout);
        let elapsed = start.elapsed();
        let result = match finished {
            None => CheckResult::fail(Some(elapsed), "timeout"),
            Some(status) => match (status.code(), status.signal()) {
                (Some(0), _) => CheckResult::up(elapsed, "exit 0"),
                (Some(1), _) => CheckResult::warn(elapsed, "exit 1 (warning)"),
                (Some(2), _) => CheckResult::fail(Some(elapsed), "exit 2 (critical)"),
                (Some(3), _) => CheckResult::fail(Some(elapsed), "exit 3 (unknown)"),
                (Some(code), _) => CheckResult::fail(Some(elapsed), format!("exit {code}")),
                (None, Some(signal)) => {
                    CheckResult::fail(Some(elapsed), format!("killed by signal {signal}"))
                }
                (None, None) => CheckResult::fail(Some(elapsed), "cannot start program"),
            },
        };
        if let (Some(output), Some(reader)) = (&self.output, reader) {
            // The process group is gone, so the pipe is closed and the
            // reader ends. A process that left the group (setsid) could
            // keep it open: then the output is not saved, and the check
            // does not wait for it.
            let grace = Instant::now();
            while !reader.is_finished() && grace.elapsed() < OUTPUT_GRACE {
                thread::sleep(POLL);
            }
            if reader.is_finished() {
                let (bytes, truncated) = reader.join().unwrap_or_default();
                save_output(output, bytes, truncated, &result, elapsed);
            }
        }
        result
    }

    fn target(&self) -> String {
        self.program.display().to_string()
    }

    fn output_file(&self) -> Option<&Path> {
        self.output.as_ref().map(|o| o.file.as_path())
    }
}

/// stdout and stderr of `command` into one pipe, read by a thread that
/// keeps at most `max` bytes and discards the rest (so the program never
/// blocks on a full pipe). Returns the output and whether it was cut.
fn pipe_into(command: &mut Command, max: u64) -> Option<thread::JoinHandle<(Vec<u8>, bool)>> {
    let (read, write) = rustix::pipe::pipe().ok()?;
    let write2 = write.try_clone().ok()?;
    command
        .stdout(Stdio::from(write))
        .stderr(Stdio::from(write2));
    let mut read = File::from(read);
    thread::Builder::new()
        .name("command-output".into())
        .spawn(move || {
            let mut kept = Vec::new();
            let _ = (&mut read).take(max).read_to_end(&mut kept);
            let rest = std::io::copy(&mut read, &mut std::io::sink()).unwrap_or(0);
            (kept, rest > 0)
        })
        .ok()
}

/// Wait for the program until `timeout`. Whatever is left of its process
/// group is killed afterwards, and at the timeout the whole group is. The
/// group is killed before the program is reaped, so its ID cannot have
/// been reused by then. `None`: timed out.
fn wait(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    let pid = i32::try_from(child.id()).ok().and_then(Pid::from_raw);
    let exited = loop {
        // Exited (a zombie, still holding its ID), without reaping it.
        let done = match pid {
            Some(pid) => rustix::process::waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )
            .map(|s| s.is_some())
            .unwrap_or(true),
            None => true,
        };
        if done {
            break true;
        }
        let left = timeout.saturating_sub(start.elapsed());
        if left.is_zero() {
            break false;
        }
        thread::sleep(left.min(POLL));
    };
    if let Some(pid) = pid {
        let _ = rustix::process::kill_process_group(pid, Signal::KILL);
    }
    let status = child.wait().ok();
    if exited { status } else { None }
}

fn save_output(
    output: &Output,
    mut bytes: Vec<u8>,
    truncated: bool,
    result: &CheckResult,
    elapsed: Duration,
) {
    let now = clock::format_rfc3339(SystemTime::now());
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    if truncated {
        bytes.extend_from_slice(
            format!("# truncated after {} bytes\n", output.max_bytes).as_bytes(),
        );
    }
    bytes.extend_from_slice(
        format!(
            "# uptime-kinomi: {} after {:.1} s, {now}\n",
            result.msg,
            elapsed.as_secs_f64()
        )
        .as_bytes(),
    );
    if let Some(dir) = output.file.parent() {
        let _ = DirBuilder::new().recursive(true).mode(0o750).create(dir);
    }
    let _ = write_atomic(&output.file, &bytes, 0o640, Durability::Regenerated);
}

/// Remove kept output files of monitors that no longer keep their output
/// (`keep`: their file names).
pub fn remove_stale_outputs(output_dir: &Path, keep: &[&Path]) {
    let Ok(entries) = fs::read_dir(output_dir.join(OUTPUT_DIR)) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ours = path.extension().is_some_and(|e| e == "txt")
            && entry.file_type().is_ok_and(|t| t.is_file());
        if ours && !keep.contains(&path.as_path()) {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitors::Status;
    use crate::testutil::TestDir;

    fn run(script: &str, timeout_ms: u64, output: Option<&Path>) -> CheckResult {
        CommandCheck {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            home: None,
            timeout: Duration::from_millis(timeout_ms),
            output: output.map(|f| Output {
                file: f.to_path_buf(),
                max_bytes: 1024,
            }),
        }
        .check()
    }

    #[test]
    fn exit_codes() {
        for (script, status, msg) in [
            ("exit 0", Status::Up, "exit 0"),
            ("exit 1", Status::Warn, "exit 1 (warning)"),
            ("exit 2", Status::Fail, "exit 2 (critical)"),
            ("exit 3", Status::Fail, "exit 3 (unknown)"),
            ("exit 42", Status::Fail, "exit 42"),
            ("kill -SEGV $$", Status::Fail, "killed by signal 11"),
        ] {
            let result = run(script, 5000, None);
            assert_eq!(
                (result.status, result.msg.as_str()),
                (status, msg),
                "{script}"
            );
        }
    }

    #[test]
    fn environment_is_cleared() {
        let result = run(
            "[ \"$PATH\" = /usr/local/bin:/usr/bin:/bin ] && [ \"$LC_ALL\" = C ] \
             && [ -z \"$CARGO\" ] && [ \"$(pwd)\" = / ]",
            5000,
            None,
        );
        assert_eq!(result.msg, "exit 0");
    }

    #[test]
    fn timeout_kills_the_whole_group() {
        let dir = TestDir::new("command-timeout");
        let marker = dir.path().join("survived");
        let start = Instant::now();
        // A child in the background that would write the marker later.
        let script = format!("(sleep 1; touch {}) & sleep 30", marker.display());
        let result = run(&script, 300, None);
        assert_eq!(
            (result.status, result.msg.as_str()),
            (Status::Fail, "timeout")
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        thread::sleep(Duration::from_millis(1200));
        assert!(!marker.exists(), "a child of the program survived");
    }

    #[test]
    fn leftover_children_are_killed_after_exit() {
        let dir = TestDir::new("command-leftover");
        let marker = dir.path().join("survived");
        let out = dir.path().join("out.txt");
        // The background child keeps the output pipe open; the check must
        // still end when the program does.
        let script = format!("(sleep 1; touch {}) & exit 0", marker.display());
        let start = Instant::now();
        let result = run(&script, 5000, Some(&out));
        assert_eq!(result.msg, "exit 0");
        assert!(start.elapsed() < Duration::from_millis(900));
        thread::sleep(Duration::from_millis(1200));
        assert!(!marker.exists());
    }

    #[test]
    fn output_is_kept_only_on_request() {
        let dir = TestDir::new("command-output");
        let out = dir.path().join(OUTPUT_DIR).join("disk.txt");
        let result = run("echo out; echo err >&2; exit 2", 5000, Some(&out));
        assert_eq!(result.msg, "exit 2 (critical)");
        let text = fs::read_to_string(&out).unwrap();
        assert!(
            text.starts_with("out\nerr\n# uptime-kinomi: exit 2 (critical) after "),
            "{text}"
        );
        assert_eq!(fs::metadata(&out).unwrap().mode() & 0o777, 0o640);
        // The message never contains the output.
        assert!(!result.msg.contains("out"));

        // Over the limit: cut, and the program is not blocked.
        run(
            "head -c 200000 /dev/zero | tr '\\0' x; exit 0",
            5000,
            Some(&out),
        );
        let text = fs::read_to_string(&out).unwrap();
        assert!(
            text.contains("\n# truncated after 1024 bytes\n# uptime-kinomi: exit 0"),
            "{text}"
        );
        assert!(text.len() < 1200);

        remove_stale_outputs(dir.path(), &[]);
        assert!(!out.exists());
    }

    /// Problems of a config directory with `commands` in kinomi.toml and
    /// one command monitor with `table`.
    fn config_problems(commands: &str, table: &str) -> Vec<String> {
        let dir = TestDir::new("command-config");
        let data = dir.path().join("data");
        dir.write(
            "kinomi.toml",
            &format!(
                "config_version = 1\n[storage]\ndata_dir = \"{}\"\n{commands}",
                data.display()
            ),
        );
        dir.write(
            "monitors/check.toml",
            &format!("config_version = 1\ntype = \"command\"\nname = \"x\"\n[command]\n{table}"),
        );
        let env = |key: &str| (key == "HOME").then(|| std::ffi::OsString::from("/home/u"));
        let (_, problems) = crate::config::load(dir.path(), &env, None);
        problems
            .all()
            .filter(|p| p.severity == crate::config::Severity::Error)
            .map(|p| p.to_string())
            .collect()
    }

    #[test]
    fn only_allowed_programs_run() {
        if paths::running_as_root() {
            return; // every command monitor is an error then
        }
        let allow_true = "[commands]\nallow = [\"/bin/true\"]\n";
        assert_eq!(
            config_problems(allow_true, "program = \"/bin/true\"\nargs = [\"-x\"]"),
            Vec::<String>::new()
        );
        for (commands, table, error) in [
            ("", "program = \"/bin/true\"", "need [commands] allow"),
            (allow_true, "program = \"/bin/false\"", "is not allowed"),
            (allow_true, "program = \"/nonexistent/x\"", "does not exist"),
            (
                "[commands]\nallow = [\"/usr/bin/\"]\n",
                "program = \"/bin/true\"",
                "contains shells",
            ),
            (
                "[commands]\nallow = [\"/bin/\"]\n",
                "program = \"/bin/true\"",
                "contains shells",
            ),
            (
                "[commands]\nallow = [\"/nonexistent/\"]\n",
                "program = \"/bin/true\"",
                "does not exist",
            ),
            (
                "[commands]\nallow = [\"/bin/true\"]\noutput_max_bytes = 10\n",
                "program = \"/bin/true\"",
                "output_max_bytes",
            ),
            (
                allow_true,
                &format!(
                    "program = \"/bin/true\"\nargs = [{}]",
                    vec!["\"a\""; 33].join(",")
                ),
                "at most 32",
            ),
        ] {
            let errors = config_problems(commands, table);
            assert!(
                errors.iter().any(|e| e.contains(error)),
                "{table}: {errors:?}"
            );
        }
        // A script in a directory others can write to (the test dir is
        // below /tmp, which everyone can write to).
        let dir = TestDir::new("command-script");
        let script = dir.write("check.sh", "#!/bin/sh\nexit 0\n");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let errors = config_problems(
            &format!("[commands]\nallow = [\"{}/\"]\n", dir.path().display()),
            &format!("program = \"{}\"", script.display()),
        );
        if writable_by_others(dir.path()).is_some() {
            assert!(
                errors
                    .iter()
                    .any(|e| e.contains("can be changed by other users")),
                "{errors:?}"
            );
        }
    }

    #[test]
    fn writable_paths_are_refused() {
        let dir = TestDir::new("command-perms");
        let script = dir.path().join("check.sh");
        fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        // Only if the test directory itself is safe (not below /tmp).
        if writable_by_others(dir.path()).is_none() {
            assert_eq!(writable_by_others(&script), None);
            fs::set_permissions(&script, fs::Permissions::from_mode(0o775)).unwrap();
            assert_eq!(writable_by_others(&script), Some(script.clone()));
        }
        assert_eq!(
            writable_by_others(Path::new("/tmp/x")),
            Some(PathBuf::from("/tmp/x"))
        );
        assert_eq!(writable_by_others(Path::new("/bin/sh")), None);
    }
}
