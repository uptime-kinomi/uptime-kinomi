//! `dns` monitor: one query over UDP to a resolver, checking that records of
//! a type exist and optionally that they equal an expected set (SPEC.md §4.5).
//!
//! Minimal client: one question, EDNS with a 1232-byte buffer, no recursion
//! of our own, no caching, no DNSSEC. The response parser checks every
//! length and limits compression pointers. Received records are only
//! compared with `expected`, never reported.

use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::{Check, CheckResult};
use crate::config::FileProblems;

const MAX_RESPONSE: usize = 1232;
/// Compression pointers followed per name before giving up.
const MAX_POINTERS: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RecordType {
    #[default]
    A,
    Aaaa,
    Cname,
    Mx,
    Txt,
    Ns,
}

impl RecordType {
    fn code(self) -> u16 {
        match self {
            RecordType::A => 1,
            RecordType::Ns => 2,
            RecordType::Cname => 5,
            RecordType::Mx => 15,
            RecordType::Txt => 16,
            RecordType::Aaaa => 28,
        }
    }

    fn name(self) -> &'static str {
        match self {
            RecordType::A => "A",
            RecordType::Aaaa => "AAAA",
            RecordType::Cname => "CNAME",
            RecordType::Mx => "MX",
            RecordType::Txt => "TXT",
            RecordType::Ns => "NS",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DnsFile {
    name: String,
    #[serde(default)]
    record_type: RecordType,
    server: Option<String>,
    expected: Option<Vec<String>>,
}

pub struct DnsCheck {
    name: String,
    record_type: RecordType,
    server: SocketAddr,
    /// Normalized (see [`normalize`]) and sorted.
    expected: Option<Vec<String>>,
    timeout: Duration,
}

pub fn build(file: DnsFile, timeout: Duration, p: &mut FileProblems) -> Option<DnsCheck> {
    let name = file.name.trim_end_matches('.').to_ascii_lowercase();
    if !is_valid_name(&name) {
        p.error(format!("dns.name '{}' is not a domain name", file.name));
    }
    let server = match &file.server {
        Some(s) => parse_server(s).unwrap_or_else(|| {
            p.error(format!("dns.server '{s}' must be an IP address, optionally with port (1.1.1.1:53, [::1]:53)"));
            SocketAddr::from(([127, 0, 0, 1], 53))
        }),
        None => system_resolver().unwrap_or_else(|| {
            p.warning("no nameserver in /etc/resolv.conf; using 127.0.0.1 (set dns.server)");
            SocketAddr::from(([127, 0, 0, 1], 53))
        }),
    };
    let expected = file.expected.map(|list| {
        if list.is_empty() || list.len() > 32 {
            p.error("dns.expected must have 1 to 32 entries");
        }
        let mut values: Vec<String> = list
            .iter()
            .map(|v| normalize(file.record_type, v))
            .collect();
        values.sort();
        values.dedup();
        values
    });
    Some(DnsCheck {
        name,
        record_type: file.record_type,
        server,
        expected,
        timeout,
    })
}

/// A domain name to query. Unlike a host name it may contain `_` (e.g.
/// `_dmarc.example.org`, `_acme-challenge...`, SRV-style labels).
fn is_valid_name(name: &str) -> bool {
    name.parse::<IpAddr>().is_err()
        && !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

fn parse_server(text: &str) -> Option<SocketAddr> {
    text.parse::<SocketAddr>().ok().or_else(|| {
        text.parse::<IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, 53))
    })
}

/// First `nameserver` of /etc/resolv.conf.
fn system_resolver() -> Option<SocketAddr> {
    let text = fs::read_to_string("/etc/resolv.conf").ok()?;
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("nameserver"))
        .filter_map(|rest| rest.trim().split('%').next()?.parse::<IpAddr>().ok())
        .map(|ip| SocketAddr::new(ip, 53))
        .next()
}

/// Comparable form of a record value: names lowercase without trailing dot,
/// addresses in canonical form, MX as "<preference> <name>".
fn normalize(kind: RecordType, value: &str) -> String {
    let value = value.trim();
    match kind {
        RecordType::A | RecordType::Aaaa => value
            .parse::<IpAddr>()
            .map_or_else(|_| value.to_string(), |ip| ip.to_string()),
        RecordType::Cname | RecordType::Ns => value.trim_end_matches('.').to_ascii_lowercase(),
        RecordType::Mx => {
            let mut parts = value.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some(pref), Some(host)) => {
                    format!("{pref} {}", host.trim_end_matches('.').to_ascii_lowercase())
                }
                _ => value.to_ascii_lowercase(),
            }
        }
        RecordType::Txt => value.to_string(),
    }
}

