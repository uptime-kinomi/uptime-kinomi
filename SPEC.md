# uptime-kinomi — Specification

Status: **draft**, milestones M0 to M4 implemented and tested on a Raspberry Pi 1 (see §13)
License: MIT

uptime-kinomi is a self-hosted uptime monitor in the spirit of
[uptime-kuma](https://github.com/louislam/uptime-kuma), designed for
single-board computers (SBCs). The Raspberry Pi 1 (ARMv6, 700 MHz, single
core, 256/512 MB RAM) is a first-class target.

Keywords in this document: **MUST**, **SHOULD**, **MAY** as in RFC 2119.

---

## 1. Goals and non-goals

### Goals
- Written in Rust. The crate uses `#![forbid(unsafe_code)]`. Dependencies are
  few, audited (`cargo-deny`, `cargo-geiger` report in CI), and justified in
  `AGENTS.md`.
- Simple code structure: **one source file per monitor type** and **one source
  file per notification type**. Adding a type means adding one file and one
  registry line.
- Configuration only via TOML files: one global file, **one file per
  monitor, notification, status page, incident and maintenance window**. This
  makes deployment with Ansible/Salt/NixOS/plain `scp` trivial.
- Output is **static HTML5 + CSS only. No JavaScript, ever.**
- Runs well on one slow CPU core with little RAM and an SD card.
- Prebuilt static binaries for all common SBC architectures (§12).
- **Runs as an ordinary, unprivileged user.** Root is never required: not to
  install, not to configure, not to run (§10.6). A system-wide installation
  with a dedicated service user is supported as well, but kinomi itself never
  needs root privileges or capabilities.
- Security first: all untrusted input is size-limited and validated before it
  is parsed; nothing fetched from a monitored target is ever reflected into
  pages or notifications.

### Non-goals
- No web UI for configuration, no database, no user accounts beyond HTTP
  Basic Auth for private pages.
- No inbound TLS. TLS termination is done by a reverse proxy
  (nginx/Apache/Caddy, certificates e.g. via certbot / Let's Encrypt).
- No JavaScript, no charts that require JavaScript.
- Not a metrics/observability platform (use Prometheus for that; kinomi
  MAY expose a minimal `/metrics`, §9.5).

### Resource targets (Raspberry Pi 1, 50 monitors, 60 s interval)

Measured on 2026-10-23 on a Raspberry Pi Model B Rev 1 (ARMv6 700 MHz,
256 MB, Raspbian 13) with 10 HTTPS, 10 DNS and 30 TCP/HTTP monitors, two
pages and 100 badges, over 10 minutes:

| Metric | Target | Measured (0.3.0) |
|---|---|---|
| Resident memory | < 10 MB | 4.8 MB (peak 6.0 MB), 12 threads |
| Average CPU | < 5 % | 1.8 % of the single core |
| Startup until the server answers | < 1 s (excluding log replay of > 1 day) | 0.4–0.5 s |
| Binary size (stripped, static) | < 6 MB | 3.3 MB |
| SD card writes | ≤ 1 log flush per `flush_interval` (default 30 s) | log + state: ~0.3 MB / 10 min; pages on a tmpfs by default (§10.2) |
| Page render (2 pages, JSON, feeds, 100 badges) | – | 162 ms |
| First login (bcrypt cost 10) / cached | – | 0.76 s / 10 ms |
| HTTPS check incl. TLS handshake | – | 80–370 ms |
| `self-update` (download, verify, test run, replace) | – | 3.5 s |

---

## 2. Terminology

| Term | Meaning |
|---|---|
| **Monitor** | One thing being checked, defined by one file in `monitors/`. |
| **Active monitor** | kinomi actively performs the check (HTTP, TCP, DNS). |
| **Passive monitor** | Another system calls kinomi (push). kinomi only acknowledges. |
| **Check** | A single execution of an active monitor, or the evaluation of a push deadline. |
| **Notifier** | A notification channel, defined by one file in `notifications/`. |
| **Page** | A status page, defined by one file in `pages/`. Public or private. |
| **Incident** | A human-written announcement shown on pages (`incidents/`). |
| **Maintenance** | A time window during which monitors are in `MAINTENANCE` and notifications are muted (`maintenance/`). |

---

## 3. Configuration

### 3.1 Layout

The configuration directory is found in this order (first match wins):

1. `--config-dir DIR` on the command line,
2. the environment variable `KINOMI_CONFIG_DIR`,
3. `$XDG_CONFIG_HOME/uptime-kinomi` (usually `~/.config/uptime-kinomi`) if it exists,
4. `/etc/uptime-kinomi`.

So a normal user gets `~/.config/uptime-kinomi`, and a system-wide
installation uses `/etc/uptime-kinomi`. The directory structure is the same:

```
~/.config/uptime-kinomi/          or /etc/uptime-kinomi/
├── kinomi.toml                   global configuration
├── htpasswd                      bcrypt htpasswd file for private pages (optional)
├── custom.css                    optional user style sheet
├── monitors/*.toml               one file per monitor
├── notifications/*.toml          one file per notifier
├── pages/*.toml                  one file per status page
├── incidents/*.toml              one file per incident (optional)
└── maintenance/*.toml            one file per maintenance window (optional)
```

### 3.2 General rules
- The **ID** of every object is its file name without `.toml`. IDs MUST match
  `^[a-z0-9][a-z0-9_-]{0,63}$`. Files not ending in `.toml` and hidden files
  (starting with `.`) are ignored, so editor backups and `.swp` files are harmless.
- Every file MUST contain `config_version = 1`.
- Unknown keys are an **error** (`deny_unknown_fields`). Typos never pass
  silently.
- Durations are strings with a unit: `"500ms"`, `"30s"`, `"5m"`, `"2h"`, `"1d"`.
- Config files are limited to 64 KiB each.
- Relative paths in any config file are resolved against the configuration
  directory, so a configuration directory can be copied between machines and
  users unchanged.
- Secrets (tokens, passwords) MAY be given inline or via a `*_file` key that
  points to a file whose content (trimmed) is the secret. kinomi warns at startup
  and in `check-config` if a file containing secrets is world-readable.
- Config is read **once at startup**. To apply changes, restart the service
  (`systemctl restart uptime-kinomi`). Startup is designed to be fast.
- `uptime-kinomi check-config [--config-dir DIR]` validates the whole
  configuration (syntax, types, references between files, duplicate push
  tokens, file permissions) and exits non-zero on error. Suitable for Ansible's
  `validate:` parameter and CI.

### 3.3 Global file `kinomi.toml`

```toml
config_version = 1

[server]
enabled = true                       # built-in HTTP server (§9)
listen = "127.0.0.1:8080"            # default; behind a reverse proxy
hostnames = ["status.example.org"]   # allowed Host headers; empty = any (warns)
base_url = "https://status.example.org"  # absolute URLs in feeds, badges, notifications
htpasswd_file = "htpasswd"           # relative to the config dir; needed if any page is private

[pages]
# output_dir = "..."                 # default: see §10.6; may be a tmpfs, see §10.2
render_interval = "60s"
timezone = "Europe/Berlin"           # IANA name; display only, storage is UTC
custom_css = "custom.css"            # optional, appended after default CSS
auto_refresh = "60s"                 # <meta http-equiv="refresh">, "0s" disables
default_page = "home"                # page served at "/"
recent_checks = 50                   # length of the heartbeat bar

[storage]
# data_dir = "..."                   # state files; default: see §10.6
# log_dir = "..."                    # daily check logs; default: see §10.6
flush_interval = "30s"               # buffered log writes
state_interval = "10m"               # how often state files are written
log_retention = "90d"                # log files older than this are deleted
history_retention = "400d"           # daily aggregates kept in state files

[scheduler]
workers = 4                          # concurrent checks (I/O bound, 4 is fine on 1 core)
max_queue = 32                       # due checks waiting for a worker before skipping

[heartbeat]                          # optional outbound "dead man's switch"
url = "https://other-pi.example.org/push/abcdef..."
interval = "60s"

[defaults]                           # defaults for all monitors, overridable per monitor
interval = "60s"
timeout = "10s"
retries = 1
notifications = ["ntfy-home"]
```

---

## 4. Monitors

### 4.1 Common keys (all monitor types)

```toml
config_version = 1
type = "http"                 # http | tcp | dns | push
name = "My blog"              # display name (HTML-escaped on output)
description = "..."           # optional, shown on private pages only
interval = "60s"              # min "10s"
timeout = "10s"               # must be < interval
retries = 1                   # consecutive failures before DOWN (state PENDING in between)
notifications = ["ntfy-home", "telegram-me"]   # notifier IDs
resend_interval = "0s"        # repeat DOWN notification while down; 0 = never
enabled = true
tags = ["web", "family"]      # optional, usable for page groups
```

### 4.2 States

| State | Meaning | Counts as |
|---|---|---|
| `UP` | Last check succeeded | up |
| `WARN` | Succeeded, but a warning condition holds (e.g. certificate expires soon, slow response) | up |
| `PENDING` | Failed, but `retries` not yet exhausted | up |
| `DOWN` | Failed `retries + 1` times in a row | down |
| `MAINTENANCE` | Inside a maintenance window (§14.2) | excluded |
| `UNKNOWN` | No data yet (after first start) | excluded |

A **skipped** check (§6) is not a state. It is logged as `result=SKIPPED`,
shown as a grey gap, and excluded from uptime percentages.

### 4.3 `http` — HTTP(S)

```toml
type = "http"
[http]
url = "https://blog.example.org/"     # http:// or https:// only
method = "GET"                        # GET | HEAD
expected_status = [200]               # list; ranges allowed as "200-299"
keyword = "Welcome"                   # optional; must appear in body (case-sensitive)
invert_keyword = false                # true: keyword must NOT appear
max_body_bytes = 65536                # body read limit (default 64 KiB, max 1 MiB)
max_redirects = 0                     # 0 = do not follow
headers = { "User-Agent" = "uptime-kinomi" }   # optional, validated header names/values
basic_auth_user = "..."               # optional
basic_auth_password_file = "..."      # optional
tls_verify = true
tls_expiry_warn = "14d"               # WARN when certificate expires within this time; "0s" off
tls_expiry_down = "0s"                # DOWN when certificate expires within this time; "0s" off
slow_warn = "0s"                      # WARN if response slower than this; "0s" off
```

Security requirements:
- The body is read up to `max_body_bytes` and then the connection is closed.
  The body is never decoded beyond what keyword search needs (byte search,
  no HTML parsing, no decompression bombs: `Accept-Encoding: identity`).
- Only the *fact* of a keyword match is recorded, never body content.
- Redirect targets must be `http(s)://`. A redirect from `https` to `http` fails the check.
- The certificate expiry (`tls_expiry_warn`, `tls_expiry_down`) is read in a
  separate TLS handshake with the URL's host, at most every 6 hours (every
  10 minutes after a failure), so a Pi 1 does not pay for two handshakes per
  check. Only the validity dates of the leaf certificate are read. The expiry
  date is logged as `cert_expires=YYYY-MM-DD`.

### 4.4 `tcp` — TCP port open

```toml
type = "tcp"
[tcp]
host = "192.168.1.10"     # IP or host name
port = 22
expect_banner = "SSH-"    # optional: bytes the server sends first must start with this (max 256 bytes read)
```

### 4.5 `dns` — DNS record

```toml
type = "dns"
[dns]
name = "example.org"
record_type = "A"           # A | AAAA | CNAME | MX | TXT | NS
server = "1.1.1.1:53"       # optional (IPv4 or IPv6, port 53 if omitted);
                            # default: first nameserver in /etc/resolv.conf
expected = ["93.184.216.34"]  # optional; check fails if the answer set differs
```

Implemented with a minimal built-in DNS client: UDP, one question, EDNS with
a 1232-byte buffer, random query ID, responses with another ID ignored,
strict bounds-checked parser (at most 16 compression pointers per name). No
recursion of its own, no caching, no DNSSEC validation in v1.

- Records of the requested type behind a CNAME chain in the answer count
  (e.g. `www.github.com` type `A`). `record_type = "CNAME"` checks the CNAME
  itself.
- `expected` is compared as a set after normalization: names lowercase
  without trailing dot, addresses in canonical form, MX as
  `"<preference> <name>"`, TXT strings concatenated.
- Messages: `2 A records`, `no MX record`, `domain does not exist`,
  `unexpected A records` (the received values are never shown), `timeout`.

### 4.6 `push` — passive / heartbeat

```toml
type = "push"
interval = "5m"            # expected push interval
grace = "1m"               # extra time before the monitor fails
[push]
token_file = "secrets/backup-job.token"   # relative to the config dir; or token = "..."
```

- URL: `GET` or `POST` `{base_url}/push/{token}`. Compatible with uptime-kuma push URLs.
- Tokens MUST be ≥ 32 characters from `[A-Za-z0-9_-]` and unique across monitors.
  `uptime-kinomi gen-token` prints a new random token.
- Query parameters: `status=up|down` (default `up`). `msg` and `ping` are
  accepted for uptime-kuma compatibility but **ignored and never stored**.
  Any other parameter is ignored. Query string max 256 bytes.
- The request body is never read.
- Responses: `204 No Content` (accepted), `404` (unknown token, same timing as a
  valid token thanks to constant-time comparison), `429` (more than one push per
  second for this token), `400` (malformed `status`). Response bodies are empty.
- A missing push after `interval + grace` counts as a failed check
  (`msg="no push received"`), and again after every further `interval`
  without a push. `retries` applies as usual. `grace` defaults to `1m`.
- `?status=down` counts as a failed check (`msg="push: status=down"`).
- After a restart, the first deadline is `interval + grace` from the start.
- Push monitors are not polled, so they have no `timeout` and do not use the
  queue. They need the built-in server (`check-config` warns otherwise).

### 4.7 Planned for later versions
`ping` (ICMP; needs `CAP_NET_RAW` or `net.ipv4.ping_group_range`), `smtp`,
`command` (local script, exit code), `mqtt`, `json` (HTTP + JSON path check),
`tls` (certificate check without HTTP), `udp`.

---

## 5. Notifications

### 5.1 Events
| Event | When |
|---|---|
| `down` | Monitor enters `DOWN` |
| `up` | Monitor leaves `DOWN` (recovery), includes downtime duration |
| `warn` | Monitor enters `WARN` (e.g. certificate expiry), at most once per 24 h |
| `reminder` | Still `DOWN` after `resend_interval` |

No notifications during `MAINTENANCE` (M4). Notifications are never skipped
by the queue (§6). They are sent one after the other by a dedicated thread,
with up to 3 attempts (retries after 5 s and 30 s). Every result is logged:

```
2026-10-22T14:05:11Z event=NOTIFY monitor=blog notifier=phone kind=down ok=true
2026-10-22T14:05:11Z event=NOTIFY monitor=blog notifier=chat kind=down ok=false msg="HTTP 401"
```

A monitor that is already DOWN when kinomi starts (restored from its state
file) gets no new `down` notification; reminders continue from the start.

Notification text is generated by kinomi only, e.g.
`DOWN: My blog (HTTP 503) at 2026-10-22 16:03 CEST`. The text contains only
kinomi's own check summary (status code, error class), never content fetched
from a target.

### 5.2 Common keys
```toml
config_version = 1
type = "ntfy"
events = ["down", "up", "warn", "reminder"]   # default: all
```

### 5.3 Types in v1

Endpoint URLs may be `http://` (e.g. ntfy or Gotify in the LAN) or
`https://`, never with a user name or password in the URL. Redirects are not
followed. Errors are logged as coarse classes (`HTTP 401`, `timeout`, `TLS
certificate expired`, ...), never with the server's response.

```toml
# notifications/hook.toml: HTTP POST with a JSON body
config_version = 1
type = "webhook"
[webhook]
url = "https://chat.example.org/hooks/abc"
format = "json"        # json (default) | discord | slack (also Mattermost, Rocket.Chat)
```
`format = "json"` sends
`{"event","monitor","name","detail","time","down_for_s","text"}`
(`down_for_s` only for `up` and `reminder`); `discord` sends `{"content"}`,
`slack` sends `{"text"}`.

```toml
# notifications/phone.toml: ntfy (JSON publishing API, so any title works)
config_version = 1
type = "ntfy"
[ntfy]
url = "https://ntfy.sh"          # default
topic = "my-secret-topic-name"   # A-Z a-z 0-9 _ -, max 64
token_file = "secrets/ntfy.token"   # optional (access token), or token = "..."
priority = 4                     # optional, 1-5; default 4 for down/reminder, 3 otherwise
```

```toml
# notifications/gotify.toml
config_version = 1
type = "gotify"
[gotify]
url = "https://gotify.example.org"
token_file = "secrets/gotify.token"   # application token, sent as X-Gotify-Key header
priority = 8                          # optional, 0-10; default 8 for down/reminder, 5 otherwise
```

```toml
# notifications/telegram.toml
config_version = 1
type = "telegram"
[telegram]
bot_token_file = "secrets/telegram.token"   # "123456:ABC-..." from @BotFather
chat_id = "-1001234567890"                  # numeric chat ID or "@channelname"
```

Planned later: `smtp` (email), `matrix`, `pushover`, `signal` (via signal-cli REST), `command`.

---

## 6. Scheduler and queue

- Every active monitor has a fixed **time grid**: a start offset, then one
  point every `interval`, on the **monotonic clock** (immune to wall-clock
  jumps). Start offsets are spread over the first 10 s so not all monitors
  fire at once after startup.
- A dispatcher thread moves due checks into a bounded queue (`max_queue`).
  `workers` threads take checks from the queue. Checks use blocking I/O with
  timeouts on connect, TLS handshake, read and total duration.
- **Overload and fairness:** a due check that does not fit into the queue
  waits. When room becomes free, the waiting check that was **started least
  recently** goes first (round-robin; completion time and due time do not
  matter, so fast or "lucky" monitors cannot win). When a check starts late,
  every grid point it missed is **skipped** and logged once as
  `result=SKIPPED count=N`. So under overload every monitor is checked
  equally often, no monitor starves, and runs plus skips always add up to the
  elapsed intervals.
- A check that is still running when its next one is due is never started twice.
- Push monitors do not use the queue; their deadline is evaluated by the
  dispatcher (cheap).
- Queue length, skip counts and average check latency are exposed on private
  pages and `/metrics`.

---

## 7. Storage

All timestamps stored are **UTC, RFC 3339, second precision**.

### 7.1 Check log (source of truth, human-readable)
- One file per UTC day: `{log_dir}/2026-10-22.log`.
- One line per event in [logfmt](https://brandur.org/logfmt) format, timestamp first:

```
2026-10-22T14:03:11Z monitor=blog result=UP latency_ms=123 cert_expires=2026-12-01 msg="HTTP 200"
2026-10-22T14:04:11Z monitor=blog result=FAIL latency_ms=10000 msg="timeout"
2026-10-22T14:05:11Z monitor=blog result=DOWN latency_ms=10000 msg="timeout"
2026-10-22T14:05:11Z event=NOTIFY monitor=blog notifier=ntfy-home kind=down ok=true
2026-10-22T14:06:00Z monitor=nas result=SKIPPED count=1 msg="queue full"
2026-10-22T14:06:02Z monitor=backup-job result=UP msg="push"
2026-10-22T14:07:00Z event=CLOCK_JUMP delta_s=-31536000
2026-10-22T18:26:13Z event=PAUSED duration_s=11756
2026-10-22T18:26:13Z monitor=nas result=SKIPPED count=196 msg="paused"
2026-10-22T14:07:00Z event=START version=0.1.0 monitors=42
```

- `msg` values are generated by kinomi (§5.1), max 120 characters, escaped.
- Writes are buffered and flushed every `flush_interval` and on shutdown
  (SIGTERM/SIGINT) to reduce SD card wear.
- Files older than `log_retention` are deleted at the first flush after midnight UTC.

### 7.2 State files (derived, human-readable)
- One TOML file per monitor: `{data_dir}/state/{monitor}.toml`, written every
  `state_interval` and on shutdown, atomically (write temp file + rename).
- Contains current state and aggregates, and is enough to render pages without
  reading logs:

```toml
# Generated by uptime-kinomi. Rebuild with: uptime-kinomi rebuild-state
monitor = "blog"
covered_until = "2026-10-22T14:10:00Z"   # log events up to here are included
state = "UP"
state_since = "2026-10-20T08:00:00Z"
tls_expires = "2026-12-01T00:00:00Z"

[hourly]   # last 48 hours
"2026-10-22T13" = { up = 60, down = 0, skipped = 0, maint = 0, avg_ms = 118 }

[daily]    # last history_retention days
"2026-10-21" = { up = 1440, down = 0, skipped = 0, maint = 0, avg_ms = 121 }
```

- State files also keep the last 100 state changes (`changes`, for the
  feed, §8.7).
- At startup, kinomi loads state files and **replays log lines after
  `covered_until`**. This also rebuilds the recent-checks bar in memory.
- `uptime-kinomi rebuild-state` regenerates all state files from the logs
  (limited to what `log_retention` still holds). Deleting the state directory is
  always safe within the log retention.

---

## 8. Status pages

### 8.1 Page config `pages/{id}.toml`

```toml
config_version = 1
title = "Family services"
public = true                 # false: requires HTTP Basic Auth (§9.3)
description = "Is the internet broken, or just our Wi-Fi?"
show_incidents = true
show_badges = true            # generate SVG badges for monitors on this page

[[groups]]
name = "Internet"
monitors = ["router", "dns-cloudflare"]

[[groups]]
name = "Home"
tags = ["family"]             # alternatively select by tag

# A private "everything" page is easy to define:
# [[groups]]
# name = "All"
# monitors = ["*"]
```

All monitors listed must exist (`check-config`). Without any page,
`check-config` warns that no status pages will be generated.

### 8.2 Content (similar to uptime-kuma)
Per monitor:
- name, current state (text + symbol + colour, never colour alone);
- heartbeat bar of the last `recent_checks` checks (CSS, hover via `title`
  attribute);
- uptime percentage over **24 h** and **30 d**;
- average response time over 24 h (successful checks only: the time a
  timeout or a refused connection takes says nothing about the service);
- certificate expiry date (HTTPS monitors with TLS checks).

Per page: overall status banner ("All systems operational" / "Partial outage" /
"Major outage"), active and recent incidents, upcoming maintenance, time of last
update (in the configured time zone).

Private pages additionally show: monitor type and target, last error message,
kinomi self-status (version, uptime, queue length, skipped checks in the last
24 h, render duration, last log flush).

### 8.3 Generated files
```
{output_dir}/                          URL path (built-in server)
├── public/assets/kinomi.css           /assets/kinomi.css
├── public/{page}/index.html           /{page}/
├── public/{page}/status.json          /{page}/status.json         (§8.6)
├── public/{page}/feed.atom            /{page}/feed.atom           (§8.7)
├── public/{page}/badge/{monitor}.svg  /{page}/badge/{monitor}.svg (§8.8, if show_badges)
├── private/assets/kinomi.css          /private/assets/kinomi.css
└── private/{page}/...                 /private/{page}/...        (public = false)
```
- The file tree mirrors the URL space, so in mode B (§9.1) a proxy serves
  `public/` at `/` and `private/` at `/private/` with no rewriting. All links
  are relative, so pages also work below a proxy prefix such as `/status/`.
- Directories are created with mode `0750` and files `0640`, so a web server
  group can read them. Page directories of removed pages are deleted at
  startup (only directories that contain nothing but `index.html`).
- Page IDs `assets`, `private`, `push`, `healthz` and `metrics` are reserved.
- All files are written atomically (temp file + rename) and **only rewritten
  when their content changed**.
- HTML: valid HTML5, `<meta http-equiv="refresh">` per `auto_refresh`, dark mode
  via `prefers-color-scheme`, readable on phones, works in text browsers
  (lynx/w3m). No external resources (fonts, CDNs).
- All user-provided strings are HTML/XML-escaped. `custom.css` is copied as-is
  (trusted admin input).

### 8.4 Rendering
- A renderer thread runs every `render_interval`, only if something changed
  (and at least every 10 minutes, so "last updated" and durations stay
  current). Pages are also served from memory by the built-in server.
- It waits until the check queue is empty (at most one `render_interval`) and
  yields between pages, so rendering does not delay checks. The shipped
  systemd unit additionally sets `Nice=10`.

---

### 8.5 Visibility of all generated files

Every file of a page has the page's visibility. For a private page
(`public = false`), its `status.json`, feed and badges live below
`/private/{page}/` and need HTTP Basic Auth exactly like its HTML (§9.3);
in mode B (§9.1) the proxy protects `/private/` as a whole.

Public files contain **only what the public HTML page shows**: no targets
(URLs, hosts), no last check messages, no descriptions, no self status.
Those appear only in the private variants.

### 8.6 `status.json`

The machine-readable status of one page, for scripts, dashboards or another
kinomi. UTF-8 JSON, `Content-Type: application/json`, regenerated with the
page.

```json
{
  "schema": 1,
  "kinomi_version": "0.3.0",
  "page": "home",
  "title": "Our services",
  "generated": "2026-10-23T08:15:00Z",
  "overall": "partial",
  "monitors": [
    {
      "id": "blog",
      "name": "My blog",
      "group": "Web",
      "state": "UP",
      "since": "2026-10-22T18:52:27Z",
      "uptime_24h": 99.98,
      "uptime_30d": 99.95,
      "avg_ms_24h": 120,
      "cert_expires": "2026-12-01",

      "type": "http",
      "target": "https://blog.example.org/",
      "last_check": "2026-10-23T08:14:58Z",
      "last_msg": "HTTP 200"
    }
  ]
}
```

- `schema` is incremented on incompatible changes; new fields may be added
  without incrementing it.
- `overall`: `up` | `partial` | `down` | `maintenance` | `unknown` (the
  banner of §8.2).
- `state`: `UP` | `WARN` | `PENDING` | `DOWN` | `MAINTENANCE` | `UNKNOWN` |
  `PAUSED` (disabled in the configuration).
- Times are UTC (RFC 3339); `cert_expires` is a date. Values that are not
  known (no data yet, no certificate check) are `null`.
- A monitor in several groups of the page appears once per group.
- `type`, `target`, `last_check` and `last_msg` are **only present in the
  private variant** (§8.5).

### 8.7 Atom feed `feed.atom`

Per page, an [Atom](https://www.rfc-editor.org/rfc/rfc4287) feed with the
incidents (§14) of the page and the state changes to and from `DOWN` of its
monitors during the last 30 days, newest first, at most 50 entries. Entry
IDs are stable `tag:` URIs (`tag:uptime-kinomi,2026:{page}/{monitor}/{unix
time}`). Links are absolute if `server.base_url` is set, otherwise omitted.
For this, each state file keeps the last 100 state changes (§7.2).

### 8.8 Badges

With `show_badges = true`, `badge/{monitor}.svg` (state, e.g. `My blog | UP`)
and `badge/{monitor}-uptime.svg` (`uptime 30d | 99.95 %`) are generated for
every monitor of the page, in the style of shields.io: plain SVG, no
scripts, no external resources, text XML-escaped. For embedding in READMEs
and wikis; badges of private pages need Basic Auth like everything else.

## 9. Built-in HTTP server

### 9.1 Purpose and deployment modes
The built-in server is small and optional. Two documented modes:

- **Mode A (default):** reverse proxy (TLS) → `127.0.0.1:8080` built-in server.
  kinomi serves pages, Basic Auth and push.
- **Mode B (paranoid):** the reverse proxy serves `{output_dir}/public` and
  `{output_dir}/private` directly and does Basic Auth itself with the same
  htpasswd file. Only `location /push/` is proxied to kinomi.
  If there are no push monitors, `server.enabled = false`.

Example configs for nginx and Apache for both modes are shipped in `contrib/`.

### 9.2 Routes
| Route | Auth | Content |
|---|---|---|
| `GET/HEAD /` | – | redirect to `default_page` |
| `GET/HEAD /{page}/...` | public pages | files from `public/` |
| `GET/HEAD /private/{page}/...` | Basic Auth | files from `private/` |
| `GET/HEAD /assets/...` | – | CSS |
| `GET/POST /push/{token}` | token | §4.6 |
| `GET /healthz` | – | `200 ok` if scheduler alive |
| `GET /metrics` | Basic Auth | Prometheus text format (optional, `server.metrics = true`, §9.5) |

### 9.3 HTTP Basic Auth
- Users and passwords live in an **htpasswd file with bcrypt hashes** (`htpasswd -B -C 10`),
  the same format Apache and nginx use. Other hash formats are rejected.
  `uptime-kinomi hash-password USER` prints such a line. If stdin is not a
  terminal, the password is read from stdin; otherwise a random password is
  generated and printed once to stderr (no terminal handling needed, so no
  extra dependency). The `htpasswd` tool (package `apache2-utils`, needs
  root to install) is not required.
- Successful verifications are cached in memory for 10 minutes (key: SHA-256 of
  the `Authorization` header, compared in constant time). A Pi 1 needs about
  0.5 s per bcrypt check, and the cache avoids paying that on every request.
- Failed attempts: global exponential back-off (max 1 verification per second
  after 5 failures), logged with `event=AUTH_FAIL`.
- If `listen` is not a loopback address and private pages exist, kinomi warns
  at startup that passwords would travel unencrypted without a TLS proxy.

### 9.4 Hardening
- Request parsing with `httparse`; request head max 8 KiB, max 32 headers;
  request bodies are never read.
- `Connection: close` (no keep-alive), read timeout 5 s, write timeout 10 s,
  max 16 concurrent connections (excess: immediate close).
- Only `GET`, `HEAD`, and `POST` for `/push/` → otherwise `405`.
- `Host` header must match `hostnames` → otherwise `421`.
- Paths: percent-decoding once, reject `..`, `\`, NUL, and anything outside the
  generated file set (lookup in an in-memory index of generated files, not a
  file system path join).
- Response headers: `Content-Security-Policy: default-src 'none'; style-src 'self';
  img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`,
  `Cache-Control` short for HTML, `X-Frame-Options: DENY`.
- `X-Forwarded-*` headers are ignored (not needed: rate limits are per push token
  and global for auth).

---

### 9.5 `/metrics`

With `server.metrics = true` (needs `server.htpasswd_file`), `/metrics`
serves the [Prometheus text format](https://prometheus.io/docs/instrumenting/exposition_formats/)
behind Basic Auth, regenerated with the pages:

```
kinomi_build_info{version="0.3.0"} 1
kinomi_start_time_seconds 1792678991
kinomi_queue_length 0
kinomi_skipped_checks_24h 3
kinomi_monitor_up{monitor="blog",type="http"} 1
kinomi_monitor_uptime_ratio{monitor="blog",window="24h"} 0.9998
kinomi_monitor_uptime_ratio{monitor="blog",window="30d"} 0.9995
kinomi_monitor_response_ms_24h{monitor="blog"} 120
kinomi_monitor_cert_expiry_timestamp_seconds{monitor="blog"} 1795996800
```

`kinomi_monitor_up` is 1 for UP and WARN, 0 for PENDING and DOWN; monitors
without data, disabled or in maintenance are left out. Other values without
data are left out as well.

---

## 10. Platform and operations

### 10.1 Time
- Storage and logs: UTC. Display: `pages.timezone`, using the system tz database
  (`/usr/share/zoneinfo`).
- SBCs often have no real-time clock: the time is wrong until NTP syncs.
  Scheduling uses the monotonic clock. A wall-clock jump of more than 60 s is
  logged as `event=CLOCK_JUMP`. If the system time is before the build date of
  the binary, kinomi does not write log lines and waits until the clock is plausible.
- A pause of the whole process (standby of a VM host, frozen system: the
  dispatcher, which wakes at least once per second, did not run for more
  than 30 s) is logged once as `event=PAUSED duration_s=N`; the checks missed
  meanwhile are logged as `result=SKIPPED msg="paused"` instead of
  `"queue full"`. Where the monotonic clock stops during suspend (bare
  metal), the wall-clock jump is logged as `event=CLOCK_JUMP` instead.

### 10.2 SD card friendliness
- Buffered log writes (§7.1), state files every 10 min (§7.2), pages only
  rewritten on change (§8.3) and without fsync (they are regenerated anyway).
- Pages change at almost every render (heartbeat bars, durations). Measured
  on a Raspberry Pi 1 with 50 monitors every 60 s: about 370 KB of page
  files per render, i.e. roughly 500 MB per day. Therefore the default
  `output_dir` is a tmpfs (`$RUNTIME_DIRECTORY` with the systemd units,
  otherwise `$XDG_RUNTIME_DIR`, §10.6); pages are regenerated within one
  render cycle after a reboot. Log and state files together are about
  0.3 MB per 10 minutes for the same setup. If `output_dir` must be on the
  SD card (e.g. for a proxy in mode B), consider `render_interval = "5m"`.

### 10.3 Self-monitoring
- Optional outbound heartbeat (`[heartbeat]`): a `GET` of `url` every
  `interval`, e.g. the push URL of a second kinomi (two Pis monitoring each
  other) or of a service like healthchecks.io. Only changes are logged
  (`event=HEARTBEAT ok=true|false`), so a long outage does not flood the log.
- Self-status on private pages and `/metrics` (§8.2).

### 10.4 CLI
```
uptime-kinomi run            [--config-dir DIR]   # default command
uptime-kinomi check-config   [--config-dir DIR]
uptime-kinomi rebuild-state  [--config-dir DIR]
uptime-kinomi gen-token
uptime-kinomi hash-password USER     # prints an htpasswd line (bcrypt)
uptime-kinomi self-update [--check] [--release X.Y.Z [--allow-downgrade]] [--rollback]
uptime-kinomi version
```
`--config-dir` is resolved as described in §3.1.
Exit codes: 0 ok, 1 runtime error, 2 configuration or usage error,
3 update available (`self-update --check` only).

### 10.5 Shipped files (`contrib/`)
- `examples/config/`: a complete configuration directory with every file
  type; CI runs `check-config` on it.
- `systemd/user/uptime-kinomi.service`: **user unit** for `systemctl --user`
  (no root needed, §10.6).
- `systemd/system/uptime-kinomi.service`: system unit running as the dedicated
  user `kinomi` (never root), hardened (`NoNewPrivileges`, empty
  `CapabilityBoundingSet`, `ProtectSystem=strict`, `ProtectHome`,
  `PrivateTmp`, `PrivateDevices`, `MemoryDenyWriteExecute`,
  `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX`,
  `SystemCallFilter=@system-service`, ...) with `StateDirectory=`,
  `LogsDirectory=`, `RuntimeDirectory=` and `Nice=10`.
- `nginx/mode-a.conf`, `nginx/mode-b.conf`, `apache/mode-a.conf`,
  `apache/mode-b.conf`: reverse proxy examples for both modes (§9.1),
  including `/push/` and `/metrics` in mode B.
- `ansible/playbook.yml`: deploys binary, configuration and user unit; the
  configuration is copied to a staging directory and checked with
  `check-config` before it goes live.
- `release/sign-release.sh`: maintainer tool (§12.1).

### 10.6 Running without root
kinomi is designed to run as an ordinary user. Nothing in v1 needs privileges:
it listens on an unprivileged port (default 8080), uses no raw sockets, and
writes only to its own directories.

**Default directories** (when not set in `kinomi.toml`), first match wins:

| Purpose | systemd variable (set by `StateDirectory=` etc.) | Otherwise (XDG) |
|---|---|---|
| `data_dir` | `$STATE_DIRECTORY` | `$XDG_STATE_HOME/uptime-kinomi` (`~/.local/state/uptime-kinomi`) |
| `log_dir` | `$LOGS_DIRECTORY` | `$XDG_STATE_HOME/uptime-kinomi/log` |
| `output_dir` | `$RUNTIME_DIRECTORY/www` | `$XDG_RUNTIME_DIR/uptime-kinomi/www` (a tmpfs), else `$XDG_STATE_HOME/uptime-kinomi/www` |

With the system unit, these become `/var/lib/uptime-kinomi`,
`/var/log/uptime-kinomi` and `/run/uptime-kinomi/www`. The user unit only
sets `RuntimeDirectory=` (pages in `$XDG_RUNTIME_DIR/uptime-kinomi/www`, a
tmpfs); state and logs use the XDG defaults. It deliberately does **not** use
`StateDirectory=`: for user units, systemd ≥ 255 treats an existing
`~/.config/<name>` as old state and links `~/.local/state/<name>` to it,
which would be kinomi's configuration directory. kinomi creates missing directories with mode `0700`
(`0750` for `output_dir`, so a web server group can read the pages in mode B).

**Starting automatically without root:**
- `systemctl --user enable --now uptime-kinomi`. To keep it running while
  the user is logged out and to start it at boot, *lingering* must be enabled
  once (`loginctl enable-linger USER`, which may require an administrator, or
  is already enabled by the distribution).
- Alternative without systemd or lingering: a crontab entry
  `@reboot ~/.local/bin/uptime-kinomi run`.

**Root is discouraged:** if started with UID 0 (read from `/proc/self/status`,
no `unsafe` needed), kinomi logs a warning recommending a dedicated user.

**Features that will need privileges later** (not in v1) are optional and
documented with the least privilege needed, e.g. ICMP `ping` via the kernel
setting `net.ipv4.ping_group_range` instead of root.

---

## 11. Security summary
1. No `unsafe` in kinomi's code (`#![forbid(unsafe_code)]`).
2. Every byte from the network is size-limited before parsing; every parser
   of network data (HTTP response head, DNS, HTTP request) is fuzzed in CI.
3. Nothing fetched from monitored targets is stored, displayed or forwarded,
   only kinomi's own summary.
4. Push endpoint: constant-time token check, rate limit, body never read,
   extra data ignored.
5. No JavaScript, strict CSP, no external resources on pages.
6. Inbound TLS by the proxy; outbound TLS verifies certificates by default.
7. Secrets via files with permission warnings; bcrypt-hashed passwords.
8. Minimal, audited dependencies (`cargo-deny` for licenses/advisories in CI).
9. Runs as an unprivileged user, no capabilities required (§10.6).
10. Releases: checksums signed with offline minisign keys, verified by
    `self-update` before anything is replaced (§12.1, §12.2).

---

## 12. Build targets and releases

Static musl binaries, built in GitHub Actions with
[`cross`](https://github.com/cross-rs/cross), smoke-tested under QEMU, attached
to every GitHub release together with a signed checksum file (§12.1).
Asset names do not contain the version (`uptime-kinomi-armv6`), so
`https://github.com/uptime-kinomi/uptime-kinomi/releases/latest/download/<asset>`
always points to the latest release.

| Asset suffix | Rust target | For |
|---|---|---|
| `armv6` | `arm-unknown-linux-musleabihf` | Raspberry Pi 1, Zero, Zero W, CM1; also any 32-bit ARM |
| `armv7` | `armv7-unknown-linux-musleabihf` | Raspberry Pi 2/3/4/5 and Zero 2 W with 32-bit OS, Banana Pi M1/M2/M3, Orange Pi (32-bit), BeagleBone |
| `aarch64` | `aarch64-unknown-linux-musl` | Raspberry Pi 3/4/5, Zero 2 W with 64-bit OS, Banana Pi M5/M7, Orange Pi 5, Rock Pi, Odroid |
| `x86_64` | `x86_64-unknown-linux-musl` | PCs, NUCs, VMs, servers |
| `riscv64` | `riscv64gc-unknown-linux-musl` | VisionFive 2, Banana Pi BPI-F3 (best effort) |

`.deb` packages for `armhf` (built for **ARMv6**, so they also work on
Raspberry Pi OS on the Pi 1), `arm64` and `amd64` are a later goal.

**Early technical risk:** outbound TLS (needed for HTTPS monitors,
notifications and `self-update`) on ARMv6 with `rustls`. `self-update` (§12.2)
is the first HTTPS client in kinomi and thereby the TLS proof of concept of
milestone M0. Fallback: dynamic OpenSSL for that target.

### 12.1 Release signing

Checksums alone only protect against corrupted downloads. Whoever can replace
a binary on the release page can also replace the checksum file. Therefore
the checksum file is **signed with a key that is not stored on GitHub**, and
the matching public keys are compiled into kinomi.

- **Format:** [minisign](https://jedisct1.github.io/minisign/) (Ed25519,
  compatible with OpenBSD signify). Verification in kinomi uses the small,
  dependency-free `minisign-verify` crate.
- **Assets of every release:**

  | File | Content |
  |---|---|
  | `uptime-kinomi-<suffix>` | static binaries (§12, table above) |
  | `SHA256SUMS` | `<64 hex digits>  <file name>` per binary, sorted |
  | `SHA256SUMS.minisig` | minisign signature of `SHA256SUMS` |

- **Trusted comment:** the signature's (signed) trusted comment is exactly
  `uptime-kinomi X.Y.Z`. This binds the checksums to a version and prevents
  rollback attacks (serving an old, validly signed release with known bugs).
- **Keys:** two key pairs, a primary and a backup, both kept **offline** by the
  maintainer (password-protected, backed up off-site). Both public keys are
  embedded in the binary (from `keys/`); a signature by either is accepted.
  Rotation: a release signed with an old key ships new public keys.

  | Key | ID | Public key |
  |---|---|---|
  | primary | `16F640342DF96A16` | `RWQWavktNED2FrnDC8yLcpUPBKv4JeUoguh4zMF2wCBxwRJKVs3vRZu3` |
  | backup | `6AD1615F6A7966B2` | `RWSyZnlqX2HRasxyKYKZQUmcQYpm5e9zHsDUFWTgvWDerhQj+DccOfUH` |
- **Additionally**, CI attaches GitHub
  [artifact attestations](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations)
  (Sigstore, SLSA build provenance). They prove that a binary was built by the
  workflow from a given commit and can be checked by hand with
  `gh attestation verify uptime-kinomi-armv6 --repo uptime-kinomi/uptime-kinomi`.
  kinomi itself does not verify them (too heavy for a small binary).
- **Release process:**
  1. Pushing a tag `vX.Y.Z` triggers the release workflow: build all targets,
     run the QEMU smoke tests, create `SHA256SUMS` and attestations, and
     upload everything to a **pre-release**. Its files are publicly
     downloadable, but `self-update` ignores it: a pre-release is never
     "latest", and without `SHA256SUMS.minisig` it is refused even with
     `--release`.
  2. The maintainer runs `contrib/release/sign-release.sh vX.Y.Z` on their
     own machine. It needs only `sh`, `curl` and `minisign`. It downloads the
     assets, verifies the checksums (and the attestations if `gh` is
     installed), signs `SHA256SUMS` with `minisign -S -t "uptime-kinomi X.Y.Z"`
     and checks the signature against `keys/`.
  3. The signature is uploaded and the release is marked as "latest": by the
     script with a fine-grained `GITHUB_TOKEN` (this repository only,
     "Contents: read and write"), or by hand in the browser.

  So a compromised GitHub account or CI can publish a release, but not one
  that any installed kinomi accepts.
- **Manual verification** (documented in the README):
  `minisign -Vm SHA256SUMS -P <public key from the table above>` and then
  `sha256sum --ignore-missing -c SHA256SUMS`.

### 12.2 `self-update`

`uptime-kinomi self-update` replaces the running binary with the latest
signed release for the same platform. It never runs automatically; the
administrator (or an Ansible task, or a cron job) decides when to update.

1. **Preconditions:** the binary's own directory must be writable by the
   current user (e.g. `~/.local/bin`). kinomi refuses to update binaries in
   `/usr/bin`, `/bin` or `/usr/sbin` (those belong to a package manager) and
   never asks for or uses root.
2. **Fetch metadata:** download `SHA256SUMS` (max 16 KiB) and
   `SHA256SUMS.minisig` (max 1 KiB) from
   `releases/latest/download/` (or `releases/download/vX.Y.Z/` with
   `--release`). HTTPS only, certificate verification always on, at most 5
   redirects, all of them HTTPS, timeouts on every step.
3. **Verify:** the signature must be valid for one of the embedded public
   keys. The trusted comment must be exactly `uptime-kinomi X.Y.Z` and
   `X.Y.Z` must be newer than the running version (an older or equal version
   needs `--release X.Y.Z --allow-downgrade`). `SHA256SUMS` is parsed strictly
   (§12.1 format, only known asset names).
4. **Download** the asset for the platform the binary was compiled for (max
   32 MiB) into a temporary file in the same directory, computing SHA-256
   while streaming. A mismatch deletes the file and aborts.
5. **Test run:** the new file is made executable (`0755`) and run as
   `<new> version` with a 10 s timeout. The output must start with
   `uptime-kinomi X.Y.Z (`. This catches binaries that cannot run on this
   CPU (e.g. an ARMv7 build on a Pi 1).
6. **Replace atomically:** the current binary is renamed to
   `uptime-kinomi.old`, the new one is renamed into place. A running service
   keeps using the old program until it is restarted.
7. **Tell the user** to restart the service (e.g.
   `systemctl --user restart uptime-kinomi`); kinomi does not restart services
   itself.

Options: `--check` only reports whether a newer signed release exists (exit
code 3 if so, 0 if up to date). `--rollback` swaps `uptime-kinomi.old` back
into place.

---

## 13. Roadmap

| Milestone | Content |
|---|---|
| ✅ **M0** | Repo, CI, cross builds for all targets with QEMU smoke tests, release workflow with signed checksums (§12.1), `self-update` (§12.2) as TLS proof of concept on ARMv6 (QEMU + real Pi 1), logo |
| ✅ **M1** | Config loading + `check-config`, scheduler/queue, `http` and `tcp` monitors, check log |
| ✅ **M2** | State files, replay, page rendering (public/private), built-in server, Basic Auth |
| ✅ **M3** | `push` and `dns` monitors, notifiers (webhook, ntfy, gotify, telegram) |
| ✅ **M4** | Incidents, maintenance, badges, Atom feed, status.json, `/metrics`, contrib files |
| **v1.0** | Hardening, fuzzing, documentation, first stable release |
| later | `ping`, `smtp` notifier, `.deb` packages, uptime-kuma config importer |

---

## 14. Incidents and maintenance

### 14.1 Incidents

Human-written announcements, one file per incident.

```toml
# incidents/2026-10-22-router.toml
config_version = 1
title = "Router firmware update"
severity = "minor"            # info | minor | major
start = "2026-10-22T18:00:00+02:00"   # RFC 3339 with offset (or Z)
end = "2026-10-22T19:00:00+02:00"     # optional; open incidents omit it
pages = ["home"]              # optional; default: all pages with show_incidents
text = """
The router is being updated. Internet will be unavailable for a few minutes.
"""   # plain text, max 4 KiB; paragraphs separated by blank lines, no Markdown/HTML
```

- Shown on the pages above the monitors: **ongoing** (started, not ended),
  **scheduled** (start in the future) and **resolved** (ended within the
  last 7 days). Older incidents stay in the feed for 30 days (§8.7).
- Incidents are pure information: they do not change monitor states.

### 14.2 Maintenance windows

```toml
# maintenance/weekly-backup.toml
config_version = 1
title = "Weekly NAS backup"
monitors = ["nas", "nextcloud"]   # monitor IDs, or ["*"]
tags = ["backup"]                 # optional: also all monitors with these tags
schedule = "weekly"               # once | daily | weekly
start = "2026-10-25T03:00:00"     # local time in pages.timezone (no offset)
duration = "2h"                   # 1m to 7d; daily at most 1d
```

- `daily` repeats at the time of `start` every day from the date of `start`,
  `weekly` on the weekday of `start`. Windows across midnight work.
- During a window the affected monitors show `MAINTENANCE`:
  - checks still run and are logged, but as `result=MAINT`, so the log shows
    what happened;
  - they count neither as up nor as down (`maint` in the totals, §7.2), and
    the heartbeat bar shows them separately;
  - no notifications are sent, and the monitor's state machine (retries,
    DOWN) is left untouched: after the window, the next check continues
    from the state before it;
  - a push monitor whose push is overdue does not fail during the window.
- Pages list the windows of their monitors that start within the next 7
  days.

---

## 15. Branding
- Name: **kinomi** (木の実, Japanese for "tree fruit / nuts / berries"), a nod to
  uptime-kuma (熊, "bear") and to fruit-named SBCs.
- Logo: a raspberry shaped like a banana ("Raspberry Pi" + "Banana Pi" =
  "Panana Pi"). SVG, simple, works at 16 px (favicon) and in monochrome.
