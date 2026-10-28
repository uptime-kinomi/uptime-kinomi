# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Security
- `HEAD` on a percent-encoded push URL (`/%70ush/<token>`) still counted
  as a push: the method check used the raw path. Now every decision uses
  the decoded path.
- Built-in server: every response has `Cache-Control` (before, `401`,
  `429` and redirects below `/private/` had none, and a `301` is cacheable
  by default); the 10 s write timeout covers the whole response; the
  accept loop pauses after an error (e.g. out of file descriptors) instead
  of spinning.
- `http.url` with a user name or password (`https://user:pw@...`) is
  rejected (it was shown on private pages); `check-config` warns about
  credential headers (`Authorization`, API keys, ...) in monitor files that
  are readable by all users.

### Added
- `check-config` warns when `data_dir`, `log_dir` or `output_dir` is inside
  the configuration directory (deleted when the directory is deployed by
  replacing it, as the Ansible roles do).

### Fixed
- IPv6: `server.hostnames` accepts IPv6 addresses (`"::1"`, compared as
  addresses), so a server on `[::1]:8080` can be used without accepting any
  Host header. install.sh `--listen` accepts IPv6 addresses and then adds
  `::1` to `hostnames`; `--uninstall` no longer names a fixed data path.
- Reminders kept being postponed by restarts: the first one came
  `resend_interval` after the start. They now keep their schedule from when
  the monitor went DOWN.
- After `rebuild-state` (or without state files), a disabled push monitor
  was treated as enabled. Disabled monitors are now logged after `START`
  (`event=DISABLED monitor=ID`).
