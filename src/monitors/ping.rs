//! `ping` monitor: ICMP echo over Linux's unprivileged datagram sockets
//! (SPEC.md §4.7). No raw sockets, no root, no `ping` program.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use rustix::io::Errno;
use rustix::net::{AddressFamily, SocketFlags, SocketType, ipproto};
use serde::Deserialize;

use super::{Check, CheckResult, is_valid_host};
use crate::config::FileProblems;
use crate::config::duration::Dur;

/// Time between two echo requests of one check.
const SPACING: Duration = Duration::from_millis(500);
/// Random bytes in every request; a reply must carry them back.
pub const PAYLOAD: usize = 16;
/// Longer packets are never a reply to us.
pub const MAX_REPLY: usize = 64;
const MAX_COUNT: u8 = 10;
const HEADER: usize = 8;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PingFile {
    host: String,
    count: Option<u8>,
    loss_warn: Option<bool>,
    slow_warn: Option<Dur>,
}

pub struct PingCheck {
    host: String,
    count: u8,
    loss_warn: bool,
    slow_warn: Option<Duration>,
    timeout: Duration,
}

pub fn build(file: PingFile, timeout: Duration, p: &mut FileProblems) -> Option<PingCheck> {
    if !is_valid_host(&file.host) {
        p.error(format!(
            "ping.host '{}' is not a host name or IP address",
            file.host
        ));
    }
    let count = file.count.unwrap_or(3);
    if !(1..=MAX_COUNT).contains(&count) {
        p.error(format!("ping.count must be between 1 and {MAX_COUNT}"));
    } else if SPACING * u32::from(count - 1) >= timeout {
        p.error(format!(
            "ping.count {count} needs a timeout longer than {} ms (500 ms between requests)",
            500 * u32::from(count - 1)
        ));
    }
    let slow_warn = file.slow_warn.filter(|d| !d.is_zero()).map(|d| d.0);
    if slow_warn.is_some_and(|s| s >= timeout) {
        p.error("ping.slow_warn must be less than timeout");
    }
    if open(false).is_err_and(|e| e == Errno::ACCESS || e == Errno::PERM) {
        p.warning(
            "ping is not permitted for this user: its group must be in \
             net.ipv4.ping_group_range (an administrator: \
             sysctl -w net.ipv4.ping_group_range=\"0 2147483647\")",
        );
    }
    Some(PingCheck {
        host: file.host,
        count,
        loss_warn: file.loss_warn.unwrap_or(false),
        slow_warn,
        timeout,
    })
}

/// An unprivileged ICMP (or ICMPv6) datagram socket. std's `UdpSocket`
/// carries it: send_to, recv and timeouts work the same way.
fn open(v6: bool) -> Result<UdpSocket, Errno> {
    let (family, protocol) = if v6 {
        (AddressFamily::INET6, ipproto::ICMPV6)
    } else {
        (AddressFamily::INET, ipproto::ICMP)
    };
    // CLOEXEC: not inherited by programs of command monitors.
    rustix::net::socket_with(
        family,
        SocketType::DGRAM,
        SocketFlags::CLOEXEC,
        Some(protocol),
    )
    .map(UdpSocket::from)
}

/// An ICMP socket as the checks open it, for tests elsewhere (`None` if
/// the kernel does not allow ping sockets).
#[cfg(test)]
pub fn tests_open_socket() -> Option<UdpSocket> {
    open(false).ok()
}

/// Echo request with sequence number `seq`. Identifier and checksum are
/// filled in by the kernel.
fn request(v6: bool, seq: u16, payload: &[u8; PAYLOAD]) -> [u8; HEADER + PAYLOAD] {
    let mut packet = [0u8; HEADER + PAYLOAD];
    packet[0] = if v6 { 128 } else { 8 };
    packet[6..8].copy_from_slice(&seq.to_be_bytes());
    packet[HEADER..].copy_from_slice(payload);
    packet
}

/// The sequence number of `packet` if it is an echo reply carrying exactly
/// `payload`; anything else is not ours.
pub fn reply_seq(packet: &[u8], v6: bool, payload: &[u8]) -> Option<u16> {
    let reply_type = if v6 { 129 } else { 0 };
    if packet.len() > MAX_REPLY {
        return None;
    }
    match packet {
        [t, 0, _, _, _, _, s0, s1, rest @ ..] if *t == reply_type && rest == payload => {
            Some(u16::from_be_bytes([*s0, *s1]))
        }
        _ => None,
    }
}

fn send_error(e: &io::Error) -> &'static str {
    match e.kind() {
        io::ErrorKind::NetworkUnreachable => "network unreachable",
        io::ErrorKind::HostUnreachable => "host unreachable",
        _ => "no reply",
    }
}

impl Check for PingCheck {
    fn check(&self) -> CheckResult {
        let start = Instant::now();
        let deadline = start + self.timeout;
        let fail = |msg: &str| CheckResult::fail(Some(start.elapsed()), msg);

        let addr = match super::resolve(&self.host, 0, self.timeout) {
            Ok(addrs) => match addrs.first() {
                Some(addr) => *addr,
                None => return fail("host not found"),
            },
            Err(msg) => return fail(msg),
        };
        let v6 = matches!(addr, SocketAddr::V6(_));
        let socket = match open(v6) {
            Ok(s) => s,
            Err(e) if e == Errno::ACCESS || e == Errno::PERM => {
                return fail("ping not permitted (net.ipv4.ping_group_range)");
            }
            Err(e) if e == Errno::AFNOSUPPORT && v6 => return fail("IPv6 not available"),
            Err(_) => return fail("no reply"),
        };
        let mut payload = [0u8; PAYLOAD];
        let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut payload);

