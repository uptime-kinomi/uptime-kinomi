//! The built-in HTTP server (SPEC.md §9): serves the generated pages from
//! memory, protects private pages with Basic Auth, and nothing else.
//!
//! Deliberately small: GET/HEAD only (POST only for /push/, milestone M3),
//! request bodies are never read, one request per connection, strict limits
//! and timeouts, fixed thread pool. TLS is the reverse proxy's job.

pub mod auth;
pub mod push;

use crate::health::Health;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::render::Published;
use auth::{Auth, Verdict};
use push::PushRegistry;

const MAX_HEAD_BYTES: usize = 8 * 1024;
const MAX_HEADERS: usize = 32;
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const HANDLERS: usize = 4;
/// Connections waiting for a handler; more are closed at once.
const BACKLOG: usize = 12;

const CSP: &str = "default-src 'none'; style-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

pub struct Options {
    pub listen: SocketAddr,
    /// Allowed Host header values (lowercase); empty = any.
    pub hostnames: Vec<String>,
    /// URL path of the page served at "/", e.g. "/home/".
    pub default_page: Option<String>,
    pub auth: Option<Arc<Auth>>,
    pub push: Option<PushRegistry>,
    /// Liveness for `/healthz`; `None` means always OK.
    pub health: Option<Arc<Health>>,
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Arc<Vec<u8>>,
}

impl Response {
    fn text(status: u16, text: &str) -> Self {
        Response {
            status,
            headers: vec![("Content-Type", "text/plain; charset=utf-8".into())],
            body: Arc::new(text.as_bytes().to_vec()),
        }
    }

    /// No body and no content type (push responses).
    fn empty(status: u16) -> Self {
        Response {
            status,
            headers: Vec::new(),
            body: Arc::new(Vec::new()),
        }
    }

    fn redirect(status: u16, location: String) -> Self {
        let mut r = Response::text(status, "");
        r.headers.push(("Location", location));
        r
    }
}

/// Bind and start serving. Errors (e.g. port in use) are returned at once.
pub fn start(options: Options, published: Published) -> Result<(), String> {
    let listener = TcpListener::bind(options.listen)
        .map_err(|e| format!("cannot listen on {}: {e}", options.listen))?;
    let options = Arc::new(options);
    let (tx, rx): (SyncSender<TcpStream>, Receiver<TcpStream>) = mpsc::sync_channel(BACKLOG);
    let rx = Arc::new(Mutex::new(rx));
    for n in 0..HANDLERS {
        let (rx, options, published) = (rx.clone(), options.clone(), published.clone());
        thread::Builder::new()
            .name(format!("http-{n}"))
            .spawn(move || {
                loop {
                    let stream = match rx.lock().unwrap_or_else(|e| e.into_inner()).recv() {
                        Ok(stream) => stream,
                        Err(_) => return,
                    };
                    let _ = serve(stream, &options, &published);
                }
            })
            .map_err(|e| format!("cannot start the server: {e}"))?;
    }
    thread::Builder::new()
        .name("http-accept".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let stream = match stream {
                    Ok(stream) => stream,
                    // E.g. out of file descriptors: do not spin at 100 %.
                    Err(_) => {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                };
                match tx.try_send(stream) {
                    Ok(()) => {}
                    // Overloaded: close the connection instead of queueing.
                    Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => return,
                }
            }
        })
        .map_err(|e| format!("cannot start the server: {e}"))?;
    Ok(())
}

fn serve(mut stream: TcpStream, options: &Options, published: &Published) -> io::Result<()> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let (response, head_only) = match read_head(&mut stream, READ_TIMEOUT)? {
        Some(head) => match parse_request(&head) {
            Some(req) => {
                let site = published.read().map(|s| s.clone()).unwrap_or_default();
                let head_only = req.method == "HEAD";
                (route(&req, options, &site), head_only)
            }
            None => (no_cache(Response::text(400, "Bad Request\n")), false),
        },
        None => (
            no_cache(Response::text(431, "Request Header Fields Too Large\n")),
            false,
        ),
    };
    write_response(&mut stream, &response, head_only)
}

/// Errors before routing get the Cache-Control `route` would give them.
fn no_cache(mut r: Response) -> Response {
    r.headers.push(("Cache-Control", "no-cache".into()));
    r
}

/// Read until the end of the request head. `None` if it is too large. The
/// whole head must arrive within `timeout`, so a client that sends one byte
/// at a time cannot hold a handler (there are only `HANDLERS`).
fn read_head(stream: &mut TcpStream, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
    let deadline = Instant::now() + timeout;
    let mut head = Vec::with_capacity(1024);
    let mut buf = [0u8; 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(left))?;
        // Never more than MAX_HEAD_BYTES in total.
        let room = (MAX_HEAD_BYTES - head.len()).min(buf.len());
        let n = stream.read(&mut buf[..room])?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        head.extend_from_slice(&buf[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(Some(head));
        }
        if head.len() >= MAX_HEAD_BYTES {
            return Ok(None);
        }
    }
}

