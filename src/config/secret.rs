//! Secrets given inline or via a `*_file` key (SPEC.md §3.2).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Largest secret file we read; secrets are tokens and passwords, not data.
const MAX_SECRET_BYTES: u64 = 4096;

/// A resolved secret. `Debug` never shows the value.
#[derive(Clone, PartialEq)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// Resolve `inline` / `file` (already resolved against the config dir) into a
/// secret. Warnings (e.g. world-readable files) are appended to `warnings`.
pub fn resolve(
    key: &str,
    inline: Option<&str>,
    file: Option<&Path>,
    warnings: &mut Vec<String>,
) -> Result<Option<Secret>, String> {
    match (inline, file) {
        (Some(_), Some(_)) => Err(format!("set either {key} or {key}_file, not both")),
        (Some(value), None) => {
            if value.is_empty() {
                return Err(format!("{key} is empty"));
            }
            Ok(Some(Secret(value.to_string())))
        }
        (None, Some(path)) => read_secret_file(key, path, warnings).map(Some),
        (None, None) => Ok(None),
    }
}

fn read_secret_file(key: &str, path: &Path, warnings: &mut Vec<String>) -> Result<Secret, String> {
    let meta = fs::metadata(path).map_err(|e| format!("{key}_file {}: {e}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_SECRET_BYTES {
        return Err(format!(
            "{key}_file {} must be a regular file of at most {MAX_SECRET_BYTES} bytes",
            path.display()
        ));
    }
    if is_world_readable(&meta) {
        warnings.push(format!(
            "{key}_file {} is readable by all users (chmod o-r)",
            path.display()
        ));
    }
    let text =
        fs::read_to_string(path).map_err(|e| format!("{key}_file {}: {e}", path.display()))?;
    let value = text.trim();
    if value.is_empty() {
        return Err(format!("{key}_file {} is empty", path.display()));
    }
    Ok(Secret(value.to_string()))
}

pub fn is_world_readable(meta: &fs::Metadata) -> bool {
    meta.permissions().mode() & 0o004 != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestDir;

    #[test]
    fn inline_and_file() {
        let dir = TestDir::new("secret");
        let file = dir.write("token", "  s3cret\n");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        let mut warnings = Vec::new();

        let inline = resolve("token", Some("abc"), None, &mut warnings).unwrap();
        assert_eq!(inline.unwrap().expose(), "abc");
        let from_file = resolve("token", None, Some(&file), &mut warnings).unwrap();
        assert_eq!(from_file.unwrap().expose(), "s3cret");
        assert_eq!(resolve("token", None, None, &mut warnings).unwrap(), None);
        assert!(warnings.is_empty());
    }

    #[test]
    fn errors_and_warnings() {
        let dir = TestDir::new("secret-err");
        let file = dir.write("token", "x");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        let empty = dir.write("empty", " \n");
        let mut warnings = Vec::new();

        assert!(resolve("token", Some("a"), Some(&file), &mut warnings).is_err());
        assert!(resolve("token", Some(""), None, &mut warnings).is_err());
        assert!(resolve("token", None, Some(&empty), &mut warnings).is_err());
        assert!(
            resolve(
                "token",
                None,
                Some(&dir.path().join("missing")),
                &mut warnings
            )
            .is_err()
        );
        assert!(resolve("token", None, Some(dir.path()), &mut warnings).is_err());

        warnings.clear();
        resolve("token", None, Some(&file), &mut warnings).unwrap();
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn debug_hides_value() {
        assert_eq!(format!("{:?}", Secret("pw".into())), "Secret(***)");
    }
}