        let count = usize::from(self.count);
        let mut sent: [Option<Instant>; MAX_COUNT as usize] = [None; MAX_COUNT as usize];
        let mut rtts: Vec<Duration> = Vec::with_capacity(count);
        let mut next = 0;
        let first = Instant::now();
        let mut buf = [0u8; MAX_REPLY + 1];
        loop {
            let now = Instant::now();
            let due = first + SPACING * next as u32;
            if next < count && now >= due {
                let seq = next as u16;
                if let Err(e) = socket.send_to(&request(v6, seq, &payload), addr) {
                    if rtts.is_empty() && next == 0 {
                        return fail(send_error(&e));
                    }
                }
                sent[next] = Some(Instant::now());
                next += 1;
                continue;
            }
            if rtts.len() == count || now >= deadline {
                break;
            }
            let until = if next < count {
                due.min(deadline)
            } else {
                deadline
            };
            let wait = until.saturating_duration_since(now);
            if wait.is_zero() || socket.set_read_timeout(Some(wait)).is_err() {
                continue;
            }
            match socket.recv(&mut buf) {
                Ok(n) => {
                    let seq = buf
                        .get(..n)
                        .and_then(|packet| reply_seq(packet, v6, &payload));
                    if let Some(sent_at) = seq.and_then(|s| sent.get_mut(usize::from(s))) {
                        // take(): a duplicate reply finds None.
                        if let Some(t) = sent_at.take() {
                            rtts.push(t.elapsed());
                        }
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => {}
            }
        }

        let replies = rtts.len();
        if replies == 0 {
            return fail("no reply");
        }
        let average = rtts.iter().sum::<Duration>() / replies as u32;
        let msg = format!("{replies}/{count} replies");
        if self.loss_warn && replies < count {
            CheckResult::warn(average, format!("{msg}, packet loss"))
        } else if self.slow_warn.is_some_and(|slow| average > slow) {
            CheckResult::warn(average, format!("{msg}, slow response"))
        } else {
            CheckResult::up(average, msg)
        }
    }

    fn target(&self) -> String {
        self.host.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitors::Status;

    const P: [u8; PAYLOAD] = *b"0123456789abcdef";

    fn reply(t: u8, code: u8, seq: u16, payload: &[u8]) -> Vec<u8> {
        let mut packet = vec![t, code, 0xab, 0xcd, 0x12, 0x34];
        packet.extend_from_slice(&seq.to_be_bytes());
        packet.extend_from_slice(payload);
        packet
    }

    #[test]
    fn replies_are_matched() {
        assert_eq!(reply_seq(&reply(0, 0, 7, &P), false, &P), Some(7));
        assert_eq!(reply_seq(&reply(129, 0, 2, &P), true, &P), Some(2));
        // Wrong family, type, code, payload or length.
        assert_eq!(reply_seq(&reply(0, 0, 7, &P), true, &P), None);
        assert_eq!(reply_seq(&reply(8, 0, 7, &P), false, &P), None);
        assert_eq!(reply_seq(&reply(3, 1, 7, &P), false, &P), None);
        assert_eq!(
            reply_seq(&reply(0, 0, 7, b"0123456789abcdeX"), false, &P),
            None
        );
        assert_eq!(reply_seq(&reply(0, 0, 7, &P[..15]), false, &P), None);
        assert_eq!(reply_seq(&reply(0, 0, 7, &[0; 60]), false, &[0; 60]), None);
        assert_eq!(reply_seq(&[0, 0, 0], false, &P), None);
        // The request itself is not a reply.
        assert_eq!(reply_seq(&request(false, 1, &P), false, &P), None);
        assert_eq!(reply_seq(&request(true, 1, &P), true, &P), None);
    }

    fn check(host: &str, count: u8, timeout_ms: u64) -> PingCheck {
        PingCheck {
            host: host.into(),
            count,
            loss_warn: true,
            slow_warn: None,
            timeout: Duration::from_millis(timeout_ms),
        }
    }

    /// The kernel must allow ping sockets (as on Raspberry Pi OS and most
    /// systemd distributions); skipped otherwise.
    fn permitted() -> bool {
        open(false).is_ok()
    }

    #[test]
    fn localhost_answers() {
        if !permitted() {
            return;
        }
        let result = check("127.0.0.1", 3, 3000).check();
        assert_eq!(result.status, Status::Up, "{result:?}");
        assert_eq!(result.msg, "3/3 replies");
        if open(true).is_ok() {
            let result = check("::1", 1, 1000).check();
            assert_eq!(result.msg, "1/1 replies", "{result:?}");
        }
    }

    #[test]
    fn silence_fails_within_timeout() {
        if !permitted() {
            return;
        }
        // TEST-NET-1 (RFC 5737): never answers.
        let start = Instant::now();
        let result = check("192.0.2.1", 2, 800).check();
        assert_eq!(result.status, Status::Fail);
        assert!(
            ["no reply", "network unreachable", "host unreachable"].contains(&result.msg.as_str()),
            "{result:?}"
        );
        assert!(start.elapsed() < Duration::from_millis(1500));
    }

    #[test]
    fn config_errors() {
        let build_with = |count: Option<u8>, timeout_ms: u64| {
            let mut p = crate::config::Problems::default();
            let mut fp = p.for_file(std::path::Path::new("m.toml"));
            let file = PingFile {
                host: "router".into(),
                count,
                loss_warn: None,
                slow_warn: None,
            };
            build(file, Duration::from_millis(timeout_ms), &mut fp);
            fp.has_errors()
        };
        assert!(!build_with(None, 10_000));
        assert!(build_with(Some(0), 10_000));
        assert!(build_with(Some(11), 10_000));
        assert!(!build_with(Some(10), 5000));
        assert!(build_with(Some(10), 4500));
    }
}
