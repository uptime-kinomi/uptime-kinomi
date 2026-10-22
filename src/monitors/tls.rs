//! TLS certificate expiry (SPEC.md §4.3 `tls_expiry_warn` / `tls_expiry_down`).
//!
//! ureq does not expose the server certificate, so we do a separate TLS
//! handshake with rustls and read `notAfter` from the leaf certificate with a
//! minimal, bounds-checked DER reader. Only the validity dates are read;
//! nothing else from the certificate is used or reported.

use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, RootCertStore, SignatureScheme,
};

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
    let server_name = ServerName::try_from(host.to_string()).map_err(|_| "invalid host name")?;
    let config = Arc::new(client_config(verify).map_err(|_| "TLS setup failed")?);
    let mut conn = ClientConnection::new(config, server_name).map_err(|_| "TLS setup failed")?;

    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|_| "host not found")?;
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
    let mut sock = sock.ok_or("connection failed")?;

    while conn.is_handshaking() {
        let left = remaining();
        if left.is_zero() {
            return Err("timeout");
        }
        let io = sock
            .set_read_timeout(Some(left))
            .and_then(|()| sock.set_write_timeout(Some(left)));
        io.map_err(|_| "connection failed")?;
        match conn.complete_io(&mut sock) {
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Err("timeout");
            }
            Err(_) => return Err("TLS handshake failed"),
        }
    }
    let leaf = conn
        .peer_certificates()
        .and_then(|certs| certs.first())
        .ok_or("no certificate")?;
    cert_not_after(leaf.as_ref()).ok_or("unreadable certificate")
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
    use rustls::pki_types::PrivatePkcs8KeyDer;
    use rustls::{ServerConfig, ServerConnection};
    use std::net::TcpListener;
    use std::thread;

    const UTC_CERT: &[u8] = include_bytes!("../../testdata/tls/utctime.der");
    const GENERALIZED_CERT: &[u8] = include_bytes!("../../testdata/tls/generalized.der");
    const KEY: &[u8] = include_bytes!("../../testdata/tls/key.pkcs8.der");

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

    /// A TLS server with the self-signed test certificate.
    fn tls_server(cert: &'static [u8]) -> u16 {
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
            for stream in listener.incoming().take(2) {
                let mut stream = stream.unwrap();
                let mut conn = ServerConnection::new(config.clone()).unwrap();
                while conn.is_handshaking() {
                    if conn.complete_io(&mut stream).is_err() {
                        break;
                    }
                }
            }
        });
        port
    }

    #[test]
    fn handshake_reads_expiry() {
        let port = tls_server(GENERALIZED_CERT);
        let timeout = Duration::from_secs(5);
        // Self-signed: accepted without verification ...
        assert_eq!(
            not_after("localhost", port, false, timeout),
            Ok(2_569_233_600)
        );
        // ... and rejected with it.
        assert_eq!(
            not_after("localhost", port, true, timeout),
            Err("TLS handshake failed")
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
