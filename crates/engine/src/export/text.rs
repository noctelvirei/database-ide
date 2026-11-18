//! Lowercase hexadecimal shared by display text and SQL literal encoding.

use std::fmt::Write;

/// Lowercase hexadecimal without a prefix, shared with SQL literal encoding.
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").expect("formatting into a String cannot fail");
    }
    text
}
