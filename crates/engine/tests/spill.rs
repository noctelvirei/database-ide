use dbide_engine::session::temporal::{Date, Time, TimeWithZone, Timestamp, TimestampWithZone};
use dbide_engine::session::{Column, TypeCategory, Value};
use dbide_engine::spill::Error;
use dbide_engine::spill::Store;

fn columns(count: usize) -> Vec<Column> {
    vec![
        Column {
            name: "value".into(),
            type_name: "text".into(),
            category: TypeCategory::Text,
            nullable: None,
            base_table: None,
            base_column: None,
        };
        count
    ]
}

#[test]
fn every_value_variant_round_trips_exactly() {
    let row = vec![
        Value::Null,
        Value::Bool(false),
        Value::Bool(true),
        Value::Integer(i64::MIN),
        Value::Integer(i64::MAX),
        Value::UnsignedInteger(u64::MAX),
        Value::Decimal("-12345678901234567890.00100".into()),
        Value::Float(-0.0),
        Value::Float(f64::INFINITY),
        Value::Float(f64::NEG_INFINITY),
        Value::Float(f64::from_bits(0x7ff8_0000_0000_0042)),
        Value::Text(String::new()),
        Value::Text("é\0🦀".into()),
        Value::Bytes(Vec::new()),
        Value::Bytes(vec![0, 255, 128]),
        Value::Date(Date::parse("2024-02-29").unwrap()),
        Value::Time(Time::parse("23:59:59.123456789000").unwrap()),
        Value::TimeWithZone(TimeWithZone::parse("12:00:00.000-05:30").unwrap()),
        Value::Timestamp(Timestamp::parse("0001-01-01T00:00:00.000").unwrap()),
        Value::TimestampWithZone(
            TimestampWithZone::parse("9999-12-31T23:59:59.100+23:59").unwrap(),
        ),
        Value::Uuid([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 255]),
        Value::Json(" { \"x\": 1 } ".into()),
        Value::Other("infinity".into()),
        Value::Text("x".repeat(1_000_000)),
        Value::Bytes(vec![255; 1_000_000]),
    ];
    let mut store = Store::new(columns(row.len()), None).unwrap();
    store.append(vec![row]).unwrap();
    let mut rows = store.reader().rows(0..1).unwrap();
    let row = rows.remove(0);
    assert_eq!(
        &row[..7],
        &[
            Value::Null,
            Value::Bool(false),
            Value::Bool(true),
            Value::Integer(-9223372036854775808),
            Value::Integer(9223372036854775807),
            Value::UnsignedInteger(18446744073709551615),
            Value::Decimal("-12345678901234567890.00100".into()),
        ]
    );
    assert!(matches!(row[7], Value::Float(v) if v.to_bits() == 0x8000_0000_0000_0000));
    assert!(matches!(row[8], Value::Float(v) if v.to_bits() == 0x7ff0_0000_0000_0000));
    assert!(matches!(row[9], Value::Float(v) if v.to_bits() == 0xfff0_0000_0000_0000));
    assert!(matches!(row[10], Value::Float(v) if v.to_bits() == 0x7ff8_0000_0000_0042));
    assert_eq!(
        &row[11..23],
        &[
            Value::Text(String::new()),
            Value::Text("é\0🦀".into()),
            Value::Bytes(Vec::new()),
            Value::Bytes(vec![0, 255, 128]),
            Value::Date(Date::parse("2024-02-29").unwrap()),
            Value::Time(Time::parse("23:59:59.123456789000").unwrap()),
            Value::TimeWithZone(TimeWithZone::parse("12:00:00.000-05:30").unwrap()),
            Value::Timestamp(Timestamp::parse("0001-01-01T00:00:00.000").unwrap()),
            Value::TimestampWithZone(
                TimestampWithZone::parse("9999-12-31T23:59:59.100+23:59").unwrap()
            ),
            Value::Uuid([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 255]),
            Value::Json(" { \"x\": 1 } ".into()),
            Value::Other("infinity".into()),
        ]
    );
    assert!(
        matches!(&row[23], Value::Text(v) if v.len() == 1_000_000 && v.bytes().all(|b| b == b'x'))
    );
    assert!(
        matches!(&row[24], Value::Bytes(v) if v.len() == 1_000_000 && v.iter().all(|b| *b == 255))
    );
}

