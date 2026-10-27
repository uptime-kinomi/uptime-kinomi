//! `self-update`: install the latest signed release (SPEC.md §12.1, §12.2).
//!
//! Trust chain: the public keys in `keys/` are compiled in. They verify the
//! minisign signature of `SHA256SUMS`, whose trusted comment names the
//! version. `SHA256SUMS` then vouches for the binary. Nothing downloaded is
//! used before it has been verified.

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use minisign_verify::{PublicKey, Signature};

const RELEASES_URL: &str = "https://github.com/uptime-kinomi/uptime-kinomi/releases";

const PUBLIC_KEYS: [&str; 2] = [
    include_str!("../keys/uptime-kinomi-primary.pub"),
    include_str!("../keys/uptime-kinomi-backup.pub"),
];

/// Target triple → release asset name (SPEC.md §12).
const ASSETS: [(&str, &str); 5] = [
    ("arm-unknown-linux-musleabihf", "uptime-kinomi-armv6"),
    ("armv7-unknown-linux-musleabihf", "uptime-kinomi-armv7"),
    ("aarch64-unknown-linux-musl", "uptime-kinomi-aarch64"),
    ("x86_64-unknown-linux-musl", "uptime-kinomi-x86_64"),
    ("riscv64gc-unknown-linux-musl", "uptime-kinomi-riscv64"),
];

const MAX_SUMS_BYTES: u64 = 16 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 1024;
const MAX_BINARY_BYTES: u64 = 32 * 1024 * 1024;
const TEST_RUN_TIMEOUT: Duration = Duration::from_secs(10);

/// Package-manager territory: never replace binaries there.
const SYSTEM_DIRS: [&str; 4] = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"];

#[derive(Debug, Default, PartialEq)]
pub struct Options {
    pub check: bool,
    pub release: Option<Version>,
    pub allow_downgrade: bool,
    pub rollback: bool,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    UpToDate,
    UpdateAvailable,
    Updated,
    RolledBack,
}

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn err(msg: impl Into<String>) -> Error {
    Error(msg.into())
}

/// A release version `X.Y.Z` (no pre-release suffixes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u32, u32, u32);

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('.');
        let mut next = || -> Option<u32> {
            let part = parts.next()?;
            let valid =
                !part.is_empty() && part.len() <= 9 && part.bytes().all(|b| b.is_ascii_digit());
            if valid { part.parse().ok() } else { None }
        };
        let version = Version(next()?, next()?, next()?);
        parts.next().is_none().then_some(version)
    }

    pub fn current() -> Result<Self, Error> {
        Version::parse(env!("CARGO_PKG_VERSION"))
            .ok_or_else(|| err("this build has no plain X.Y.Z version and cannot self-update"))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

pub fn run(options: &Options) -> Result<Outcome, Error> {
    let current = Version::current()?;
    let exe = installed_binary()?;
    if options.rollback {
        return rollback(&exe);
    }

    let asset = asset_name(env!("KINOMI_TARGET")).ok_or_else(|| {
        err(format!(
            "no release binaries exist for this platform ({})",
            env!("KINOMI_TARGET")
        ))
    })?;
    let base = match options.release {
        Some(v) => format!("{RELEASES_URL}/download/v{v}"),
        None => format!("{RELEASES_URL}/latest/download"),
    };
    let agent = http_agent();

    let sums = fetch(&agent, &format!("{base}/SHA256SUMS"), MAX_SUMS_BYTES)?;
    let signature = fetch(
        &agent,
        &format!("{base}/SHA256SUMS.minisig"),
        MAX_SIGNATURE_BYTES,
    )?;
    let keys = release_keys()?;
    let release = verify_signature(&sums, &signature, &keys)?;
    if options.release.is_some_and(|wanted| wanted != release) {
        return Err(err(format!(
            "the signed checksums are for {release}, not the requested release"
        )));
    }
    let expected_sha256 = checksum_for(&sums, asset)?;

    if options.check {
        return Ok(if release > current {
            say!("update available: {current} -> {release}");
            Outcome::UpdateAvailable
        } else {
            say!("up to date ({current}, latest signed release {release})");
            Outcome::UpToDate
        });
    }
    if release <= current && !(options.allow_downgrade && options.release.is_some()) {
        if release == current {
            say!("already up to date ({current})");
            return Ok(Outcome::UpToDate);
        }
        return Err(err(format!(
            "release {release} is older than the running version {current}; \
             use --release {release} --allow-downgrade to install it anyway"
        )));
    }

    say!("downloading {asset} {release}");
    let temp = TempFile::create(&exe)?;
    download_verified(
        &agent,
        &format!("{base}/{asset}"),
        &temp.path,
        &expected_sha256,
    )?;
    let tested = test_run(&temp.path, TEST_RUN_TIMEOUT)?;
    if tested != release {
        return Err(err(format!(
            "downloaded binary reports version {tested}, expected {release}"
        )));
    }
    replace(&exe, temp)?;
    say!(
        "updated {current} -> {release} ({}). The previous version is kept as {}.\n\
         Restart the service to use it, e.g.: systemctl --user restart uptime-kinomi",
        exe.display(),
        backup_path(&exe).display()
    );
    Ok(Outcome::Updated)
}

fn asset_name(target: &str) -> Option<&'static str> {
    ASSETS
        .iter()
        .find(|(t, _)| *t == target)
        .map(|(_, asset)| *asset)
}

