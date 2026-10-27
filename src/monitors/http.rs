//! `http` monitor: HTTP(S) status code, keyword and response time
//! (SPEC.md §4.3).
//!
//! Redirects are followed by hand so that every hop is checked (http(s)
//! only, never from https to http). The body is read up to
//! `max_body_bytes` and only searched for the keyword; nothing received is
//! kept or reported.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;

use super::{Check, CheckResult, Context, Status, contains_bytes, tls};
use crate::clock;
use crate::config::duration::Dur;
use crate::config::secret::{self, is_world_readable};
use crate::config::{FileProblems, resolve_path};

const DEFAULT_MAX_BODY: u64 = 64 * 1024;
const MAX_BODY: u64 = 1024 * 1024;
const MAX_REDIRECTS: u32 = 10;

/// A successful certificate check is reused this long (one extra TLS
/// handshake per check would be expensive on a Pi 1).
const CERT_CACHE: Duration = Duration::from_secs(6 * 3600);
/// A failed certificate check is retried after this time.
const CERT_RETRY: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
enum Method {
    #[default]
    Get,
    Head,
}

/// `200` or `"200-299"`.
#[derive(Deserialize)]
#[serde(untagged)]
enum StatusSpec {
    Code(u16),
    Range(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpFile {
    url: String,
    #[serde(default)]
    method: Method,
    expected_status: Option<Vec<StatusSpec>>,
    keyword: Option<String>,
    #[serde(default)]
    invert_keyword: bool,
    max_body_bytes: Option<u64>,
    #[serde(default)]
    max_redirects: u32,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    basic_auth_user: Option<String>,
    basic_auth_password: Option<String>,
    basic_auth_password_file: Option<PathBuf>,
    tls_verify: Option<bool>,
    tls_expiry_warn: Option<Dur>,
    tls_expiry_down: Option<Dur>,
    slow_warn: Option<Dur>,
}

pub struct HttpCheck {
    url: String,
    method: Method,
    expected: Vec<(u16, u16)>,
    keyword: Option<Vec<u8>>,
    invert_keyword: bool,
    max_body: u64,
    max_redirects: u32,
    headers: Vec<(String, String)>,
    authorization: Option<String>,
    slow_warn: Option<Duration>,
    cert: Option<CertCheck>,
    agent: ureq::Agent,
}

/// Certificate expiry check of the configured https URL's host.
struct CertCheck {
    host: String,
    port: u16,
    verify: bool,
    warn: Duration,
    down: Duration,
    timeout: Duration,
    /// (checked at, notAfter or error)
    cache: Mutex<Option<(Instant, Result<i64, &'static str>)>>,
}

pub fn build(
    file: HttpFile,
    timeout: Duration,
    ctx: &Context,
    p: &mut FileProblems,
) -> Option<HttpCheck> {
    let is_https = file.url.starts_with("https://");
    if !(is_https || file.url.starts_with("http://")) {
        p.error("http.url must start with http:// or https://");
    } else if file.url.len() > 2048 || ureq::http::Uri::try_from(file.url.as_str()).is_err() {
        p.error("http.url is not a valid URL");
    }

    let expected = match &file.expected_status {
        None => vec![(200, 299)],
        Some(list) if list.is_empty() => {
            p.error("http.expected_status must not be empty");
            vec![]
        }
        Some(list) => list
            .iter()
            .filter_map(|spec| {
                let range = parse_status(spec);
                if range.is_none() {
                    p.error("http.expected_status entries must be codes 100-599 or ranges like \"200-299\"");
                }
                range
            })
            .collect(),
    };

    if let Some(k) = &file.keyword {
        if k.is_empty() || k.len() > 256 {
            p.error("http.keyword must be 1 to 256 bytes");
        }
        if file.method == Method::Head {
            p.error("http.keyword needs method = \"GET\" (HEAD responses have no body)");
        }
    } else if file.invert_keyword {
        p.error("http.invert_keyword needs http.keyword");
    }

    let max_body = file.max_body_bytes.unwrap_or(DEFAULT_MAX_BODY);
    if max_body == 0 || max_body > MAX_BODY {
        p.error(format!(
            "http.max_body_bytes must be between 1 and {MAX_BODY}"
        ));
    }
    if file.max_redirects > MAX_REDIRECTS {
        p.error(format!(
            "http.max_redirects must be at most {MAX_REDIRECTS}"
        ));
    }

    let mut headers = Vec::new();
    for (name, value) in &file.headers {
        if !is_header_name(name) || !is_header_value(value) {
            p.error(format!("http.headers: invalid header '{name}'"));
        } else if name.eq_ignore_ascii_case("authorization") && file.basic_auth_user.is_some() {
            p.error("http.headers: Authorization conflicts with basic_auth_user");
        } else {
            headers.push((name.clone(), value.clone()));
        }
    }

    let mut warnings = Vec::new();
    let password_file = file
        .basic_auth_password_file
        .map(|f| resolve_path(ctx.config_dir, &f));
    let password = secret::resolve(
        "http.basic_auth_password",
        file.basic_auth_password.as_deref(),
        password_file.as_deref(),
        &mut warnings,
    )
    .unwrap_or_else(|e| {
        p.error(e);
        None
    });
    if file.basic_auth_password.is_some() && is_world_readable(ctx.file_meta) {
        warnings.push("contains http.basic_auth_password but is readable by all users (chmod o-r, or use basic_auth_password_file)".into());
    }
    for w in warnings {
        p.warning(w);
    }
    let authorization = match (&file.basic_auth_user, &password) {
        (Some(user), Some(pw)) => {
            if user.contains(':') || !is_header_value(user) {
                p.error("http.basic_auth_user must not contain ':' or control characters");
            }
            Some(format!(
                "Basic {}",
                crate::encoding::base64(format!("{user}:{}", pw.expose()).as_bytes())
            ))
        }
        (None, None) => None,
        _ => {
            p.error("http.basic_auth_user and a password must be given together");
            None
        }
    };

    let tls_verify = file.tls_verify.unwrap_or(true);
    let tls_expiry_warn = file.tls_expiry_warn.unwrap_or(Dur::secs(14 * 86400)).0;
    let tls_expiry_down = file.tls_expiry_down.unwrap_or(Dur::secs(0)).0;
    if !is_https
        && (file.tls_expiry_warn.is_some()
            || file.tls_expiry_down.is_some()
            || file.tls_verify.is_some())
    {
        p.error("http.tls_* options need an https:// URL");
    }
    if !tls_verify {
        p.warning("http.tls_verify = false: certificate errors are ignored");
    }
    let cert = if is_https && !(tls_expiry_warn.is_zero() && tls_expiry_down.is_zero()) {
        match host_port(&file.url) {
            Some((host, port)) => Some(CertCheck {
                host,
                port,
                verify: tls_verify,
                warn: tls_expiry_warn,
                down: tls_expiry_down,
                timeout,
                cache: Mutex::new(None),
            }),
            None => {
                p.error("http.url: cannot determine host and port for the certificate check");
                None
            }
        }
    } else {
        None
    };
    let slow_warn = file.slow_warn.filter(|d| !d.is_zero()).map(|d| d.0);
    if slow_warn.is_some_and(|s| s >= timeout) {
        p.error("http.slow_warn must be less than timeout");
    }

    let agent = ureq::Agent::config_builder()
        // Redirects are handled in check() so every hop can be validated.
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .user_agent(format!("uptime-kinomi/{}", env!("CARGO_PKG_VERSION")))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .disable_verification(!tls_verify)
                .build(),
        )
        .build()
        .new_agent();

    Some(HttpCheck {
        url: file.url,
        method: file.method,
        expected,
        keyword: file.keyword.map(String::into_bytes),
        invert_keyword: file.invert_keyword,
        max_body,
        max_redirects: file.max_redirects,
        headers,
        authorization,
        slow_warn,
        cert,
        agent,
    })
}

impl Check for HttpCheck {
    fn check(&self) -> CheckResult {
        let result = self.http_check();
        match &self.cert {
            Some(cert) if result.status != Status::Fail => cert.apply(result),
            _ => result,
        }
    }

    fn target(&self) -> String {
        self.url.clone()
    }
}

impl CertCheck {
    fn not_after(&self) -> Result<i64, &'static str> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((checked, result)) = *cache {
            let max_age = if result.is_ok() {
                CERT_CACHE
            } else {
                CERT_RETRY
            };
            if checked.elapsed() < max_age {
                return result;
            }
        }
        let result = tls::not_after(&self.host, self.port, self.verify, self.timeout);
        *cache = Some((Instant::now(), result));
        result
    }

