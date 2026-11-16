use dbide_engine::export::{self, Format};
use dbide_engine::session::{Column, TypeCategory, Value};
use dbide_engine::spill::Store;

fn columns(names: &[&str]) -> Vec<Column> {
    names
        .iter()
        .map(|name| Column {
            name: (*name).into(),
            type_name: "text".into(),
            category: TypeCategory::Text,
            nullable: Some(true),
            base_table: Some("dbo.items".into()),
            base_column: None,
        })
        .collect()
}

fn store(names: &[&str], rows: Vec<Vec<Value>>) -> Store {
    let mut store = Store::new(columns(names), None).unwrap();
    store.append(rows).unwrap();
    store.complete().unwrap();
    store
}

#[test]
fn csv_quotes_commas_quotes_and_newlines() {
    let store = store(&["value"], vec![vec![Value::Text("a,\"b\"\nc".into())]]);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.csv");
    let report = export::write(&store.reader(), &path, &Format::Csv { bom: false }).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "value\r\n\"a,\"\"b\"\"\nc\"\r\n"
    );
    assert_eq!(report.rows, 1);
    assert_eq!(report.path, path);
}

#[test]
fn csv_null_is_an_empty_field() {
    let store = store(&["a", "b"], vec![vec![Value::Null, Value::Integer(7)]]);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.csv");
    export::write(&store.reader(), &path, &Format::Csv { bom: false }).unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), "a,b\r\n,7\r\n");
}

#[test]
fn csv_has_a_utf8_bom_by_default() {
    let store = store(&["a"], vec![]);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.csv");
    export::write(&store.reader(), &path, &Format::default()).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"\xef\xbb\xbfa\r\n");
}

#[test]
fn clipboard_keeps_tabs_newlines_and_quotes_in_one_cell() {
    let store = store(
        &["a"],
        vec![vec![Value::Text("one\ttwo\n\"three\"".into())]],
    );
    assert_eq!(
        export::clipboard(&store.reader(), 1, 100).unwrap(),
        "a\r\n\"one\ttwo\n\"\"three\"\"\"\r\n"
    );
}

#[test]
fn clipboard_refuses_the_row_limit() {
    let store = store(&["a"], vec![vec![Value::Null]]);
    assert_eq!(
        export::clipboard(&store.reader(), 0, 100)
            .unwrap_err()
            .to_string(),
        "Clipboard row limit exceeded (0 rows)"
    );
}

#[test]
fn clipboard_counts_utf8_bytes_and_headers_in_the_limit() {
    let store = store(&["a"], vec![vec![Value::Text("é".into())]]);
    assert_eq!(
        export::clipboard(&store.reader(), 1, 6)
            .unwrap_err()
            .to_string(),
        "Clipboard byte limit exceeded (6 bytes)"
    );
    assert_eq!(
        export::clipboard(&store.reader(), 1, 7).unwrap(),
        "a\r\né\r\n"
    );
}

