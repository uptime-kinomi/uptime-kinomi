# AGENTS.md

Guidance for AI coding agents and human contributors working on
uptime-kinomi. The specification is [SPEC.md](SPEC.md). If code and spec
disagree, the spec wins until it is explicitly changed in the same PR.

## Project invariants (never break these)

1. **No `unsafe`.** `#![forbid(unsafe_code)]` in the crate root. Do not work
   around it.
2. **No JavaScript in generated output.** Pages are HTML5 + CSS only. No
   `<script>`, no `on*=` attributes, no `javascript:` URLs.
3. **Raspberry Pi 1 is a first-class target.** Every change must build for
   `arm-unknown-linux-musleabihf` and stay within the resource targets in
   SPEC.md §1. No async runtime. Use blocking I/O on a small thread pool.
4. **Untrusted input is bounded before it is parsed.** Every network read has
   a byte limit and a timeout. Every parser of network data has a fuzz target.
5. **Never reflect fetched content.** Data received from a monitored target or
   a push request is never written to logs, pages or notifications. Only
   kinomi's own summary is (`"HTTP 503"`, `"timeout"`, `"keyword missing"`).
6. **Strict configuration.** `#[serde(deny_unknown_fields)]` on every config
   struct. New options need a default, documentation in SPEC.md, and a
   `check-config` validation.
7. **Escape all output.** Every user-provided string goes through the HTML/XML
   escaping helper. Never build HTML by concatenating unescaped strings.
8. **Storage is human-readable.** Logs are logfmt with a UTC timestamp first.
   State files are TOML and can always be rebuilt from logs.
9. **No root required.** kinomi must install, configure and run as an ordinary
   user (SPEC.md §10.6). No hard-coded absolute paths: use the config-dir
   resolution (§3.1) and default directories (§10.6). Every example, test and
   doc must work without `sudo`. A feature that needs privileges must be optional.

## Source layout

```
src/
├── main.rs                CLI dispatch (run, check-config, rebuild-state, gen-token, hash-password, self-update, version)
├── run.rs                 the service: event loop, notifications, push deadlines, maintenance; rebuild-state
├── config/                loading + validation of all TOML files
│   ├── mod.rs             directory walk, problems, cross-file checks
│   ├── global.rs          kinomi.toml
│   ├── pages.rs           pages/*.toml
│   ├── incidents.rs       incidents/*.toml
│   ├── maintenance.rs     maintenance/*.toml (+ window computation)
│   ├── duration.rs        "30s" / "5m" parsing
│   └── secret.rs          inline vs *_file secrets, permission checks
├── paths.rs               config-dir resolution, XDG/systemd default directories
├── monitors/              one file per type: http.rs, tcp.rs, dns.rs, push.rs (+ tls.rs for certificates)
├── notifiers/             one file per type: webhook.rs, ntfy.rs, gotify.rs, telegram.rs (+ sender thread in mod.rs)
├── scheduler.rs           time grid, bounded queue, workers, round-robin under overload, pause detection
├── state.rs               state machine (UP/WARN/PENDING/DOWN/MAINTENANCE)
├── history.rs             heartbeat bar, hourly/daily totals, state changes
├── storage/
│   ├── log.rs             buffered logfmt writer, retention
│   ├── replay.rs          logfmt parser, replay into histories
│   └── statefile.rs       per-monitor TOML state
├── render/
│   ├── mod.rs             renderer thread, site, change detection, atomic writes
│   ├── html.rs            pages and the escaping helper `esc` (the only way to emit text)
│   ├── json.rs            status.json
│   ├── feed.rs            Atom feed
│   ├── badge.rs           SVG badges
│   └── metrics.rs         /metrics
├── server/
│   ├── mod.rs             accept loop, limits, routing
│   ├── auth.rs            htpasswd (bcrypt) + verification cache
│   └── push.rs            push endpoint
├── heartbeat.rs           outbound heartbeat
├── clock.rs               UTC formatting, clock plausibility, jump detection
├── encoding.rs            base64 variants, JSON strings, constant-time comparison
└── update.rs              self-update: signed checksums, download, test run, atomic replace
assets/kinomi.css          default style sheet (embedded with include_str!)
contrib/                   example config, systemd units, nginx/apache, ansible, release tools
testdata/                  generated test vectors (with their generator scripts)
```