#[test]
fn readers_page_published_rows_while_appends_continue() {
    let mut store = Store::new(columns(1), None).unwrap();
    let reader = store.reader();
    let other = reader.clone();
    let (published, next) = std::sync::mpsc::sync_channel(0);
    let (read, resume) = std::sync::mpsc::sync_channel(0);
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || {
            store.append(vec![vec![Value::Integer(10)]]).unwrap();
            published.send(()).unwrap();
            resume.recv().unwrap();
            store.append(vec![vec![Value::Integer(20)]]).unwrap();
            store.complete().unwrap();
            published.send(()).unwrap();
            resume.recv().unwrap();
            store
        });
        next.recv().unwrap();
        assert_eq!(reader.row_count().unwrap(), 1);
        assert!(!reader.is_complete().unwrap());
        assert_eq!(reader.rows(0..100).unwrap(), vec![vec![Value::Integer(10)]]);
        read.send(()).unwrap();
        next.recv().unwrap();
        assert_eq!(other.row_count().unwrap(), 2);
        assert!(other.is_complete().unwrap());
        assert_eq!(
            other.rows(0..100).unwrap(),
            vec![vec![Value::Integer(10)], vec![Value::Integer(20)]]
        );
        read.send(()).unwrap();
        let store = writer.join().unwrap();
        assert_eq!(store.row_count().unwrap(), 2);
    });
}

#[test]
fn ranges_past_the_end_are_clamped_and_reversed_ranges_are_empty() {
    let mut store = Store::new(columns(1), None).unwrap();
    let reader = store.reader();
    assert_eq!(
        reader.rows(0..usize::MAX).unwrap(),
        Vec::<Vec<Value>>::new()
    );
    store
        .append(vec![vec![Value::Integer(1)], vec![Value::Integer(2)]])
        .unwrap();
    assert_eq!(
        reader.rows(1..usize::MAX).unwrap(),
        vec![vec![Value::Integer(2)]]
    );
    assert_eq!(reader.rows(2..3).unwrap(), Vec::<Vec<Value>>::new());
    assert_eq!(
        reader.rows(usize::MAX..usize::MAX).unwrap(),
        Vec::<Vec<Value>>::new()
    );
    assert_eq!(
        reader.rows(std::ops::Range { start: 2, end: 1 }).unwrap(),
        Vec::<Vec<Value>>::new()
    );
    assert_eq!(reader.rows(1..1).unwrap(), Vec::<Vec<Value>>::new());
}

#[test]
fn close_deletes_the_file_even_with_live_readers() {
    let folder = tempfile::tempdir().unwrap();
    let mut store = Store::new(columns(1), Some(folder.path())).unwrap();
    let reader = store.reader();
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    store.close().unwrap();
    store.close().unwrap();
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
    assert!(matches!(reader.rows(0..1), Err(Error::Closed)));
    assert!(matches!(store.append(vec![]), Err(Error::Closed)));
    assert!(matches!(store.complete(), Err(Error::Closed)));
}

#[test]
fn drop_deletes_the_file_even_with_live_readers() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::new(columns(1), Some(folder.path())).unwrap();
    let reader = store.reader();
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    drop(store);
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
    assert!(matches!(reader.rows(0..1), Err(Error::Closed)));
}

