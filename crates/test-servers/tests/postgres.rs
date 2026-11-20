use std::path::Path;

use dbide_test_servers::postgres::Postgres;

#[test]
fn pg_server_starts_and_accepts_connections() {
    let database = Postgres::start().unwrap();
    let second = Postgres::start().unwrap();
    assert_eq!(database.host(), "127.0.0.1");
    assert_eq!(database.port(), second.port());
    assert_eq!(database.server_folder(), second.server_folder());
    assert_ne!(database.database(), second.database());
    assert!(database.database().starts_with("dbide_test_"));
    assert!(!database.password().is_empty());
    assert_eq!(database.user(), "dbide_test");
    assert_eq!(database.query("SELECT 1").unwrap(), "1\n");
    database
        .query("CREATE TABLE isolated (id integer)")
        .unwrap();
    assert_eq!(
        second
            .query("SELECT to_regclass('isolated') IS NULL")
            .unwrap(),
        "t\n"
    );
    let name = database.database().to_owned();
    drop(database);
    assert_eq!(
        second
            .query(&format!(
                "SELECT count(*) FROM pg_database WHERE datname = '{name}'"
            ))
            .unwrap(),
        "0\n"
    );
    assert!(second.query("SELECT missing_column").is_err());
    assert_eq!(second.query("SELECT 2").unwrap(), "2\n");
}

#[test]
fn pg_test_data_loads() {
    let database = Postgres::start().unwrap();
    database
        .load_script(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/postgres.sql"))
        .unwrap();
    let counts = database
        .query(
            "SELECT (SELECT count(*) FROM shop.customers), \
                (SELECT count(*) FROM shop.orders), \
                (SELECT count(*) FROM shop.order_lines), \
                (SELECT count(*) FROM shop.audit_log), \
                (SELECT count(*) FROM shop.common_types), \
                (SELECT count(*) FROM shop.order_summary)",
        )
        .unwrap();
    assert_eq!(counts, "40|80|160|20|6|80\n");
    record("pg_test_data_loads.txt", &counts);
    assert_eq!(
        database
            .query(
                "SELECT (SELECT count(*) FROM shop.customer_orders(1)), shop.line_total(2, 2.50)"
            )
            .unwrap(),
        "2|5.00\n"
    );
    assert_eq!(
        database
            .query("SELECT display_name, email IS NULL FROM shop.customers WHERE id = 3")
            .unwrap(),
        "Zorb-3 雪é|t\n"
    );
    assert_eq!(
        database
            .query(
                "SELECT small_value, int_value, big_value, decimal_value, \
                text_value = '', octet_length(binary_value) \
         FROM shop.common_types WHERE id = 2"
            )
            .unwrap(),
        "-32768|-2147483648|-9223372036854775808|-9999999999999999999999999999.9999999999|t|0\n"
    );
    assert_eq!(
        database
            .query(
                "SELECT length(text_value), encode(binary_value, 'hex'), \
                jsonb_value->'invented'->>2 FROM shop.common_types WHERE id = 3"
            )
            .unwrap(),
        "10000|00017fff80ff|雪\n"
    );
    assert_eq!(database.query(
        "SELECT id, real_value, double_value FROM shop.common_types WHERE id >= 4 ORDER BY id"
    ).unwrap(), "4|NaN|NaN\n5|Infinity|Infinity\n6|-Infinity|-Infinity\n");
    assert_eq!(
        database
            .query(
                "SELECT count(*) FROM pg_constraint WHERE conrelid = 'shop.audit_log'::regclass \
         AND contype IN ('p', 'u')"
            )
            .unwrap(),
        "0\n"
    );
    assert!(
        database
            .query(
                "INSERT INTO shop.orders (customer_id, ordered_at, total) VALUES (999, now(), 1)"
            )
            .is_err()
    );
    assert!(
        database
            .query("INSERT INTO shop.order_lines VALUES (1, 1, 'Duplicate', 1, 1)")
            .is_err()
    );
    let invalid = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        invalid.path(),
        "SELECT missing_column; CREATE TABLE must_not_exist (id int);",
    )
    .unwrap();
    assert!(database.load_script(invalid.path()).is_err());
    assert_eq!(
        database
            .query("SELECT to_regclass('must_not_exist') IS NULL")
            .unwrap(),
        "t\n"
    );
}

#[test]
fn pg_large_table_has_one_million_rows() {
    let database = Postgres::start().unwrap();
    database
        .load_script(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/postgres-large.sql"),
        )
        .unwrap();
    let counts = database
        .query("SELECT count(*), min(id), max(id) FROM shop.big_rows")
        .unwrap();
    assert_eq!(counts, "1000000|1|1000000\n");
    record("pg_large_table_has_one_million_rows.txt", &counts);
}

fn record(name: &str, output: &str) {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let artifacts = target.join("test-artifacts");
    std::fs::create_dir_all(&artifacts).unwrap();
    std::fs::write(artifacts.join(name), output).unwrap();
}