## How to add a monitor type

1. Create `src/monitors/<type>.rs` (see `tcp.rs` for a small example) with:
   - a file struct `<Type>File` for the `[<type>]` table (`Deserialize`,
     `deny_unknown_fields`),
   - `pub fn build(file, timeout, ..., p: &mut FileProblems) -> Option<<Type>Check>`
     that validates everything and reports problems via `p.error(...)`,
   - an implementation of the `Check` trait:
     ```rust
     pub trait Check: Send + Sync {
         /// One check. Must return within the monitor's timeout and never panic.
         /// `CheckResult::msg` is kinomi's own summary, never received data.
         fn check(&self) -> CheckResult;
         /// Target for private pages, e.g. the URL.
         fn target(&self) -> String;
     }
     ```
   - unit tests in the same file, against local `TcpListener` servers.
2. In `src/monitors/mod.rs`: add a variant to `Kind`, an `Option<<Type>File>`
   field to `MonitorFile`, and an arm to `build_check`.
3. Document the type in SPEC.md §4 and add an example to `contrib/examples/`.
4. If it parses network data: add a fuzz target in `fuzz/`.

Adding a notifier works the same way in `src/notifiers/`.

## Dependency policy

Every new dependency needs a justification in the PR description: why it's
needed, its size, its transitive dependencies, its `unsafe` usage, and whether
it builds for ARMv6 musl. Prefer the standard library. Disable default
features.

Dependencies (✅ = in use, others are candidates):

| Crate | Purpose |
|---|---|
| `serde`, `toml` ✅ | configuration (parsing only) and state files (written by hand) |
| `ureq` with `rustls` (ring provider) and `webpki-roots` ✅ | outbound HTTP(S): http monitor, notifications, heartbeat, self-update; bundled Mozilla CA roots |
| `rustls` ✅ | direct use for the certificate expiry handshake and TLS error classification |
| `ring` ✅ | SHA-256 and random numbers (already pulled in by rustls) |
| `httparse` ✅ | inbound HTTP request parsing |
| `jiff` ✅ | time zones from the system tz database |
| `bcrypt` ✅ | bcrypt core for htpasswd (without `alloc`; the htpasswd format is ours) |
| `minisign-verify` ✅ | release signature check in `self-update` (no dependencies) |
| `signal-hook` ✅ | SIGTERM/SIGINT for clean shutdown |

Done in-house instead of with a crate (small, tested, no extra
dependencies): base64 variants, JSON writing for notifications, the DNS
client, X.509 `notAfter` reading, UTC date conversion.

Not allowed: async runtimes (`tokio`, `async-std`), template engines, web
frameworks, logging frameworks with heavy dependency trees, anything requiring
a C toolchain at build time except where unavoidable for TLS on ARMv6.

## Commands

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cross build --release --target arm-unknown-linux-musleabihf
CC_x86_64_unknown_linux_musl=gcc cargo build --release --target x86_64-unknown-linux-musl   # quick local static build
python3 testdata/update/generate.py   # regenerate self-update test vectors (TEST keys only)
cargo deny check
cargo +nightly fuzz run <target>     # in fuzz/
```

CI runs all of the above for every PR, plus a QEMU smoke test of the ARMv6
binary (`check-config` on `contrib/examples/config/` and a short `run`).

## Code style

- Small functions, plain structs, few traits. Readability over cleverness. A
  new contributor should understand a monitor file without reading the rest.
- Errors: one `Error` enum per module with `Display`. No `unwrap()`/`expect()`
  outside tests and provably infallible spots (comment why).
- No panics on any input from network or config. Return errors.
- Comments explain *why*, not *what*.
- Every user-visible behaviour change updates SPEC.md and, if relevant,
  README.md and CHANGELOG.md.

## Testing

- Unit tests next to the code.
- Integration tests in `tests/` with local `TcpListener` servers (no internet
  access in tests).
- Golden-file tests for rendered HTML (`tests/golden/`). The HTML must contain
  no `<script` and must pass an escaping test with hostile monitor names.
- Performance check: a test with 200 monitors and a simulated slow network
  verifies fair skipping and bounded memory.
