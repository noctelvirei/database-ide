//! Private cell tags and little-endian payloads; row boundaries live only in the index.

use std::io::{self, Read, Write};

use crate::session::Value;
use crate::session::temporal::{Date, Time, TimeWithZone, Timestamp, TimestampWithZone};

/// Writes one tagged cell without normalizing text or floating-point bits.
pub(super) fn write(writer: &mut impl Write, value: &Value) -> io::Result<()> {
    let tag = match value {
        Value::Null => 0,
        Value::Bool(false) => 1,
        Value::Bool(true) => 2,
        Value::Integer(_) => 3,
        Value::UnsignedInteger(_) => 4,
        Value::Float(_) => 5,
        Value::Decimal(_) => 6,
        Value::Text(_) => 7,
        Value::Bytes(_) => 8,
        Value::Date(_) => 9,
        Value::Time(_) => 10,
        Value::TimeWithZone(_) => 11,
        Value::Timestamp(_) => 12,
        Value::TimestampWithZone(_) => 13,
        Value::Uuid(_) => 14,
        Value::Json(_) => 15,
        Value::Other(_) => 16,
    };
    writer.write_all(&[tag])?;
    match value {
        Value::Null | Value::Bool(_) => Ok(()),
        Value::Integer(value) => writer.write_all(&value.to_le_bytes()),
        Value::UnsignedInteger(value) => writer.write_all(&value.to_le_bytes()),
        Value::Float(value) => writer.write_all(&value.to_bits().to_le_bytes()),
        Value::Uuid(value) => writer.write_all(value),
        Value::Bytes(value) => write_bytes(writer, value),
        Value::Decimal(value) | Value::Text(value) | Value::Json(value) | Value::Other(value) => {
            write_bytes(writer, value.as_bytes())
        }
        Value::Date(value) => write_bytes(writer, value.as_str().as_bytes()),
        Value::Time(value) => write_bytes(writer, value.as_str().as_bytes()),
        Value::TimeWithZone(value) => write_bytes(writer, value.as_str().as_bytes()),
        Value::Timestamp(value) => write_bytes(writer, value.as_str().as_bytes()),
        Value::TimestampWithZone(value) => write_bytes(writer, value.as_str().as_bytes()),
    }
}

fn write_bytes(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    writer.write_all(&(bytes.len() as u64).to_le_bytes())?;
    writer.write_all(bytes)
}

fn fixed<const N: usize>(reader: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid spill cell")
}

fn bytes(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let length = u64::from_le_bytes(fixed(reader)?);
    let mut bytes = Vec::new();
    reader.take(length).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != length {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(bytes)
}

fn text(reader: &mut impl Read) -> io::Result<String> {
    String::from_utf8(bytes(reader)?).map_err(|_| invalid())
}

/// Reads one tagged cell; rejects truncated payloads, invalid UTF-8 and temporal text.
pub(super) fn read(reader: &mut impl Read) -> io::Result<Value> {
    Ok(match fixed::<1>(reader)?[0] {
        0 => Value::Null,
        1 => Value::Bool(false),
        2 => Value::Bool(true),
        3 => Value::Integer(i64::from_le_bytes(fixed(reader)?)),
        4 => Value::UnsignedInteger(u64::from_le_bytes(fixed(reader)?)),
        5 => Value::Float(f64::from_bits(u64::from_le_bytes(fixed(reader)?))),
        6 => Value::Decimal(text(reader)?),
        7 => Value::Text(text(reader)?),
        8 => Value::Bytes(bytes(reader)?),
        9 => Value::Date(Date::parse(&text(reader)?).map_err(|_| invalid())?),
        10 => Value::Time(Time::parse(&text(reader)?).map_err(|_| invalid())?),
        11 => Value::TimeWithZone(TimeWithZone::parse(&text(reader)?).map_err(|_| invalid())?),
        12 => Value::Timestamp(Timestamp::parse(&text(reader)?).map_err(|_| invalid())?),
        13 => Value::TimestampWithZone(
            TimestampWithZone::parse(&text(reader)?).map_err(|_| invalid())?,
        ),
        14 => Value::Uuid(fixed(reader)?),
        15 => Value::Json(text(reader)?),
        16 => Value::Other(text(reader)?),
        _ => return Err(invalid()),
    })
}
