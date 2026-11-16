//! PostgreSQL text values; unsupported or out-of-range values retain their exact text.

use tokio_postgres::{SimpleColumn, types::Type};

use crate::session::{Column, TypeCategory, Value, temporal};

/// The fork supplies no provenance or nullability; unknown type names retain their OID.
pub(super) fn column(column: &SimpleColumn) -> Column {
    let oid = column.type_oid();
    Column {
        name: column.name().into(),
        type_name: Type::from_oid(oid).map_or_else(|| format!("oid:{oid}"), |ty| ty.name().into()),
        category: category(oid),
        nullable: None,
        base_table: None,
        base_column: None,
    }
}

fn category(oid: u32) -> TypeCategory {
    match oid {
        16 => TypeCategory::Bool,
        20 | 21 | 23 | 26 => TypeCategory::Integer,
        1700 => TypeCategory::Decimal,
        700 | 701 => TypeCategory::Float,
        18 | 19 | 25 | 1042 | 1043 => TypeCategory::Text,
        17 => TypeCategory::Bytes,
        1082 => TypeCategory::Date,
        1083 | 1266 => TypeCategory::Time,
        1114 | 1184 => TypeCategory::Timestamp,
        2950 => TypeCategory::Uuid,
        114 | 3802 => TypeCategory::Json,
        _ => TypeCategory::Other,
    }
}

/// Converts text only when representable, preserving unsupported values verbatim.
pub(super) fn value(oid: u32, text: Option<&str>) -> Value {
    let Some(text) = text else {
        return Value::Null;
    };
    let converted = match oid {
        16 => match text {
            "t" => Some(Value::Bool(true)),
            "f" => Some(Value::Bool(false)),
            _ => None,
        },
        20 | 21 | 23 | 26 => text.parse().ok().map(Value::Integer),
        1700 => Some(Value::Decimal(text.into())),
        700 | 701 => match text {
            "Infinity" => Some(Value::Float(f64::INFINITY)),
            "-Infinity" => Some(Value::Float(f64::NEG_INFINITY)),
            _ => text.parse().ok().map(Value::Float),
        },
        18 | 19 | 25 | 1042 | 1043 => Some(Value::Text(text.into())),
        17 => bytea(text).map(Value::Bytes),
        1082 => temporal::Date::parse(text).ok().map(Value::Date),
        1083 => temporal::Time::parse(text).ok().map(Value::Time),
        1266 => temporal::TimeWithZone::parse(&zone(text))
            .ok()
            .map(Value::TimeWithZone),
        1114 => temporal::Timestamp::parse(&text.replacen(' ', "T", 1))
            .ok()
            .map(Value::Timestamp),
        1184 => temporal::TimestampWithZone::parse(&zone(&text.replacen(' ', "T", 1)))
            .ok()
            .map(Value::TimestampWithZone),
        2950 => hex(&text.replace('-', ""))
            .and_then(|bytes| bytes.try_into().ok())
            .map(Value::Uuid),
        114 | 3802 => Some(Value::Json(text.into())),
        _ => None,
    };
    converted.unwrap_or_else(|| Value::Other(text.into()))
}

fn zone(text: &str) -> String {
    let offset = text.rfind(['+', '-']).map(|index| &text[index..]);
    if offset.is_some_and(|offset| offset.len() == 3) {
        format!("{text}:00")
    } else {
        text.into()
    }
}

fn hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            Some((high * 16 + low) as u8)
        })
        .collect()
}

fn bytea(text: &str) -> Option<Vec<u8>> {
    if let Some(text) = text.strip_prefix("\\x") {
        return hex(text);
    }
    let mut bytes = Vec::new();
    let mut input = text.bytes();
    while let Some(byte) = input.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        let next = input.next()?;
        if next == b'\\' {
            bytes.push(b'\\');
            continue;
        }
        let digits = [next, input.next()?, input.next()?];
        if !(b'0'..=b'3').contains(&digits[0])
            || !digits[1..]
                .iter()
                .all(|digit| (b'0'..=b'7').contains(digit))
        {
            return None;
        }
        bytes.push((digits[0] - b'0') * 64 + (digits[1] - b'0') * 8 + (digits[2] - b'0'));
    }
    Some(bytes)
}
