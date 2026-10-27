//! HTTP Basic Auth against an htpasswd file with bcrypt hashes
//! (SPEC.md §9.3), the same format Apache and nginx use.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::encoding::{base64_decode, bcrypt_decode, bcrypt_encode, constant_time_eq};

/// Successful verifications are remembered this long (bcrypt is slow on a Pi 1).
const CACHE_TIME: Duration = Duration::from_secs(600);
const CACHE_ENTRIES: usize = 64;
/// After this many failures in a row, at most one verification per second.
const FREE_FAILURES: u32 = 5;
const MAX_HEADER: usize = 1024;
const MAX_HTPASSWD_BYTES: u64 = 64 * 1024;

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Allowed,
    Denied,
    /// Too many recent failures; try again later.
    Throttled,
}

struct Hash {
    cost: u32,
    salt: [u8; 16],
    /// bcrypt output without the last byte (as in OpenBSD's encoding).
    digest: [u8; 23],
}

struct Limiter {
    failures: u32,
    last: Option<Instant>,
}

pub struct Auth {
    users: Vec<(String, Hash)>,
    cache: Mutex<HashMap<[u8; 32], Instant>>,
    limiter: Mutex<Limiter>,
}

impl Auth {
    pub fn load(path: &Path) -> Result<Self, String> {
        let meta = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if meta.len() > MAX_HTPASSWD_BYTES {
            return Err(format!(
                "{}: larger than {MAX_HTPASSWD_BYTES} bytes",
                path.display()
            ));
        }
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut users = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (user, hash) = line
                .split_once(':')
                .ok_or(format!("line {}: expected user:hash", n + 1))?;
            let hash = parse_hash(hash).ok_or(format!(
                "line {}: only bcrypt hashes ($2y$, created with 'htpasswd -B' or 'uptime-kinomi hash-password') are supported",
                n + 1
            ))?;
            if user.is_empty() || users.iter().any(|(u, _)| u == user) {
                return Err(format!("line {}: empty or duplicate user", n + 1));
            }
            users.push((user.to_string(), hash));
        }
        if users.is_empty() {
            return Err("no users".into());
        }
        Ok(Auth {
            users,
            cache: Mutex::new(HashMap::new()),
            limiter: Mutex::new(Limiter {
                failures: 0,
                last: None,
            }),
        })
    }

    /// Check an `Authorization` header value.
    pub fn check(&self, header: Option<&str>) -> Verdict {
        let Some(header) = header.filter(|h| h.len() <= MAX_HEADER) else {
            return Verdict::Denied;
        };
        let key: [u8; 32] = sha256(header.as_bytes());
        let now = Instant::now();
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if cache
                .get(&key)
                .is_some_and(|t| now.duration_since(*t) < CACHE_TIME)
            {
                return Verdict::Allowed;
            }
        }
        {
            let mut limiter = self.limiter.lock().unwrap_or_else(|e| e.into_inner());
            if limiter.failures >= FREE_FAILURES
                && limiter
                    .last
                    .is_some_and(|t| now.duration_since(t) < Duration::from_secs(1))
            {
                return Verdict::Throttled;
            }
            limiter.last = Some(now);
        }

        let ok = credentials(header).is_some_and(|(user, password)| self.verify(&user, &password));
        let mut limiter = self.limiter.lock().unwrap_or_else(|e| e.into_inner());
        if ok {
            limiter.failures = 0;
            drop(limiter);
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.retain(|_, t| now.duration_since(*t) < CACHE_TIME);
            if cache.len() >= CACHE_ENTRIES {
                cache.clear();
            }
            cache.insert(key, now);
            Verdict::Allowed
        } else {
            limiter.failures = limiter.failures.saturating_add(1);
            Verdict::Denied
        }
    }

    fn verify(&self, user: &str, password: &[u8]) -> bool {
        match self.users.iter().find(|(u, _)| u == user) {
            Some((_, hash)) => {
                let digest = bcrypt_digest(hash.cost, hash.salt, password);
                constant_time_eq(&digest[..23], &hash.digest)
            }
            None => {
                // Spend the same time for unknown users.
                let (_, first) = &self.users[0];
                let _ = bcrypt_digest(first.cost, first.salt, password);
                false
            }
        }
    }
}

/// `Basic base64(user:password)` → (user, password)
fn credentials(header: &str) -> Option<(String, Vec<u8>)> {
    let encoded = header.strip_prefix("Basic ")?.trim();
    let decoded = base64_decode(encoded)?;
    let colon = decoded.iter().position(|&b| b == b':')?;
    let user = String::from_utf8(decoded[..colon].to_vec()).ok()?;
    Some((user, decoded[colon + 1..].to_vec()))
}

/// `$2y$10$<22 salt chars><31 hash chars>` (also $2a$ and $2b$).
fn parse_hash(text: &str) -> Option<Hash> {
    let rest = text
        .strip_prefix("$2y$")
        .or_else(|| text.strip_prefix("$2b$"))
        .or_else(|| text.strip_prefix("$2a$"))?;
    let (cost, rest) = rest.split_once('$')?;
    // ASCII only, so the slicing below is on character boundaries.
    if cost.len() != 2 || rest.len() != 53 || !rest.is_ascii() {
        return None;
    }
    let cost: u32 = cost.parse().ok()?;
    if !(4..=31).contains(&cost) {
        return None;
    }
    let salt_bytes = bcrypt_decode(&rest[..22])?;
    let digest_bytes = bcrypt_decode(&rest[22..])?;
    Some(Hash {
        cost,
        salt: salt_bytes.get(..16)?.try_into().ok()?,
        digest: digest_bytes.get(..23)?.try_into().ok()?,
    })
}

