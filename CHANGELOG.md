# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Security
- The `http` monitor no longer sends `basic_auth_*` credentials and custom
  `headers` to redirect targets on another origin (scheme, host, port).
- `HEAD /push/<token>` no longer counts as a push (link previews and probes
  kept push monitors up); it gets `405` with `Allow: GET, POST`.
- `contrib/nginx/mode-b.conf`: private pages lost the CSP and the other
  security headers, because nginx does not inherit `add_header` into a
  location that sets its own. They are now repeated there.

### Fixed
- A restart during a maintenance window forgot the state from before the
  window: a monitor that was DOWN started over and sent a second DOWN
  notification. MAINTENANCE is now derived from the windows when pages are
  built, so it is also shown from the start of a window (not only after the
  first check in it) and ends with the window.
- After a failed write of the status pages (e.g. a full disk), files whose
  content did not change afterwards (style sheet, favicon, badges) were never
  written again. Now everything is written again after a failure.
- `log_dir` and the fallback page directory follow a configured `data_dir`
  (`<data_dir>/log`, `<data_dir>/www`); before, they stayed below
  `~/.local/state/uptime-kinomi`.
- `dns` monitors accept names with `_`, such as `_dmarc.example.org`.
- `check-config` with a broken `kinomi.toml` now still checks monitors and
  pages (with the built-in defaults), so it reports every problem at once.
- Replay at startup dropped log events from the same second as the state
  file's `covered_until` that were logged after the state was saved. State
  files now also store `covered_events`, the number of events of that
  second they include.
- `/healthz` answered `200 ok` even with a stuck scheduler. It now checks
  that the dispatcher and the event loop ran within the last 10 s, and
  answers `503` otherwise.
- Failed logins are now logged, as promised by the spec: one line
  `event=AUTH_FAIL count=N throttled=M` per log flush with failures.
- Page states and the banner carry a symbol (✓ ⚠ … ✗ ⚙ ‖ ?) besides text
  and colour, as the spec requires (never colour alone).
- The private self status shows the average check time of the last 24 h
  and the time of the last log flush; `/metrics` has them as
  `kinomi_check_response_ms_24h` and `kinomi_last_log_flush_timestamp_seconds`.
- Private pages and the private `status.json` show the last error (time
  and message of the last FAIL or DOWN), not only the last check's message.
  State files keep it (`last_error_time`, `last_error`).
- Stale badges (monitor removed from a page, or `show_badges` turned off)
  and the private assets without private pages are removed from
  `output_dir` at startup. In mode B the proxy served them from disk.

### Changed
- Logo and favicon: the white ring is as wide as the pulse line; the berry
  is correspondingly larger.

## [0.4.0] - 2026-10-27

**Upgrading:** with the default `output_dir`, pages of the user service move
from `~/.local/state/uptime-kinomi/www` to the tmpfs
`/run/user/<uid>/uptime-kinomi/www` (if lingering is enabled or you are
logged in). If a web server reads the files directly (mode B), point it to
the new path or set `output_dir` in `kinomi.toml`. Data and logs stay where
they were.

### Added
- `contrib/install.sh`: one-line installation for the current user (no
  root): picks the binary for the CPU, verifies the release signature with
  minisign or OpenSSL 3 and the checksum, creates a starter configuration,
  checks an existing configuration with the new version before replacing
  anything, and installs a systemd user service. `--listen`, `--version`,
  `--no-service`, `--no-auto-update`, `--uninstall`.
- Ansible: the playbook is replaced by two roles. `contrib/ansible-role-user/`
  deploys for the login user without any become. `contrib/ansible-role-root/`
  logs in as root, installs `curl`, `wget` and `minisign` with apt, and
  creates `KINOMI_USER` (default `kinomi`) if it does not exist. Both enable
  lingering, so the service starts at boot without a login, and download and
  verify the latest release only if no binary is installed; an installed one
  is never replaced.
- Weekly self-update: `contrib/systemd/user/uptime-kinomi-update.timer` and
  `.service` run `self-update`, check the configuration with the new version,
  restart the service and roll back if the check fails or the service does
  not stay up. Enabled by `install.sh` (`--no-auto-update` to skip) and the
  Ansible roles (`kinomi_auto_update`).
