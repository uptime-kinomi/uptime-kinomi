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
