#!/bin/sh
# Install uptime-kinomi for the current user, without root.
#
#   curl -fsSL https://raw.githubusercontent.com/uptime-kinomi/uptime-kinomi/main/contrib/install.sh | sh
#
# Options (after "sh -s --" when piping):
#   --version X.Y.Z   install this release instead of the latest
#   --listen ADDR     address of the built-in server in a new configuration
#                     (default 127.0.0.1:8080; 0.0.0.0:8080 for the whole LAN)
#   --no-service      do not install or start the systemd user service
#   --uninstall       stop and remove binary and service (keeps config and data)
#
# What it does: downloads the release for this CPU, verifies the minisign
# signature of the checksums against the project's public keys (embedded
# below) and the checksum of the binary, installs it to ~/.local/bin, creates
# a starter configuration in ~/.config/uptime-kinomi (never overwrites one),
# and installs and starts a systemd user service.
#
# Prefer reading before running? Download it, read it, then run: sh install.sh
#
# Everything is wrapped in main(), so a partially downloaded script does
# nothing.

set -eu

REPO="uptime-kinomi/uptime-kinomi"
# The release public keys (keys/ in the repository, SPEC.md §12.1).
KEY_PRIMARY="RWQWavktNED2FrnDC8yLcpUPBKv4JeUoguh4zMF2wCBxwRJKVs3vRZu3"
KEY_BACKUP="RWSyZnlqX2HRasxyKYKZQUmcQYpm5e9zHsDUFWTgvWDerhQj+DccOfUH"

say() { printf '%s\n' "$*"; }
step() { printf '==> %s\n' "$*"; }
die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }

fetch() {
    # HTTPS only, fail on HTTP errors, bounded size and time.
    if have curl; then
        curl --proto '=https' --tlsv1.2 -fsSL --max-filesize 33554432 --max-time 300 -o "$2" "$1"
    elif have wget; then
        wget -q --https-only --timeout=60 -O "$2" "$1"
    else
        die "neither curl nor wget is installed"
    fi
}

b64decode() { base64 -d 2>/dev/null || openssl base64 -d -A; }

detect_asset() {
    case "$(uname -s)" in
        Linux) ;;
        *) die "only Linux is supported (found $(uname -s))" ;;
    esac
    case "$(uname -m)" in
        armv6l | armv6*) echo armv6 ;;
        armv7l | armv7* | armv8l) echo armv7 ;;
        aarch64 | arm64) echo aarch64 ;;
        x86_64 | amd64) echo x86_64 ;;
        riscv64) echo riscv64 ;;
        *) die "no release binary for this CPU ($(uname -m)); see the README for building from source" ;;
    esac
}

# Verify a minisign signature (prehashed Ed25519, "ED") of $1 with the
# signature file $2 using openssl 3. Prints the trusted comment on success.
verify_openssl() {
    file=$1
    sigfile=$2
    w=$(mktemp -d)
    sed -n 2p "$sigfile" | b64decode >"$w/sig" 2>/dev/null || {
        rm -rf "$w"
        return 1
    }
    sed -n 4p "$sigfile" | b64decode >"$w/global" 2>/dev/null || {
        rm -rf "$w"
        return 1
    }
    trusted=$(sed -n 3p "$sigfile")
    case "$trusted" in
        "trusted comment: "*) comment=${trusted#trusted comment: } ;;
        *)
            rm -rf "$w"
            return 1
            ;;
    esac
    if [ "$(head -c 2 "$w/sig")" != "ED" ] || [ "$(wc -c <"$w/sig" | tr -d ' ')" != 74 ]; then
        rm -rf "$w"
        return 1
    fi
    tail -c 64 "$w/sig" >"$w/sig.bin"
    head -c 10 "$w/sig" | tail -c 8 >"$w/sig.keyid"
    openssl dgst -blake2b512 -binary "$file" >"$w/hash"
    printf '%s' "$comment" >"$w/comment"
    cat "$w/sig.bin" "$w/comment" >"$w/global.msg"
    for key in "$KEY_PRIMARY" "$KEY_BACKUP"; do
        printf '%s' "$key" | b64decode >"$w/pk"
        head -c 10 "$w/pk" | tail -c 8 >"$w/pk.keyid"
        cmp -s "$w/pk.keyid" "$w/sig.keyid" || continue
        # Ed25519 SubjectPublicKeyInfo prefix + the 32 key bytes.
        printf '\060\052\060\005\006\003\053\145\160\003\041\000' >"$w/pk.der"
        tail -c 32 "$w/pk" >>"$w/pk.der"
        openssl pkey -pubin -inform DER -in "$w/pk.der" -out "$w/pk.pem" 2>/dev/null || continue
        if openssl pkeyutl -verify -pubin -inkey "$w/pk.pem" -rawin -in "$w/hash" -sigfile "$w/sig.bin" >/dev/null 2>&1 &&
            openssl pkeyutl -verify -pubin -inkey "$w/pk.pem" -rawin -in "$w/global.msg" -sigfile "$w/global" >/dev/null 2>&1; then
            rm -rf "$w"
            printf '%s\n' "$comment"
            return 0
        fi
    done
    rm -rf "$w"
    return 1
}

