//! `tcp` monitor: TCP port open, optionally with an expected banner
//! (SPEC.md §4.4).

use std::io::{self, Read};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::{Check, CheckResult, is_valid_host};
use crate::config::FileProblems;

/// Most bytes read from the server when checking a banner.
const MAX_BANNER_BYTES: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TcpFile {
    host: String,
    port: u16,
    expect_banner: Option<String>,
}

pub struct TcpCheck {
    host: String,
    port: u16,
    banner: Option<Vec<u8>>,
    timeout: Duration,
}

pub fn build(file: TcpFile, timeout: Duration, p: &mut FileProblems) -> Option<TcpCheck> {
    if !is_valid_host(&file.host) {
        p.error(format!(
            "tcp.host '{}' is not a host name or IP address",
            file.host
        ));
    }
    if file.port == 0 {
        p.error("tcp.port must be between 1 and 65535");
    }
    if let Some(b) = &file.expect_banner {
        if b.is_empty() || b.len() > MAX_BANNER_BYTES {
            p.error(format!(
                "tcp.expect_banner must be 1 to {MAX_BANNER_BYTES} bytes"
            ));
        }
    }
    Some(TcpCheck {
        host: file.host,
        port: file.port,
        banner: file.expect_banner.map(String::into_bytes),
        timeout,
    })
}

impl Check for TcpCheck {
    fn check(&self) -> CheckResult {
        let start = Instant::now();
        let remaining = || self.timeout.saturating_sub(start.elapsed());

        let addrs = match super::resolve(&self.host, self.port, remaining()) {
            Ok(addrs) => addrs,
            Err(msg) => return CheckResult::fail(Some(start.elapsed()), msg),
        };
        let mut last_error = None;
        let mut stream = None;
        for addr in addrs {
            let left = remaining();
            if left.is_zero() {
                break;
            }
            match TcpStream::connect_timeout(&addr, left) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => last_error = Some(e),
            }
        }
        let Some(mut stream) = stream else {
            let msg = match last_error {
                Some(e) => connect_error(&e),
                None if remaining().is_zero() => "timeout",
                None => "host not found",
            };
            return CheckResult::fail(Some(start.elapsed()), msg);
        };
        let latency = start.elapsed();

        let Some(expected) = &self.banner else {
            return CheckResult::up(latency, "connected");
        };
        match read_banner(&mut stream, expected.len(), remaining()) {
            Ok(received) if received.starts_with(expected) => CheckResult::up(latency, "banner ok"),
            Ok(_) => CheckResult::fail(Some(latency), "unexpected banner"),
            Err(e) if is_timeout(&e) => {
                CheckResult::fail(Some(latency), "timeout waiting for banner")
            }
            Err(_) => CheckResult::fail(Some(latency), "connection closed before banner"),
        }
    }

    fn target(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// Read until `wanted` bytes arrived, the peer closes, or the limit is hit.
fn read_banner(stream: &mut TcpStream, wanted: usize, timeout: Duration) -> io::Result<Vec<u8>> {
    if timeout.is_zero() {
        return Err(io::ErrorKind::TimedOut.into());
    }
    stream.set_read_timeout(Some(timeout))?;
    let mut received = Vec::with_capacity(wanted);
    let mut buf = [0u8; MAX_BANNER_BYTES];
    while received.len() < wanted {
        let n = stream.read(&mut buf[..MAX_BANNER_BYTES - received.len()])?;
        if n == 0 {
            break;
        }
        received.extend_from_slice(&buf[..n]);
    }
    Ok(received)
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

pub fn connect_error(e: &io::Error) -> &'static str {
    match e.kind() {
        io::ErrorKind::ConnectionRefused => "connection refused",
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => "timeout",
        io::ErrorKind::HostUnreachable => "host unreachable",
        io::ErrorKind::NetworkUnreachable => "network unreachable",
        _ => "connection failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitors::Status;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    fn check(port: u16, banner: Option<&str>, timeout_ms: u64) -> CheckResult {
        TcpCheck {
            host: "127.0.0.1".into(),
            port,
            banner: banner.map(|b| b.as_bytes().to_vec()),
            timeout: Duration::from_millis(timeout_ms),
        }
        .check()
    }

    /// A server that sends `greeting` to each of `connections` clients.
    fn server(greeting: &'static [u8], connections: usize) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for stream in listener.incoming().take(connections) {
                let mut stream = stream.unwrap();
                let _ = stream.write_all(greeting);
                thread::sleep(Duration::from_millis(300));
            }
        });
        port
    }

    #[test]
    fn open_port() {
        let port = server(b"", 1);
        let result = check(port, None, 2000);
        assert_eq!(result.status, Status::Up, "{result:?}");
    }

    #[test]
    fn closed_port() {
        // Bind and drop to get a port that is (very likely) closed.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let result = check(port, None, 2000);
        assert_eq!(result.status, Status::Fail);
        assert_eq!(result.msg, "connection refused");
    }

    #[test]
    fn banners() {
        let port = server(b"SSH-2.0-OpenSSH_9.6\r\n", 3);
        assert_eq!(check(port, Some("SSH-"), 2000).status, Status::Up);
        let wrong = check(port, Some("220 "), 2000);
        assert_eq!(wrong.status, Status::Fail);
        // The message never contains what the server sent.
        assert_eq!(wrong.msg, "unexpected banner");
        let silent_port = server(b"", 1);
        let silent = check(silent_port, Some("SSH-"), 100);
        assert_eq!(silent.status, Status::Fail);
        assert!(silent.msg.contains("banner"), "{silent:?}");
    }

    #[test]
    fn unknown_host() {
        let result = TcpCheck {
            host: "does-not-exist.invalid".into(),
            port: 80,
            banner: None,
            timeout: Duration::from_secs(5),
        }
        .check();
        assert_eq!(result.status, Status::Fail);
        assert_eq!(result.msg, "host not found");
    }

    #[test]
    fn target_format() {
        let t = |host: &str| {
            TcpCheck {
                host: host.into(),
                port: 22,
                banner: None,
                timeout: Duration::ZERO,
            }
            .target()
        };
        assert_eq!(t("example.org"), "example.org:22");
        assert_eq!(t("::1"), "[::1]:22");
    }
}
