//! Every configuration shown in the documentation (docs/examples/) and the
//! example configuration in contrib/ must pass `check-config`, with no
//! warnings other than the ones the documentation explains.

use std::path::Path;
use std::process::Command;

/// Warnings the examples trigger on purpose. Git does not keep the
/// "others" permission bits, so in a checkout every secret is readable by
/// all users.
const EXPECTED_WARNINGS: &[&str] = &[
    "readable by all users",
    "http.tls_verify = false",
    "private pages on a non-loopback address",
];

#[test]
fn documented_configurations_are_valid() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut dirs: Vec<_> = std::fs::read_dir(root.join("docs/examples"))
        .expect("docs/examples")
        .map(|e| e.expect("directory entry").path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    assert!(dirs.len() >= 3, "docs/examples has too few configurations");
    dirs.push(root.join("contrib/examples/config"));

    for dir in dirs {
        let out = Command::new(env!("CARGO_BIN_EXE_uptime-kinomi"))
            .arg("check-config")
            .arg("--config-dir")
            .arg(&dir)
            .output()
            .expect("run uptime-kinomi");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "check-config failed for {}:\n{stderr}{stdout}",
            dir.display()
        );
        for line in stderr.lines().chain(stdout.lines()) {
            if line.starts_with("warning:") {
                assert!(
                    EXPECTED_WARNINGS.iter().any(|w| line.contains(w)),
                    "unexpected warning for {}: {line}",
                    dir.display()
                );
            }
        }
    }
}
