//! TLS certificate expiry (SPEC.md §4.3 `tls_expiry_warn` / `tls_expiry_fail`).
//!
//! ureq does not expose the server certificate, so we do a separate TLS
//! handshake with rustls and read `notAfter` from the leaf certificate with a
//! minimal, bounds-checked DER reader. Only the validity dates are read;
//! nothing else from the certificate is used or reported.

use std::io::{self, Read};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, RootCertStore, SignatureScheme,
};

use super::{CheckResult, Status};
use crate::clock;

/// Fetch the leaf certificate of `host:port` and return its `notAfter` as
/// Unix seconds. Errors are coarse classes, never remote data.
pub fn not_after(
    host: &str,
    port: u16,
    verify: bool,
    timeout: Duration,
) -> Result<i64, &'static str> {
    let start = Instant::now();
    let remaining = || timeout.saturating_sub(start.elapsed());
    let addrs = super::resolve(host, port, remaining())?;
    let mut sock = None;
    for addr in addrs {
        if remaining().is_zero() {
            break;
        }
        if let Ok(s) = TcpStream::connect_timeout(&addr, remaining()) {
            sock = Some(s);
            break;
        }
    }
    let sock = sock.ok_or("connection failed")?;
    let conn = handshake(&sock, host, verify, remaining())?;
    leaf_not_after(&conn)
}

/// A TLS handshake as client on a connected socket, within `timeout`.
/// The certificate is verified against the bundled roots unless `verify`
/// is false. Errors are coarse classes (see [`message`]).
pub fn handshake(
    sock: &TcpStream,
    host: &str,
    verify: bool,
    timeout: Duration,
) -> Result<ClientConnection, &'static str> {
    let deadline = Instant::now() + timeout;
    let server_name = ServerName::try_from(host.to_string()).map_err(|_| "invalid host name")?;
    let config = Arc::new(client_config(verify).map_err(|_| "TLS setup failed")?);
    let mut conn = ClientConnection::new(config, server_name).map_err(|_| "TLS setup failed")?;
    while conn.is_handshaking() {
        match exchange(&mut conn, sock, deadline)? {
            Exchange::Progress => {}
            Exchange::Closed => return Err("TLS handshake failed"),
        }
    }
    // TLS 1.3: the client's Finished may still be waiting.
    flush(&mut conn, sock, deadline)?;
    Ok(conn)
}

enum Exchange {
    Progress,
    Closed,
}

/// One step of TLS I/O by hand: write what is pending, read once,
/// process. The deadline is checked before every system call, so a peer
/// that sends one byte at a time cannot stretch a check (rustls'
/// `complete_io` and `Stream` loop until a record is complete, with only
/// the per-call socket timeout).
fn exchange(
    conn: &mut ClientConnection,
    sock: &TcpStream,
    deadline: Instant,
) -> Result<Exchange, &'static str> {
    flush(conn, sock, deadline)?;
    if !conn.wants_read() {
        return Ok(Exchange::Progress);
    }
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err("timeout");
    }
    sock.set_read_timeout(Some(left))
        .map_err(|_| "connection failed")?;
    let mut io = sock;
    match conn.read_tls(&mut io) {
        Ok(0) => return Ok(Exchange::Closed),
        Ok(_) => {}
        Err(e) => return Err(io_message(&e)),
    }
    if let Err(e) = conn.process_new_packets() {
        // Send the alert, but the error is what counts.
        let _ = flush(conn, sock, deadline);
        return Err(message(&e));
    }
    Ok(Exchange::Progress)
}

fn flush(
    conn: &mut ClientConnection,
    sock: &TcpStream,
    deadline: Instant,
) -> Result<(), &'static str> {
    let mut io = sock;
    while conn.wants_write() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("timeout");
        }
        sock.set_write_timeout(Some(left))
            .map_err(|_| "connection failed")?;
        conn.write_tls(&mut io).map_err(|e| io_message(&e))?;
    }
    Ok(())
}