impl Check for DnsCheck {
    fn check(&self) -> CheckResult {
        let start = Instant::now();
        let id = query_id();
        let query = build_query(id, &self.name, self.record_type);
        let response = match exchange(self.server, &query, id, self.timeout) {
            Ok(r) => r,
            Err(msg) => return CheckResult::fail(Some(start.elapsed()), msg),
        };
        let latency = start.elapsed();
        let answers = match parse_response(&response, id, &self.name, self.record_type) {
            Ok(a) => a,
            Err(msg) => return CheckResult::fail(Some(latency), msg),
        };
        let kind = self.record_type.name();
        if answers.is_empty() {
            return CheckResult::fail(Some(latency), format!("no {kind} record"));
        }
        if let Some(expected) = &self.expected {
            let mut got: Vec<String> = answers
                .iter()
                .map(|a| normalize(self.record_type, a))
                .collect();
            got.sort();
            got.dedup();
            if &got != expected {
                // The received values are not shown (they come from the network).
                return CheckResult::fail(Some(latency), format!("unexpected {kind} records"));
            }
        }
        let plural = if answers.len() == 1 { "" } else { "s" };
        CheckResult::up(latency, format!("{} {kind} record{plural}", answers.len()))
    }

    fn target(&self) -> String {
        format!("{} {} @{}", self.name, self.record_type.name(), self.server)
    }
}

fn query_id() -> u16 {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 2];
    // A predictable ID only weakens spoofing protection; the check still works.
    let _ = ring::rand::SystemRandom::new().fill(&mut bytes);
    u16::from_be_bytes(bytes)
}

fn build_query(id: u16, name: &str, kind: RecordType) -> Vec<u8> {
    let mut q = Vec::with_capacity(64);
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00]); // recursion desired
    q.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 1]); // 1 question, 1 additional (EDNS)
    for label in name.split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&kind.code().to_be_bytes());
    q.extend_from_slice(&[0, 1]); // class IN
    // EDNS OPT record: root name, type 41, UDP payload size, no flags.
    q.push(0);
    q.extend_from_slice(&41u16.to_be_bytes());
    q.extend_from_slice(&(MAX_RESPONSE as u16).to_be_bytes());
    q.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    q
}

fn exchange(
    server: SocketAddr,
    query: &[u8],
    id: u16,
    timeout: Duration,
) -> Result<Vec<u8>, &'static str> {
    let start = Instant::now();
    let local: SocketAddr = match server.ip() {
        IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(local).map_err(|_| "cannot open UDP socket")?;
    // connect() makes the kernel drop datagrams from other addresses.
    socket.connect(server).map_err(|_| "network unreachable")?;
    socket.send(query).map_err(|_| "network unreachable")?;
    let mut buf = [0u8; MAX_RESPONSE + 1];
    loop {
        let left = timeout.saturating_sub(start.elapsed());
        if left.is_zero() {
            return Err("timeout");
        }
        socket
            .set_read_timeout(Some(left))
            .map_err(|_| "cannot open UDP socket")?;
        match socket.recv(&mut buf) {
            Ok(n) if n > MAX_RESPONSE => return Err("response too large"),
            // Responses to other queries are ignored, not errors.
            Ok(n) if n >= 2 && buf[..2] == id.to_be_bytes() => return Ok(buf[..n].to_vec()),
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err("timeout");
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                return Err("connection refused");
            }
            Err(_) => return Err("receive failed"),
        }
    }
}

struct Reader<'a> {
    msg: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let b = *self.msg.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes([self.u8()?, self.u8()?]))
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        let end = self.pos.checked_add(n)?;
        (end <= self.msg.len()).then(|| self.pos = end)
    }

    /// A domain name at the current position (following compression
    /// pointers), lowercase and without the trailing dot.
    fn name(&mut self) -> Option<String> {
        let mut out = String::new();
        let mut pos = self.pos;
        let mut jumped = false;
        let mut pointers = 0;
        loop {
            let len = *self.msg.get(pos)?;
            match len {
                0 => {
                    if !jumped {
                        self.pos = pos + 1;
                    }
                    return Some(out);
                }
                l if l & 0xc0 == 0xc0 => {
                    let target =
                        usize::from(u16::from_be_bytes([l & 0x3f, *self.msg.get(pos + 1)?]));
                    pointers += 1;
                    if pointers > MAX_POINTERS || target >= self.msg.len() {
                        return None;
                    }
                    if !jumped {
                        self.pos = pos + 2;
                        jumped = true;
                    }
                    pos = target;
                }
                l if l < 64 => {
                    let label = self.msg.get(pos + 1..pos + 1 + usize::from(l))?;
                    if !out.is_empty() {
                        out.push('.');
                    }
                    out.push_str(&String::from_utf8_lossy(label).to_ascii_lowercase());
                    if out.len() > 255 {
                        return None;
                    }
                    pos += 1 + usize::from(l);
                }
                _ => return None, // reserved label types
            }
        }
    }
}

