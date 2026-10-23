//! Small encodings: standard base64 (HTTP Basic Auth), base64url (tokens)
//! and the base64 variant of bcrypt hashes.

const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
/// bcrypt uses its own alphabet and no padding.
const BCRYPT: &[u8; 64] = b"./ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

fn encode_with(table: &[u8; 64], input: &[u8], pad: bool) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(table[(n >> (18 - 6 * i)) as usize & 63]));
            } else if pad {
                out.push('=');
            }
        }
    }
    out
}

fn decode_with(table: &[u8; 64], input: &str) -> Option<Vec<u8>> {
    let value = |c: u8| table.iter().position(|&t| t == c).map(|p| p as u32);
    let bytes = input.as_bytes();
    if bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3 + 2);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i);
        }
        let len = chunk.len() - 1;
        out.extend_from_slice(&n.to_be_bytes()[1..1 + len]);
    }
    Some(out)
}

pub fn base64(input: &[u8]) -> String {
    encode_with(STANDARD, input, true)
}

/// Decode standard base64 with padding.
pub fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let trimmed = input.trim_end_matches('=');
    if input.len() % 4 != 0 || input.len() - trimmed.len() > 2 {
        return None;
    }
    decode_with(STANDARD, trimmed)
}

pub fn base64url(input: &[u8]) -> String {
    encode_with(URL_SAFE, input, false)
}

pub fn bcrypt_encode(input: &[u8]) -> String {
    encode_with(BCRYPT, input, false)
}

pub fn bcrypt_decode(input: &str) -> Option<Vec<u8>> {
    decode_with(BCRYPT, input)
}

/// A JSON string literal (with quotes).
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Comparison whose time does not depend on where the inputs differ.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_vectors() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(plain.as_bytes()), encoded);
            assert_eq!(base64_decode(encoded).unwrap(), plain.as_bytes());
        }
        for bad in ["Zg", "Zg=", "Z===", "Zm9v!", "Zm9vY", "====", "Zg==Zg=="] {
            assert_eq!(base64_decode(bad), None, "{bad}");
        }
    }

    #[test]
    fn url_safe_has_no_padding() {
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn bcrypt_round_trip() {
        let salt: Vec<u8> = (0u8..16).collect();
        let encoded = bcrypt_encode(&salt);
        assert_eq!(encoded.len(), 22);
        assert_eq!(&bcrypt_decode(&encoded).unwrap()[..16], &salt[..]);
    }

    #[test]
    fn constant_time() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