pub struct Request {
    pub method: String,
    pub path: String,
    pub host: Option<String>,
    pub authorization: Option<String>,
}

pub(crate) fn parse_request(head: &[u8]) -> Option<Request> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut headers);
    if !matches!(req.parse(head), Ok(httparse::Status::Complete(_))) {
        return None;
    }
    let header = |name: &str| {
        req.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .and_then(|h| std::str::from_utf8(h.value).ok())
            .map(str::to_string)
    };
    Some(Request {
        method: req.method?.to_string(),
        path: req.path?.to_string(),
        host: header("host"),
        authorization: header("authorization"),
    })
}

/// Paths that need a login (SPEC.md §9.3).
fn is_private(path: &str) -> bool {
    path == "/private" || path.starts_with("/private/") || path == "/metrics"
}

/// Decide the response. Pure function of the request and the current site.
/// Every response has a Cache-Control header (SPEC.md §9.4): files set
/// their own; everything else (errors, redirects, 401) gets one by path.
pub fn route(req: &Request, options: &Options, site: &crate::render::Site) -> Response {
    let mut r = route_inner(req, options, site);
    if !r.headers.iter().any(|(name, _)| *name == "Cache-Control") {
        let path = clean_path(&req.path).unwrap_or_default();
        let value = if is_private(&path) {
            "private, no-store"
        } else {
            "no-cache"
        };
        r.headers.push(("Cache-Control", value.into()));
    }
    r
}

fn route_inner(req: &Request, options: &Options, site: &crate::render::Site) -> Response {
    // Decode first: every decision below is about the decoded path, so
    // e.g. /%70ush/ is a push path too.
    let Some(path) = clean_path(&req.path) else {
        return Response::text(400, "Bad Request\n");
    };
    let is_push = path.starts_with("/push/");
    // A push is GET or POST only: HEAD (link previews, probes) must not
    // count as a push.
    let allowed = matches!(
        (req.method.as_str(), is_push),
        ("GET", _) | ("HEAD", false) | ("POST", true)
    );
    if !allowed {
        let mut r = Response::text(405, "Method Not Allowed\n");
        let allow = if is_push { "GET, POST" } else { "GET, HEAD" };
        r.headers.push(("Allow", allow.into()));
        return r;
    }
    if !host_allowed(req.host.as_deref(), &options.hostnames) {
        return Response::text(421, "Misdirected Request\n");
    }

    if path == "/" {
        return match &options.default_page {
            // Relative, so it also works below a proxy prefix like /status/.
            Some(page) => Response::redirect(302, page.trim_start_matches('/').to_string()),
            None => Response::text(404, "Not Found\n"),
        };
    }
    if path == "/healthz" {
        return if options.health.as_ref().is_none_or(|h| h.ok()) {
            Response::text(200, "ok\n")
        } else {
            Response::text(503, "scheduler not running\n")
        };
    }
    if let Some(token) = path.strip_prefix("/push/") {
        let query = req
            .path
            .split_once('?')
            .map(|(_, q)| q.split('#').next().unwrap_or_default());
        let status = match &options.push {
            Some(registry) if !token.contains('/') => registry.accept(token, query),
            _ => 404,
        };
        return Response::empty(status);
    }

    // Everything below /private/ needs a login, even if it does not exist,
    // so the names of private pages are not revealed.
    if is_private(&path) {
        let verdict = options
            .auth
            .as_ref()
            .map_or(Verdict::Denied, |a| a.check(req.authorization.as_deref()));
        match verdict {
            Verdict::Allowed => {}
            Verdict::Denied => {
                let mut r = Response::text(401, "Unauthorized\n");
                r.headers.push((
                    "WWW-Authenticate",
                    "Basic realm=\"uptime-kinomi\", charset=\"UTF-8\"".into(),
                ));
                return r;
            }
            Verdict::Throttled => {
                let mut r = Response::text(429, "Too Many Requests\n");
                r.headers.push(("Retry-After", "1".into()));
                return r;
            }
        }
    }

    if let Some(file) = site.files.get(&path) {
        let cache = if file.private {
            "private, no-store"
        } else if path.starts_with("/assets/") {
            "public, max-age=300"
        } else {
            "no-cache"
        };
        return Response {
            status: 200,
            headers: vec![
                ("Content-Type", file.content_type.into()),
                ("Cache-Control", cache.into()),
            ],
            body: Arc::new(file.body.clone()),
        };
    }
    // "/home" → "home/" (relative to the parent).
    if !path.ends_with('/') && site.files.contains_key(&format!("{path}/")) {
        let last = path.rsplit('/').next().unwrap_or_default();
        return Response::redirect(301, format!("{last}/"));
    }
    Response::text(404, "Not Found\n")
}

