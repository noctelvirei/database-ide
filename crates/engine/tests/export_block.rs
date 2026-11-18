use dbide_engine::export;
use dbide_engine::session::temporal::{Date, Time, TimeWithZone, Timestamp, TimestampWithZone};
use dbide_engine::session::{Column, TypeCategory, Value};
use dbide_engine::spill::Store;

fn store(names: &[&str], rows: Vec<Vec<Value>>) -> Store {
    let columns = names
        .iter()
        .map(|name| Column {
            name: (*name).into(),
            type_name: "text".into(),
            category: TypeCategory::Text,
            nullable: Some(true),
            base_table: None,
            base_column: None,
        })
        .collect();
    let mut store = Store::new(columns, None).unwrap();
    store.append(rows).unwrap();
    store
}

#[test]
fn export_block_selects_middle_rows_and_columns_with_optional_headers() {
    let mut store = store(
        &["outside", "a", "b", "outside"],
        (0..5)
            .map(|i| {
                vec![
                    Value::Null,
                    Value::Integer(i),
                    Value::Integer(i + 10),
                    Value::Null,
                ]
            })
            .collect(),
    );
    store.complete().unwrap();
    let reader = store.reader();
    assert_eq!(
        export::clipboard_block(&reader, 1..4, 1..3, true, 3, 100).unwrap(),
        "a\tb\r\n1\t11\r\n2\t12\r\n3\t13\r\n"
    );
    assert_eq!(
        export::clipboard_block(&reader, 1..4, 1..3, false, 3, 100).unwrap(),
        "1\t11\r\n2\t12\r\n3\t13\r\n"
    );
}

#[test]
fn export_block_quotes_headers_tabs_quotes_and_line_breaks_and_leaves_null_empty() {
    let store = store(
        &["a\tb", "null", "quote", "newline", "return"],
        vec![vec![
            Value::Text("one\ttwo".into()),
            Value::Null,
            Value::Text("a\"b".into()),
            Value::Text("a\nb".into()),
            Value::Text("a\rb".into()),
        ]],
    );
    assert_eq!(
        export::clipboard_block(&store.reader(), 0..1, 0..5, true, 1, 100).unwrap(),
        "\"a\tb\"\tnull\tquote\tnewline\treturn\r\n\"one\ttwo\"\t\t\"a\"\"b\"\t\"a\nb\"\t\"a\rb\"\r\n"
    );
}

#[test]
fn export_block_clamps_to_published_rows_and_columns_before_enforcing_limits() {
    let store = store(
        &["a", "b"],
        vec![vec![Value::Integer(1), Value::Integer(2)]],
    );
    assert!(!store.reader().is_complete().unwrap());
    assert_eq!(
        export::clipboard_block(&store.reader(), 0..usize::MAX, 1..usize::MAX, true, 1, 100)
            .unwrap(),
        "b\r\n2\r\n"
    );
    assert!(matches!(
        export::clipboard(&store.reader(), 1, 100),
        Err(export::Error::IncompleteResult)
    ));
}

#[test]
fn export_block_refuses_selected_row_count_and_encoded_byte_limits() {
    let store = store(&["a"], vec![vec![Value::Text("é\"".into())]; 3]);
    let reader = store.reader();
    assert_eq!(
        export::clipboard_block(&reader, 1..3, 0..1, false, 1, 100)
            .unwrap_err()
            .to_string(),
        "Clipboard row limit exceeded (1 rows)"
    );
    assert_eq!(
        export::clipboard_block(&reader, 1..2, 0..1, true, 1, 10)
            .unwrap_err()
            .to_string(),
        "Clipboard byte limit exceeded (10 bytes)"
    );
    assert_eq!(
        export::clipboard_block(&reader, 1..2, 0..1, true, 1, 11).unwrap(),
        "a\r\n\"é\"\"\"\r\n"
    );
    assert_eq!(
        export::clipboard_block(&reader, 1..2, 0..1, false, 1, 8).unwrap(),
        "\"é\"\"\"\r\n"
    );
    assert!(export::clipboard_block(&reader, 1..2, 0..1, false, 1, 7).is_err());
    assert!(export::clipboard_block(&reader, 0..0, 0..1, true, 0, 2).is_err());
}