verify_minisign() {
    for key in "$KEY_PRIMARY" "$KEY_BACKUP"; do
        if comment=$(minisign -V -m "$1" -x "$2" -P "$key" -Q 2>/dev/null); then
            printf '%s\n' "$comment"
            return 0
        fi
    done
    return 1
}

openssl_can_verify() {
    have openssl || return 1
    openssl version 2>/dev/null | grep -q '^OpenSSL [3-9]' || return 1
    openssl dgst -blake2b512 -binary /dev/null >/dev/null 2>&1
}

# Print the signed version of SHA256SUMS, or fail.
verify_signature() {
    if have minisign; then
        comment=$(verify_minisign "$1" "$2") || die "the signature of SHA256SUMS is INVALID; not installing"
    elif openssl_can_verify; then
        comment=$(verify_openssl "$1" "$2") || die "the signature of SHA256SUMS is INVALID; not installing"
    elif [ "${KINOMI_ALLOW_UNSIGNED:-}" = 1 ]; then
        say "warning: no minisign and no OpenSSL 3: the signature is NOT checked (KINOMI_ALLOW_UNSIGNED=1)" >&2
        comment="uptime-kinomi unverified"
    else
        die "cannot check the release signature: install minisign (or OpenSSL 3), or set KINOMI_ALLOW_UNSIGNED=1 to rely on HTTPS and checksums only"
    fi
    case "$comment" in
        "uptime-kinomi "*) printf '%s\n' "${comment#uptime-kinomi }" ;;
        *) die "unexpected trusted comment in the signature: $comment" ;;
    esac
}

sha256_of() {
    if have sha256sum; then
        sha256sum "$1" | cut -d' ' -f1
    elif have shasum; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        openssl dgst -sha256 -r "$1" | cut -d' ' -f1
    fi
}

detect_timezone() {
    tz=""
    if have timedatectl; then
        tz=$(timedatectl show -p Timezone --value 2>/dev/null || true)
    fi
    if [ -z "$tz" ] && [ -r /etc/timezone ]; then
        tz=$(head -n 1 /etc/timezone)
    fi
    if [ -z "$tz" ] && [ -L /etc/localtime ]; then
        tz=$(readlink /etc/localtime | sed -n 's|.*/zoneinfo/||p')
    fi
    case "$tz" in
        "" | *[!A-Za-z0-9_/+-]*) echo UTC ;;
        *) echo "$tz" ;;
    esac
}

# Allowed Host headers for the built-in server: local names and addresses.
host_names() {
    names="\"localhost\", \"127.0.0.1\""
    h=$(hostname 2>/dev/null || true)
    case "$h" in
        "" | *[!A-Za-z0-9.-]*) ;;
        *) names="$names, \"$h\", \"$h.local\"" ;;
    esac
    if hostname -I >/dev/null 2>&1; then
        for ip in $(hostname -I); do
            case "$ip" in
                *:*) ;; # IPv6 addresses need brackets in URLs; skipped
                *[!0-9.]*) ;;
                *) names="$names, \"$ip\"" ;;
            esac
        done
    fi
    printf '%s' "$names"
}