/// Plaintext from an established connection, at most `buf.len()` bytes,
/// until `deadline` (see [`exchange`]). `Ok(0)`: the peer closed.
pub fn read(
    conn: &mut ClientConnection,
    sock: &TcpStream,
    deadline: Instant,
    buf: &mut [u8],
) -> io::Result<usize> {
    loop {
        match conn.reader().read(buf) {
            Ok(n) => return Ok(n),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            // Closed without close_notify: for a banner, the same as closed.
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(0),
            Err(e) => return Err(e),
        }
        match exchange(conn, sock, deadline) {
            Ok(Exchange::Progress) => {}
            Ok(Exchange::Closed) => return Ok(0),
            Err("timeout") => return Err(io::ErrorKind::TimedOut.into()),
            Err(msg) => return Err(io::Error::other(msg)),
        }
    }
}

/// `notAfter` of the server's leaf certificate after a handshake.
pub fn leaf_not_after(conn: &ClientConnection) -> Result<i64, &'static str> {
    let leaf = conn
        .peer_certificates()
        .and_then(|certs| certs.first())
        .ok_or("no certificate")?;
    cert_not_after(leaf.as_ref()).ok_or("unreadable certificate")
}

/// An I/O error during a handshake: rustls errors arrive wrapped in it.
fn io_message(e: &io::Error) -> &'static str {
    if matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        return "timeout";
    }
    match e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
    {
        Some(tls) => message(tls),
        None => "TLS handshake failed",
    }
}

/// Coarse classes of TLS errors. Some error types (e.g. certificate name
/// mismatch) carry data from the remote side, which must not be reflected.
pub fn message(e: &rustls::Error) -> &'static str {
    use rustls::{CertificateError, Error as TlsError};
    match e {
        TlsError::InvalidCertificate(cert) => match cert {
            CertificateError::Expired | CertificateError::ExpiredContext { .. } => {
                "TLS certificate expired"
            }
            CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => {
                "TLS certificate not yet valid"
            }
            CertificateError::UnknownIssuer => "TLS certificate from unknown issuer",
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
                "TLS certificate not valid for this host name"
            }
            CertificateError::Revoked => "TLS certificate revoked",
            _ => "TLS certificate invalid",
        },
        TlsError::AlertReceived(_) => "TLS alert from server",
        _ => "TLS handshake failed",
    }
}

/// Turn a successful result into WARN or FAIL if the certificate expires
/// within `warn` or `fail` ("0s": off), and record the expiry date.
pub fn rate_expiry(
    mut result: CheckResult,
    not_after: Result<i64, &'static str>,
    now: i64,
    warn: Duration,
    fail: Duration,
) -> CheckResult {
    let Ok(not_after) = not_after else {
        if result.status == Status::Up {
            result.status = Status::Warn;
        }
        result.msg.push_str(", certificate check failed");
        return result;
    };
    result.cert_expires = Some(not_after);
    let left = not_after - now;
    let text = if left <= 0 {
        "certificate expired".to_string()
    } else {
        format!("certificate expires in {} days", left / 86400)
    };
    let within = |limit: Duration| {
        !limit.is_zero() && left <= i64::try_from(limit.as_secs()).unwrap_or(i64::MAX)
    };
    if within(fail) {
        result.status = Status::Fail;
        result.msg = format!("{}, {text}", result.msg);
    } else if within(warn) {
        if result.status == Status::Up {
            result.status = Status::Warn;
        }
        result.msg = format!("{}, {text}", result.msg);
    }
    result
}

fn client_config(verify: bool) -> Result<ClientConfig, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?;
    let config = if verify {
        let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate(provider)))
            .with_no_client_auth()
    };
    Ok(config)
}

