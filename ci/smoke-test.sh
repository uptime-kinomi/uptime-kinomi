#!/bin/sh
# Smoke test for a release binary, optionally run through an emulator.
#
# Usage: [KINOMI_SMOKE_NETWORK=1] ci/smoke-test.sh BINARY [EMULATOR ARGS...]
# Example: ci/smoke-test.sh target/arm-unknown-linux-musleabihf/release/uptime-kinomi qemu-arm -cpu arm1176

set -eu

bin=$1
shift
max_size=$((6 * 1024 * 1024)) # SPEC.md §1, resource targets

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

expected_version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)

# 1. Static binary within the size budget.
file "$bin"
file "$bin" | grep -Eq "statically linked|static-pie linked" || fail "not statically linked"
size=$(stat -c %s "$bin")
echo "size: $size bytes"
[ "$size" -le "$max_size" ] || fail "binary larger than $max_size bytes"

# 2. It runs and reports the right version.
version=$("$@" "$bin" version)
echo "version: $version"
case "$version" in
    "uptime-kinomi $expected_version ("*) ;;
    *) fail "unexpected version output" ;;
esac

# 3. Exit codes (SPEC.md §10.4).
expect_exit() {
    expected=$1
    shift
    set +e
    "$@" >/dev/null 2>&1
    actual=$?
    set -e
    [ "$actual" -eq "$expected" ] || fail "'$*' exited with $actual, expected $expected"
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
config_dir=$work/config
mkdir -p "$config_dir/monitors"

expect_exit 2 "$@" "$bin" check-config --config-dir "$config_dir" # no kinomi.toml
touch "$config_dir/kinomi.toml"
expect_exit 2 "$@" "$bin" check-config --config-dir "$config_dir" # config_version missing
cat >"$config_dir/kinomi.toml" <<EOF
config_version = 1
[server]
hostnames = ["localhost"]
[storage]
data_dir = "$work/data"
log_dir = "$work/log"
flush_interval = "1s"
[pages]
output_dir = "$work/www"
EOF
cat >"$config_dir/monitors/closed-port.toml" <<EOF
config_version = 1
type = "tcp"
name = "Closed port"
interval = "10s"
timeout = "2s"
retries = 0
[tcp]
host = "127.0.0.1"
port = 1
EOF
expect_exit 0 "$@" "$bin" check-config --config-dir "$config_dir"
expect_exit 2 "$@" "$bin" no-such-command
expect_exit 0 "$@" "$bin" --help

# 4. run: one check, then SIGTERM must flush the log and exit cleanly.
"$@" "$bin" run --config-dir "$config_dir" 2>"$work/stderr" &
pid=$!
sleep 4
kill -TERM "$pid"
set +e
wait "$pid"
status=$?
set -e
cat "$work/stderr"
[ "$status" -eq 0 ] || fail "run exited with $status after SIGTERM"
log=$(cat "$work"/log/*.log)
echo "$log"
echo "$log" | grep -q 'event=START' || fail "no START in the log"
echo "$log" | grep -q 'monitor=closed-port result=DOWN' || fail "no check result in the log"
echo "$log" | grep -q 'event=STOP' || fail "no STOP in the log"

# 5. Real TLS handshake with github.com (only with KINOMI_SMOKE_NETWORK=1).
#    Under QEMU this runs ring/rustls on the emulated CPU, e.g. a Pi 1.
if [ "${KINOMI_SMOKE_NETWORK:-0}" = 1 ]; then
    set +e
    output=$("$@" "$bin" self-update --check 2>&1)
    status=$?
    set -e
    echo "self-update --check: exit $status: $output"
    case "$status" in
        0 | 3) ;;
        1) echo "$output" | grep -q "no published release found" ||
            fail "self-update --check failed" ;;
        *) fail "self-update --check crashed (exit $status)" ;;
    esac
fi

echo "smoke test passed"
