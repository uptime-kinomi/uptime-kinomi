<p align="center"><img src="docs/logo.svg" width="160" alt="uptime-kinomi logo: a raspberry shaped like a banana"></p>

# uptime-kinomi

**The SBC-friendly cousin of uptime-kuma.** A self-hosted uptime monitor and
static status page generator written in Rust, built to run on a
**Raspberry Pi 1**, configured with plain **TOML files**, and producing
**JavaScript-free HTML + CSS status pages**.

> ⚠️ **Status: design phase.** Nothing is implemented yet. See
> [SPEC.md](SPEC.md) for the full specification and roadmap. This README
> describes the planned v1.0.

---

## What can I use my Raspberry Pi 1 for?

That old Pi 1 (or Pi Zero) in your drawer is enough to:

- 🌐 **Watch your websites and home server** (HTTP/HTTPS status codes, keywords, response time).
- 🔒 **Warn you before your Let's Encrypt certificate expires.**
- 💾 **Check that your nightly backup actually ran:** the backup script calls
  a push URL, and kinomi alerts you if it doesn't.
- 📡 **Tell you whether "the internet is down" or just your Wi-Fi:** router, DNS,
  and upstream checks on a public status page the whole family can open.
- 🔔 **Send alerts** to ntfy, Gotify, Telegram, Discord, Slack, Mattermost, or any webhook.
- 👀 **Watch another Pi, which watches it back.** Two cheap boards keep an eye on each other.

It uses a few MB of RAM, a few percent of a single 700 MHz core, and is gentle
on SD cards.

## Why not uptime-kuma?