/// Values of the answers of `kind` for `name`. Errors are coarse classes.
pub(crate) fn parse_response(
    msg: &[u8],
    id: u16,
    name: &str,
    kind: RecordType,
) -> Result<Vec<String>, &'static str> {
    const MALFORMED: &str = "malformed DNS response";
    let mut r = Reader { msg, pos: 0 };
    if r.u16().ok_or(MALFORMED)? != id {
        return Err(MALFORMED);
    }
    let flags = r.u16().ok_or(MALFORMED)?;
    if flags & 0x8000 == 0 {
        return Err(MALFORMED); // not a response
    }
    if flags & 0x0200 != 0 {
        return Err("DNS response truncated");
    }
    match flags & 0x000f {
        0 => {}
        2 => return Err("DNS server failure"),
        3 => return Err("domain does not exist"),
        5 => return Err("DNS query refused"),
        _ => return Err("DNS error"),
    }
    let questions = r.u16().ok_or(MALFORMED)?;
    let answers = r.u16().ok_or(MALFORMED)?;
    r.skip(4).ok_or(MALFORMED)?; // authority and additional counts
    for _ in 0..questions {
        r.name().ok_or(MALFORMED)?;
        r.skip(4).ok_or(MALFORMED)?;
    }
    let mut values = Vec::new();
    // The answer may start with a CNAME chain; the records for its target
    // belong to the query too.
    let mut owners = vec![name.to_string()];
    for _ in 0..answers {
        let owner = r.name().ok_or(MALFORMED)?;
        let rtype = r.u16().ok_or(MALFORMED)?;
        let class = r.u16().ok_or(MALFORMED)?;
        r.skip(4).ok_or(MALFORMED)?; // TTL
        let len = usize::from(r.u16().ok_or(MALFORMED)?);
        let start = r.pos;
        r.skip(len).ok_or(MALFORMED)?;
        let rdata = &msg[start..start + len];
        if class != 1 || !owners.contains(&owner) {
            continue;
        }
        if rtype == RecordType::Cname.code() && kind != RecordType::Cname {
            let mut sub = Reader { msg, pos: start };
            owners.push(sub.name().ok_or(MALFORMED)?);
            continue;
        }
        if rtype != kind.code() {
            continue;
        }
        let value = match kind {
            RecordType::A if len == 4 => {
                Ipv4Addr::new(rdata[0], rdata[1], rdata[2], rdata[3]).to_string()
            }
            RecordType::Aaaa if len == 16 => {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(rdata);
                Ipv6Addr::from(octets).to_string()
            }
            RecordType::Cname | RecordType::Ns => {
                Reader { msg, pos: start }.name().ok_or(MALFORMED)?
            }
            RecordType::Mx if len >= 3 => {
                let mut sub = Reader { msg, pos: start };
                let preference = sub.u16().ok_or(MALFORMED)?;
                format!("{preference} {}", sub.name().ok_or(MALFORMED)?)
            }
            RecordType::Txt => {
                let mut text = Vec::new();
                let mut i = 0;
                while i < rdata.len() {
                    let n = usize::from(rdata[i]);
                    text.extend_from_slice(rdata.get(i + 1..i + 1 + n).ok_or(MALFORMED)?);
                    i += 1 + n;
                }
                String::from_utf8_lossy(&text).into_owned()
            }
            _ => return Err(MALFORMED),
        };
        values.push(value);
        if values.len() > 64 {
            return Err("too many records");
        }
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitors::Status;
    use std::thread;

    /// A response to `query` with the given answer records (owner name as a
    /// pointer to the question, type, rdata).
    fn response(query: &[u8], rcode: u8, answers: &[(u16, Vec<u8>)]) -> Vec<u8> {
        // Copy header and question (up to the EDNS record, 11 bytes at the end).
        let question_end = query.len() - 11;
        let mut r = query[..question_end].to_vec();
        r[2] = 0x81;
        r[3] = 0x80 | rcode;
        r[6..8].copy_from_slice(&(answers.len() as u16).to_be_bytes());
        r[10..12].copy_from_slice(&[0, 0]);
        for (rtype, rdata) in answers {
            r.extend_from_slice(&[0xc0, 12]); // pointer to the question name
            r.extend_from_slice(&rtype.to_be_bytes());
            r.extend_from_slice(&[0, 1, 0, 0, 0x0e, 0x10]);
            r.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
            r.extend_from_slice(rdata);
        }
        r
    }

    fn name_bytes(name: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for label in name.split('.') {
            out.push(label.len() as u8);
            out.extend_from_slice(label.as_bytes());
        }
        out.push(0);
        out
    }

    fn parse(answers: &[(u16, Vec<u8>)], kind: RecordType) -> Result<Vec<String>, &'static str> {
        let q = build_query(0x1234, "example.org", kind);
        parse_response(&response(&q, 0, answers), 0x1234, "example.org", kind)
    }

    #[test]
    fn query_format() {
        let q = build_query(0xabcd, "example.org", RecordType::Aaaa);
        assert_eq!(&q[..12], &[0xab, 0xcd, 1, 0, 0, 1, 0, 0, 0, 0, 0, 1]);
        assert_eq!(&q[12..25], b"\x07example\x03org\x00");
        assert_eq!(&q[25..29], &[0, 28, 0, 1]);
        assert_eq!(q.len(), 29 + 11);
    }

    #[test]
    fn record_types() {
        assert_eq!(
            parse(&[(1, vec![93, 184, 216, 34])], RecordType::A).unwrap(),
            ["93.184.216.34"]
        );
        let v6 = "2606:2800:220:1::1"
            .parse::<Ipv6Addr>()
            .unwrap()
            .octets()
            .to_vec();
        assert_eq!(
            parse(&[(28, v6)], RecordType::Aaaa).unwrap(),
            ["2606:2800:220:1::1"]
        );
        let mut mx = vec![0, 10];
        mx.extend(name_bytes("Mail.Example.org"));
        assert_eq!(
            parse(&[(15, mx)], RecordType::Mx).unwrap(),
            ["10 mail.example.org"]
        );
        assert_eq!(
            parse(&[(16, b"\x05hello\x06 world".to_vec())], RecordType::Txt).unwrap(),
            ["hello world"]
        );
        assert_eq!(
            parse(&[(2, name_bytes("ns1.example.org"))], RecordType::Ns).unwrap(),
            ["ns1.example.org"]
        );
        // Other types in the answer are ignored.
        assert!(
            parse(&[(16, b"\x01x".to_vec())], RecordType::A)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cname_chain() {
        let q = build_query(1, "www.example.org", RecordType::A);
        let mut r = response(&q, 0, &[(5, name_bytes("cdn.example.net"))]);
        // Second answer: cdn.example.net A 1.2.3.4
        r[6..8].copy_from_slice(&2u16.to_be_bytes());
        r.extend(name_bytes("cdn.example.net"));
        r.extend_from_slice(&[0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 1, 2, 3, 4]);
        assert_eq!(
            parse_response(&r, 1, "www.example.org", RecordType::A).unwrap(),
            ["1.2.3.4"]
        );
    }

    #[test]
    fn errors() {
        let q = build_query(7, "example.org", RecordType::A);
        assert_eq!(
            parse_response(&response(&q, 3, &[]), 7, "example.org", RecordType::A),
            Err("domain does not exist")
        );
        assert_eq!(
            parse_response(&response(&q, 2, &[]), 7, "example.org", RecordType::A),
            Err("DNS server failure")
        );
        assert!(parse_response(&response(&q, 0, &[]), 8, "example.org", RecordType::A).is_err());
        let mut truncated = response(&q, 0, &[]);
        truncated[2] |= 0x02;
        assert_eq!(
            parse_response(&truncated, 7, "example.org", RecordType::A),
            Err("DNS response truncated")
        );
        // A pointer loop.
        let mut looped = response(&q, 0, &[(5, vec![0xc0, 0])]);
        let len = looped.len();
        looped[len - 2..].copy_from_slice(&[0xc0, (len - 2) as u8]);
        assert!(parse_response(&looped, 7, "example.org", RecordType::Cname).is_err());
    }

    #[test]
    fn malformed_responses_never_panic() {
        let q = build_query(9, "example.org", RecordType::Mx);
        let mut mx = vec![0, 10];
        mx.extend(name_bytes("mail.example.org"));
        let good = response(&q, 0, &[(15, mx), (1, vec![1, 2, 3, 4])]);
        for len in 0..good.len() {
            let _ = parse_response(&good[..len], 9, "example.org", RecordType::Mx);
        }
        let mut corrupted = good.clone();
        for i in 0..corrupted.len() {
            let original = corrupted[i];
            for value in [0x00, 0x3f, 0x40, 0xc0, 0xff] {
                corrupted[i] = value;
                for kind in [
                    RecordType::A,
                    RecordType::Mx,
                    RecordType::Txt,
                    RecordType::Cname,
                ] {
                    let _ = parse_response(&corrupted, 9, "example.org", kind);
                }
            }
            corrupted[i] = original;
        }
    }

    #[test]
    fn names() {
        for good in [
            "example.org",
            "_dmarc.example.org",
            "_acme-challenge.www.example.org",
            "localhost",
        ] {
            assert!(is_valid_name(good), "{good}");
        }
        for bad in [
            "",
            "1.2.3.4",
            "::1",
            "a..b",
            "-a.org",
            "a-.org",
            "a b.org",
            "\u{e4}.org",
            &"a".repeat(64),
        ] {
            assert!(!is_valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn normalization() {
        assert_eq!(
            normalize(RecordType::Aaaa, "2001:DB8:0:0::1"),
            "2001:db8::1"
        );
        assert_eq!(
            normalize(RecordType::Cname, "CDN.Example.net."),
            "cdn.example.net"
        );
        assert_eq!(
            normalize(RecordType::Mx, "10  Mail.Example.org."),
            "10 mail.example.org"
        );
    }

    /// A UDP server answering one query with `answers`.
    fn server(answers: Vec<(u16, Vec<u8>)>, rcode: u8) -> SocketAddr {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = socket.local_addr().unwrap();
        thread::spawn(move || {
            let mut buf = [0u8; 512];
            let (n, from) = socket.recv_from(&mut buf).unwrap();
            // First a reply with the wrong ID (must be ignored), then the real one.
            let mut wrong = response(&buf[..n], rcode, &answers);
            wrong[0] ^= 0xff;
            socket.send_to(&wrong, from).unwrap();
            socket
                .send_to(&response(&buf[..n], rcode, &answers), from)
                .unwrap();
        });
        addr
    }

    fn check(server: SocketAddr, expected: Option<Vec<&str>>) -> CheckResult {
        DnsCheck {
            name: "example.org".into(),
            record_type: RecordType::A,
            server,
            // Sorted, as build() does.
            expected: expected.map(|e| {
                let mut v: Vec<String> = e.iter().map(|v| normalize(RecordType::A, v)).collect();
                v.sort();
                v
            }),
            timeout: Duration::from_secs(2),
        }
        .check()
    }

    #[test]
    fn checks_over_udp() {
        let ok = check(
            server(vec![(1, vec![1, 2, 3, 4]), (1, vec![5, 6, 7, 8])], 0),
            Some(vec!["5.6.7.8", "1.2.3.4"]),
        );
        assert_eq!((ok.status, ok.msg.as_str()), (Status::Up, "2 A records"));
        let wrong = check(
            server(vec![(1, vec![1, 2, 3, 4])], 0),
            Some(vec!["9.9.9.9"]),
        );
        assert_eq!(
            (wrong.status, wrong.msg.as_str()),
            (Status::Fail, "unexpected A records")
        );
        let none = check(server(vec![], 0), None);
        assert_eq!(none.msg, "no A record");
        let nx = check(server(vec![], 3), None);
        assert_eq!(nx.msg, "domain does not exist");
        // Nobody listening.
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = silent.local_addr().unwrap();
        let start = Instant::now();
        let r = DnsCheck {
            name: "example.org".into(),
            record_type: RecordType::A,
            server: addr,
            expected: None,
            timeout: Duration::from_millis(300),
        }
        .check();
        assert_eq!(r.msg, "timeout");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn servers() {
        assert_eq!(parse_server("1.1.1.1"), Some("1.1.1.1:53".parse().unwrap()));
        assert_eq!(
            parse_server("9.9.9.9:5353"),
            Some("9.9.9.9:5353".parse().unwrap())
        );
        assert_eq!(parse_server("[::1]:53"), Some("[::1]:53".parse().unwrap()));
        assert_eq!(parse_server("::1"), Some("[::1]:53".parse().unwrap()));
        assert_eq!(parse_server("dns.google"), None);
    }
}
