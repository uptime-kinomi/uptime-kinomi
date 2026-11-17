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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use rustix::pipe::PipeFlags;
use rustix::process::{Pid, Signal, WaitId, WaitIdOptions};
use serde::Deserialize;

use super::{Check, CheckResult, Context};
use crate::clock;
use crate::config::FileProblems;
use crate::config::global::{Allowed, lexical_path};
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
    /// The resolved file that runs ...
    program: PathBuf,
    /// ... and the name it is called by (argv[0]), as written.
    name: PathBuf,
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
    let Some(written) = lexical_path(ctx.config_dir, &file.program) else {
        p.error("command.program must not contain ..");
        return None;
    };
    let program = match fs::canonicalize(&written) {
        Ok(real) => real,
        Err(_) => {
            p.error(format!(
                "command.program {} does not exist",
                written.display()
            ));
            return None;
        }
    };
    // Both the path as written and the file it resolves to must be
    // allowed: a symlink in an allowed directory cannot lead to a shell,
    // and a symlink to a multi-call program (busybox) keeps its name.
    let matches = |path: &Path, of: fn(&Allowed) -> &Path| {
        commands.allow.iter().any(|a| {
            if a.dir {
                path.parent() == Some(of(a))
            } else {
                path == of(a)
            }
        })
    };
    if !matches(&written, |a| &a.written) || !matches(&program, |a| &a.real) {
        p.error(format!(
            "command.program {} is not allowed by [commands] allow in kinomi.toml",
            written.display()
        ));
    }
    match fs::metadata(&program) {
        Ok(meta) if meta.is_file() && meta.permissions().mode() & 0o111 != 0 => {}
        _ => p.error(format!(
            "command.program {} is not an executable file",
            program.display()
        )),
    }
    // The symlinks on the way count too: whoever can change them decides
    // what runs.
    if let Some(unsafe_path) = writable_by_others(&written).or_else(|| writable_by_others(&program))
    {
        p.error(format!(
            "command.program: {} can be changed by other users (owner must be root or you, \
             not writable by others or by a group with other members)",
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
        name: written,
        args: file.args,
        home: std::env::var("HOME").ok(),
        timeout,
        output,
    })
}

/// The first of `path` and its parent directories that another user could
/// change: owned by someone other than root or us, writable by others, or
/// writable by a group that has other members (like OpenSSH's StrictModes,
/// with Debian's exception for user private groups).
fn writable_by_others(path: &Path) -> Option<PathBuf> {
    let uid = paths::current_uid();
    let own_group = (|| {
        let passwd = fs::read_to_string("/etc/passwd").ok()?;
        let group = fs::read_to_string("/etc/group").ok()?;
        private_group(uid, &passwd, &group)
    })();
    path.ancestors().find_map(|p| match fs::metadata(p) {
        Ok(meta) => {
            let owner_ok = meta.uid() == 0 || meta.uid() == uid;
            let group_ok = meta.mode() & 0o020 == 0 || Some(meta.gid()) == own_group;
            (!owner_ok || !group_ok || meta.mode() & 0o002 != 0).then(|| p.to_path_buf())
        }
        Err(_) => Some(p.to_path_buf()),
    })
}

/// The primary group of user `uid` if no one else is in it: no other user
/// has it as primary group (`passwd`) and it lists no other members
/// (`group`). Debian and Raspberry Pi OS create such a group for every user
/// and make new files group-writable (umask 002); write permission for
/// that group gives no one else access. Only the local files count: a
/// group from another source (LDAP) is never treated as private.
fn private_group(uid: u32, passwd: &str, group: &str) -> Option<u32> {
    let users: Vec<(&str, u32, u32)> = passwd
        .lines()
        .filter_map(|line| {
            let mut f = line.split(':');
            let name = f.next()?;
            let uid = f.nth(1)?.parse().ok()?;
            let gid = f.next()?.parse().ok()?;
            Some((name, uid, gid))
        })
        .collect();
    let &(name, _, gid) = users.iter().find(|u| u.1 == uid)?;
    if users.iter().any(|u| u.2 == gid && u.0 != name) {
        return None;
    }
    let mut entries = group.lines().filter_map(|line| {
        let mut f = line.split(':');
        let _name = f.next()?;
        let g: u32 = f.nth(1)?.parse().ok()?;
        Some((g, f.next()?))
    });
    let (_, members) = entries.find(|e| e.0 == gid)?;
    members
        .split(',')
        .all(|m| m.is_empty() || m == name)
        .then_some(gid)
}

impl Check for CommandCheck {
    fn check(&self) -> CheckResult {
        let start = Instant::now();
        let mut command = Command::new(&self.program);
        command
            .arg0(&self.name)
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
        let child = match spawned {
            Ok(child) => child,
            Err(_) => return CheckResult::fail(Some(start.elapsed()), "cannot start program"),
        };
        let finished = wait(child, self.timeout);
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
        if let (Some(output), Some((reader, stop))) = (&self.output, reader) {
            // The process group is gone, so the pipe is closed and the
            // reader ends. A process that left the group (setsid) could
            // keep it open: after a short grace the reader stops anyway,
            // and what arrived so far is saved.
            let grace = Instant::now();
            while !reader.is_finished() && grace.elapsed() < OUTPUT_GRACE {
                thread::sleep(POLL);
            }
            stop.store(true, Ordering::Relaxed);
            let (bytes, truncated) = reader.join().unwrap_or_default();
            save_output(output, bytes, truncated, &result, elapsed);
        }
        result
    }