    /// Turn a successful HTTP result into WARN or FAIL if the certificate
    /// expires soon.
    fn apply(&self, result: CheckResult) -> CheckResult {
        let now = clock::unix_seconds(SystemTime::now());
        rate_expiry(result, self.not_after(), now, self.warn, self.down)
    }
}

fn rate_expiry(
    mut result: CheckResult,
    not_after: Result<i64, &'static str>,
    now: i64,
    warn: Duration,
    down: Duration,
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
    if within(down) {
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

/// Host and port of an `https://` URL (default port 443).
fn host_port(url: &str) -> Option<(String, u16)> {
    let rest = url.strip_prefix("https://")?;
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    if authority.contains('@') {
        return None;
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (host, after) = v6.split_once(']')?;
        (host, after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let port = match port {
        Some(p) => p.parse().ok().filter(|&p| p != 0)?,
        None => 443,
    };
    (!host.is_empty()).then(|| (host.to_string(), port))
}

impl HttpCheck {
    fn http_check(&self) -> CheckResult {
        let start = Instant::now();
        let mut url = self.url.clone();
        let mut hops = 0;
        loop {
            let mut response = match self.request(&url) {
                Ok(r) => r,
                Err(e) => return CheckResult::fail(Some(start.elapsed()), error_message(&e)),
            };
            let status = response.status().as_u16();

            if (300..400).contains(&status) && hops < self.max_redirects {
                let Some(location) = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                else {
                    return CheckResult::fail(
                        Some(start.elapsed()),
                        format!("HTTP {status} without Location"),
                    );
                };
                match next_url(&url, location) {
                    Ok(next) => url = next,
                    Err(msg) => return CheckResult::fail(Some(start.elapsed()), msg),
                }
                hops += 1;
                continue;
            }

            if !self
                .expected
                .iter()
                .any(|(lo, hi)| (*lo..=*hi).contains(&status))
            {
                return CheckResult::fail(Some(start.elapsed()), format!("HTTP {status}"));
            }

            if let Some(keyword) = &self.keyword {
                let mut body = Vec::new();
                let read = response
                    .body_mut()
                    .as_reader()
                    .take(self.max_body)
                    .read_to_end(&mut body);
                if read.is_err() {
                    return CheckResult::fail(
                        Some(start.elapsed()),
                        format!("HTTP {status}, body read failed"),
                    );
                }
                let found = contains_bytes(&body, keyword);
                if found == self.invert_keyword {
                    let what = if self.invert_keyword {
                        "keyword present"
                    } else {
                        "keyword missing"
                    };
                    return CheckResult::fail(
                        Some(start.elapsed()),
                        format!("HTTP {status}, {what}"),
                    );
                }
            }

            let latency = start.elapsed();
            if self.slow_warn.is_some_and(|slow| latency > slow) {
                return CheckResult::warn(latency, format!("HTTP {status}, slow response"));
            }
            return CheckResult::up(latency, format!("HTTP {status}"));
        }
    }

    fn request(&self, url: &str) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
        let mut request = match self.method {
            Method::Get => self.agent.get(url),
            Method::Head => self.agent.head(url),
        }
        // Never ask for compressed bodies (no decompression bombs).
        .header("Accept-Encoding", "identity");
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        if let Some(auth) = &self.authorization {
            request = request.header("Authorization", auth);
        }
        request.call()
    }
}

fn parse_status(spec: &StatusSpec) -> Option<(u16, u16)> {
    let valid = |c: u16| (100..=599).contains(&c);
    let (lo, hi) = match spec {
        StatusSpec::Code(c) => (*c, *c),
        StatusSpec::Range(r) => {
            let (lo, hi) = r.split_once('-')?;
            (lo.trim().parse().ok()?, hi.trim().parse().ok()?)
        }
    };
    (valid(lo) && valid(hi) && lo <= hi).then_some((lo, hi))
}

/// Resolve a redirect target and enforce the scheme rules.
pub(crate) fn next_url(current: &str, location: &str) -> Result<String, String> {
    let next = if location.starts_with("https://") || location.starts_with("http://") {
        location.to_string()
    } else if let Some(rest) = location.strip_prefix("//") {
        format!("{}://{rest}", scheme(current))
    } else if location.starts_with('/') {
        format!("{}{location}", origin(current))
    } else if has_scheme(location) {
        return Err("redirect to unsupported scheme".into());
    } else {
        // Relative path: replace the last path segment.
        let path_start = origin(current).len();
        let base = &current[..current[path_start..]
            .rfind('/')
            .map_or(current.len(), |i| path_start + i + 1)];
        if base.len() == path_start {
            format!("{base}/{location}")
        } else {
            format!("{base}{location}")
        }
    };
    if current.starts_with("https://") && next.starts_with("http://") {
        return Err("redirect from https to http".into());
    }
    if next.len() > 2048 || ureq::http::Uri::try_from(next.as_str()).is_err() {
        return Err("invalid redirect target".into());
    }
    Ok(next)
}

/// `scheme:` prefix as in RFC 3986 (`javascript:`, `ftp://`, ...).
fn has_scheme(location: &str) -> bool {
    match location.split_once(':') {
        Some((scheme, _)) => {
            scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
        }
        None => false,
    }
}

fn scheme(url: &str) -> &str {
    url.split_once("://").map_or("https", |(s, _)| s)
}

/// `scheme://authority` of an absolute URL.
fn origin(url: &str) -> &str {
    let after_scheme = url.find("://").map_or(0, |i| i + 3);
    let end = url[after_scheme..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |i| after_scheme + i);
    &url[..end]
}

/// Coarse error classes. Some error types (e.g. certificate name mismatch)
/// carry data from the remote side, which must not be reflected.
pub fn error_message(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Timeout(_) => "timeout".into(),
        ureq::Error::HostNotFound => "host not found".into(),
        // TLS errors during the handshake arrive wrapped in an I/O error.
        ureq::Error::Io(io) => match io
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<rustls::Error>())
        {
            Some(tls) => tls_message(tls).into(),
            None => super::tcp::connect_error(io).into(),
        },
        ureq::Error::ConnectionFailed => "connection failed".into(),
        ureq::Error::Rustls(tls) => tls_message(tls).into(),
        ureq::Error::Tls(_) => "TLS handshake failed".into(),
        ureq::Error::Protocol(_) | ureq::Error::BadUri(_) | ureq::Error::Http(_) => {
            "invalid HTTP response".into()
        }
        _ => "request failed".into(),
    }
}

fn tls_message(e: &rustls::Error) -> &'static str {
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

fn is_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

fn is_header_value(value: &str) -> bool {
    value.len() <= 1024
        && value
            .bytes()
            .all(|b| b == b'\t' || (0x20..0x7f).contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Problems;
    use crate::config::global::Defaults;
    use crate::monitors::Status;
    use crate::testutil::TestDir;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    /// Build an HttpCheck from the body of an `[http]` table.
    fn build_text(table: &str) -> Result<HttpCheck, String> {
        let dir = TestDir::new("http");
        let file = dir.write("m.toml", table);
        let meta = std::fs::metadata(&file).unwrap();
        let defaults = Defaults {
            interval: Dur::secs(60),
            timeout: Dur::secs(10),
            retries: 0,
            notifications: vec![],
        };
        let ctx = Context {
            config_dir: dir.path(),
            defaults: &defaults,
            notifiers: &[],
            file_meta: &meta,
        };
        let parsed: HttpFile = toml::from_str(table).map_err(|e| e.to_string())?;
        let mut problems = Problems::default();
        let check = build(
            parsed,
            Duration::from_secs(2),
            &ctx,
            &mut problems.for_file(&file),
        );
        if problems.has_errors() {
            return Err(format!("{problems:?}"));
        }
        check.ok_or_else(|| "no check".into())
    }

    /// A one-shot HTTP server: answers each request with the next response.
    fn server(responses: Vec<String>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for (stream, response) in listener.incoming().zip(responses) {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                    line.clear();
                }
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    fn response(status: &str, extra_headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}",
            body.len()
        )
    }

    fn run(table: &str) -> CheckResult {
        build_text(table).unwrap().check()
    }

    #[test]
    fn status_codes() {
        let base = server(vec![
            response("200 OK", "", "hi"),
            response("503 Service Unavailable", "", ""),
        ]);
        assert_eq!(run(&format!("url = \"{base}/\"")).status, Status::Up);
        let down = run(&format!("url = \"{base}/\""));
        assert_eq!(down.status, Status::Fail);
        assert_eq!(down.msg, "HTTP 503");
    }

    #[test]
    fn expected_status_ranges() {
        let base = server(vec![response("301 Moved", "Location: /x\r\n", "")]);
        let r = run(&format!(
            "url = \"{base}/\"\nexpected_status = [200, \"300-399\"]"
        ));
        assert_eq!(r.status, Status::Up, "{r:?}");
    }

    #[test]
    fn keywords() {
        let base = server(vec![
            response("200 OK", "", "<h1>Welcome</h1>"),
            response("200 OK", "", "<h1>Maintenance</h1>"),
            response("200 OK", "", "<h1>Error</h1>"),
        ]);
        let url = format!("url = \"{base}/\"");
        assert_eq!(
            run(&format!("{url}\nkeyword = \"Welcome\"")).status,
            Status::Up
        );
        let missing = run(&format!("{url}\nkeyword = \"Welcome\""));
        assert_eq!(missing.msg, "HTTP 200, keyword missing");
        let present = run(&format!(
            "{url}\nkeyword = \"Error\"\ninvert_keyword = true"
        ));
        assert_eq!(present.msg, "HTTP 200, keyword present");
    }

    #[test]
    fn keyword_beyond_body_limit_is_not_found() {
        let body = format!("{}Welcome", "x".repeat(100));
        let base = server(vec![response("200 OK", "", &body)]);
        let r = run(&format!(
            "url = \"{base}/\"\nkeyword = \"Welcome\"\nmax_body_bytes = 50"
        ));
        assert_eq!(r.msg, "HTTP 200, keyword missing");
    }

    #[test]
    fn redirects() {
        let target = server(vec![response("200 OK", "", "")]);
        let base = server(vec![
            response("302 Found", &format!("Location: {target}/next\r\n"), ""),
            response("302 Found", "Location: /elsewhere\r\n", ""),
        ]);
        let followed = run(&format!("url = \"{base}/\"\nmax_redirects = 1"));
        assert_eq!(followed.status, Status::Up, "{followed:?}");
        // Not followed by default: 302 is not in 200-299.
        let not_followed = run(&format!("url = \"{base}/\""));
        assert_eq!(not_followed.msg, "HTTP 302");
    }

    #[test]
    fn redirect_targets() {
        assert_eq!(
            next_url("https://a.org/x/y", "/z").unwrap(),
            "https://a.org/z"
        );
        assert_eq!(
            next_url("https://a.org/x/y", "z").unwrap(),
            "https://a.org/x/z"
        );
        assert_eq!(next_url("https://a.org", "z").unwrap(), "https://a.org/z");
        assert_eq!(
            next_url("https://a.org/x", "//b.org/q").unwrap(),
            "https://b.org/q"
        );
        assert_eq!(
            next_url("http://a.org/", "https://b.org/").unwrap(),
            "https://b.org/"
        );
        assert!(next_url("https://a.org/", "http://b.org/").is_err());
        assert!(next_url("https://a.org/", "ftp://b.org/").is_err());
        assert!(next_url("https://a.org/", "javascript:alert(1)").is_err());
        assert!(next_url("https://a.org/", "file:/etc/passwd").is_err());
    }

    #[test]
    fn connection_refused() {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let r = run(&format!("url = \"http://127.0.0.1:{port}/\""));
        assert_eq!(r.status, Status::Fail);
        assert!(
            r.msg.contains("refused") || r.msg.contains("connection"),
            "{r:?}"
        );
    }

    #[test]
    fn timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let _held = listener.accept();
            thread::sleep(Duration::from_secs(5));
        });
        let start = Instant::now();
        let r = run(&format!("url = \"http://{addr}/\""));
        assert_eq!(r.msg, "timeout");
        assert!(start.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn config_errors() {
        for (table, expected) in [
            ("url = \"ftp://x/\"", "http://"),
            ("url = \"https://exa mple.org/\"", "valid URL"),
            (
                "url = \"https://x/\"\nexpected_status = []",
                "expected_status",
            ),
            (
                "url = \"https://x/\"\nexpected_status = [99]",
                "expected_status",
            ),
            (
                "url = \"https://x/\"\nexpected_status = [\"300-200\"]",
                "expected_status",
            ),
            (
                "url = \"https://x/\"\nmethod = \"HEAD\"\nkeyword = \"a\"",
                "HEAD",
            ),
            (
                "url = \"https://x/\"\ninvert_keyword = true",
                "invert_keyword",
            ),
            (
                "url = \"https://x/\"\nmax_body_bytes = 2000000",
                "max_body_bytes",
            ),
            ("url = \"https://x/\"\nmax_redirects = 11", "max_redirects"),
            (
                "url = \"https://x/\"\nheaders = { \"Bad Name\" = \"v\" }",
                "header",
            ),
            (
                "url = \"https://x/\"\nheaders = { \"X-A\" = \"line\\nbreak\" }",
                "header",
            ),
            ("url = \"https://x/\"\nbasic_auth_user = \"u\"", "together"),
            (
                "url = \"https://x/\"\nbasic_auth_user = \"u:x\"\nbasic_auth_password = \"p\"",
                "basic_auth_user",
            ),
            ("url = \"http://x/\"\ntls_expiry_warn = \"7d\"", "https"),
            ("url = \"https://x/\"\nslow_warn = \"5s\"", "slow_warn"),
            ("url = \"https://x/\"\nbogus = 1", "bogus"),
        ] {
            let err = build_text(table)
                .err()
                .unwrap_or_else(|| panic!("accepted: {table}"));
            assert!(err.contains(expected), "{table}\n=> {err}");
        }
    }

    #[test]
    fn certificate_expiry_rating() {
        let day = 86400;
        let now = 1_800_000_000;
        let up = || CheckResult::up(Duration::from_millis(5), "HTTP 200");
        let warn = Duration::from_secs(14 * day as u64);
        let rate = |not_after, down: u64| {
            rate_expiry(
                up(),
                not_after,
                now,
                warn,
                Duration::from_secs(down * day as u64),
            )
        };

        let fine = rate(Ok(now + 60 * day), 0);
        assert_eq!((fine.status, fine.msg.as_str()), (Status::Up, "HTTP 200"));
        assert_eq!(fine.cert_expires, Some(now + 60 * day));

        let soon = rate(Ok(now + 10 * day + 5), 0);
        assert_eq!(
            (soon.status, soon.msg.as_str()),
            (Status::Warn, "HTTP 200, certificate expires in 10 days")
        );

        let down = rate(Ok(now + 3 * day), 7);
        assert_eq!(
            (down.status, down.msg.as_str()),
            (Status::Fail, "HTTP 200, certificate expires in 3 days")
        );

        let expired = rate(Ok(now - 1), 0);
        assert_eq!(
            (expired.status, expired.msg.as_str()),
            (Status::Warn, "HTTP 200, certificate expired")
        );

        let failed = rate(Err("timeout"), 0);
        assert_eq!(
            (failed.status, failed.msg.as_str()),
            (Status::Warn, "HTTP 200, certificate check failed")
        );
        assert_eq!(failed.cert_expires, None);
    }

    #[test]
    fn host_and_port() {
        let hp = |u: &str| host_port(u);
        assert_eq!(
            hp("https://example.org/"),
            Some(("example.org".into(), 443))
        );
        assert_eq!(
            hp("https://example.org:8443/x?y"),
            Some(("example.org".into(), 8443))
        );
        assert_eq!(hp("https://example.org"), Some(("example.org".into(), 443)));
        assert_eq!(hp("https://[::1]:8443/"), Some(("::1".into(), 8443)));
        assert_eq!(hp("https://[::1]/"), Some(("::1".into(), 443)));
        assert_eq!(hp("https://user@example.org/"), None);
        assert_eq!(hp("https://example.org:0/"), None);
        assert_eq!(hp("https://example.org:x/"), None);
        assert_eq!(hp("http://example.org/"), None);
    }

    #[test]
    fn basic_auth_header() {
        let check = build_text("url = \"https://x/\"\nbasic_auth_user = \"Aladdin\"\nbasic_auth_password = \"open sesame\"").unwrap();
        assert_eq!(
            check.authorization.as_deref(),
            Some("Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==")
        );
    }
}