fn sql(engine: dbide_engine::danger::Engine) -> String {
    let store = store(
        &["na]me\"`", "binary", "empty"],
        vec![vec![
            Value::Text("O'Brien 雪\\x".into()),
            Value::Bytes(vec![0, 255]),
            Value::Null,
        ]],
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.sql");
    export::write(
        &store.reader(),
        &path,
        &Format::Insert {
            engine,
            table: None,
        },
    )
    .unwrap();
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn sql_server_insert_quotes_identifiers_unicode_and_binary() {
    assert_eq!(
        sql(dbide_engine::danger::Engine::SqlServer),
        "INSERT INTO [dbo].[items] ([na]]me\"`], [binary], [empty]) VALUES (N'O''Brien 雪\\x', 0x00ff, NULL);\n"
    );
}

#[test]
fn postgres_insert_quotes_identifiers_unicode_backslashes_and_bytea() {
    assert_eq!(
        sql(dbide_engine::danger::Engine::PostgreSql),
        "INSERT INTO \"dbo\".\"items\" (\"na]me\"\"`\", \"binary\", \"empty\") VALUES (E'O''Brien 雪\\\\x', E'\\\\x00ff'::bytea, NULL);\n"
    );
}

#[test]
fn mysql_insert_is_independent_of_backslash_escape_mode() {
    assert_eq!(
        sql(dbide_engine::danger::Engine::MySql),
        "INSERT INTO `dbo`.`items` (`na]me\"```, `binary`, `empty`) VALUES (CONVERT(X'4f27427269656e20e99baa5c78' USING utf8mb4), _binary X'00ff', NULL);\n"
    );
}

#[test]
fn sql_server_inserts_never_exceed_the_values_list_limit() {
    let store = store(&["a"], vec![vec![Value::Integer(1)]; 1001]);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.sql");
    let report = export::write(
        &store.reader(),
        &path,
        &Format::Insert {
            engine: dbide_engine::danger::Engine::SqlServer,
            table: None,
        },
    )
    .unwrap();
    let text = std::fs::read_to_string(path).unwrap();
    assert_eq!(text.lines().count(), 1001);
    assert_eq!(text.matches("VALUES (1);\n").count(), 1001);
    assert_eq!(report.rows, 1001);
}

#[test]
fn insert_requires_a_table_for_expressions() {
    let mut columns = columns(&["a"]);
    columns[0].base_table = None;
    let mut store = Store::new(columns, None).unwrap();
    store.complete().unwrap();
    let directory = tempfile::tempdir().unwrap();
    assert!(
        export::write(
            &store.reader(),
            &directory.path().join("rows.sql"),
            &Format::Insert {
                engine: dbide_engine::danger::Engine::SqlServer,
                table: None
            }
        )
        .unwrap_err()
        .to_string()
        .contains("Supply an INSERT target table")
    );
}

#[test]
fn insert_uses_explicit_table_components_and_base_column_names() {
    let mut columns = columns(&["alias"]);
    columns[0].base_column = Some("original".into());
    let mut store = Store::new(columns, None).unwrap();
    store.append(vec![vec![Value::Integer(42)]]).unwrap();
    store.complete().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.sql");
    export::write(
        &store.reader(),
        &path,
        &Format::Insert {
            engine: dbide_engine::danger::Engine::SqlServer,
            table: Some(vec!["table.with.dot".into()]),
        },
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "INSERT INTO [table.with.dot] ([original]) VALUES (42);\n"
    );
}

#[test]
fn insert_rejects_unvalidated_decimal_sql() {
    let store = store(
        &["a"],
        vec![vec![Value::Decimal("1); DROP TABLE items;--".into())]],
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.sql");
    assert!(
        export::write(
            &store.reader(),
            &path,
            &Format::Insert {
                engine: dbide_engine::danger::Engine::SqlServer,
                table: None
            }
        )
        .is_err()
    );
    assert!(!path.exists());
    assert!(directory.path().join("rows.sql.partial").exists());
}

#[test]
fn insert_float_literals_use_scientific_notation() {
    let store = store(
        &["a", "b"],
        vec![vec![Value::Float(1e100), Value::Float(1e-100)]],
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.sql");
    export::write(
        &store.reader(),
        &path,
        &Format::Insert {
            engine: dbide_engine::danger::Engine::SqlServer,
            table: None,
        },
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "INSERT INTO [dbo].[items] ([a], [b]) VALUES (1e100, 1e-100);\n"
    );
}

// Windows' built-in ZIP reader checks the actual workbook cells without another crate.
#[cfg(windows)]
fn workbook_part(path: &std::path::Path, part: &str) -> String {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            r#"
            $ErrorActionPreference = 'Stop'
            [Console]::OutputEncoding = [Text.UTF8Encoding]::new()
            Add-Type -AssemblyName System.IO.Compression.FileSystem
            $zip = [IO.Compression.ZipFile]::OpenRead($env:DBIDE_EXPORT_TEST_FILE)
            try {
                $reader = [IO.StreamReader]::new($zip.GetEntry($env:DBIDE_EXPORT_TEST_PART).Open())
                try { [Console]::Write($reader.ReadToEnd()) } finally { $reader.Dispose() }
            } finally { $zip.Dispose() }
        "#,
        ])
        .env("DBIDE_EXPORT_TEST_FILE", path)
        .env("DBIDE_EXPORT_TEST_PART", part)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[cfg(windows)]
fn xlsx_preserves_decimals_that_underflow_as_text() {
    let tiny = format!("0.{}1", "0".repeat(400));
    let store = store(&["a"], vec![vec![Value::Decimal(tiny.clone())]]);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.xlsx");
    export::write(&store.reader(), &path, &Format::Xlsx).unwrap();
    let xml = workbook_part(&path, "xl/worksheets/sheet1.xml");
    assert!(xml.contains("<c r=\"A2\" t=\"inlineStr\">"));
    assert!(xml.contains(&tiny));
}

#[test]
#[cfg(windows)]
fn xlsx_preserves_floats_outside_excel_range_as_text() {
    let store = store(
        &["small", "large"],
        vec![vec![Value::Float(1e-320), Value::Float(1e308)]],
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.xlsx");
    export::write(&store.reader(), &path, &Format::Xlsx).unwrap();
    let xml = workbook_part(&path, "xl/worksheets/sheet1.xml");
    assert!(xml.contains("<c r=\"A2\" t=\"inlineStr\">"));
    assert!(xml.contains("<c r=\"B2\" t=\"inlineStr\">"));
}

#[test]
#[cfg(windows)]
fn xlsx_keeps_formula_like_values_as_text() {
    let store = store(&["a"], vec![vec![Value::Text("=1+1".into())]]);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.xlsx");
    export::write(&store.reader(), &path, &Format::Xlsx).unwrap();
    assert!(
        workbook_part(&path, "xl/worksheets/sheet1.xml")
            .contains("<c r=\"A2\" t=\"inlineStr\"><is><t>=1+1</t></is></c>")
    );
}

#[test]
#[cfg(windows)]
fn xlsx_writes_supported_numeric_values_as_numbers() {
    let store = store(
        &["integer", "decimal", "float"],
        vec![vec![
            Value::Integer(42),
            Value::Decimal("123456789012345.000".into()),
            Value::Float(1.25),
        ]],
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.xlsx");
    export::write(&store.reader(), &path, &Format::Xlsx).unwrap();
    let xml = workbook_part(&path, "xl/worksheets/sheet1.xml");
    assert!(xml.contains("<c r=\"A2\"><v>42</v></c>"));
    assert!(xml.contains("<c r=\"B2\"><v>123456789012345</v></c>"));
    assert!(xml.contains("<c r=\"C2\"><v>1.25</v></c>"));
}

#[test]
#[cfg(windows)]
fn xlsx_preserves_numbers_beyond_excel_precision_as_text() {
    let store = store(
        &["integer", "decimal"],
        vec![vec![
            Value::Integer(12345678901234567),
            Value::Decimal("1234567890123456.78".into()),
        ]],
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.xlsx");
    export::write(&store.reader(), &path, &Format::Xlsx).unwrap();
    let xml = workbook_part(&path, "xl/worksheets/sheet1.xml");
    assert!(xml.contains("<c r=\"A2\" t=\"inlineStr\"><is><t>12345678901234567</t></is></c>"));
    assert!(xml.contains("<c r=\"B2\" t=\"inlineStr\"><is><t>1234567890123456.78</t></is></c>"));
}

#[test]
#[cfg(windows)]
fn xlsx_writes_booleans_as_booleans() {
    let store = store(&["a"], vec![vec![Value::Bool(true)]]);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.xlsx");
    export::write(&store.reader(), &path, &Format::Xlsx).unwrap();
    assert!(
        workbook_part(&path, "xl/worksheets/sheet1.xml")
            .contains("<c r=\"A2\" t=\"b\"><v>1</v></c>")
    );
}

#[test]
#[cfg(windows)]
fn xlsx_writes_dates_and_timestamps_as_formatted_excel_dates() {
    use dbide_engine::session::temporal::{Date, Timestamp};
    let store = store(
        &["date", "timestamp"],
        vec![vec![
            Value::Date(Date::parse("2026-11-16").unwrap()),
            Value::Timestamp(Timestamp::parse("2026-11-16T12:00:00").unwrap()),
        ]],
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.xlsx");
    export::write(&store.reader(), &path, &Format::Xlsx).unwrap();
    let xml = workbook_part(&path, "xl/worksheets/sheet1.xml");
    assert!(xml.contains("<c r=\"A2\" s=\"1\"><v>46342</v></c>"));
    assert!(xml.contains("<c r=\"B2\" s=\"2\"><v>46342.5</v></c>"));
    let styles = workbook_part(&path, "xl/styles.xml");
    assert!(styles.contains("yyyy-mm-dd"));
    assert!(styles.contains("yyyy-mm-dd hh:mm:ss.000"));
}

#[test]
fn xlsx_writes_a_nonempty_zip_file() {
    let store = store(
        &["a", "b", "c", "d", "e"],
        vec![vec![
            Value::Integer(42),
            Value::Bool(true),
            Value::Date(dbide_engine::session::temporal::Date::parse("2026-11-16").unwrap()),
            Value::Decimal("1234567890123456.78".into()),
            Value::Text("=1+1".into()),
        ]],
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.xlsx");
    let report = export::write(&store.reader(), &path, &Format::Xlsx).unwrap();
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..2], b"PK");
    assert!(bytes.len() > 1000);
    assert_eq!(report.rows, 1);
}

#[test]
fn xlsx_repeats_headers_on_a_second_sheet_after_the_excel_row_limit() {
    let mut store = Store::new(columns(&["a"]), None).unwrap();
    for _ in 0..4096 {
        store.append(vec![vec![Value::Integer(1)]; 256]).unwrap();
    }
    store.append(vec![vec![Value::Integer(2)]]).unwrap();
    store.complete().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.xlsx");
    let report = export::write(&store.reader(), &path, &Format::Xlsx).unwrap();
    let bytes = std::fs::read(path).unwrap();
    assert!(
        bytes
            .windows(b"xl/worksheets/sheet2.xml".len())
            .any(|window| window == b"xl/worksheets/sheet2.xml")
    );
    assert_eq!(report.rows, 1_048_577);
    assert_eq!(report.max_page_rows, 256);
    #[cfg(windows)]
    {
        let xml = workbook_part(&report.path, "xl/worksheets/sheet2.xml");
        assert!(xml.contains("<c r=\"A1\" t=\"inlineStr\"><is><t>a</t></is></c>"));
        assert!(xml.contains("<c r=\"A2\"><v>1</v></c>"));
        assert!(xml.contains("<c r=\"A3\"><v>2</v></c>"));
        assert_eq!(xml.matches("<row ").count(), 3);
    }
}

#[test]
fn million_row_export_reads_only_bounded_pages() {
    let mut store = Store::new(columns(&["a"]), None).unwrap();
    for _ in 0..1000 {
        store.append(vec![vec![Value::Integer(1)]; 1000]).unwrap();
    }
    store.complete().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let report = export::write(
        &store.reader(),
        &directory.path().join("rows.csv"),
        &Format::default(),
    )
    .unwrap();
    assert_eq!(report.rows, 1_000_000);
    assert_eq!(report.max_page_rows, 256);
}

#[test]
fn export_refuses_to_overwrite_existing_files() {
    let store = store(&["a"], vec![]);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.csv");
    std::fs::write(&path, "keep").unwrap();
    assert!(export::write(&store.reader(), &path, &Format::default()).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "keep");
}

#[test]
fn export_refuses_a_result_still_receiving_rows() {
    let store = Store::new(columns(&["a"]), None).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.csv");
    assert_eq!(
        export::write(&store.reader(), &path, &Format::default())
            .unwrap_err()
            .to_string(),
        "Complete the result before exporting"
    );
    assert!(!path.exists());
}

#[tokio::test]
async fn run_to_file_writes_two_result_files_and_returns_messages_and_counts() {
    use dbide_engine::session::{Event, Run, Severity};
    let message = Event::Message {
        text: "hello".into(),
        severity: Severity::Info,
        line: None,
    };
    let run = Run::new(
        futures::stream::iter(vec![
            Event::ResultSetStarted {
                columns: columns(&["a"]),
            },
            Event::Rows(vec![vec![Value::Integer(1)]]),
            message.clone(),
            Event::RowCount(1),
            Event::ResultSetStarted {
                columns: columns(&["b"]),
            },
            Event::Rows(vec![vec![Value::Integer(2)]]),
        ]),
        tokio_util::sync::CancellationToken::new(),
    );
    let directory = tempfile::tempdir().unwrap();
    let report = export::run_to_file(
        run,
        &directory.path().join("rows.csv"),
        &Format::Csv { bom: false },
    )
    .await
    .unwrap();
    assert_eq!(report.files.len(), 2);
    assert_eq!(report.files[0].path, directory.path().join("rows-1.csv"));
    assert_eq!(report.files[1].path, directory.path().join("rows-2.csv"));
    assert_eq!(
        std::fs::read_to_string(&report.files[0].path).unwrap(),
        "a\r\n1\r\n"
    );
    assert_eq!(
        std::fs::read_to_string(&report.files[1].path).unwrap(),
        "b\r\n2\r\n"
    );
    assert_eq!(report.events[0], message);
    assert_eq!(report.events[1], Event::RowCount(1));
    assert!(matches!(report.events[2], Event::Done { .. }));
    assert_eq!(report.files[0].rows, 1);
    assert_eq!(report.files[1].max_page_rows, 0);
}

#[tokio::test]
async fn cancelled_run_to_file_leaves_a_partial_file_and_returns_the_error() {
    use dbide_engine::session::{Event, Run};
    use futures::StreamExt;
    let token = tokio_util::sync::CancellationToken::new();
    let cancel = token.clone();
    let source = futures::stream::iter(vec![
        Event::ResultSetStarted {
            columns: columns(&["a"]),
        },
        Event::Rows(vec![vec![Value::Integer(1)]]),
        Event::RowCount(1),
    ])
    .inspect(move |event| {
        if matches!(event, Event::RowCount(_)) {
            cancel.cancel();
        }
    });
    let run = Run::new(source, token);
    let directory = tempfile::tempdir().unwrap();
    let report = export::run_to_file(
        run,
        &directory.path().join("rows.csv"),
        &Format::Csv { bom: false },
    )
    .await
    .unwrap();
    assert_eq!(
        report.files[0].path,
        directory.path().join("rows-1.csv.partial")
    );
    assert_eq!(
        std::fs::read_to_string(&report.files[0].path).unwrap(),
        "a\r\n1\r\n"
    );
    assert!(!directory.path().join("rows-1.csv").exists());
    assert!(matches!(&report.events[1], Event::Error { text, .. } if text == "Run was cancelled"));
}

#[tokio::test]
async fn run_to_file_rejects_rows_without_a_header() {
    use dbide_engine::session::{Event, Run};
    let run = Run::new(
        futures::stream::iter(vec![Event::Rows(vec![vec![Value::Integer(1)]])]),
        tokio_util::sync::CancellationToken::new(),
    );
    let directory = tempfile::tempdir().unwrap();
    assert!(
        export::run_to_file(run, &directory.path().join("rows.csv"), &Format::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn run_to_file_rejects_a_row_with_the_wrong_width() {
    use dbide_engine::session::{Event, Run};
    let run = Run::new(
        futures::stream::iter(vec![
            Event::ResultSetStarted {
                columns: columns(&["a"]),
            },
            Event::Rows(vec![vec![]]),
        ]),
        tokio_util::sync::CancellationToken::new(),
    );
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(
        export::run_to_file(run, &directory.path().join("rows.csv"), &Format::default())
            .await
            .unwrap_err()
            .to_string(),
        "Invalid result row: expected 1 cells, received 0"
    );
    assert!(directory.path().join("rows-1.csv.partial").exists());
    assert!(!directory.path().join("rows-1.csv").exists());
}

#[tokio::test]
async fn run_to_file_preserves_server_errors_and_empty_results() {
    use dbide_engine::session::{Event, Run};
    let error = Event::Error {
        text: "statement failed".into(),
        line: Some(std::num::NonZeroU64::new(7).unwrap()),
        code: Some("42".into()),
    };
    let run = Run::new(
        futures::stream::iter(vec![
            Event::ResultSetStarted {
                columns: columns(&["a"]),
            },
            error.clone(),
        ]),
        tokio_util::sync::CancellationToken::new(),
    );
    let directory = tempfile::tempdir().unwrap();
    let report = export::run_to_file(
        run,
        &directory.path().join("rows.csv"),
        &Format::Csv { bom: false },
    )
    .await
    .unwrap();
    assert_eq!(report.events[0], error);
    assert_eq!(report.files[0].rows, 0);
    assert_eq!(
        report.files[0].path,
        directory.path().join("rows-1.csv.partial")
    );
    assert_eq!(
        std::fs::read_to_string(&report.files[0].path).unwrap(),
        "a\r\n"
    );
}

async fn cancelled_file(format: Format, extension: &str) -> Vec<u8> {
    use dbide_engine::session::{Event, Run};
    use futures::StreamExt;
    let token = tokio_util::sync::CancellationToken::new();
    let cancel = token.clone();
    let source = futures::stream::iter(vec![
        Event::ResultSetStarted {
            columns: columns(&["a"]),
        },
        Event::Rows(vec![vec![Value::Integer(1)]]),
        Event::RowCount(1),
    ])
    .inspect(move |event| {
        if matches!(event, Event::RowCount(_)) {
            cancel.cancel();
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let run = Run::new(source, token);
    let report = export::run_to_file(
        run,
        &directory.path().join(format!("rows.{extension}")),
        &format,
    )
    .await
    .unwrap();
    assert_eq!(report.files[0].path.extension().unwrap(), "partial");
    assert_eq!(report.files[0].rows, 1);
    std::fs::read(&report.files[0].path).unwrap()
}

#[tokio::test]
async fn cancelled_run_finalizes_a_valid_partial_workbook() {
    let bytes = cancelled_file(Format::Xlsx, "xlsx").await;
    assert_eq!(&bytes[..2], b"PK");
    assert!(
        bytes
            .windows(b"xl/worksheets/sheet1.xml".len())
            .any(|window| window == b"xl/worksheets/sheet1.xml")
    );
}

#[tokio::test]
async fn cancelled_run_flushes_the_partial_insert_script() {
    let bytes = cancelled_file(
        Format::Insert {
            engine: dbide_engine::danger::Engine::SqlServer,
            table: None,
        },
        "sql",
    )
    .await;
    assert_eq!(bytes, b"INSERT INTO [dbo].[items] ([a]) VALUES (1);\n");
}