#[test]
fn export_block_empty_and_reversed_ranges_do_not_panic() {
    let store = store(&["a"], vec![vec![Value::Integer(1)]]);
    let reader = store.reader();
    let reversed = std::ops::Range { start: 2, end: 0 };
    assert_eq!(
        export::clipboard_block(&reader, reversed.clone(), 0..1, true, 0, 3).unwrap(),
        "a\r\n"
    );
    assert_eq!(
        export::clipboard_block(&reader, 2..usize::MAX, 0..1, false, 0, 0).unwrap(),
        ""
    );
    assert_eq!(
        export::clipboard_block(&reader, 0..1, reversed, false, 1, 2).unwrap(),
        "\r\n"
    );
    assert_eq!(
        export::clipboard_block(&reader, 0..1, 1..usize::MAX, true, 1, 4).unwrap(),
        "\r\n\r\n"
    );
    assert_eq!(
        export::clipboard_block(&reader, 0..0, 0..1, false, 0, 0).unwrap(),
        ""
    );
}

#[test]
fn export_clipboard_preserves_empty_records_for_zero_column_rows() {
    let mut store = store(&[], vec![vec![], vec![]]);
    store.complete().unwrap();
    assert_eq!(
        export::clipboard(&store.reader(), 2, 6).unwrap(),
        "\r\n\r\n\r\n"
    );
}

#[test]
fn export_block_stops_at_selected_end_across_pages() {
    let store = store(&["a"], vec![vec![Value::Integer(7)]; export::PAGE_ROWS + 4]);
    let text = export::clipboard_block(
        &store.reader(),
        1..export::PAGE_ROWS + 2,
        0..1,
        false,
        export::PAGE_ROWS + 1,
        usize::MAX,
    )
    .unwrap();
    assert_eq!(text, "7\r\n".repeat(export::PAGE_ROWS + 1));
}

#[test]
fn export_cell_text_matches_text_exports_for_every_value_variant() {
    let cases = [
        (Value::Null, ""),
        (Value::Bool(true), "true"),
        (Value::Integer(i64::MIN), "-9223372036854775808"),
        (Value::UnsignedInteger(u64::MAX), "18446744073709551615"),
        (Value::Decimal("123.4500".into()), "123.4500"),
        (Value::Float(1.25), "1.25"),
        (Value::Text("a\t\"b\"\nc".into()), "a\t\"b\"\nc"),
        (Value::Bytes(vec![0, 171, 255]), "0x00abff"),
        (
            Value::Date(Date::parse("2026-11-16").unwrap()),
            "2026-11-16",
        ),
        (
            Value::Time(Time::parse("12:34:56.1234567").unwrap()),
            "12:34:56.1234567",
        ),
        (
            Value::TimeWithZone(TimeWithZone::parse("12:34:56+05:30").unwrap()),
            "12:34:56+05:30",
        ),
        (
            Value::Timestamp(Timestamp::parse("2026-11-16T12:34:56.123").unwrap()),
            "2026-11-16T12:34:56.123",
        ),
        (
            Value::TimestampWithZone(
                TimestampWithZone::parse("2026-11-16T12:34:56-04:00").unwrap(),
            ),
            "2026-11-16T12:34:56-04:00",
        ),
        (
            Value::Uuid([
                0, 17, 34, 51, 68, 85, 102, 119, 136, 153, 170, 187, 204, 221, 238, 255,
            ]),
            "00112233-4455-6677-8899-aabbccddeeff",
        ),
        (Value::Json("{\"a\": 1}".into()), "{\"a\": 1}"),
        (Value::Other("server text".into()), "server text"),
    ];
    for (value, expected) in cases {
        assert_eq!(export::cell_text(&value), expected);
        let mut store = store(&["value"], vec![vec![value]]);
        store.complete().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cell.csv");
        export::write(&store.reader(), &path, &export::Format::Csv { bom: false }).unwrap();
        let mut csv = csv::Reader::from_path(path).unwrap();
        assert_eq!(&csv.records().next().unwrap().unwrap()[0], expected);
    }
}