/// For `tls_verify = false`: accept any certificate, but still check the
/// handshake signatures so the connection is a real TLS connection.
#[derive(Debug)]
struct AcceptAnyCertificate(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

// ---- minimal DER reader --------------------------------------------------

const SEQUENCE: u8 = 0x30;
const INTEGER: u8 = 0x02;
const UTC_TIME: u8 = 0x17;
const GENERALIZED_TIME: u8 = 0x18;
const EXPLICIT_0: u8 = 0xa0;

/// Split one TLV off the front of `input`: (tag, value, rest).
fn tlv(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = input.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let n = usize::from(first & 0x7f);
        // Certificates are small: at most 4 length bytes, no indefinite form.
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        let len = rest[..n]
            .iter()
            .fold(0usize, |acc, &b| (acc << 8) | usize::from(b));
        (len, &rest[n..])
    };
    if rest.len() < len {
        return None;
    }
    let (value, rest) = rest.split_at(len);
    Some((tag, value, rest))
}

fn expect(input: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (t, value, rest) = tlv(input)?;
    (t == tag).then_some((value, rest))
}

/// `notAfter` of an X.509 certificate (RFC 5280 §4.1) as Unix seconds.
pub fn cert_not_after(der: &[u8]) -> Option<i64> {
    let (cert, _) = expect(der, SEQUENCE)?;
    let (tbs, _) = expect(cert, SEQUENCE)?;
    let mut rest = tbs;
    if rest.first() == Some(&EXPLICIT_0) {
        rest = tlv(rest)?.2; // version
    }
    rest = expect(rest, INTEGER)?.1; // serialNumber
    rest = expect(rest, SEQUENCE)?.1; // signature algorithm
    rest = expect(rest, SEQUENCE)?.1; // issuer
    let (validity, _) = expect(rest, SEQUENCE)?;
    let (_, _, after_not_before) = tlv(validity)?; // notBefore
    let (tag, not_after, _) = tlv(after_not_before)?;
    parse_time(tag, not_after)
}

