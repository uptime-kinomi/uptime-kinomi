#!/bin/sh
# Verify, sign and publish a draft release (SPEC.md §12.1).
#
# Runs on the maintainer's machine, where the offline minisign key lives.
# Needs: a recent gh with `gh attestation` (logged in), minisign, sha256sum or shasum, and a checkout of
# this repository (for keys/).
#
# Usage: contrib/release/sign-release.sh vX.Y.Z [SECRET_KEY_FILE]
#        default key: ~/.minisign/uptime-kinomi-primary.key

set -eu

repo=uptime-kinomi/uptime-kinomi
assets="uptime-kinomi-aarch64 uptime-kinomi-armv6 uptime-kinomi-armv7 uptime-kinomi-riscv64 uptime-kinomi-x86_64"

die() {
    echo "error: $*" >&2
    exit 1
}

[ $# -ge 1 ] && [ $# -le 2 ] || die "usage: $0 vX.Y.Z [SECRET_KEY_FILE]"
tag=$1
key=${2:-$HOME/.minisign/uptime-kinomi-primary.key}
echo "$tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || die "tag must look like vX.Y.Z"
version=${tag#v}

keys_dir=$(cd "$(dirname "$0")/../../keys" && pwd)
for tool in gh minisign; do
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

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

echo "==> Checking that $tag is a draft release"
is_draft=$(gh release view "$tag" --repo "$repo" --json isDraft --jq .isDraft)
[ "$is_draft" = true ] || die "release $tag is not a draft (already published?)"

echo "==> Downloading assets"
gh release download "$tag" --repo "$repo" --dir "$work"
cd "$work"

expected=$(printf '%s\n' SHA256SUMS $assets | LC_ALL=C sort | tr '\n' ' ')
actual=$(ls | LC_ALL=C sort | tr '\n' ' ')
[ "$actual" = "$expected" ] || die "unexpected release assets: $actual"

echo "==> Verifying checksums"
[ "$(wc -l <SHA256SUMS | tr -d ' ')" -eq 5 ] || die "SHA256SUMS must list exactly 5 binaries"
sha256_check SHA256SUMS

echo "==> Verifying build provenance (built by release.yml from $tag on a GitHub-hosted runner)"
for asset in $assets; do
    gh attestation verify "$asset" \
        --repo "$repo" \
        --signer-workflow "$repo/.github/workflows/release.yml" \
        --source-ref "refs/tags/$tag" \
        --deny-self-hosted-runners >/dev/null
    echo "$asset: OK"
done

echo
cat SHA256SUMS
echo
printf 'Sign and publish uptime-kinomi %s? [y/N] ' "$version"
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

echo "==> Uploading signature and publishing $tag"
gh release upload "$tag" SHA256SUMS.minisig --repo "$repo"
gh release edit "$tag" --repo "$repo" --draft=false --latest

echo "Published: https://github.com/$repo/releases/tag/$tag"