write_starter_config() {
    dir=$1
    listen=$2
    mkdir -p "$dir/monitors" "$dir/pages" "$dir/notifications"
    chmod 700 "$dir"
    cat >"$dir/kinomi.toml" <<EOF
# uptime-kinomi configuration (created by install.sh). Documentation:
# https://github.com/uptime-kinomi/uptime-kinomi/blob/main/SPEC.md
# After changes: uptime-kinomi check-config && systemctl --user restart uptime-kinomi
config_version = 1

[server]
listen = "$listen"
hostnames = [$(host_names)]

[pages]
timezone = "$(detect_timezone)"
default_page = "home"
EOF
    cat >"$dir/monitors/example-web.toml" <<'EOF'
# An example HTTP monitor. Replace it with your own (one file per monitor).
config_version = 1
type = "http"
name = "example.org"

[http]
url = "https://example.org/"
EOF
    cat >"$dir/monitors/internet-dns.toml" <<'EOF'
# Is name resolution working? (asks the public resolver 9.9.9.9)
config_version = 1
type = "dns"
name = "Internet (DNS)"
interval = "5m"

[dns]
name = "example.org"
server = "9.9.9.9"
EOF
    cat >"$dir/pages/home.toml" <<'EOF'
config_version = 1
title = "Status"
public = true

[[groups]]
name = "All"
monitors = ["*"]
EOF
    cat >"$dir/README.txt" <<'EOF'
One file per monitor in monitors/, per notifier in notifications/, per
status page in pages/. Examples of every file type:
https://github.com/uptime-kinomi/uptime-kinomi/tree/main/contrib/examples/config
EOF
}

# Keep in sync with contrib/systemd/user/uptime-kinomi.service.
write_unit() {
    cat >"$1" <<'EOF'
[Unit]
Description=uptime-kinomi uptime monitor
Documentation=https://github.com/uptime-kinomi/uptime-kinomi

[Service]
ExecStart=%h/.local/bin/uptime-kinomi run
Restart=on-failure
RestartSec=10
Nice=10
# No StateDirectory=: see contrib/systemd/user/uptime-kinomi.service.
RuntimeDirectory=uptime-kinomi
NoNewPrivileges=yes
UMask=0027

[Install]
WantedBy=default.target
EOF
}

user_systemd_works() {
    have systemctl && systemctl --user show-environment >/dev/null 2>&1
}

uninstall() {
    if user_systemd_works; then
        systemctl --user disable --now uptime-kinomi >/dev/null 2>&1 || true
        rm -f "$UNIT_DIR/uptime-kinomi.service"
        systemctl --user daemon-reload || true
    fi
    rm -f "$BIN_DIR/uptime-kinomi" "$BIN_DIR/uptime-kinomi.old"
    say "Removed the binary and the service."
    say "Kept: $CONFIG_DIR (configuration) and ~/.local/state/uptime-kinomi (data, logs)."
}

