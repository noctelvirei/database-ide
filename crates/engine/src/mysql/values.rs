//! Column definitions and lossless text-protocol cell conversion.

use mysql_async::consts::{ColumnFlags, ColumnType};

use crate::session::{Column, TypeCategory, Value, temporal};

/// Uses original names for provenance and the binary character set to distinguish blobs.
pub(super) fn column(source: &mysql_async::Column) -> Column {
    let table = source.org_table_str();
    let name = source.org_name_str();
    let schema = source.schema_str();
    let (type_name, category) = kind(source);
    Column {
        name: source.name_str().into_owned(),
        type_name: type_name.into(),
        category,
        nullable: Some(!source.flags().contains(ColumnFlags::NOT_NULL_FLAG)),
        base_table: (!table.is_empty() && !name.is_empty()).then(|| {
            if schema.is_empty() {
                table.into_owned()
            } else {
                format!("{schema}.{table}")
            }
        }),
        base_column: (!name.is_empty()).then(|| name.into_owned()),
    }
}

fn kind(column: &mysql_async::Column) -> (&'static str, TypeCategory) {
    use ColumnType::*;
    use TypeCategory as C;
    let binary = column.character_set() == 63;
    match column.column_type() {
        MYSQL_TYPE_TINY => ("tinyint", C::Integer),
        MYSQL_TYPE_SHORT => ("smallint", C::Integer),
        MYSQL_TYPE_INT24 => ("mediumint", C::Integer),
        MYSQL_TYPE_LONG => ("int", C::Integer),
        MYSQL_TYPE_LONGLONG => ("bigint", C::Integer),
        MYSQL_TYPE_YEAR => ("year", C::Integer),
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => ("decimal", C::Decimal),
        MYSQL_TYPE_FLOAT => ("float", C::Float),
        MYSQL_TYPE_DOUBLE => ("double", C::Float),
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => ("date", C::Date),
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => ("time", C::Time),
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 => ("datetime", C::Timestamp),
        MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => ("timestamp", C::Timestamp),
        MYSQL_TYPE_JSON => ("json", C::Json),
        MYSQL_TYPE_BIT => ("bit", C::Bytes),
        MYSQL_TYPE_ENUM => ("enum", C::Text),
        MYSQL_TYPE_SET => ("set", C::Text),
        MYSQL_TYPE_STRING if column.flags().contains(ColumnFlags::ENUM_FLAG) => ("enum", C::Text),
        MYSQL_TYPE_STRING if column.flags().contains(ColumnFlags::SET_FLAG) => ("set", C::Text),
        MYSQL_TYPE_STRING if binary => ("binary", C::Bytes),
        MYSQL_TYPE_STRING => ("char", C::Text),
        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING if binary => ("varbinary", C::Bytes),
        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING => ("varchar", C::Text),
        MYSQL_TYPE_TINY_BLOB if binary => ("tinyblob", C::Bytes),
        MYSQL_TYPE_MEDIUM_BLOB if binary => ("mediumblob", C::Bytes),
        MYSQL_TYPE_LONG_BLOB if binary => ("longblob", C::Bytes),
        MYSQL_TYPE_BLOB if binary => ("blob", C::Bytes),
        MYSQL_TYPE_TINY_BLOB => ("tinytext", C::Text),
        MYSQL_TYPE_MEDIUM_BLOB => ("mediumtext", C::Text),
        MYSQL_TYPE_LONG_BLOB => ("longtext", C::Text),
        MYSQL_TYPE_BLOB => ("text", C::Text),
        MYSQL_TYPE_NULL => ("null", C::Other),
        MYSQL_TYPE_GEOMETRY => ("geometry", C::Other),
        MYSQL_TYPE_VECTOR => ("vector", C::Other),
        _ => ("unknown", C::Other),
    }
}

/// Text-protocol values are bytes or NULL; conversion follows column type, never inference.
pub(super) fn value(column: &mysql_async::Column, value: mysql_async::Value) -> Value {
    let bytes = match value {
        mysql_async::Value::NULL => return Value::Null,
        mysql_async::Value::Bytes(bytes) => bytes,
        _ => unreachable!("text-protocol rows contain only bytes or NULL"),
    };
    let (_, category) = kind(column);
    if category == TypeCategory::Bytes {
        return Value::Bytes(bytes);
    }
    let text = String::from_utf8_lossy(&bytes);
    let converted = match category {
        TypeCategory::Integer if column.flags().contains(ColumnFlags::UNSIGNED_FLAG) => {
            text.parse().ok().map(Value::UnsignedInteger)
        }
        TypeCategory::Integer => text.parse().ok().map(Value::Integer),
        TypeCategory::Decimal => Some(Value::Decimal(text.to_string())),
        TypeCategory::Float => text.parse().ok().map(Value::Float),
        TypeCategory::Text => Some(Value::Text(text.to_string())),
        TypeCategory::Json => Some(Value::Json(text.to_string())),
        TypeCategory::Date => temporal::Date::parse(&text).ok().map(Value::Date),
        TypeCategory::Time => temporal::Time::parse(&text).ok().map(Value::Time),
        TypeCategory::Timestamp => temporal::Timestamp::parse(&text.replacen(' ', "T", 1))
            .ok()
            .map(Value::Timestamp),
        _ => None,
    };
    converted.unwrap_or_else(|| Value::Other(text.into_owned()))
}
