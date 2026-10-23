# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.2.0] - 2026-10-23

First version that monitors: `http` and `tcp` monitors, the check log and
status pages with the built-in server (milestones M1 and M2).

### Added
- `run`: the monitoring service. A dispatcher with a bounded queue and
  worker threads; every monitor has a fixed time grid; under overload,
  waiting checks are served round-robin and missed grid points are logged as
  `result=SKIPPED count=N`, so all monitors are checked equally often.
  SIGTERM/SIGINT flush the log. Waits for a plausible clock (no RTC) and
  logs clock jumps. Warns when running as root.
- Check log: one logfmt file per UTC day, buffered writes, retention.
- State machine: UP, WARN, PENDING (retries), DOWN; state changes are also
  printed to stderr (journal).
- `check-config` validates `kinomi.toml` and `monitors/*.toml` completely
  and lists the monitors.
- Status pages: public and private pages from `pages/*.toml`, heartbeat
  bar, uptime over 24 h and 30 d, average response time, certificate
  expiry, overall banner, private details and self status; static HTML5 +
  CSS without JavaScript, light and dark mode, time zone from the system
  tz database. Written to `output_dir` only when changed.
- Built-in HTTP server: serves the pages from memory, Basic Auth with
  bcrypt htpasswd files (Apache/nginx compatible) and a verification cache,
  throttling after failed logins, Host header check, strict headers (CSP,
  nosniff, no framing), GET/HEAD only, bounded threads and request sizes.
- State files per monitor (human-readable TOML) and log replay at startup;
  `rebuild-state` regenerates them from the log.
- `hash-password USER` and `gen-token`.
- Pauses of the whole process (e.g. standby of a VM host) are logged as
  `event=PAUSED` and their missed checks as `SKIPPED msg="paused"`.
- Monitor types `http` (status codes and ranges, keyword, redirects checked
  hop by hop, body limit, headers, basic auth, slow warning, TLS
  certificate expiry with WARN/DOWN thresholds) and `tcp` (connect, banner).
  TLS errors are reported as coarse classes (expired, unknown issuer, wrong
  host name, ...), never with data from the server.

## [0.1.1] - 2026-10-22

Second infrastructure preview, to test `self-update` from 0.1.0.

### Fixed
- `contrib/release/sign-release.sh` no longer aborts when an old `gh`
  without `gh attestation` is installed; the provenance check is skipped
  with a message instead.

## [0.1.0] - 2026-10-22

Infrastructure preview: no monitoring yet. Tests the build, release,
signing and self-update chain.

### Added
- Specification (`SPEC.md`), README and contributor/agent guide (`AGENTS.md`).
- Cargo project skeleton with `#![forbid(unsafe_code)]`, strict lints and
  `cargo-deny` configuration.
- Command line interface: `version`, `help`, and `check-config` (resolves the
  configuration directory and default data directories; content validation
  follows in M1). Other commands are recognised but not implemented yet.
- CI (GitHub Actions): fmt, clippy, tests, MSRV check, cargo-deny, static
  builds for armv6, armv7, aarch64, x86_64 and riscv64 with tests and smoke
  tests under QEMU (Raspberry Pi 1 CPU for armv6). Actions are pinned to
  commit SHAs and kept current by Dependabot.
- `self-update` (SPEC.md §12.2): installs the latest release after verifying
  the minisign signature of `SHA256SUMS` (embedded primary and backup keys,
  version bound to the signature), the SHA-256 of the binary, and a test run
  on the local CPU; keeps the previous binary for `--rollback`. `--check`
  exits with code 3 when an update is available. First HTTPS code in kinomi
  (ureq + rustls + ring).
- `version` now shows the exact target triple.
- Release workflow: tag `vX.Y.Z` builds, tests and attests all binaries and
  creates an unsigned pre-release; `contrib/release/sign-release.sh`
  (needs only curl and minisign) verifies, signs and publishes it on the
  maintainer's machine.

[Unreleased]: https://github.com/uptime-kinomi/uptime-kinomi/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/uptime-kinomi/uptime-kinomi/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/uptime-kinomi/uptime-kinomi/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/uptime-kinomi/uptime-kinomi/releases/tag/v0.1.0