- The mode-B proxy examples send `Cache-Control: no-cache` for public pages
  (kinomi's own server does the same; the proxies also use it for `/assets/`).
- Log cleanup: a failed deletion of expired log files was reported as a
  failed log write; it is now a warning, tried again the next day. A file
  that cannot be deleted no longer stops the deletion of the others.
- `400` and `431` responses and `/private` (without slash) now get the same
  `Cache-Control` as everything else.
- State files with more than 100 state changes are trimmed when loaded.
- A push monitor never failed if kinomi restarted more often than
  `interval + grace` (deploys, updates, reboots): every start set a new
  deadline `interval + grace` from the start. Now the deadline counts from
  the last push before the restart, and an overdue monitor fails `grace`
  after the start. A missed push before the restart counts too (next
  failure `interval` later), a monitor that was disabled waits a full
  `interval + grace`, and a wrong clock at the start (fake-hwclock before
  NTP) is corrected when the clock jumps.
- Reminders (`resend_interval`) only came with a result, so a daily push
  monitor reminded at most once a day. They are now sent on time.
- A push monitor that was DOWN stayed DOWN after a maintenance window even
  if pushes came in during it. A push (`status=up`) now counts inside a
  window too.
- The push examples set `retries = 0`: with `[defaults] retries = 1`, a
  missed daily push only made the monitor DOWN a day later.
- A state file with an incomplete `last_error` pair was half loaded before
  being rejected; the following replay then skipped the data it had not
  loaded (heartbeat bar, totals, changes). Now a broken file leaves nothing
  behind (regression in 0.5.0).
- Replay counted `event=NOTIFY` lines against `covered_events`, so a result
  of the same second could be counted twice (regression in 0.5.0).
- If removing stale pages at startup kept failing, every render rewrote
  all files (SD card wear). The cleanup is now tried once and its error
  reported; the pages count as written.
- A notification endpoint that answered with a redirect (e.g. an `http://`
  ntfy URL redirecting to https) was logged as delivered (`ok=true`) without
  delivering anything. Any non-2xx answer is now an error (`HTTP 301`) and
  retried.
- `timeout` now covers the whole check: before, an `http` check got the
  full timeout for every redirect hop (up to 11 times with
  `max_redirects = 10`), and a `tcp` banner read for every read.
- Paused monitors showed "PAUSED for …" with the time since their last
  state change; now no duration (and `since: null` in `status.json`).
- Results inside a maintenance window did not log `cert_expires`, so a
  rebuilt state lacked it.
- State files of 0.4.0 with `state = "MAINTENANCE"` kept showing
  MAINTENANCE outside any window; they are read as UNKNOWN.
- `event=AUTH_FAIL` could be logged after `event=STOP` at shutdown.
- Update timer: a new version that crashed and was restarted by systemd
  within the 15 s check counted as running; now the main process must be
  the same after 15 s.
- `check-config` with a broken `kinomi.toml` also reports duplicate push
  tokens.
- `install.sh` never overwrites a starter file (before, all of them were
  written whenever `kinomi.toml` was missing), says that `--listen` only
  applies to a new configuration, and its summary shows the address and
  directories of the configuration in effect.
- Ansible root role: installs `sudo` (needed for commands as
  `KINOMI_USER`), and test-runs a downloaded binary as `KINOMI_USER`, not
  as root.
- The Atom feed was invalid without `server.base_url`: entries without a
  link need content. Every entry now has content (its title if there is no
  text).
- A monitor that came back from DOWN straight into WARN (e.g. with an
  expiring certificate) sent only `up`; now also `warn` (at most once per
  24 h as before).

### Changed
- The uptime badge moved from `badge/{monitor}-uptime.svg` to
  `badge/uptime/{monitor}.svg`: monitor `web-uptime`'s state badge and
  monitor `web`'s uptime badge had the same name, and one overwrote the
  other.
- Ansible roles: the configuration is sent as one archive instead of one
  transfer per file (twice). Ansible Vault files are decrypted and symlinks
  followed on the controller; on the target the archive is unpacked into an
  emptied staging directory (by `KINOMI_USER` in the root role), checked,
  and replaces the live directory only if something differs. The roles stop
  if `data_dir`, `log_dir` or `output_dir` is inside the configuration
  directory (relative, or an absolute path into the live directory). A complete run takes about 4.5 minutes on an idle Raspberry
  Pi 1.

## [0.5.0] - 2026-10-27

No action is needed for upgrading: state files of 0.4.0 are read as before
(new keys are added on the next save).

### Security
- Built-in server: the 5 s read timeout applied to each read, so four
  clients sending one byte every few seconds could block it. The whole
  request head now has one 5 s deadline, and the 8 KiB head limit is exact.
- The `http` monitor no longer sends `basic_auth_*` credentials and custom
  `headers` to redirect targets on another origin (scheme, host, port).
- `HEAD /push/<token>` no longer counts as a push (link previews and probes
  kept push monitors up); it gets `405` with `Allow: GET, POST`.
- `contrib/nginx/mode-b.conf`: private pages lost the CSP and the other
  security headers, because nginx does not inherit `add_header` into a
  location that sets its own. They are now repeated there.

### Added
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
- `check-config` also warns when the htpasswd file, `kinomi.toml` with a
  `[heartbeat]` URL, or a webhook file with an inline URL is readable by all
  users. Webhooks accept `url_file` instead of `url`.

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
- A check could take longer than its `timeout`: the certificate handshake
  of `http` got a full `timeout` of its own after the request, and the
  name lookups of `tcp` and of the certificate check waited for the system
  resolver as long as it took. Now the whole check stays within `timeout`.
- `/healthz` answered `200 ok` even with a stuck scheduler. It now checks
  that the dispatcher and the event loop ran within the last 10 s, and
  answers `503` otherwise.
- Stale badges (monitor removed from a page, or `show_badges` turned off)
  and the private assets without private pages are removed from
  `output_dir` at startup. In mode B the proxy served them from disk.
- Ansible roles: a renamed or removed configuration file stayed on the
  target (in the staging and the live configuration directory), so e.g. a
  renamed push monitor existed twice and `check-config` reported a
  duplicate token. The staging directory is now emptied before each check,
  and files no longer in the source are removed from the live directory.
- Ansible roles: the first installation now also checks the trusted
  comment and test-runs the binary, like `install.sh`.
- Weekly update timer: a service that was enabled but stopped was rolled
  back (and then started) after an update. Now a stopped service stays
  stopped and uses the new version at its next start.
- A monitor that was PENDING before a restart started counting failures
  from 1 again, so with `retries` of 2 or more it went DOWN later than it
  should. The count now continues from the heartbeat bar.

### Changed
- Logo and favicon: the white ring is as wide as the pulse line; the berry
  is correspondingly larger.
- `/metrics` leaves disabled monitors out of every per-monitor metric.
- `check-config` warns about `timeout` in a push monitor (it has no effect).
- SPEC.md and AGENTS.md describe the code as it is: all defaults and limits,
  the state file and log formats, notification texts, server limits.

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

[Unreleased]: https://github.com/uptime-kinomi/uptime-kinomi/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/uptime-kinomi/uptime-kinomi/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/uptime-kinomi/uptime-kinomi/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/uptime-kinomi/uptime-kinomi/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/uptime-kinomi/uptime-kinomi/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/uptime-kinomi/uptime-kinomi/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/uptime-kinomi/uptime-kinomi/releases/tag/v0.1.0