main() {
    version=""
    listen="127.0.0.1:8080"
    service=1
    action=install
    while [ $# -gt 0 ]; do
        case "$1" in
            --version)
                [ $# -ge 2 ] || die "--version needs X.Y.Z"
                version=$2
                shift
                ;;
            --listen)
                [ $# -ge 2 ] || die "--listen needs an address like 0.0.0.0:8080"
                listen=$2
                shift
                ;;
            --no-service) service=0 ;;
            --uninstall) action=uninstall ;;
            -h | --help)
                sed -n '2,20p' "$0" 2>/dev/null || say "see https://github.com/$REPO"
                return 0
                ;;
            *) die "unknown option $1" ;;
        esac
        shift
    done
    if [ -n "$version" ]; then
        printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || die "--version must look like 0.3.0"
    fi
    printf '%s' "$listen" | grep -Eq '^[0-9.]+:[0-9]+$' || die "--listen must look like 127.0.0.1:8080"

    if [ "$(id -u)" = 0 ]; then
        die "do not run this as root: log in as the user who should run uptime-kinomi (it needs no root)"
    fi
    [ -n "${HOME:-}" ] || die "HOME is not set"
    BIN_DIR="$HOME/.local/bin"
    CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/uptime-kinomi"
    UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

    if [ "$action" = uninstall ]; then
        uninstall
        return 0
    fi

    asset="uptime-kinomi-$(detect_asset)"
    if [ -n "$version" ]; then
        base="https://github.com/$REPO/releases/download/v$version"
    else
        base="https://github.com/$REPO/releases/latest/download"
    fi
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT INT TERM

    step "Downloading $asset"
    fetch "$base/SHA256SUMS" "$work/SHA256SUMS"
    fetch "$base/SHA256SUMS.minisig" "$work/SHA256SUMS.minisig" || die "the release has no signature (not published yet?)"
    fetch "$base/$asset" "$work/$asset"

    step "Checking the signature and the checksum"
    signed=$(verify_signature "$work/SHA256SUMS" "$work/SHA256SUMS.minisig")
    if [ -n "$version" ] && [ "$signed" != "$version" ] && [ "$signed" != unverified ]; then
        die "the signature is for $signed, not $version"
    fi
    expected=$(awk -v a="$asset" '$2 == a { print $1 }' "$work/SHA256SUMS")
    [ -n "$expected" ] || die "$asset is not listed in SHA256SUMS"
    [ "$(sha256_of "$work/$asset")" = "$expected" ] || die "checksum mismatch for $asset; not installing"
    chmod 755 "$work/$asset"
    reported=$("$work/$asset" version 2>/dev/null) || die "the binary does not run on this system"
    case "$reported" in
        "uptime-kinomi $signed ("*) ;;
        "uptime-kinomi "*) [ "$signed" = unverified ] || die "the binary reports '$reported', expected $signed" ;;
        *) die "unexpected output of the binary: $reported" ;;
    esac
    say "    $reported, signed release $signed"

    if [ -e "$CONFIG_DIR/kinomi.toml" ]; then
        step "Keeping the existing configuration in $CONFIG_DIR"
    else
        step "Creating a starter configuration in $CONFIG_DIR"
        write_starter_config "$CONFIG_DIR" "$listen"
    fi
    # Check the configuration with the NEW binary before replacing anything.
    "$work/$asset" check-config --config-dir "$CONFIG_DIR" >/dev/null 2>&1 ||
        {
            "$work/$asset" check-config --config-dir "$CONFIG_DIR" 2>&1 | grep -E '^(error|warning)' >&2 || true
            die "the configuration in $CONFIG_DIR does not work with $signed; nothing was changed"
        }

    step "Installing to $BIN_DIR"
    mkdir -p "$BIN_DIR"
    cp "$work/$asset" "$BIN_DIR/.uptime-kinomi.new"
    mv "$BIN_DIR/.uptime-kinomi.new" "$BIN_DIR/uptime-kinomi"

    # Older versions of this unit had StateDirectory=, which made systemd
    # link ~/.local/state/uptime-kinomi to the config directory. Undo that.
    state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/uptime-kinomi"
    if [ -L "$state_dir" ]; then
        step "Removing the link $state_dir (left by an older service file)"
        rm -f "$state_dir"
        if [ -d "$CONFIG_DIR/state" ]; then
            mkdir -p "$state_dir"
            mv "$CONFIG_DIR/state" "$state_dir/state"
        fi
    fi

    started=0
    if [ "$service" = 1 ] && user_systemd_works; then
        step "Installing the systemd user service"
        mkdir -p "$UNIT_DIR"
        write_unit "$UNIT_DIR/uptime-kinomi.service"
        systemctl --user daemon-reload
        systemctl --user enable uptime-kinomi >/dev/null 2>&1
        systemctl --user restart uptime-kinomi
        started=1
        linger=$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null || echo unknown)
        if [ "$linger" != yes ]; then
            # Allowed without root on many systems; otherwise explain.
            if loginctl enable-linger >/dev/null 2>&1; then
                linger=yes
            fi
        fi
    elif [ "$service" = 1 ]; then
        say "note: no systemd user session here; the service was not installed."
    fi

    say ""
    say "uptime-kinomi is installed."
    say "  binary:        $BIN_DIR/uptime-kinomi"
    say "  configuration: $CONFIG_DIR"
    if [ "$started" = 1 ]; then
        say "  service:       systemctl --user status uptime-kinomi"
        say "  logs:          journalctl --user -u uptime-kinomi"
        say "  status page:   http://$listen/"
        if [ "$linger" != yes ]; then
            say ""
            say "To keep it running after you log out and to start it at boot, run once:"
            say "  sudo loginctl enable-linger $(id -un)"
        fi
    else
        say "  start:         $BIN_DIR/uptime-kinomi run"
        say "  at boot:       add  @reboot $BIN_DIR/uptime-kinomi run  to 'crontab -e'"
    fi
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *) say "" && say "Add $BIN_DIR to your PATH to use 'uptime-kinomi' directly." ;;
    esac
    say ""
    say "Next: add your monitors to $CONFIG_DIR/monitors/, then"
    say "  uptime-kinomi check-config && systemctl --user restart uptime-kinomi"
    say "Updates: uptime-kinomi self-update"
}

main "$@"