- Fuzzing: nine cargo-fuzz targets in `fuzz/` for every parser of network
  data (HTTP requests and routing, Basic Auth, push, redirect targets, DNS,
  certificates, release metadata) and of configuration, log and state files,
  some with round-trip properties. CI fuzzes each target for a minute on
  every change and for 30 minutes weekly; `cargo test` runs the seeds on
  stable. The code is now a library plus a small `main.rs`, so that the
  parsers can be fuzzed.
- Favicon: every page links `assets/favicon.svg`, served at
  `/assets/favicon.svg` (and below `/private/`).

### Changed
- New logo: a blackberry with a pulse line, as a round sticker. `docs/logo.py`
  generates `docs/logo.svg` and `assets/favicon.svg`.
- Default directories depend only on the user, never on how kinomi was
  started: the systemd service and a manual `uptime-kinomi run` use the same
  data, log and page directories. `$STATE_DIRECTORY` and the other systemd
  variables are no longer read; pages go to `/run/user/<uid>/uptime-kinomi/www`
  (a tmpfs) when that directory exists. The user unit sets no directories;
  for the system unit, the paths are set in `kinomi.toml`.
- `run` prints the config, data, log and page directories and the server
  address at startup.
- SPEC.md §1 lists resource measurements from a Raspberry Pi 1; §10.2
  explains why pages belong on a tmpfs.

### Fixed
- An htpasswd line with a multi-byte character in the hash crashed
  `check-config` and startup instead of being reported (found by fuzzing).
- Status pages are written without fsync (they are regenerated anyway):
  on a Raspberry Pi 1 with `output_dir` on the SD card, 50 monitors wrote
  4.2 MB in 10 minutes. State files are still written durably.
- The systemd user unit no longer uses `StateDirectory=`: systemd 255+
  linked `~/.local/state/uptime-kinomi` to the configuration directory
  `~/.config/uptime-kinomi` (found on a Raspberry Pi with systemd 257).
- Output to a closed pipe (e.g. `uptime-kinomi check-config | head`) no
  longer panics with "Broken pipe".

## [0.3.0] - 2026-10-23

All planned features: push and DNS monitors, notifications, heartbeat
(M3), incidents, maintenance windows, status.json, feeds, badges,
/metrics and deployment examples (M4).

### Added
- `push` monitors: `GET`/`POST /push/<token>` (uptime-kuma compatible,
  `?status=down` supported, everything else ignored and never stored),
  constant-time token check, one push per second per token, empty
  responses; DOWN after `interval + grace` without a push.
- `dns` monitors with a built-in minimal DNS client (A, AAAA, CNAME, MX,
  TXT, NS; CNAME chains; optional expected answer set).
- Notifications: `webhook` (JSON, Discord, Slack/Mattermost), `ntfy`,
  `gotify` and `telegram`, for the events down, up (with downtime), warn (at
  most once per 24 h) and reminder (`resend_interval`); sent by a separate
  thread with retries, every result logged as `event=NOTIFY`.
- Outbound heartbeat (`[heartbeat]`), changes logged as `event=HEARTBEAT`.
- Incidents (`incidents/*.toml`) on pages and in feeds.
- Maintenance windows (`maintenance/*.toml`, once/daily/weekly in local
  time): results are logged as `MAINT`, counted separately, and neither
  change the state machine nor send notifications.
- `status.json` per page (schema 1), Atom feed per page, SVG badges
  (`show_badges`), `/metrics` in the Prometheus text format
  (`server.metrics`, behind Basic Auth). Public files contain only what the
  public page shows.
- `contrib/`: example configuration (checked in CI), systemd user and
  system units, nginx and Apache examples for both deployment modes, an
  Ansible playbook.
- `check-config` lists pages, notifiers, incidents and maintenance windows.

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

[Unreleased]: https://github.com/uptime-kinomi/uptime-kinomi/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/uptime-kinomi/uptime-kinomi/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/uptime-kinomi/uptime-kinomi/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/uptime-kinomi/uptime-kinomi/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/uptime-kinomi/uptime-kinomi/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/uptime-kinomi/uptime-kinomi/releases/tag/v0.1.0
