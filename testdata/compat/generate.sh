#!/bin/sh
# Generate the data directory of an old release for the upgrade test
# (tests/compat.rs): run the release for about a minute with a few
# monitors, then keep its log and state files in testdata/compat/<version>/.
#
# Usage: testdata/compat/generate.sh <path to an uptime-kinomi binary>
#
# Download and verify the binary first, e.g. for 0.6.0:
#   base=https://github.com/uptime-kinomi/uptime-kinomi/releases/download/v0.6.0
#   curl -fsSLO $base/uptime-kinomi-x86_64 -O $base/SHA256SUMS -O $base/SHA256SUMS.minisig
#   minisign -Vm SHA256SUMS -p keys/uptime-kinomi-primary.pub && sha256sum --ignore-missing -c SHA256SUMS
#
# Needs python3 (a local web server) and curl. No internet access.
set -eu

bin=$(realpath "$1")
version=$("$bin" version | sed -n 's/^uptime-kinomi \([0-9.]*\) .*/\1/p')
[ -n "$version" ] || { echo "cannot read the version of $bin" >&2; exit 1; }
out=$(cd "$(dirname "$0")" && pwd)/$version
work=$(mktemp -d)
trap 'kill $web $kinomi 2>/dev/null || true; rm -rf "$work"' EXIT

# A web server for the http and tcp monitors.
mkdir -p "$work/www"
echo "Welcome" > "$work/www/index.html"
web_port=18081
python3 -m http.server -b 127.0.0.1 -d "$work/www" $web_port >/dev/null 2>&1 &
web=$!
kinomi_port=18082
token=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa

cfg=$work/config
mkdir -p "$cfg/monitors" "$cfg/pages"
cat > "$cfg/kinomi.toml" <<EOF
config_version = 1
[server]
listen = "127.0.0.1:$kinomi_port"
hostnames = ["127.0.0.1"]
[pages]
output_dir = "$work/www-out"
[storage]
data_dir = "$work/data"
log_dir = "$work/log"
flush_interval = "1s"
[defaults]
interval = "10s"
timeout = "5s"
retries = 0
EOF
cat > "$cfg/pages/home.toml" <<EOF
config_version = 1
title = "Home"
public = true
[[groups]]
name = "All"
monitors = ["*"]
EOF
cat > "$cfg/monitors/web.toml" <<EOF
config_version = 1
type = "http"
name = "Web"
[http]
url = "http://127.0.0.1:$web_port/"
keyword = "Welcome"
EOF
cat > "$cfg/monitors/port.toml" <<EOF
config_version = 1
type = "tcp"
name = "Open port"
[tcp]
host = "127.0.0.1"
port = $web_port
EOF
# Nothing listens on port 1: DOWN.
cat > "$cfg/monitors/closed.toml" <<EOF
config_version = 1
type = "tcp"
name = "Closed port"
[tcp]
host = "127.0.0.1"
port = 1
EOF
cat > "$cfg/monitors/old.toml" <<EOF
config_version = 1
type = "tcp"
name = "Disabled"
enabled = false
[tcp]
host = "127.0.0.1"
port = 1
EOF
# Push monitors exist from 0.3.0 on.
if [ "$version" != 0.2.0 ]; then
  cat > "$cfg/monitors/job.toml" <<EOF
config_version = 1
type = "push"
name = "Job"
interval = "1h"
[push]
token = "$token"
EOF
fi
chmod -R go-rwx "$cfg"
"$bin" check-config --config-dir "$cfg" >/dev/null

"$bin" run --config-dir "$cfg" &
kinomi=$!
sleep 15
if [ -f "$cfg/monitors/job.toml" ]; then
  curl -fsS "http://127.0.0.1:$kinomi_port/push/$token"
  sleep 2
  curl -fsS "http://127.0.0.1:$kinomi_port/push/$token?status=down"
fi
sleep 45
kill -TERM $kinomi
wait $kinomi || true

rm -rf "$out"
mkdir -p "$out"
cp -r "$work/log" "$out/log"
cp -r "$work/data/state" "$out/state"
echo "$out:"
ls -R "$out"
