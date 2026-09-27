//! Machine keys on the wire: the one form of a public key the host takes, and the way a key file
//! becomes that form. Both sides use this module, so the key the engine sends is checked by the
//! rule the host applies.

/// The only machine key type accepted.
pub const ED25519: &str = "ssh-ed25519";
/// Bytes of an ed25519 public key.
const ED25519_KEY_BYTES: usize = 32;

/// `ssh-ed25519 <base64>` and nothing else: one line, no comment, and a blob that is exactly an
/// ed25519 key — the line becomes a line of `authorized_keys`, where anything more is an option.
pub fn is_ed25519_line(line: &str) -> bool {
    if line.chars().any(char::is_control) {
        return false;
    }
    let Some(encoded) = line
        .strip_prefix(ED25519)
        .and_then(|rest| rest.strip_prefix(' '))
    else {
        return false;
    };
    let Some(blob) = base64(encoded) else {
        return false;
    };
    // The wire form: a string naming the type, then a string holding the key.
    let mut expected = Vec::with_capacity(4 + ED25519.len() + 4 + ED25519_KEY_BYTES);
    expected.extend_from_slice(&u32::try_from(ED25519.len()).unwrap_or(0).to_be_bytes());
    expected.extend_from_slice(ED25519.as_bytes());
    expected.extend_from_slice(&u32::try_from(ED25519_KEY_BYTES).unwrap_or(0).to_be_bytes());
    blob.len() == expected.len() + ED25519_KEY_BYTES && blob.starts_with(&expected)
}

/// Standard base64 with padding, strictly: no spaces, no line breaks, no missing padding.
fn base64(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = text.as_bytes();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }
    let padding = bytes.iter().rev().take_while(|byte| **byte == b'=').count();
    if padding > 2 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in bytes.get(..bytes.len() - padding)? {
        let value = ALPHABET.iter().position(|candidate| candidate == byte)?;
        buffer = (buffer << 6) | u32::try_from(value).ok()?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// The key's bytes, whatever surrounds them: two lines that differ only in spacing or a comment
/// carry the same key.
#[must_use]
pub fn key_blob(line: &str) -> Option<Vec<u8>> {
    line.strip_prefix(ED25519)
        .and_then(|rest| base64(rest.trim_start().split(' ').next().unwrap_or_default()))
}

/// The wire form of a public key file as `ssh-keygen` writes it: the type and the key, without the
/// comment and the line end. `None` when what is left is not an ed25519 key.
#[must_use]
pub fn wire_line(key_file: &str) -> Option<String> {
    let mut fields = key_file.split_whitespace();
    let line = format!("{} {}", fields.next()?, fields.next()?);
    is_ed25519_line(&line).then_some(line)
}