    fn target(&self) -> String {
        self.name.display().to_string()
    }

    fn output_file(&self) -> Option<&Path> {
        self.output.as_ref().map(|o| o.file.as_path())
    }
}

/// stdout and stderr of `command` into one pipe, read by a thread that
/// keeps at most `max` bytes and discards the rest (so the program never
/// blocks on a full pipe). The thread ends at the end of the output or
/// when the returned flag is set. It returns the output and whether it
/// was cut.
fn pipe_into(command: &mut Command, max: u64) -> Option<(Reader, Arc<AtomicBool>)> {
    // CLOEXEC: programs of other monitors, started meanwhile, must not
    // inherit either end. std's dup2 onto stdout/stderr clears it there.
    let (read, write) = rustix::pipe::pipe_with(PipeFlags::CLOEXEC).ok()?;
    let write2 = write.try_clone().ok()?;
    // Non-blocking, so the reader can notice the stop flag.
    rustix::io::ioctl_fionbio(&read, true).ok()?;
    command
        .stdout(Stdio::from(write))
        .stderr(Stdio::from(write2));
    let mut read = File::from(read);
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let reader = thread::Builder::new()
        .name("command-output".into())
        .spawn(move || {
            let mut kept = Vec::new();
            let mut truncated = false;
            let mut buf = [0u8; 8192];
            loop {
                match read.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let room = usize::try_from(max)
                            .unwrap_or(usize::MAX)
                            .saturating_sub(kept.len());
                        kept.extend_from_slice(&buf[..n.min(room)]);
                        truncated |= n > room;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if stopped.load(Ordering::Relaxed) {
                            break;
                        }
                        thread::sleep(POLL);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            (kept, truncated)
        })
        .ok()?;
    Some((reader, stop))
}

type Reader = thread::JoinHandle<(Vec<u8>, bool)>;

/// Process groups of the programs running now, killed when kinomi stops.
static RUNNING: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// Kill every program that is still running (at shutdown): they are in
/// their own process groups, so they would not get kinomi's signal.
pub fn kill_running() {
    let running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    for pid in running.iter().filter_map(|p| Pid::from_raw(*p)) {
        let _ = rustix::process::kill_process_group(pid, Signal::KILL);
    }
}

/// Wait for the program until `timeout`. Whatever is left of its process
/// group is killed afterwards, and at the timeout the program and its whole
/// group are. Both are killed before the program is reaped, so its ID
/// cannot have been reused by then. `None`: timed out (or the program
/// could not be stopped; it is then reaped in the background).
fn wait(mut child: Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    let pid = i32::try_from(child.id()).ok().and_then(Pid::from_raw);
    let raw = pid.map(Pid::as_raw_pid);
    if let Some(raw) = raw {
        RUNNING.lock().unwrap_or_else(|e| e.into_inner()).push(raw);
    }
    let exited = |pid: Option<Pid>| match pid {
        // Exited (a zombie, still holding its ID), without reaping it.
        Some(pid) => rustix::process::waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        )
        .map(|s| s.is_some())
        .unwrap_or(true),
        None => true,
    };
    let in_time = loop {
        if exited(pid) {
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
    let mut reapable = in_time;
    if !in_time {
        // Also by PID: it may have moved to another process group.
        let _ = child.kill();
        let grace = Instant::now();
        while !reapable && grace.elapsed() < OUTPUT_GRACE {
            reapable = exited(pid);
            if !reapable {
                thread::sleep(POLL);
            }
        }
    }
    if let Some(raw) = raw {
        RUNNING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|p| *p != raw);
    }
    if !reapable {
        // Could not be killed (e.g. it changed its user): never block the
        // worker; reap it whenever it ends.
        let _ = thread::Builder::new()
            .name("command-reaper".into())
            .spawn(move || {
                let _ = child.wait();
            });
        return None;
    }
    let status = child.wait().ok();
    if in_time { status } else { None }
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
            name: "/bin/sh".into(),
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
    fn program_keeps_the_name_it_was_called_by() {
        let dir = TestDir::new("command-arg0");
        let out = dir.path().join("arg0.txt");
        let check = CommandCheck {
            program: "/bin/sh".into(),
            name: "/opt/checks/check_x".into(),
            args: vec![
                "-c".into(),
                "tr '\\0' '\\n' < /proc/$$/cmdline | head -n 1".into(),
            ],
            home: None,
            timeout: Duration::from_secs(5),
            output: Some(Output {
                file: out.clone(),
                max_bytes: 1024,
            }),
        };
        assert_eq!(check.check().msg, "exit 0");
        let text = fs::read_to_string(&out).unwrap();
        assert!(text.starts_with("/opt/checks/check_x\n"), "{text}");
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
    fn no_descriptors_are_inherited() {
        use std::os::fd::AsRawFd;
        let dir = TestDir::new("command-fds");
        let out = dir.path().join("fds.txt");
        // A ping socket and another check's pipe open meanwhile must not
        // reach the program, nor this check's own pipe (ours: every fd
        // above 2 that the program sees and the test process did not get
        // from its own parent, e.g. a CI runner).
        let socket = crate::monitors::ping::tests_open_socket();
        let (r, w) = rustix::pipe::pipe_with(PipeFlags::CLOEXEC).unwrap();
        let inherited = |pid: &str| -> Vec<String> {
            fs::read_dir(format!("/proc/{pid}/fd"))
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        };
        let before = inherited("self");
        run("ls /proc/$$/fd", 5000, Some(&out));
        let text = fs::read_to_string(&out).unwrap();
        let mut ours: Vec<i32> = vec![r.as_raw_fd(), w.as_raw_fd()];
        ours.extend(socket.as_ref().map(|s| s.as_raw_fd()));
        let leaked: Vec<&str> = text
            .lines()
            .filter(|l| !l.starts_with('#') && !["0", "1", "2"].contains(l))
            .filter(|l| {
                // The test process's own non-kinomi fds may be inherited;
                // kinomi's (opened after `before`, or listed in `ours`) not.
                !before.iter().any(|b| b == l) || ours.iter().any(|o| o.to_string() == *l)
            })
            .collect();
        assert!(leaked.is_empty(), "{text}");
    }

    #[test]
    fn output_held_open_by_an_escaped_process_does_not_block() {
        let dir = TestDir::new("command-setsid");
        let out = dir.path().join("out.txt");
        let start = Instant::now();
        let result = run("echo before; setsid sleep 3 & exit 0", 5000, Some(&out));
        assert_eq!(result.msg, "exit 0");
        assert!(
            start.elapsed() < Duration::from_millis(2000),
            "{:?}",
            start.elapsed()
        );
        let text = fs::read_to_string(&out).unwrap();
        assert!(
            text.starts_with("before\n# uptime-kinomi: exit 0"),
            "{text}"
        );
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
        // A symlink in an allowed directory to a program outside it.
        let dir = TestDir::new("command-symlink");
        std::os::unix::fs::symlink("/bin/sh", dir.path().join("check_x")).unwrap();
        let errors = config_problems(
            &format!("[commands]\nallow = [\"{}/\"]\n", dir.path().display()),
            &format!("program = \"{}/check_x\"", dir.path().display()),
        );
        assert!(
            errors.iter().any(|e| e.contains("is not allowed")),
            "{errors:?}"
        );
        for program in ["/bin/../bin/true", "/usr/bin/../../bin/true"] {
            let errors = config_problems(allow_true, &format!("program = \"{program}\""));
            assert!(
                errors.iter().any(|e| e.contains("must not contain ..")),
                "{errors:?}"
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
            fs::set_permissions(&script, fs::Permissions::from_mode(0o757)).unwrap();
            assert_eq!(writable_by_others(&script), Some(script.clone()));
            // Group-writable only with the user's private group.
            fs::set_permissions(&script, fs::Permissions::from_mode(0o775)).unwrap();
            let own_group = private_group(
                paths::current_uid(),
                &fs::read_to_string("/etc/passwd").unwrap(),
                &fs::read_to_string("/etc/group").unwrap(),
            );
            let expected =
                (Some(fs::metadata(&script).unwrap().gid()) != own_group).then(|| script.clone());
            assert_eq!(writable_by_others(&script), expected);
        }
        assert_eq!(
            writable_by_others(Path::new("/tmp/x")),
            Some(PathBuf::from("/tmp/x"))
        );
        assert_eq!(writable_by_others(Path::new("/bin/sh")), None);
    }

    #[test]
    fn private_groups_may_write() {
        let passwd = "root:x:0:0::/root:/bin/sh\n\
                      kinomi:x:1000:1000::/home/kinomi:/bin/bash\n\
                      anna:x:1001:100::/home/anna:/bin/bash\n\
                      bob:x:1002:100::/home/bob:/bin/bash\n\
                      carl:x:1003:1003::/home/carl:/bin/bash\n\
                      dora:x:1004:1004::/home/dora:/bin/bash\n";
        let group = "root:x:0:\n\
                     users:x:100:\n\
                     kinomi:x:1000:\n\
                     carl:x:1003:carl\n\
                     dora:x:1004:dora,kinomi\n";
        // Debian's user private group, also when it lists the user itself.
        assert_eq!(private_group(1000, passwd, group), Some(1000));
        assert_eq!(private_group(1003, passwd, group), Some(1003));
        // Shared as primary group, or with another member.
        assert_eq!(private_group(1001, passwd, group), None);
        assert_eq!(private_group(1004, passwd, group), None);
        // Unknown user, or a group missing from /etc/group (e.g. LDAP).
        assert_eq!(private_group(4242, passwd, group), None);
        assert_eq!(private_group(1000, passwd, "users:x:100:\n"), None);
    }
}