[uptime-kuma](https://github.com/louislam/uptime-kuma) is great, but:

| | uptime-kuma | uptime-kinomi |
|---|---|---|
| Runs on Raspberry Pi 1 / Zero (ARMv6) | ❌ | ✅ first-class target |
| Runtime | Node.js + SQLite/MariaDB | single static binary |
| Memory | ~100+ MB | target < 10 MB |
| Configuration | point-and-click web UI | **TOML files** (one per monitor), Ansible/Git friendly |
| Status pages | JavaScript single-page app | **static HTML5 + CSS, no JavaScript** |
| Private overview | login to web UI | private status pages behind HTTP Basic Auth |
| Data | database | **human-readable log files** + human-readable state files |
| Attack surface | full web application | tiny read-only server (or none at all) |

If you want a configuration UI, use uptime-kuma. If you want config-as-code on
minimal hardware, use kinomi.

## Features (planned v1.0)

- **Monitors:** HTTP(S) (status code, keyword, TLS certificate expiry, slow
  response), TCP port (optional banner), DNS records, push/heartbeat
  (compatible with uptime-kuma push URLs).
- **Notifications:** generic webhook (JSON, Discord, Slack/Mattermost format),
  ntfy, Gotify, Telegram. Down, recovery, warning and reminder events.
- **Status pages:** as many as you like, each **public or private** (HTTP Basic
  Auth). Heartbeat bar, 24 h and 30 d uptime, response time, certificate
  expiry, incidents, maintenance windows. Dark mode, auto-refresh via `<meta>`,
  works in text browsers. Custom CSS supported.
- **Extras:** SVG status badges, Atom feed per page, `status.json`, optional
  Prometheus `/metrics`.
- **Config as code:** one `.toml` file per monitor, notifier, page, incident
  and maintenance window. `uptime-kinomi check-config` validates everything,
  and you can plug it into Ansible's `validate:`.
- **Built for weak hardware:** one binary, thread-based check queue with fair
  skipping under overload, low-priority background page rendering, buffered
  log writes, works without a real-time clock.
- **Runs without root:** install, configure and run everything as your
  normal user (`~/.config`, `~/.local/state`, `systemctl --user`). System-wide
  installation with a dedicated service user is supported too.
- **Signed self-update:** `uptime-kinomi self-update` installs the latest
  release after checking its minisign signature and checksum.
- **Security first:** no `unsafe` code, strict input limits, nothing fetched
  from targets is ever shown or forwarded, strict CSP, no inbound TLS (left to
  your reverse proxy), hardened systemd unit.

## Download: which binary do I need?

Binaries are static (musl), so they don't depend on your distribution's libc.
Run `uname -m` if unsure.

| Your board / system | `uname -m` | Binary |
|---|---|---|
| Raspberry Pi 1 (A, B, A+, B+), Pi Zero, Pi Zero W, Compute Module 1 | `armv6l` | `uptime-kinomi-armv6` |
| Raspberry Pi 2, 3, 4, 5, Zero 2 W with a **32-bit** OS | `armv7l` | `uptime-kinomi-armv7` (`armv6` also works) |
| Banana Pi M1/M2/M3, Orange Pi PC/One/Zero, BeagleBone, other 32-bit ARMv7 | `armv7l` | `uptime-kinomi-armv7` |
| Raspberry Pi 3, 4, 5, Zero 2 W with a **64-bit** OS | `aarch64` | `uptime-kinomi-aarch64` |
| Banana Pi M5/M7, Orange Pi 5, Rock Pi, Odroid, other 64-bit ARM | `aarch64` | `uptime-kinomi-aarch64` |
| PC, NUC, VM, server | `x86_64` | `uptime-kinomi-x86_64` |
| VisionFive 2, Banana Pi BPI-F3, other RISC-V (best effort) | `riscv64` | `uptime-kinomi-riscv64` |

Rule of thumb: when in doubt on 32-bit ARM, take `armv6`. It runs everywhere,
just a bit slower.

## Quick start (planned), no root needed

kinomi runs as your normal user. Everything below works without `sudo`.

```sh
# 1. install the binary into your home directory
mkdir -p ~/.local/bin
install -m 755 uptime-kinomi-armv6 ~/.local/bin/uptime-kinomi

# 2. create a configuration
mkdir -p ~/.config/uptime-kinomi/{monitors,notifications,pages,secrets}
cd ~/.config/uptime-kinomi
```

`~/.config/uptime-kinomi/kinomi.toml`:
```toml
config_version = 1

[server]
listen = "127.0.0.1:8080"
hostnames = ["status.example.org"]
base_url = "https://status.example.org"
htpasswd_file = "htpasswd"          # relative paths are relative to this directory

[pages]
timezone = "Europe/Berlin"
default_page = "home"
```

`monitors/blog.toml`:
```toml
config_version = 1
type = "http"
name = "My blog"
interval = "60s"
notifications = ["phone"]

[http]
url = "https://blog.example.org/"
keyword = "Welcome"
tls_expiry_warn = "14d"
```

`monitors/backup.toml` (your backup script runs
`curl -fsS https://status.example.org/push/<token>` at the end):
```toml
config_version = 1
type = "push"
name = "Nightly backup"
interval = "1d"
grace = "2h"
notifications = ["phone"]

[push]
token_file = "secrets/backup.token"   # create with: uptime-kinomi gen-token > secrets/backup.token
```

`notifications/phone.toml`:
```toml
config_version = 1
type = "ntfy"

[ntfy]
url = "https://ntfy.sh"
topic = "my-secret-topic-name"
```

`pages/home.toml` (public) and `pages/all.toml` (private):
```toml
config_version = 1
title = "Our services"
public = true

[[groups]]
name = "Web"
monitors = ["blog"]
```
```toml
config_version = 1
title = "Everything"
public = false

[[groups]]
name = "All monitors"
monitors = ["*"]
```

```sh
# 3. a password for private pages (prints a random password once; same file format as Apache/nginx htpasswd)
uptime-kinomi hash-password admin >> htpasswd
chmod 600 htpasswd secrets/*

# 4. validate and start as a systemd user service (unit file in contrib/systemd/user/)
uptime-kinomi check-config
mkdir -p ~/.config/systemd/user
curl -fsSLo ~/.config/systemd/user/uptime-kinomi.service \
  https://raw.githubusercontent.com/uptime-kinomi/uptime-kinomi/main/contrib/systemd/user/uptime-kinomi.service
systemctl --user enable --now uptime-kinomi
```

State, logs and pages end up in `~/.local/state/uptime-kinomi/`.

To keep the service running after you log out and start it at boot, *lingering*
must be enabled once for your user (`loginctl enable-linger`; on some systems an
administrator has to do this). Without systemd, a crontab line does the job:

```
@reboot ~/.local/bin/uptime-kinomi run
```

**System-wide installation** works the same way, with a dedicated `kinomi`
user (never root), config in `/etc/uptime-kinomi/` and the hardened system unit
from `contrib/systemd/system/`. Data then goes to `/var/lib/uptime-kinomi`,
`/var/log/uptime-kinomi` and `/run/uptime-kinomi`. See [SPEC.md](SPEC.md) §10.6.

Everything is explained in [SPEC.md](SPEC.md).

## Updating and verifying downloads

```sh
uptime-kinomi self-update --check     # exit code 3 if a newer release exists
uptime-kinomi self-update             # download, verify, replace; then restart the service
systemctl --user restart uptime-kinomi
```

`self-update` only accepts releases whose checksum file is signed with the
project's offline [minisign](https://jedisct1.github.io/minisign/) keys
(embedded in the binary), tests the new binary on your CPU before replacing
anything, and keeps the previous version (`self-update --rollback`). It works
without root as long as the binary lives in a directory you own, such as
`~/.local/bin`.

To verify a manual download (primary key `16F640342DF96A16`; the backup key
is listed in [SPEC.md](SPEC.md) §12.1 and in [keys/](keys/)):

```sh
minisign -Vm SHA256SUMS -P RWQWavktNED2FrnDC8yLcpUPBKv4JeUoguh4zMF2wCBxwRJKVs3vRZu3
sha256sum --ignore-missing -c SHA256SUMS
gh attestation verify uptime-kinomi-armv6 --repo uptime-kinomi/uptime-kinomi   # optional: build provenance
```

## Running behind a reverse proxy

kinomi does not do TLS itself. Put nginx, Apache or Caddy in front and get a
certificate with certbot / Let's Encrypt. Two supported setups:

- **Mode A (default):** the proxy forwards everything to `127.0.0.1:8080`.
- **Mode B (don't trust a custom HTTP server?):** the proxy serves the generated
  files directly from disk and does Basic Auth itself with the same htpasswd
  file. Only `/push/` is forwarded to kinomi, or nothing at all if you have no push
  monitors.

Example nginx and Apache configs for both modes are in `contrib/`.

## Logs you can read

```
2026-10-22T14:03:11Z monitor=blog result=UP latency_ms=123 msg="HTTP 200"
2026-10-22T14:05:11Z monitor=blog result=DOWN latency_ms=10000 msg="timeout"
2026-10-22T14:05:11Z event=NOTIFY monitor=blog notifier=phone kind=down ok=true
```

One file per day, UTC timestamps first, `grep`- and `awk`-friendly.

## Deploying with Ansible

Because every monitor is a file, a deployment is a `template:` loop plus a
handler that restarts the service. `uptime-kinomi check-config` can be used as
the `validate:` command. An example role is planned in `contrib/ansible/`.

## Contributing

kinomi is designed to be extended: **one source file per monitor type and per
notification type**. See [AGENTS.md](AGENTS.md) for architecture, code rules
and how to add a new monitor. This applies to humans and AI coding agents alike.

## License

MIT

---

*Keywords: uptime monitor, status page, Raspberry Pi 1, Raspberry Pi Zero,
ARMv6, single-board computer, SBC, Banana Pi, Orange Pi, uptime-kuma
alternative, self-hosted, Rust, static status page, no JavaScript, TOML
configuration, config as code, Ansible, heartbeat monitoring, push monitoring,
cron job monitoring, SSL certificate expiry monitoring, ntfy, Gotify, Telegram,
homelab.*
