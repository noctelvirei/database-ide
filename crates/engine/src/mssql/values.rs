//! TDS column descriptions and lossless engine values.

use mssql_tds::datatypes::column_values::ColumnValues;
use mssql_tds::datatypes::sqldatatypes::TdsDataType as T;
use mssql_tds::query::metadata::ColumnMetadata;

use crate::session::{Column, TypeCategory as C, Value, temporal};

/// Preserves wire nullability and table provenance; the driver has no base-column field.
pub(super) fn column(source: &ColumnMetadata) -> Column {
    let length = source.type_info.length;
    let (name, category) = match source.data_type {
        T::IntN => (
            match length {
                1 => "tinyint",
                2 => "smallint",
                4 => "int",
                _ => "bigint",
            },
            C::Integer,
        ),
        T::BitN => ("bit", C::Bool),
        T::FltN => (if length == 4 { "real" } else { "float" }, C::Float),
        T::MoneyN => (if length == 4 { "smallmoney" } else { "money" }, C::Decimal),
        T::DateTimeN => (
            if length == 4 {
                "smalldatetime"
            } else {
                "datetime"
            },
            C::Timestamp,
        ),
        ty => (
            ty.get_meta_type_name().unwrap_or("unknown"),
            match ty {
                T::Int1 | T::Int2 | T::Int4 | T::Int8 => C::Integer,
                T::Bit => C::Bool,
                T::Flt4 | T::Flt8 => C::Float,
                T::Decimal | T::DecimalN | T::Numeric | T::NumericN | T::Money | T::Money4 => {
                    C::Decimal
                }
                T::Char
                | T::BigChar
                | T::VarChar
                | T::BigVarChar
                | T::NChar
                | T::NVarChar
                | T::Text
                | T::NText
                | T::Xml
                | T::Json => C::Text,
                T::Binary | T::BigBinary | T::VarBinary | T::BigVarBinary | T::Image => C::Bytes,
                T::DateN => C::Date,
                T::TimeN => C::Time,
                T::DateTime | T::DateTim4 | T::DateTime2N | T::DateTimeOffsetN => C::Timestamp,
                T::Guid => C::Uuid,
                _ => C::Other,
            },
        ),
    };
    let base_table = source.multi_part_name.as_ref().map(|table| {
        [
            table.server_name(),
            table.catalog_name(),
            table.schema_name(),
            Some(table.table_name()),
        ]
        .into_iter()
        .flatten()
        .map(|part| format!("[{}]", part.replace(']', "]]")))
        .collect::<Vec<_>>()
        .join(".")
    });
    Column {
        name: source.column_name.clone(),
        type_name: name.into(),
        category,
        nullable: Some(source.is_nullable()),
        base_table,
        base_column: None,
    }
}

/// Converts exact numerics without float intermediates; UUID bytes are already normalized by TDS.
pub(super) fn value(value: ColumnValues, data_type: T) -> Value {
    if data_type == T::SsVariant && value != ColumnValues::Null {
        return Value::Other(other(value));
    }
    match value {
        ColumnValues::Null => Value::Null,
        ColumnValues::TinyInt(n) => Value::Integer(n.into()),
        ColumnValues::SmallInt(n) => Value::Integer(n.into()),
        ColumnValues::Int(n) => Value::Integer(n.into()),
        ColumnValues::BigInt(n) => Value::Integer(n),
        ColumnValues::Real(n) => Value::Float(n.into()),
        ColumnValues::Float(n) => Value::Float(n),
        ColumnValues::Bit(n) => Value::Bool(n),
        ColumnValues::Decimal(n) | ColumnValues::Numeric(n) => Value::Decimal(n.to_string()),
        ColumnValues::SmallMoney(n) => Value::Decimal(money(n.int_val.into())),
        ColumnValues::Money(n) => Value::Decimal(money(
            (i64::from(n.msb_part) << 32) | i64::from(n.lsb_part as u32),
        )),
        ColumnValues::String(s) => Value::Text(s.to_utf8_string()),
        ColumnValues::Bytes(bytes) if matches!(data_type, T::Udt | T::Vector) => {
            Value::Other(format!("{bytes:02x?}"))
        }
        ColumnValues::Bytes(bytes) => Value::Bytes(bytes),
        ColumnValues::Xml(xml) => {
            Value::Text(xml.as_string().trim_start_matches('\u{feff}').into())
        }
        ColumnValues::Json(json) => Value::Text(json.as_string()),
        ColumnValues::Uuid(uuid) => Value::Uuid(*uuid.as_bytes()),
        ColumnValues::Date(date) => date_value(date.get_days().into()),
        ColumnValues::Time(time) => {
            let text = clock(time.time_nanoseconds * 100, time.scale);
            temporal::Time::parse(&text)
                .map(Value::Time)
                .unwrap_or(Value::Other(text))
        }
        ColumnValues::DateTime2(date) => timestamp(
            date.days.into(),
            date.time.time_nanoseconds * 100,
            date.time.scale,
            None,
        ),
        ColumnValues::DateTimeOffset(date) => {
            let nanos = i128::from(date.datetime2.time.time_nanoseconds) * 100
                + i128::from(date.offset) * 60_000_000_000;
            let days = i64::from(date.datetime2.days) + nanos.div_euclid(86_400_000_000_000) as i64;
            timestamp(
                days,
                nanos.rem_euclid(86_400_000_000_000) as u64,
                date.datetime2.time.scale,
                Some(date.offset),
            )
        }
        ColumnValues::SmallDateTime(date) => timestamp(
            i64::from(date.days) + 693595,
            u64::from(date.time) * 60_000_000_000,
            0,
            None,
        ),
        ColumnValues::DateTime(date) => {
            let millis = (u64::from(date.time) * 1000 + 150) / 300;
            timestamp(i64::from(date.days) + 693595, millis * 1_000_000, 3, None)
        }
        value => Value::Other(other(value)),
    }
}

