#!/bin/sh
# Verify, sign and publish a pre-release (SPEC.md §12.1).
#
# Runs on the maintainer's machine, where the offline minisign key lives.
#
# Needs:    sh, curl, minisign, sha256sum or shasum, and a checkout of this
#           repository (for keys/).
# Optional: gh            - also verifies the build provenance attestations
#                           (if it has 'gh attestation' and is logged in)
#           GITHUB_TOKEN  - fine-grained token for this repository with
#                           "Contents: read and write"; used to upload the
#                           signature and publish. Without it, the script
#                           prints what to do in the browser.
#
# Usage: contrib/release/sign-release.sh vX.Y.Z [SECRET_KEY_FILE]
#        default key: ~/.minisign/uptime-kinomi-primary.key

set -eu

repo=uptime-kinomi/uptime-kinomi
api=https://api.github.com/repos/$repo
assets="uptime-kinomi-aarch64 uptime-kinomi-armv6 uptime-kinomi-armv7 uptime-kinomi-riscv64 uptime-kinomi-x86_64"

die() {
    echo "error: $*" >&2
    exit 1
}

# HTTPS only, fail on HTTP errors, bounded size and time.
fetch() {
    curl --proto '=https' --tlsv1.2 -fsSL --max-filesize 33554432 --max-time 300 "$@"
}

[ $# -ge 1 ] && [ $# -le 2 ] || die "usage: $0 vX.Y.Z [SECRET_KEY_FILE]"
tag=$1
key=${2:-$HOME/.minisign/uptime-kinomi-primary.key}
echo "$tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || die "tag must look like vX.Y.Z"
version=${tag#v}

keys_dir=$(cd "$(dirname "$0")/../../keys" && pwd)
for tool in curl minisign; do
    command -v "$tool" >/dev/null || die "$tool is not installed"
done
if command -v sha256sum >/dev/null; then
    sha256_check() { sha256sum -c "$1"; }
elif command -v shasum >/dev/null; then
    sha256_check() { shasum -a 256 -c "$1"; }
else
    die "neither sha256sum nor shasum is installed"
fi
[ -f "$key" ] || die "secret key $key not found"

start_dir=$(pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"

echo "==> Checking that $tag is an unsigned pre-release"
fetch "$api/releases/tags/$tag" >release.json || die "release $tag not found"
grep -Eq '"prerelease": *true' release.json || die "release $tag is not a pre-release (already published?)"
! grep -Eq '"name": *"SHA256SUMS.minisig"' release.json || die "release $tag is already signed"
# The release's own "id" is the first one in the JSON (before author and assets).
release_id=$(grep -Eo '"id": *[0-9]+' release.json | head -n 1 | grep -Eo '[0-9]+$')
[ -n "$release_id" ] || die "cannot read the release ID"

echo "==> Downloading assets"
for file in SHA256SUMS $assets; do
    fetch -o "$file" "https://github.com/$repo/releases/download/$tag/$file"
done

echo "==> Verifying checksums"
listed=$(awk '{ print $2 }' SHA256SUMS | LC_ALL=C sort | tr '\n' ' ')
expected=$(printf '%s\n' $assets | LC_ALL=C sort | tr '\n' ' ')
[ "$listed" = "$expected" ] || die "SHA256SUMS lists unexpected files: $listed"
grep -Evq '^[0-9a-f]{64}  uptime-kinomi-[a-z0-9_]+$' SHA256SUMS && die "SHA256SUMS is malformed"
sha256_check SHA256SUMS

if command -v gh >/dev/null && gh attestation verify --help >/dev/null 2>&1 &&
    gh auth status >/dev/null 2>&1; then
    echo "==> Verifying build provenance (built by release.yml from $tag on a GitHub-hosted runner)"
    for asset in $assets; do
        gh attestation verify "$asset" \
            --repo "$repo" \
            --signer-workflow "$repo/.github/workflows/release.yml" \
            --source-ref "refs/tags/$tag" \
            --deny-self-hosted-runners >/dev/null
        echo "$asset: OK"
    done
else
    echo "==> skipping the build provenance check (needs a logged-in gh with 'gh attestation')"
fi

echo
cat SHA256SUMS
echo
printf 'Sign uptime-kinomi %s? [y/N] ' "$version"
read -r answer
[ "$answer" = y ] || [ "$answer" = Y ] || die "aborted"

minisign -S -s "$key" -m SHA256SUMS -x SHA256SUMS.minisig \
    -t "uptime-kinomi $version" \
    -c "signature of uptime-kinomi $version release checksums"

echo "==> Checking the signature against keys/"
verified=false
for pub in "$keys_dir/uptime-kinomi-primary.pub" "$keys_dir/uptime-kinomi-backup.pub"; do
    comment=$(minisign -V -m SHA256SUMS -x SHA256SUMS.minisig -p "$pub" -Q 2>/dev/null) || continue
    [ "$comment" = "uptime-kinomi $version" ] || die "unexpected trusted comment: $comment"
    verified=true
done
$verified || die "the signature does not verify with keys/ (wrong secret key?)"

if [ -z "${GITHUB_TOKEN:-}" ]; then
    cp SHA256SUMS.minisig "$start_dir/SHA256SUMS.minisig"
    cat <<EOT

Signed. GITHUB_TOKEN is not set, so publish it yourself:
  1. Open https://github.com/$repo/releases/edit/$tag
  2. Attach $start_dir/SHA256SUMS.minisig
  3. Untick "Set as a pre-release", tick "Set as the latest release",
     then "Update release".
(Or paste the signature below to whoever publishes the release.)

EOT
    cat SHA256SUMS.minisig
    exit 0
fi

echo "==> Uploading signature"
fetch -X POST \
    -H "Authorization: Bearer $GITHUB_TOKEN" \
    -H "Accept: application/vnd.github+json" \
    -H "Content-Type: application/octet-stream" \
    --data-binary @SHA256SUMS.minisig \
    "https://uploads.github.com/repos/$repo/releases/$release_id/assets?name=SHA256SUMS.minisig" >/dev/null

echo "==> Publishing $tag as the latest release"
fetch -X PATCH \
    -H "Authorization: Bearer $GITHUB_TOKEN" \
    -H "Accept: application/vnd.github+json" \
    -d '{"prerelease": false, "make_latest": "true"}' \
    "$api/releases/$release_id" >/dev/null

echo "Published: https://github.com/$repo/releases/tag/$tag"