#[test]
fn invalid_row_width_rejects_the_whole_batch_without_losing_prior_rows() {
    let mut store = Store::new(columns(1), None).unwrap();
    store.append(vec![vec![Value::Integer(1)]]).unwrap();
    assert!(matches!(
        store.append(vec![vec![Value::Integer(2)], vec![]]),
        Err(Error::RowWidth {
            expected: 1,
            actual: 0
        })
    ));
    assert_eq!(store.row_count().unwrap(), 1);
    store.append(vec![vec![Value::Integer(3)]]).unwrap();
    assert_eq!(
        store.reader().rows(0..3).unwrap(),
        vec![vec![Value::Integer(1)], vec![Value::Integer(3)]]
    );
}

#[test]
fn completion_rejects_appends_and_preserves_columns_and_rows() {
    let mut store = Store::new(columns(1), None).unwrap();
    store.append(vec![vec![Value::Null]]).unwrap();
    store.complete().unwrap();
    store.complete().unwrap();
    assert!(matches!(
        store.append(vec![vec![Value::Null]]),
        Err(Error::Complete)
    ));
    let reader = store.reader();
    assert_eq!(
        reader.columns(),
        &[Column {
            name: "value".into(),
            type_name: "text".into(),
            category: TypeCategory::Text,
            nullable: None,
            base_table: None,
            base_column: None
        }]
    );
    assert_eq!(reader.rows(0..2).unwrap(), vec![vec![Value::Null]]);
}

#[test]
fn cell_size_does_not_increase_index_memory() {
    let mut small = Store::new(columns(1), None).unwrap();
    let mut large = Store::new(columns(1), None).unwrap();
    small
        .append(vec![vec![Value::Text(String::new())]; 10])
        .unwrap();
    large
        .append(vec![vec![Value::Text("x".repeat(1_000_000))]; 10])
        .unwrap();
    assert_eq!(small.row_count().unwrap(), 10);
    assert_eq!(large.row_count().unwrap(), 10);
    assert_eq!(
        small.reader().index_memory_bytes().unwrap(),
        large.reader().index_memory_bytes().unwrap()
    );
}

#[test]
fn zero_column_rows_still_have_distinct_row_numbers() {
    let mut store = Store::new(Vec::new(), None).unwrap();
    store.append(vec![vec![], vec![]]).unwrap();
    assert_eq!(store.row_count().unwrap(), 2);
    assert_eq!(
        store.reader().rows(1..3).unwrap(),
        vec![Vec::<Value>::new()]
    );
}

#[test]
fn a_million_five_column_rows_keep_only_a_proportional_index() {
    let mut store = Store::new(columns(5), None).unwrap();
    for batch in 0..1_000 {
        let rows = (0..1_000)
            .map(|row| {
                vec![
                    Value::Integer(batch * 1_000 + row),
                    Value::Text("x".repeat(256)),
                    Value::Bytes(vec![42; 256]),
                    Value::Null,
                    Value::Bool(true),
                ]
            })
            .collect();
        store.append(rows).unwrap();
    }
    let reader = store.reader();
    assert_eq!(reader.row_count().unwrap(), 1_000_000);
    assert!((8_000_000..=16_000_000).contains(&reader.index_memory_bytes().unwrap()));
    assert_eq!(
        reader.rows(0..1).unwrap(),
        vec![vec![
            Value::Integer(0),
            Value::Text("x".repeat(256)),
            Value::Bytes(vec![42; 256]),
            Value::Null,
            Value::Bool(true)
        ]]
    );
    assert_eq!(
        reader.rows(500_000..500_001).unwrap(),
        vec![vec![
            Value::Integer(500_000),
            Value::Text("x".repeat(256)),
            Value::Bytes(vec![42; 256]),
            Value::Null,
            Value::Bool(true)
        ]]
    );
    assert_eq!(
        reader.rows(999_999..1_000_000).unwrap(),
        vec![vec![
            Value::Integer(999_999),
            Value::Text("x".repeat(256)),
            Value::Bytes(vec![42; 256]),
            Value::Null,
            Value::Bool(true)
        ]]
    );
}