/// bcrypt with the standard key handling: the password plus a NUL byte,
/// truncated to 72 bytes. (The bcrypt crate's raw function panics on empty or
/// longer keys, which this can never pass.)
fn bcrypt_digest(cost: u32, salt: [u8; 16], password: &[u8]) -> [u8; 24] {
    let mut key: Vec<u8> = password.iter().copied().take(71).collect();
    key.push(0);
    bcrypt::bcrypt(cost.clamp(4, 31), salt, &key)
}

/// An htpasswd line for `user` (cost 10), as written by `hash-password`.
pub fn htpasswd_line(user: &str, password: &[u8], salt: [u8; 16]) -> String {
    const COST: u32 = 10;
    let digest = bcrypt_digest(COST, salt, password);
    format!(
        "{user}:$2y${COST:02}${}{}",
        bcrypt_encode(&salt),
        bcrypt_encode(&digest[..23])
    )
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let digest = ring::digest::digest(&ring::digest::SHA256, data);
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::base64;

    /// "secret", hashed independently with Python's `bcrypt` package (pyca),
    /// cost 4, with the `$2y$` prefix that `htpasswd -B` writes.
    const APACHE_LINE: &str = "alice:$2y$04$q2C9wPe5iV0OYbDGCKjpgeS2lSPX0N4D6gCLgoRaQ8bVA9zTkqyei";

    fn header(user: &str, password: &str) -> String {
        format!("Basic {}", base64(format!("{user}:{password}").as_bytes()))
    }

    #[test]
    fn known_bcrypt_vector() {
        // From the OpenBSD bcrypt test vectors: "U*U" with this salt, cost 5
        // (also confirmed with Python's bcrypt package).
        let hash =
            parse_hash("$2a$05$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW").unwrap();
        let digest = bcrypt_digest(hash.cost, hash.salt, b"U*U");
        assert_eq!(&digest[..23], &hash.digest);
    }

    #[test]
    fn htpasswd_round_trip() {
        let line = htpasswd_line("bob", b"correct horse", [7; 16]);
        assert!(line.starts_with("bob:$2y$10$"), "{line}");
        let auth = Auth::parse(&line).unwrap();
        assert_eq!(
            auth.check(Some(&header("bob", "correct horse"))),
            Verdict::Allowed
        );
        assert_eq!(auth.check(Some(&header("bob", "wrong"))), Verdict::Denied);
    }

    #[test]
    fn apache_file() {
        let auth = Auth::parse(&format!("# users\n\n{APACHE_LINE}\n")).unwrap();
        assert_eq!(
            auth.check(Some(&header("alice", "secret"))),
            Verdict::Allowed
        );
        // Cached now: also fine.
        assert_eq!(
            auth.check(Some(&header("alice", "secret"))),
            Verdict::Allowed
        );
        assert_eq!(
            auth.check(Some(&header("mallory", "secret"))),
            Verdict::Denied
        );
        assert_eq!(auth.check(None), Verdict::Denied);
        assert_eq!(auth.check(Some("Bearer xyz")), Verdict::Denied);
        assert_eq!(auth.check(Some("Basic !!!")), Verdict::Denied);
    }

    #[test]
    fn empty_and_long_passwords_do_not_panic() {
        let auth = Auth::parse(APACHE_LINE).unwrap();
        assert_eq!(auth.check(Some(&header("alice", ""))), Verdict::Denied);
        assert_eq!(
            auth.check(Some(&header("alice", &"x".repeat(500)))),
            Verdict::Denied
        );
    }

    #[test]
    fn throttling_after_failures() {
        let auth = Auth::parse(APACHE_LINE).unwrap();
        for _ in 0..FREE_FAILURES {
            assert_eq!(auth.check(Some(&header("alice", "nope"))), Verdict::Denied);
        }
        assert_eq!(
            auth.check(Some(&header("alice", "secret"))),
            Verdict::Throttled
        );
    }

    #[test]
    fn rejected_files() {
        for bad in [
            "",
            "# only comments",
            "alice",
            "alice:{SHA}W6ph5Mm5Pz8GgiULbPgzG37mj9g=",
            "alice:$apr1$abc$def",
            "alice:$2y$03$q2C9wPe5iV0OYbDGCKjpgeS2lSPX0N4D6gCLgoRaQ8bVA9zTkqyei",
            "alice:$2y$04$short",
            // Found by fuzzing: a multi-byte character used to panic
            // (slicing inside a character).
            "alice:$2y$10$vn5tdPfhSmAeoTrf.KHZp\u{77e}bbQvIOqvv1wOM8svQJOKRZ2yhGbrUK",
            &format!("{APACHE_LINE}\n{APACHE_LINE}"),
        ] {
            assert!(Auth::parse(bad).is_err(), "{bad}");
        }
    }
}