/// UTCTime `YYMMDDHHMMSSZ` or GeneralizedTime `YYYYMMDDHHMMSSZ`.
fn parse_time(tag: u8, value: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(value).ok()?;
    let digits = text.strip_suffix('Z')?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (year, rest) = match (tag, digits.len()) {
        (UTC_TIME, 12) => {
            let yy: i64 = digits[..2].parse().ok()?;
            // RFC 5280: 50-99 → 19xx, 00-49 → 20xx.
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, &digits[2..])
        }
        (GENERALIZED_TIME, 14) => (digits[..4].parse().ok()?, &digits[4..]),
        _ => return None,
    };
    let field = |i: usize| -> Option<i64> { rest.get(i..i + 2)?.parse().ok() };
    let (month, day, hour, minute, second) =
        (field(0)?, field(2)?, field(4)?, field(6)?, field(8)?);
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = clock::parse_date(&format!("{year:04}-{month:02}-{day:02}"))?;
    Some(days * 86400 + hour * 3600 + minute * 60 + second)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    const UTC_CERT: &[u8] = include_bytes!("../../testdata/tls/utctime.der");
    use testing::GENERALIZED_CERT;

    #[test]
    fn not_after_from_der() {
        // 2030-01-02T03:04:05Z and 2051-06-01T12:00:00Z (see generate.py).
        assert_eq!(cert_not_after(UTC_CERT), Some(1_893_553_445));
        assert_eq!(cert_not_after(GENERALIZED_CERT), Some(2_569_233_600));
    }

    #[test]
    fn malformed_der_never_panics() {
        for len in 0..UTC_CERT.len() {
            assert_eq!(cert_not_after(&UTC_CERT[..len]), None, "truncated at {len}");
        }
        let mut corrupted = UTC_CERT.to_vec();
        for i in 0..corrupted.len() {
            let original = corrupted[i];
            for value in [0x00, 0x30, 0x80, 0x84, 0xff] {
                corrupted[i] = value;
                let _ = cert_not_after(&corrupted);
            }
            corrupted[i] = original;
        }
        assert_eq!(cert_not_after(&[0x30, 0x84, 0xff, 0xff, 0xff, 0xff]), None);
    }

    #[test]
    fn times() {
        assert_eq!(parse_time(UTC_TIME, b"700101000000Z"), Some(0));
        assert_eq!(parse_time(UTC_TIME, b"491231235959Z"), Some(2_524_607_999));
        assert_eq!(
            parse_time(GENERALIZED_TIME, b"20500101000000Z"),
            Some(2_524_608_000)
        );
        for (tag, bad) in [
            (UTC_TIME, &b"700101000000"[..]),
            (UTC_TIME, b"7001010000Z"),
            (UTC_TIME, b"701301000000Z"),
            (UTC_TIME, b"700101240000Z"),
            (UTC_TIME, b"70010100000xZ"),
            (GENERALIZED_TIME, b"700101000000Z"),
            (0x04, b"700101000000Z"),
        ] {
            assert_eq!(parse_time(tag, bad), None, "{:?}", std::str::from_utf8(bad));
        }
    }

    #[test]
    fn handshake_reads_expiry() {
        let port = testing::server(GENERALIZED_CERT, b"", 2);
        let timeout = Duration::from_secs(5);
        // Self-signed: accepted without verification ...
        assert_eq!(
            not_after("localhost", port, false, timeout),
            Ok(2_569_233_600)
        );
        // ... and rejected with it.
        assert_eq!(
            not_after("localhost", port, true, timeout),
            Err("TLS certificate from unknown issuer")
        );
    }

    /// A peer that announces a large TLS record and then sends one byte
    /// every 50 ms must not stretch the check past its timeout.
    #[test]
    fn trickling_peer_is_bounded_by_the_timeout() {
        use std::io::Write;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Some(Ok(mut stream)) = listener.incoming().next() {
                // Handshake record header, 16 KiB announced.
                let _ = stream.write_all(&[0x16, 0x03, 0x03, 0x40, 0x00]);
                for _ in 0..200 {
                    if stream.write_all(&[0]).is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        });
        let start = Instant::now();
        let result = not_after("127.0.0.1", port, false, Duration::from_millis(800));
        assert_eq!(result, Err("timeout"));
        assert!(
            start.elapsed() < Duration::from_millis(1500),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn connection_errors() {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        assert_eq!(
            not_after("127.0.0.1", port, true, Duration::from_secs(2)),
            Err("connection failed")
        );
        assert_eq!(
            not_after("does-not-exist.invalid", 443, true, Duration::from_secs(2)),
            Err("host not found")
        );
    }
}

/// A local TLS server for tests of the monitors that use TLS.
#[cfg(test)]
pub mod testing {
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
    use rustls::{ServerConfig, ServerConnection};

    /// Self-signed for `localhost`, valid until 2051-06-01 (notAfter
    /// 2_569_233_600).
    pub const GENERALIZED_CERT: &[u8] = include_bytes!("../../testdata/tls/generalized.der");
    const KEY: &[u8] = include_bytes!("../../testdata/tls/key.pkcs8.der");

    /// Serves `connections` clients: handshake with `cert`, then sends
    /// `greeting` and keeps the connection open for 300 ms.
    pub fn server(cert: &'static [u8], greeting: &'static [u8], connections: usize) -> u16 {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = Arc::new(
            ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(
                    vec![CertificateDer::from(cert.to_vec())],
                    PrivatePkcs8KeyDer::from(KEY.to_vec()).into(),
                )
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for stream in listener.incoming().take(connections) {
                let mut stream = stream.unwrap();
                let mut conn = ServerConnection::new(config.clone()).unwrap();
                while conn.is_handshaking() {
                    if conn.complete_io(&mut stream).is_err() {
                        break;
                    }
                }
                if !conn.is_handshaking() && !greeting.is_empty() {
                    let _ = conn.writer().write_all(greeting);
                    let _ = conn.complete_io(&mut stream);
                }
                thread::sleep(Duration::from_millis(300));
            }
        });
        port
    }
}
