//! `tcp` monitor: TCP port open, optionally with TLS and an expected
//! banner (SPEC.md §4.4).

use std::io::{self, Read};
use std::net::TcpStream;
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;

use super::{Check, CheckResult, is_valid_host, tls};
use crate::clock;
use crate::config::FileProblems;
use crate::config::duration::Dur;

/// Most bytes read from the server when checking a banner.
const MAX_BANNER_BYTES: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TcpFile {
    host: String,
    port: u16,
    expected_banner: Option<String>,
    tls: Option<bool>,
    tls_verify: Option<bool>,
    tls_expiry_warn: Option<Dur>,
    tls_expiry_fail: Option<Dur>,
}

pub struct TcpCheck {
    host: String,
    port: u16,
    banner: Option<Vec<u8>>,
    tls: Option<Tls>,
    timeout: Duration,
}

/// `tls = true`: handshake after connecting, certificate checks.
struct Tls {
    verify: bool,
    warn: Duration,
    fail: Duration,
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
    if let Some(b) = &file.expected_banner {
        if b.is_empty() || b.len() > MAX_BANNER_BYTES {
            p.error(format!(
                "tcp.expected_banner must be 1 to {MAX_BANNER_BYTES} bytes"
            ));
        }
    }
    let tls = if file.tls.unwrap_or(false) {
        let verify = file.tls_verify.unwrap_or(true);
        if !verify {
            p.warning("tcp.tls_verify = false: certificate errors are ignored");
        }
        Some(Tls {
            verify,
            warn: file.tls_expiry_warn.unwrap_or(Dur::secs(14 * 86400)).0,
            fail: file.tls_expiry_fail.unwrap_or(Dur::secs(0)).0,
        })
    } else {
        if file.tls_verify.is_some()
            || file.tls_expiry_warn.is_some()
            || file.tls_expiry_fail.is_some()
        {
            p.error("tcp.tls_* options need tls = true");
        }
        None
    };
    Some(TcpCheck {
        host: file.host,
        port: file.port,
        banner: file.expected_banner.map(String::into_bytes),
        tls,
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
        let Some(stream) = stream else {
            let msg = match last_error {
                Some(e) => connect_error(&e),
                None if remaining().is_zero() => "timeout",
                None => "host not found",
            };
            return CheckResult::fail(Some(start.elapsed()), msg);
        };
        let Some(tls) = &self.tls else {
            let latency = start.elapsed();
            let mut reader = &stream;
            return self.banner_result(&stream, &mut reader, latency, remaining(), "connected");
        };
        let conn = match tls::handshake(&stream, &self.host, tls.verify, remaining()) {
            Ok(conn) => conn,
            Err(msg) => return CheckResult::fail(Some(start.elapsed()), msg),
        };
        // The latency includes the handshake: that is what a client waits for.
        let latency = start.elapsed();
        let not_after = tls::leaf_not_after(&conn);
        let mut conn = conn;
        let mut sock = &stream;
        let mut reader = rustls::Stream::new(&mut conn, &mut sock);
        let result = self.banner_result(&stream, &mut reader, latency, remaining(), "TLS ok");
        if result.status == super::Status::Fail {
            return result;
        }
        let now = clock::unix_seconds(SystemTime::now());
        tls::rate_expiry(result, not_after, now, tls.warn, tls.fail)
    }

    fn target(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

impl TcpCheck {
    /// The result after connecting (and the TLS handshake): `ok` without
    /// `expected_banner`, else the banner check through `reader`.
    fn banner_result(
        &self,
        stream: &TcpStream,
        reader: &mut dyn Read,
        latency: Duration,
        timeout: Duration,
        ok: &str,
    ) -> CheckResult {
        let Some(expected) = &self.banner else {
            return CheckResult::up(latency, ok);
        };
        match read_banner(stream, reader, expected.len(), timeout) {
            Ok(received) if received.starts_with(expected) => CheckResult::up(latency, "banner ok"),
            // Closed with nothing, or only the start of the banner, sent.
            Ok(received) if expected.starts_with(&received) => {
                CheckResult::fail(Some(latency), "connection closed before banner")
            }
            Ok(_) => CheckResult::fail(Some(latency), "unexpected banner"),
            Err(e) if is_timeout(&e) => {
                CheckResult::fail(Some(latency), "timeout waiting for banner")
            }
            Err(_) => CheckResult::fail(Some(latency), "connection closed before banner"),
        }
    }
}

/// Read until `wanted` bytes arrived, the peer closes, or the limit is hit.
/// `timeout` is for all reads together, so a server sending one byte at a
/// time cannot stretch the check.
fn read_banner(
    stream: &TcpStream,
    reader: &mut dyn Read,
    wanted: usize,
    timeout: Duration,
) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut received = Vec::with_capacity(wanted);
    let mut buf = [0u8; MAX_BANNER_BYTES];
    while received.len() < wanted {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(left))?;
        let n = reader.read(&mut buf[..MAX_BANNER_BYTES - received.len()])?;
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
            tls: None,
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
        // Closes (after 300 ms) without greeting, or after part of it.
        for greeting in [&b""[..], b"SS"] {
            let port = server(greeting, 1);
            assert_eq!(
                check(port, Some("SSH-"), 2000).msg,
                "connection closed before banner"
            );
        }
    }

    #[test]
    fn slow_banner_stays_within_timeout() {
        // One byte every 400 ms: each read is quick, the banner is not.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            if let Some(Ok(mut stream)) = listener.incoming().next() {
                for b in b"SSH-2.0" {
                    if stream.write_all(&[*b]).is_err() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(400));
                }
            }
        });
        let start = Instant::now();
        let r = check(port, Some("SSH-2.0"), 1000);
        assert_eq!(r.status, Status::Fail, "{r:?}");
        assert!(
            start.elapsed() < Duration::from_millis(1500),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn unknown_host() {
        let result = TcpCheck {
            host: "does-not-exist.invalid".into(),
            port: 80,
            banner: None,
            tls: None,
            timeout: Duration::from_secs(5),
        }
        .check();
        assert_eq!(result.status, Status::Fail);
        assert_eq!(result.msg, "host not found");
    }

    fn tls_check(port: u16, banner: Option<&str>, verify: bool, warn_days: u64) -> CheckResult {
        TcpCheck {
            host: "localhost".into(),
            port,
            banner: banner.map(|b| b.as_bytes().to_vec()),
            tls: Some(Tls {
                verify,
                warn: Duration::from_secs(warn_days * 86400),
                fail: Duration::ZERO,
            }),
            timeout: Duration::from_secs(3),
        }
        .check()
    }

    #[test]
    fn tls() {
        use crate::monitors::tls::testing::{GENERALIZED_CERT, server as tls_server};
        let port = tls_server(GENERALIZED_CERT, b"* OK IMAP ready\r\n", 5);
        let ok = tls_check(port, None, false, 14);
        assert_eq!((ok.status, ok.msg.as_str()), (Status::Up, "TLS ok"));
        assert_eq!(ok.cert_expires, Some(2_569_233_600));
        // The banner is read through TLS.
        let banner = tls_check(port, Some("* OK"), false, 14);
        assert_eq!(
            (banner.status, banner.msg.as_str()),
            (Status::Up, "banner ok")
        );
        let wrong = tls_check(port, Some("220 "), false, 14);
        assert_eq!(wrong.msg, "unexpected banner");
        // Expires within 100 years: WARN.
        let soon = tls_check(port, None, false, 36500);
        assert_eq!(soon.status, Status::Warn);
        assert!(
            soon.msg.starts_with("TLS ok, certificate expires in "),
            "{soon:?}"
        );
        // Self-signed with verification.
        let verified = tls_check(port, None, true, 14);
        assert_eq!(
            (verified.status, verified.msg.as_str()),
            (Status::Fail, "TLS certificate from unknown issuer")
        );
        // A plain-text server is not TLS.
        let plain = tls_check(server(b"SSH-2.0-x\r\n", 1), None, false, 14);
        assert_eq!(plain.status, Status::Fail);
        assert!(
            plain.msg.starts_with("TLS") || plain.msg == "timeout",
            "{plain:?}"
        );
    }

    #[test]
    fn tls_config_errors() {
        let build_text = |table: &str| {
            let mut problems = crate::config::Problems::default();
            let mut p = problems.for_file(std::path::Path::new("m.toml"));
            let file: TcpFile =
                toml::from_str(&format!("host = \"h\"\nport = 1\n{table}")).unwrap();
            build(file, Duration::from_secs(5), &mut p);
            p.has_errors()
        };
        assert!(!build_text("tls = true\ntls_expiry_warn = \"30d\""));
        assert!(build_text("tls_verify = false"));
        assert!(build_text("tls = false\ntls_expiry_fail = \"1d\""));
    }

    #[test]
    fn target_format() {
        let t = |host: &str| {
            TcpCheck {
                host: host.into(),
                port: 22,
                banner: None,
                tls: None,
                timeout: Duration::ZERO,
            }
            .target()
        };
        assert_eq!(t("example.org"), "example.org:22");
        assert_eq!(t("::1"), "[::1]:22");
    }
}