fn other(value: ColumnValues) -> String {
    match value {
        ColumnValues::String(s) => s.to_utf8_string(),
        value => format!("{value:?}"),
    }
}

fn money(scaled: i64) -> String {
    let magnitude = scaled.unsigned_abs();
    format!(
        "{}{}.{:04}",
        if scaled < 0 { "-" } else { "" },
        magnitude / 10000,
        magnitude % 10000
    )
}

fn clock(nanos: u64, scale: u8) -> String {
    let seconds = nanos / 1_000_000_000;
    let mut text = format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    if scale > 0 {
        text.push_str(&format!(
            ".{:0width$}",
            nanos % 1_000_000_000 / 10u64.pow(9 - u32::from(scale.min(9))),
            width = scale.min(9).into()
        ));
    }
    text
}

/// The driver exposes day counts but no calendar conversion. Gregorian 400-year eras avoid loops.
fn date_text(days: i64) -> String {
    let z = days + 306;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    format!("{:04}-{month:02}-{day:02}", year + i64::from(month <= 2))
}

fn date_value(days: i64) -> Value {
    let text = date_text(days);
    temporal::Date::parse(&text)
        .map(Value::Date)
        .unwrap_or(Value::Other(text))
}

fn timestamp(days: i64, nanos: u64, scale: u8, offset: Option<i16>) -> Value {
    let mut text = format!("{}T{}", date_text(days), clock(nanos, scale));
    if let Some(offset) = offset {
        text.push_str(&format!(
            "{}{:02}:{:02}",
            if offset < 0 { '-' } else { '+' },
            offset.unsigned_abs() / 60,
            offset.unsigned_abs() % 60
        ));
        temporal::TimestampWithZone::parse(&text)
            .map(Value::TimestampWithZone)
            .unwrap_or(Value::Other(text))
    } else {
        temporal::Timestamp::parse(&text)
            .map(Value::Timestamp)
            .unwrap_or(Value::Other(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_days_cover_sql_server_bounds_and_leap_day() {
        assert_eq!(date_text(0), "0001-01-01");
        assert_eq!(date_text(693595), "1900-01-01");
        assert_eq!(date_text(738944), "2024-02-29");
        assert_eq!(date_text(3652058), "9999-12-31");
    }

    #[test]
    fn money_keeps_both_extremes_and_negative_fraction() {
        assert_eq!(money(i64::MIN), "-922337203685477.5808");
        assert_eq!(money(i64::MAX), "922337203685477.5807");
        assert_eq!(money(-1), "-0.0001");
    }

    #[test]
    fn driver_time_ticks_preserve_every_scale() {
        use mssql_tds::datatypes::column_values::SqlTime;

        for (ticks, scale, expected) in [
            (452960000000, 0, "12:34:56"),
            (452961000000, 1, "12:34:56.1"),
            (452961200000, 2, "12:34:56.12"),
            (452961230000, 3, "12:34:56.123"),
            (452961234000, 4, "12:34:56.1234"),
            (452961234500, 5, "12:34:56.12345"),
            (452961234560, 6, "12:34:56.123456"),
            (452961234567, 7, "12:34:56.1234567"),
        ] {
            assert_eq!(
                value(
                    ColumnValues::Time(SqlTime {
                        time_nanoseconds: ticks,
                        scale
                    }),
                    T::TimeN
                ),
                Value::Time(temporal::Time::parse(expected).unwrap())
            );
        }
    }

    #[test]
    fn driver_timestamps_preserve_bounds_and_offsets_across_midnight() {
        use mssql_tds::datatypes::column_values::{SqlDateTime2, SqlDateTimeOffset, SqlTime};

        for (days, ticks, expected) in [
            (0, 0, "0001-01-01T00:00:00.0000000"),
            (3652058, 863999999999, "9999-12-31T23:59:59.9999999"),
        ] {
            assert_eq!(
                value(
                    ColumnValues::DateTime2(SqlDateTime2 {
                        days,
                        time: SqlTime {
                            time_nanoseconds: ticks,
                            scale: 7
                        }
                    }),
                    T::DateTime2N
                ),
                Value::Timestamp(temporal::Timestamp::parse(expected).unwrap())
            );
        }
        for (days, ticks, offset, expected) in [
            (
                738943,
                675001234567,
                330,
                "2024-02-29T00:15:00.1234567+05:30",
            ),
            (
                738944,
                243001234567,
                -420,
                "2024-02-28T23:45:00.1234567-07:00",
            ),
            (738944, 1234567, 0, "2024-02-29T00:00:00.1234567+00:00"),
        ] {
            assert_eq!(
                value(
                    ColumnValues::DateTimeOffset(SqlDateTimeOffset {
                        datetime2: SqlDateTime2 {
                            days,
                            time: SqlTime {
                                time_nanoseconds: ticks,
                                scale: 7
                            }
                        },
                        offset
                    }),
                    T::DateTimeOffsetN
                ),
                Value::TimestampWithZone(temporal::TimestampWithZone::parse(expected).unwrap())
            );
        }
    }
}
