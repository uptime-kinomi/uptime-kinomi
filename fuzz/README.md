# Fuzzing

Fuzz targets for [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz)
(libFuzzer, needs nightly Rust). The entry points live in
[`src/fuzzing.rs`](../src/fuzzing.rs); the files in `fuzz_targets/` only
call them. Besides "never panic", some targets check properties (round
trips, redirect scheme rules) and fail when one breaks.

| Target | Input | Source of the data |
|---|---|---|
| `http_request` | request head, parsed and routed (pages, Basic Auth, push) | network |
| `http_redirect` | `Location` header of a monitored server | network |
| `dns_response` | DNS response (last byte picks the record type) | network |
| `x509` | certificate from a TLS handshake (notAfter reader) | network |
| `release` | `SHA256SUMS` + minisign signature for `self-update` | network |
| `auth` | `Authorization` header, htpasswd file | network, disk |
| `config` | all configuration file types (first byte picks the type) | disk |
| `storage` | log lines and state files replayed at startup | disk |
| `encodings` | base64 variants, timestamps, durations | both |

## Running

```sh
cargo install cargo-fuzz
cargo +nightly fuzz run http_request fuzz/corpus/http_request fuzz/seeds/http_request -- -max_total_time=300
```

The first directory is the working corpus (not in Git), the second the seeds
(in Git). A crash is saved in `fuzz/artifacts/<target>/`; replay it with
`cargo +nightly fuzz run <target> <file>`. When you fix it, add the input
to `fuzz/seeds/<target>/` so the regression stays covered.

`cargo test` runs all seeds through all targets on stable Rust
(`fuzzing::tests::corpus`), and CI fuzzes every target for a minute on every
change and for 30 minutes once a week (`.github/workflows/fuzz.yml`).