/// The real path of the running binary, if we are allowed to replace it.
fn installed_binary() -> Result<PathBuf, Error> {
    let exe = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|e| {
            err(format!(
                "cannot determine the path of the running binary: {e}"
            ))
        })?;
    let dir = exe
        .parent()
        .ok_or_else(|| err("the running binary has no parent directory"))?;
    if SYSTEM_DIRS.iter().any(|sys| dir == Path::new(sys)) {
        return Err(err(format!(
            "{} is managed by the system; update it with your package manager \
             or install kinomi into a directory you own, such as ~/.local/bin",
            exe.display()
        )));
    }
    Ok(exe)
}

fn backup_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(5)
        .max_redirects_will_error(true)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(600)))
        .user_agent(format!(
            "uptime-kinomi/{} (self-update)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .new_agent()
}

fn fetch(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, Error> {
    let fail = |e: ureq::Error| err(format!("download of {url} failed: {e}"));
    let mut response = agent.get(url).call().map_err(|e| match e {
        ureq::Error::StatusCode(404) => err(format!("no published release found ({url}: 404)")),
        e => fail(e),
    })?;
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(fail)
}

pub(crate) fn release_keys() -> Result<Vec<PublicKey>, Error> {
    PUBLIC_KEYS
        .iter()
        .map(|text| {
            PublicKey::decode(text).map_err(|e| err(format!("invalid embedded release key: {e}")))
        })
        .collect()
}

/// Verify the minisign signature of `sums` and return the version from the
/// signed trusted comment.
pub(crate) fn verify_signature(
    sums: &[u8],
    signature: &[u8],
    keys: &[PublicKey],
) -> Result<Version, Error> {
    let text =
        std::str::from_utf8(signature).map_err(|_| err("SHA256SUMS.minisig is not valid UTF-8"))?;
    let signature = Signature::decode(text)
        .map_err(|e| err(format!("SHA256SUMS.minisig is malformed: {e}")))?;
    // Legacy (non-prehashed) signatures are rejected: allow_legacy = false.
    if !keys
        .iter()
        .any(|key| key.verify(sums, &signature, false).is_ok())
    {
        return Err(err(
            "SHA256SUMS is not signed by an uptime-kinomi release key; refusing to update",
        ));
    }
    signature
        .trusted_comment()
        .strip_prefix("uptime-kinomi ")
        .and_then(Version::parse)
        .ok_or_else(|| err("the signed trusted comment is not 'uptime-kinomi X.Y.Z'"))
}

/// Find the SHA-256 of `asset` in a strictly formatted `SHA256SUMS`.
pub(crate) fn checksum_for(sums: &[u8], asset: &str) -> Result<String, Error> {
    let malformed = |why: &str| err(format!("SHA256SUMS is malformed: {why}"));
    let text = std::str::from_utf8(sums).map_err(|_| malformed("not UTF-8"))?;
    let mut found = None;
    let mut names = Vec::new();
    for line in text.lines() {
        let (hash, name) = line.split_once("  ").ok_or_else(|| malformed("bad line"))?;
        let hex = hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !hex {
            return Err(malformed("bad hash"));
        }
        if !ASSETS.iter().any(|(_, known)| *known == name) {
            return Err(malformed("unknown file name"));
        }
        if names.contains(&name) {
            return Err(malformed("duplicate file name"));
        }
        names.push(name);
        if name == asset {
            found = Some(hash.to_string());
        }
    }
    found.ok_or_else(|| err(format!("the release does not contain {asset}")))
}

/// A file next to the binary that is deleted again unless it was installed.
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn create(exe: &Path) -> Result<Self, Error> {
        let dir = exe
            .parent()
            .ok_or_else(|| err("the running binary has no parent directory"))?;
        let path = dir.join(format!(".uptime-kinomi.update-{}", std::process::id()));
        // A leftover from a crashed update with the same PID is ours to remove.
        let _ = fs::remove_file(&path);
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&path)
            .map_err(|e| {
                err(format!(
                    "cannot write to {}: {e} (self-update needs write access to the directory of the binary)",
                    dir.display()
                ))
            })?;
        Ok(TempFile { path })
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn download_verified(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    expected: &str,
) -> Result<(), Error> {
    let fail = |e: ureq::Error| err(format!("download of {url} failed: {e}"));
    let response = agent.get(url).call().map_err(fail)?;
    let mut reader = response
        .into_body()
        .into_with_config()
        .limit(MAX_BINARY_BYTES)
        .reader();
    let mut file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(dest)
        .map_err(|e| err(format!("cannot write {}: {e}", dest.display())))?;
    let actual = copy_and_hash(&mut reader, &mut file)
        .map_err(|e| err(format!("download of {url} failed: {e}")))?;
    file.sync_all()
        .map_err(|e| err(format!("cannot write {}: {e}", dest.display())))?;
    if actual != expected {
        return Err(err(format!(
            "checksum mismatch for {url}; refusing to update"
        )));
    }
    Ok(())
}

/// Copy `reader` to `writer` and return the lowercase hex SHA-256.
fn copy_and_hash(reader: &mut impl Read, writer: &mut impl Write) -> io::Result<String> {
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        context.update(&buf[..n]);
        writer.write_all(&buf[..n])?;
    }
    Ok(context
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Run `<binary> version` and return the version it reports. This proves the
/// binary actually runs on this CPU.
fn test_run(binary: &Path, timeout: Duration) -> Result<Version, Error> {
    let fail = |e: io::Error| err(format!("cannot run {}: {e}", binary.display()));
    let mut attempts = 0;
    let mut child = loop {
        let spawned = Command::new(binary)
            .arg("version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            // A file that was just written can briefly be "busy" if another
            // thread forked while it was open for writing.
            Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy && attempts < 5 => {
                attempts += 1;
                thread::sleep(Duration::from_millis(50));
            }
            result => break result.map_err(fail)?,
        }
    };
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(fail)? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err(format!(
                "{} did not finish its test run",
                binary.display()
            )));
        }
        thread::sleep(Duration::from_millis(50));
    };
    let mut output = String::new();
    if let Some(stdout) = child.stdout.take() {
        stdout
            .take(1024)
            .read_to_string(&mut output)
            .map_err(fail)?;
    }
    if !status.success() {
        return Err(err(format!(
            "test run of {} failed ({status})",
            binary.display()
        )));
    }
    output
        .strip_prefix("uptime-kinomi ")
        .and_then(|rest| rest.split_once(" ("))
        .and_then(|(version, _)| Version::parse(version))
        .ok_or_else(|| {
            err(format!(
                "{} does not look like uptime-kinomi",
                binary.display()
            ))
        })
}