/// Compare the Host header (without port) with the allowed names.
fn host_allowed(host: Option<&str>, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    let Some(host) = host else {
        return false;
    };
    let name = if let Some(v6) = host.strip_prefix('[') {
        v6.split(']').next().unwrap_or_default()
    } else {
        host.rsplit_once(':').map_or(host, |(name, port)| {
            if port.bytes().all(|b| b.is_ascii_digit()) {
                name
            } else {
                host
            }
        })
    };
    allowed.iter().any(|a| a.eq_ignore_ascii_case(name))
}

/// Path without query, percent-decoded once, with no tricks allowed.
fn clean_path(raw: &str) -> Option<String> {
    let path = raw.split(['?', '#']).next().unwrap_or_default();
    if !path.starts_with('/') || path.len() > 512 {
        return None;
    }
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = path.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(out).ok()?;
    let bad = decoded.contains("..")
        || decoded.contains("//")
        || decoded
            .bytes()
            .any(|b| b == b'\\' || b == 0 || b.is_ascii_control());
    (!bad).then_some(decoded)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        421 => "Misdirected Request",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    }
}

fn write_response(stream: &mut TcpStream, r: &Response, head_only: bool) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {} {}\r\n", r.status, reason(r.status));
    let security = [
        ("Content-Security-Policy", CSP),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("X-Frame-Options", "DENY"),
        ("Connection", "close"),
    ];
    for (name, value) in security {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    for (name, value) in &r.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", r.body.len()));
    // One deadline for the whole response, so a client that reads slowly
    // cannot hold a handler (as for the request head).
    let deadline = Instant::now() + WRITE_TIMEOUT;
    write_within(stream, head.as_bytes(), deadline)?;
    if !head_only {
        write_within(stream, &r.body, deadline)?;
    }
    stream.flush()
}

