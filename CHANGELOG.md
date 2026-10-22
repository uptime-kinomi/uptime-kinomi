# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

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