/// Keep the current binary as `.old` and move the new one into place
/// atomically, so there is never a moment without a binary.
fn replace(exe: &Path, new: TempFile) -> Result<(), Error> {
    let backup = backup_path(exe);
    let fail = |e: io::Error| err(format!("cannot replace {}: {e}", exe.display()));
    match fs::remove_file(&backup) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(fail(e)),
        _ => {}
    }
    // Hard links are instant; copy where the file system does not support them.
    if fs::hard_link(exe, &backup).is_err() {
        fs::copy(exe, &backup).map_err(fail)?;
    }
    fs::rename(&new.path, exe).map_err(fail)?;
    Ok(())
}

fn rollback(exe: &Path) -> Result<Outcome, Error> {
    let backup = backup_path(exe);
    if !backup.is_file() {
        return Err(err(format!(
            "no previous version found at {}",
            backup.display()
        )));
    }
    let version = test_run(&backup, TEST_RUN_TIMEOUT)?;
    fs::rename(&backup, exe)
        .map_err(|e| err(format!("cannot restore {}: {e}", backup.display())))?;
    say!(
        "restored {version} at {}. Restart the service to use it, e.g.: \
         systemctl --user restart uptime-kinomi",
        exe.display()
    );
    Ok(Outcome::RolledBack)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestDir;

    const TEST_PUB: &str = include_str!("../testdata/update/test.pub");
    const SUMS: &[u8] = include_bytes!("../testdata/update/SHA256SUMS");

    fn test_keys() -> Vec<PublicKey> {
        vec![PublicKey::decode(TEST_PUB).unwrap()]
    }

    fn verify(signature_file: &[u8]) -> Result<Version, Error> {
        verify_signature(SUMS, signature_file, &test_keys())
    }

    #[test]
    fn version_parse_and_order() {
        assert_eq!(Version::parse("1.2.3"), Some(Version(1, 2, 3)));
        for bad in [
            "",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "1.2.x",
            "1..3",
            "1.2.3-rc1",
            "+1.2.3",
            "1234567890.0.0",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        assert_eq!(Version(1, 2, 3).to_string(), "1.2.3");
        assert!(Version::current().is_ok());
    }

    #[test]
    fn asset_names() {
        assert_eq!(
            asset_name("arm-unknown-linux-musleabihf"),
            Some("uptime-kinomi-armv6")
        );
        assert_eq!(
            asset_name("riscv64gc-unknown-linux-musl"),
            Some("uptime-kinomi-riscv64")
        );
        assert_eq!(asset_name("x86_64-unknown-linux-gnu"), None);
    }

    #[test]
    fn embedded_release_keys_decode() {
        assert_eq!(release_keys().unwrap().len(), 2);
    }

    #[test]
    fn valid_signature() {
        assert_eq!(
            verify(include_bytes!("../testdata/update/good.minisig")).unwrap(),
            Version(1, 2, 3)
        );
    }

    #[test]
    fn rejected_signatures() {
        let good = include_bytes!("../testdata/update/good.minisig");
        // Tampered checksums.
        let mut tampered = SUMS.to_vec();
        tampered[0] ^= 1;
        assert!(verify_signature(&tampered, good, &test_keys()).is_err());
        // Valid signature, but not by a trusted key.
        assert!(verify_signature(SUMS, good, &release_keys().unwrap()).is_err());
        assert!(verify(include_bytes!("../testdata/update/other-key.minisig")).is_err());
        // Signed, but for another product.
        assert!(verify(include_bytes!("../testdata/update/wrong-comment.minisig")).is_err());
        // Legacy, non-prehashed signature.
        assert!(verify(include_bytes!("../testdata/update/legacy.minisig")).is_err());
        // Garbage.
        assert!(verify(b"").is_err());
        assert!(verify(&[0xff, 0xfe]).is_err());
    }

    #[test]
    fn checksums() {
        let hash = checksum_for(SUMS, "uptime-kinomi-armv6").unwrap();
        assert_eq!(hash.len(), 64);
        let line = |hash: &str, name: &str| format!("{hash}  {name}\n");
        let h = "a".repeat(64);
        assert!(
            checksum_for(
                line(&h, "uptime-kinomi-armv6").as_bytes(),
                "uptime-kinomi-armv7"
            )
            .is_err()
        );
        for bad in [
            line(&h, "evil"),
            line(&"A".repeat(64), "uptime-kinomi-armv6"),
            line(&"a".repeat(63), "uptime-kinomi-armv6"),
            format!("{h} uptime-kinomi-armv6\n"),
            line(&h, "uptime-kinomi-armv6").repeat(2),
        ] {
            assert!(
                checksum_for(bad.as_bytes(), "uptime-kinomi-armv6").is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn hashing() {
        let mut out = Vec::new();
        let hash = copy_and_hash(&mut &b"abc"[..], &mut out).unwrap();
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(out, b"abc");
    }

    #[test]
    fn test_runs() {
        let dir = TestDir::new("test-run");
        let ok = dir.script("ok", "echo 'uptime-kinomi 9.8.7 (some-target)'");
        assert_eq!(test_run(&ok, TEST_RUN_TIMEOUT).unwrap(), Version(9, 8, 7));
        let other = dir.script("other", "echo 'something 9.8.7 (x)'");
        assert!(test_run(&other, TEST_RUN_TIMEOUT).is_err());
        let failing = dir.script("failing", "echo 'uptime-kinomi 9.8.7 (x)'; exit 1");
        assert!(test_run(&failing, TEST_RUN_TIMEOUT).is_err());
        let hanging = dir.script("hanging", "sleep 5");
        assert!(test_run(&hanging, Duration::from_millis(200)).is_err());
        assert!(test_run(&dir.path().join("missing"), TEST_RUN_TIMEOUT).is_err());
    }

    #[test]
    fn replace_and_rollback() {
        let dir = TestDir::new("replace");
        let exe = dir.script("uptime-kinomi", "echo 'uptime-kinomi 1.0.0 (t)'");
        let temp = TempFile::create(&exe).unwrap();
        fs::write(&temp.path, "#!/bin/sh\necho 'uptime-kinomi 2.0.0 (t)'\n").unwrap();
        let temp_path = temp.path.clone();
        replace(&exe, temp).unwrap();
        assert!(!temp_path.exists());
        assert_eq!(test_run(&exe, TEST_RUN_TIMEOUT).unwrap(), Version(2, 0, 0));
        assert_eq!(
            test_run(&backup_path(&exe), TEST_RUN_TIMEOUT).unwrap(),
            Version(1, 0, 0)
        );

        assert_eq!(rollback(&exe).unwrap(), Outcome::RolledBack);
        assert_eq!(test_run(&exe, TEST_RUN_TIMEOUT).unwrap(), Version(1, 0, 0));
        assert!(!backup_path(&exe).exists());
        assert!(rollback(&exe).is_err());
    }

    #[test]
    fn temp_file_is_removed_on_drop() {
        let dir = TestDir::new("temp");
        let exe = dir.script("uptime-kinomi", "true");
        let path = TempFile::create(&exe).unwrap().path.clone();
        assert!(!path.exists());
    }
}