fn write_within(stream: &mut TcpStream, mut data: &[u8], deadline: Instant) -> io::Result<()> {
    while !data.is_empty() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        stream.set_write_timeout(Some(left))?;
        let n = stream.write(&data[..data.len().min(16 * 1024)])?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        data = &data[n..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::base64;
    use crate::render::{File, Site};
    use std::sync::RwLock;

    fn site() -> Site {
        let mut site = Site::default();
        let file = |body: &str, private| {
            Arc::new(File {
                content_type: "text/html; charset=utf-8",
                body: body.as_bytes().to_vec(),
                private,
            })
        };
        site.files
            .insert("/home/".into(), file("public page", false));
        site.files
            .insert("/private/all/".into(), file("private page", true));
        site
    }

    fn options() -> Options {
        let line = auth::htpasswd_line("admin", b"pw", [1; 16]);
        Options {
            listen: "127.0.0.1:0".parse().unwrap(),
            hostnames: vec!["status.example.org".into()],
            default_page: Some("/home/".into()),
            auth: Some(Arc::new(Auth::parse(&line).unwrap())),
            push: None,
            health: None,
        }
    }

    fn get(path: &str, auth: Option<&str>) -> Response {
        let req = Request {
            method: "GET".into(),
            path: path.into(),
            host: Some("status.example.org".into()),
            authorization: auth.map(|a| format!("Basic {}", base64(a.as_bytes()))),
        };
        route(&req, &options(), &site())
    }

    fn header<'a>(r: &'a Response, name: &str) -> Option<&'a str> {
        r.headers
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn public_routes() {
        let r = get("/home/", None);
        assert_eq!((r.status, r.body.as_slice()), (200, &b"public page"[..]));
        assert_eq!(header(&r, "Cache-Control"), Some("no-cache"));
        let r = get("/", None);
        assert_eq!((r.status, header(&r, "Location")), (302, Some("home/")));
        let r = get("/home", None);
        assert_eq!((r.status, header(&r, "Location")), (301, Some("home/")));
        assert_eq!(get("/home/?x=1", None).status, 200);
        assert_eq!(get("/nope/", None).status, 404);
        assert_eq!(get("/healthz", None).status, 200);
        assert_eq!(get("/push/abc", None).status, 404);
    }

    #[test]
    fn private_routes_need_login() {
        let r = get("/private/all/", None);
        assert_eq!(r.status, 401);
        assert!(header(&r, "WWW-Authenticate").is_some());
        // Unknown private paths also ask for a login (names are not revealed).
        assert_eq!(get("/private/secret-name/", None).status, 401);
        let r = get("/private/all/", Some("admin:pw"));
        assert_eq!((r.status, r.body.as_slice()), (200, &b"private page"[..]));
        assert_eq!(header(&r, "Cache-Control"), Some("private, no-store"));
        assert_eq!(get("/private/all/", Some("admin:wrong")).status, 401);
        assert_eq!(get("/private/nope/", Some("admin:pw")).status, 404);
    }

    #[test]
    fn rejected_requests() {
        for path in [
            "/home/../private/all/",
            "/%2e%2e/x",
            "/home/%00",
            "//home/",
            "/a\\b",
            "home/",
            "/%zz",
            "/%c3",
        ] {
            assert_eq!(get(path, None).status, 400, "{path}");
        }
        let mut req = Request {
            method: "DELETE".into(),
            path: "/home/".into(),
            host: Some("status.example.org".into()),
            authorization: None,
        };
        assert_eq!(route(&req, &options(), &site()).status, 405);
        req.method = "POST".into();
        let r = route(&req, &options(), &site());
        assert_eq!(r.status, 405);
        assert_eq!(header(&r, "Allow"), Some("GET, HEAD"));
        // HEAD must not count as a push (link previews, probes).
        for path in ["/push/abc", "/%70ush/abc", "/push%2fabc"] {
            req.path = path.into();
            req.method = "HEAD".into();
            let r = route(&req, &options(), &site());
            assert_eq!(r.status, 405, "{path}");
            assert_eq!(header(&r, "Allow"), Some("GET, POST"), "{path}");
        }
        req.path = "/home/".into();
        req.method = "GET".into();
        req.host = Some("evil.example.com".into());
        assert_eq!(route(&req, &options(), &site()).status, 421);
        req.host = None;
        assert_eq!(route(&req, &options(), &site()).status, 421);
    }

    #[test]
    fn every_response_has_cache_control() {
        let cc = |r: &Response| header(r, "Cache-Control").map(str::to_string);
        // Login needed, redirect to the slash URL, missing private file.
        for path in [
            "/private",
            "/private/all/",
            "/private/all",
            "/%70rivate/nope/",
        ] {
            assert_eq!(
                cc(&get(path, None)).as_deref(),
                Some("private, no-store"),
                "{path}"
            );
        }
        assert_eq!(cc(&get("/nope/", None)).as_deref(), Some("no-cache"));
        assert_eq!(cc(&get("/", None)).as_deref(), Some("no-cache"));
    }

    #[test]
    fn hosts() {
        let allowed = vec!["status.example.org".to_string()];
        assert!(host_allowed(Some("status.example.org"), &allowed));
        assert!(host_allowed(Some("STATUS.example.org:8080"), &allowed));
        assert!(!host_allowed(Some("status.example.org.evil.com"), &allowed));
        assert!(host_allowed(Some("anything"), &[]));
        assert!(host_allowed(Some("[::1]:8080"), &["::1".to_string()]));
    }

    #[test]
    fn over_tcp() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let mut opts = options();
        opts.listen = addr;
        let published: Published = Arc::new(RwLock::new(Arc::new(site())));
        start(opts, published).unwrap();
        let request = |text: &str| -> String {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(text.as_bytes()).unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            out
        };
        let ok = request("GET /home/ HTTP/1.1\r\nHost: status.example.org\r\n\r\n");
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"), "{ok}");
        assert!(ok.contains("Content-Security-Policy: default-src 'none';"));
        assert!(ok.contains("X-Content-Type-Options: nosniff\r\n"));
        assert!(ok.contains("Connection: close\r\n"));
        assert!(ok.ends_with("\r\n\r\npublic page"));
        let head = request("HEAD /home/ HTTP/1.1\r\nHost: status.example.org\r\n\r\n");
        assert!(head.contains("Content-Length: 11\r\n") && !head.contains("public page"));
        let garbage = request("garbage\r\n\r\n");
        assert!(garbage.starts_with("HTTP/1.1 400"), "{garbage}");
        assert!(garbage.contains("Cache-Control: no-cache\r\n"), "{garbage}");
    }

    #[test]
    fn slow_clients_get_one_deadline_for_the_whole_head() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            // One byte every 300 ms: each read is quick, the head never ends.
            let mut s = TcpStream::connect(addr).unwrap();
            for _ in 0..20 {
                if s.write_all(b"G").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(300));
            }
        });
        let (mut stream, _) = listener.accept().unwrap();
        let start = Instant::now();
        let r = read_head(&mut stream, Duration::from_secs(1));
        assert!(r.is_err());
        assert!(
            start.elapsed() < Duration::from_millis(1600),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn head_limit_is_exact() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let mut s = TcpStream::connect(addr).unwrap();
            let _ = s.write_all(&vec![b'a'; MAX_HEAD_BYTES + 600]);
        });
        let (mut stream, _) = listener.accept().unwrap();
        assert!(
            read_head(&mut stream, Duration::from_secs(2))
                .unwrap()
                .is_none()
        );
    }
}
