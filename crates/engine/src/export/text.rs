//! Lossless display text shared by text formats; binary and UUID values use lowercase hex.

use std::fmt::Write;

use crate::session::Value;

/// Lowercase hexadecimal without a prefix, shared with SQL literal encoding.
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").expect("formatting into a String cannot fail");
    }
    text
}

/// Lossless text representation before a destination applies its own escaping.
pub(crate) fn cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Integer(value) => value.to_string(),
        Value::UnsignedInteger(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::Decimal(value) | Value::Text(value) | Value::Json(value) | Value::Other(value) => {
            value.clone()
        }
        Value::Bytes(value) => format!("0x{}", hex(value)),
        Value::Uuid(value) => {
            let hex = hex(value);
            format!(
                "{}-{}-{}-{}-{}",
                &hex[..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..]
            )
        }
        Value::Date(value) => value.as_str().into(),
        Value::Time(value) => value.as_str().into(),
        Value::TimeWithZone(value) => value.as_str().into(),
        Value::Timestamp(value) => value.as_str().into(),
        Value::TimestampWithZone(value) => value.as_str().into(),
    }
}
