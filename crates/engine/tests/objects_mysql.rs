use dbide_engine::objects::mysql as objects;
use dbide_engine::objects::{
    Column, Constraint, ConstraintKind, Generated, Identity, Kind, Script,
};
use dbide_engine::session::{Session, Value};

mod mysql_support;
use mysql_support::*;

async fn parameters(session: &mut dyn Session) -> Vec<Vec<Value>> {
    run(
        session,
        "SELECT PARAMETER_NAME, CHARACTER_SET_NAME, COLLATION_NAME
        FROM information_schema.PARAMETERS WHERE SPECIFIC_SCHEMA = DATABASE()
        ORDER BY SPECIFIC_NAME, ORDINAL_POSITION",
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_procedure_string_parameters_keep_the_source_database_charset() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(
        &mut session,
        "ALTER DATABASE CHARACTER SET latin1 COLLATE latin1_swedish_ci;
        CREATE PROCEDURE pear_names(IN fruit_name VARCHAR(10), OUT fruit_kind ENUM('pear','plum'))
        SET fruit_kind = 'pear'",
    )
    .await;
    run(
        &mut destination,
        "ALTER DATABASE CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci",
    )
    .await;
    let procedure = object(&mut session, "pear_names").await;
    let sql = objects::script(&mut session, &procedure, Script::Create)
        .await
        .unwrap();
    run(&mut destination, &sql).await;
    assert_eq!(
        parameters(&mut session).await,
        vec![
            vec![
                Value::Text("fruit_name".into()),
                Value::Text("latin1".into()),
                Value::Text("latin1_swedish_ci".into())
            ],
            vec![
                Value::Text("fruit_kind".into()),
                Value::Text("latin1".into()),
                Value::Text("latin1_swedish_ci".into())
            ],
        ]
    );
    assert_eq!(
        parameters(&mut destination).await,
        parameters(&mut session).await
    );
    artifact(&source, "mysql-routine-parameters.sql", &sql);
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_function_text_return_keeps_the_source_database_charset() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(
        &mut session,
        "ALTER DATABASE CHARACTER SET latin1 COLLATE latin1_swedish_ci;
        CREATE FUNCTION pear_text() RETURNS TEXT DETERMINISTIC RETURN 'pear'",
    )
    .await;
    run(
        &mut destination,
        "ALTER DATABASE CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci",
    )
    .await;
    let function = object(&mut session, "pear_text").await;
    let sql = objects::script(&mut session, &function, Script::Create)
        .await
        .unwrap();
    run(&mut destination, &sql).await;
    assert_eq!(
        parameters(&mut session).await,
        vec![vec![
            Value::Null,
            Value::Text("latin1".into()),
            Value::Text("latin1_swedish_ci".into())
        ]]
    );
    assert_eq!(
        parameters(&mut destination).await,
        parameters(&mut session).await
    );
    artifact(&source, "mysql-routine-return.sql", &sql);
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_explicit_parameter_charset_is_copied_unchanged() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(
        &mut session,
        "ALTER DATABASE CHARACTER SET latin1 COLLATE latin1_swedish_ci;
        CREATE PROCEDURE pear_unicode(IN fruit_name VARCHAR(10) CHARACTER SET utf8mb4)
        SELECT fruit_name",
    )
    .await;
    run(
        &mut destination,
        "ALTER DATABASE CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci",
    )
    .await;
    let original = run(&mut session, "SHOW CREATE PROCEDURE pear_unicode").await;
    let procedure = object(&mut session, "pear_unicode").await;
    let sql = objects::script(&mut session, &procedure, Script::Create)
        .await
        .unwrap();
    let Value::Text(definition) = &original[0][2] else {
        panic!("Expected routine definition")
    };
    assert!(sql.ends_with(&format!("{definition};\n")), "{sql}");
    run(&mut destination, &sql).await;
    assert_eq!(
        parameters(&mut session).await,
        vec![vec![
            Value::Text("fruit_name".into()),
            Value::Text("utf8mb4".into()),
            Value::Text("utf8mb4_0900_ai_ci".into())
        ]]
    );
    assert_eq!(
        parameters(&mut destination).await,
        parameters(&mut session).await
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_local_string_variables_keep_the_source_database_charset() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(&mut session, "ALTER DATABASE CHARACTER SET latin1 COLLATE latin1_swedish_ci;
        CREATE FUNCTION pear_charset() RETURNS VARCHAR(20) DETERMINISTIC
        BEGIN DECLARE v, w VARCHAR(10) DEFAULT 'pear'; RETURN CONCAT(CHARSET(v), ':', CHARSET(w)); END").await;
    run(
        &mut destination,
        "ALTER DATABASE CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci",
    )
    .await;
    let function = object(&mut session, "pear_charset").await;
    let sql = objects::script(&mut session, &function, Script::Create)
        .await
        .unwrap();
    run(&mut destination, &sql).await;
    assert_eq!(
        run(&mut destination, "SELECT pear_charset()").await,
        vec![vec![Value::Text("latin1:latin1".into())]]
    );
    artifact(&source, "mysql-routine-locals.sql", &sql);
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_routine_strings_comments_and_quoted_identifiers_are_copied_unchanged() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(
        &mut session,
        "ALTER DATABASE CHARACTER SET latin1 COLLATE latin1_swedish_ci;
        CREATE PROCEDURE pear_body() BEGIN
        DECLARE `DECLARE VARCHAR(5)` VARCHAR(10);
        -- DECLARE pretend VARCHAR(5);
        SELECT 'DECLARE pretend VARCHAR(5)' AS `VARCHAR(5)`;
        END",
    )
    .await;
    run(
        &mut destination,
        "ALTER DATABASE CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci",
    )
    .await;
    let procedure = object(&mut session, "pear_body").await;
    let sql = objects::script(&mut session, &procedure, Script::Create)
        .await
        .unwrap();
    assert!(sql.contains("DECLARE `DECLARE VARCHAR(5)` VARCHAR(10) CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`;"), "{sql}");
    assert!(sql.contains("-- DECLARE pretend VARCHAR(5);\n        SELECT 'DECLARE pretend VARCHAR(5)' AS `VARCHAR(5)`;"), "{sql}");
    run(&mut destination, &sql).await;
    assert_eq!(
        run(&mut destination, "CALL pear_body()").await,
        vec![vec![Value::Text("DECLARE pretend VARCHAR(5)".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_binary_string_types_run_with_the_source_collation() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(
        &mut session,
        "ALTER DATABASE CHARACTER SET latin1 COLLATE latin1_swedish_ci;
        CREATE PROCEDURE pear_binary(IN fruit_name CHAR(5) BINARY)
        BEGIN DECLARE pip CHAR(5) BINARY DEFAULT 'x'; SELECT COLLATION(fruit_name), COLLATION(pip); END",
    )
    .await;
    run(
        &mut destination,
        "ALTER DATABASE CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci",
    )
    .await;
    let procedure = object(&mut session, "pear_binary").await;
    let sql = objects::script(&mut session, &procedure, Script::Create)
        .await
        .unwrap();
    assert!(!sql.contains("BINARY"), "{sql}");
    run(&mut destination, &sql).await;
    assert_eq!(
        run(&mut session, "CALL pear_binary('x')").await,
        vec![vec![
            Value::Text("latin1_swedish_ci".into()),
            Value::Text("latin1_swedish_ci".into())
        ]]
    );
    assert_eq!(
        run(&mut destination, "CALL pear_binary('x')").await,
        vec![vec![
            Value::Text("latin1_swedish_ci".into()),
            Value::Text("latin1_swedish_ci".into())
        ]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_create_table_recreates_fixture_columns_and_keys_in_a_fresh_database() {
    let source = fixture().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    let table = object(&mut session, "customers").await;
    let sql = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    artifact(&source, "mysql-customers.sql", &sql);
    run(&mut destination, &sql).await;
    let copy = object(&mut destination, "customers").await;
    assert_eq!(
        objects::columns(&mut destination, &copy).await.unwrap(),
        vec![
            Column {
                name: "id".into(),
                type_name: "bigint".into(),
                nullable: false,
                default: None,
                identity: Some(Identity::ByDefault),
                generated: None
            },
            Column {
                name: "display_name".into(),
                type_name: "text".into(),
                nullable: false,
                default: None,
                identity: None,
                generated: None
            },
            Column {
                name: "email".into(),
                type_name: "text".into(),
                nullable: true,
                default: None,
                identity: None,
                generated: None
            },
            Column {
                name: "notes".into(),
                type_name: "text".into(),
                nullable: true,
                default: None,
                identity: None,
                generated: None
            },
        ]
    );
    assert_eq!(
        objects::constraints(&mut destination, &copy).await.unwrap(),
        vec![Constraint {
            name: "PRIMARY".into(),
            kind: ConstraintKind::PrimaryKey,
            definition: "PRIMARY KEY (`id`)".into()
        },]
    );
    assert_eq!(
        run(&mut destination, "SELECT COUNT(*) FROM customers").await,
        vec![vec![Value::Integer(0)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_create_table_preserves_defaults_generation_and_checks_across_sql_modes() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(&mut session, r"SET sql_mode = 'NO_BACKSLASH_ESCAPES';
        CREATE TABLE pears (id int PRIMARY KEY AUTO_INCREMENT, name varchar(20) NOT NULL DEFAULT 'pear\tree',
        quantity int NOT NULL DEFAULT 7, doubled int GENERATED ALWAYS AS (quantity * 2) STORED,
        UNIQUE KEY pear_name (name), CONSTRAINT positive_quantity CHECK (quantity > 0))").await;
    let table = object(&mut session, "pears").await;
    let sql = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    artifact(&source, "mysql-pears-defaults.sql", &sql);
    run(&mut destination, "SET sql_mode = ''").await;
    run(&mut destination, &sql).await;
    let copy = object(&mut destination, "pears").await;
    let columns = objects::columns(&mut destination, &copy).await.unwrap();
    assert_eq!(
        columns[1].default.as_deref(),
        Some("_utf8mb4 X'706561725C74726565'")
    );
    assert_eq!(columns[2].default.as_deref(), Some("7"));
    assert_eq!(
        columns[3].generated,
        Some(Generated {
            expression: "(`quantity` * 2)".into(),
            stored: true
        })
    );
    assert_eq!(
        objects::constraints(&mut destination, &copy).await.unwrap(),
        vec![
            Constraint {
                name: "PRIMARY".into(),
                kind: ConstraintKind::PrimaryKey,
                definition: "PRIMARY KEY (`id`)".into()
            },
            Constraint {
                name: "pear_name".into(),
                kind: ConstraintKind::Unique,
                definition: "UNIQUE (`name`)".into()
            },
            Constraint {
                name: "positive_quantity".into(),
                kind: ConstraintKind::Check,
                definition: "CHECK ((`quantity` > 0))".into()
            },
        ]
    );
    run(&mut destination, "INSERT INTO pears () VALUES ()").await;
    assert_eq!(
        run(
            &mut destination,
            "SELECT id, name, quantity, doubled FROM pears"
        )
        .await,
        vec![vec![
            Value::Integer(1),
            Value::Text(r"pear\tree".into()),
            Value::Integer(7),
            Value::Integer(14)
        ]]
    );
    assert_eq!(
        run(&mut session, "SELECT @@sql_mode").await,
        vec![vec![Value::Text("NO_BACKSLASH_ESCAPES".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_create_procedure_recreates_the_fixture_body() {
    let source = fixture().await;
    let mut session = open(&source).await;
    let procedure = object(&mut session, "customer_orders").await;
    let sql = objects::script(&mut session, &procedure, Script::Create)
        .await
        .unwrap();
    artifact(&source, "mysql-procedure.sql", &sql);
    run(&mut session, "DROP PROCEDURE customer_orders").await;
    run(&mut session, &sql).await;
    let rows = run(&mut session, "CALL customer_orders(1)").await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], Value::Integer(1));
    assert_eq!(rows[1][0], Value::Integer(41));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_create_function_recreates_the_fixture_body() {
    let source = fixture().await;
    let mut session = open(&source).await;
    let function = object(&mut session, "line_total").await;
    let sql = objects::script(&mut session, &function, Script::Create)
        .await
        .unwrap();
    run(&mut session, "DROP FUNCTION line_total").await;
    run(&mut session, &sql).await;
    assert_eq!(
        run(&mut session, "SELECT line_total(3, 2.50)").await,
        vec![vec![Value::Decimal("7.50".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_create_view_recreates_the_fixture_query() {
    let source = fixture().await;
    let mut session = open(&source).await;
    let view = object(&mut session, "order_summary").await;
    let sql = objects::script(&mut session, &view, Script::Create)
        .await
        .unwrap();
    artifact(&source, "mysql-view.sql", &sql);
    run(&mut session, "DROP VIEW order_summary").await;
    run(&mut session, &sql).await;
    assert_eq!(
        run(
            &mut session,
            "SELECT id, display_name, total FROM order_summary WHERE id = 1"
        )
        .await,
        vec![vec![
            Value::Integer(1),
            Value::Text("Zorb-1 雪é".into()),
            Value::Decimal("7.50".into())
        ]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_alter_view_preserves_the_fixture_query() {
    let source = fixture().await;
    let mut session = open(&source).await;
    let view = object(&mut session, "order_summary").await;
    let sql = objects::script(&mut session, &view, Script::Alter)
        .await
        .unwrap();
    assert!(sql.contains("\nALTER "));
    run(&mut session, &sql).await;
    assert_eq!(
        run(&mut session, "SELECT COUNT(*) FROM order_summary").await,
        vec![vec![Value::Integer(80)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_backtick_identifiers_round_trip_through_create_and_select() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(&mut session, "CREATE TABLE `pear``box` (`fruit``name` varchar(20) DEFAULT 'pear'); INSERT INTO `pear``box` () VALUES ()").await;
    let table = object(&mut session, "pear`box").await;
    let select = objects::script(&mut session, &table, Script::Select)
        .await
        .unwrap();
    assert_eq!(
        select,
        format!(
            "SELECT `fruit``name` FROM `{}`.`pear``box` LIMIT 1000;\n",
            source.database()
        )
    );
    assert_eq!(
        run(&mut session, &select).await,
        vec![vec![Value::Text("pear".into())]]
    );
    let create = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    run(&mut destination, &create).await;
    run(&mut destination, "INSERT INTO `pear``box` () VALUES ()").await;
    assert_eq!(
        run(&mut destination, "SELECT `fruit``name` FROM `pear``box`").await,
        vec![vec![Value::Text("pear".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_table_alter_is_only_a_commented_skeleton() {
    let source = database().await;
    let mut session = open(&source).await;
    run(&mut session, "CREATE TABLE pears (id int)").await;
    let table = object(&mut session, "pears").await;
    assert_eq!(
        objects::script(&mut session, &table, Script::Alter)
            .await
            .unwrap(),
        format!(
            "-- ALTER TABLE `{}`.`pears`\n--     ADD COLUMN column_name data_type;\n",
            source.database()
        )
    );
    assert!(!objects::supports(Kind::Procedure, Script::Alter));
    assert!(!objects::supports(Kind::Function, Script::Alter));
    assert!(!objects::supports(Kind::Index, Script::Select));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_metadata_rejects_foreign_database_identities() {
    let source = fixture().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    let table = object(&mut session, "customers").await;
    assert!(matches!(
        objects::columns(&mut destination, &table).await,
        Err(objects::Error::Identity)
    ));
    assert!(matches!(
        objects::script(&mut destination, &table, Script::Create).await,
        Err(objects::Error::Identity)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_script_rejects_dropped_objects() {
    let source = database().await;
    let mut session = open(&source).await;
    run(&mut session, "CREATE TABLE pears (id int)").await;
    let table = object(&mut session, "pears").await;
    run(&mut session, "DROP TABLE pears").await;
    assert!(matches!(
        objects::script(&mut session, &table, Script::Create).await,
        Err(objects::Error::NotFound)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_index_create_recreates_prefix_direction_and_visibility() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "CREATE TABLE pears (name varchar(30), quantity int);
        CREATE INDEX pear_name ON pears (name(10), quantity DESC) INVISIBLE",
    )
    .await;
    let table = object(&mut session, "pears").await;
    let indexes = objects::indexes(&mut session, &table).await.unwrap();
    assert_eq!(indexes[0].columns, vec!["`name`(10)", "`quantity` DESC"]);
    let sql = objects::script(&mut session, &indexes[0].object, Script::Create)
        .await
        .unwrap();
    artifact(&source, "mysql-index.sql", &sql);
    run(&mut session, "DROP INDEX pear_name ON pears").await;
    run(&mut session, &sql).await;
    assert_eq!(
        run(
            &mut session,
            "SELECT INDEX_NAME, COLUMN_NAME, CAST(SUB_PART AS CHAR), COLLATION, IS_VISIBLE
        FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'pears'
        ORDER BY SEQ_IN_INDEX"
        )
        .await,
        vec![
            vec![
                Value::Text("pear_name".into()),
                Value::Text("name".into()),
                Value::Text("10".into()),
                Value::Text("A".into()),
                Value::Text("NO".into())
            ],
            vec![
                Value::Text("pear_name".into()),
                Value::Text("quantity".into()),
                Value::Null,
                Value::Text("D".into()),
                Value::Text("NO".into())
            ],
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_view_create_preserves_unicode_from_a_latin1_creation_context() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "SET NAMES latin1; CREATE VIEW pears AS SELECT _latin1 X'E9' AS name,
            CAST(COERCIBILITY(_latin1 X'E9') AS SIGNED) AS coercibility,
            _latin1 X'615C62276300' COLLATE latin1_bin AS escaped_text,
            _utf8mb4 X'E99BAA' AS mixed_name; SET NAMES utf8mb4",
    )
    .await;
    let view = object(&mut session, "pears").await;
    let sql = objects::script(&mut session, &view, Script::Create)
        .await
        .unwrap();
    artifact(&source, "mysql-view-latin1.sql", &sql);
    run(&mut session, "DROP VIEW pears").await;
    run(&mut session, &sql).await;
    assert_eq!(
        run(
            &mut session,
            "SELECT name, coercibility, escaped_text, mixed_name FROM pears"
        )
        .await,
        vec![vec![
            Value::Text("é".into()),
            Value::Integer(4),
            Value::Text("a\\b'c\0".into()),
            Value::Text("雪".into())
        ]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_select_scripts_omit_invisible_columns() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "CREATE TABLE pears (id int, hidden int INVISIBLE)",
    )
    .await;
    let table = object(&mut session, "pears").await;
    assert_eq!(
        objects::columns(&mut session, &table).await.unwrap(),
        vec![Column {
            name: "id".into(),
            type_name: "int".into(),
            nullable: true,
            default: None,
            identity: None,
            generated: None
        },]
    );
    assert_eq!(
        objects::script(&mut session, &table, Script::Select)
            .await
            .unwrap(),
        format!(
            "SELECT `id` FROM `{}`.`pears` LIMIT 1000;\n",
            source.database()
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_create_refuses_unquoted_show_definitions_without_changing_settings() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "CREATE TABLE `pear``box` (id int); SET sql_quote_show_create = OFF",
    )
    .await;
    let table = object(&mut session, "pear`box").await;
    assert!(matches!(
        objects::script(&mut session, &table, Script::Create).await,
        Err(objects::Error::QuotingDisabled)
    ));
    assert_eq!(
        run(&mut session, "SELECT CAST(@@sql_quote_show_create AS CHAR)").await,
        vec![vec![Value::Text("0".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_spatial_index_scripts_recreate_the_spatial_access_method() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "CREATE TABLE pears (location POINT NOT NULL SRID 4326);
        CREATE SPATIAL INDEX pear_location ON pears (location)",
    )
    .await;
    let table = object(&mut session, "pears").await;
    let indexes = objects::indexes(&mut session, &table).await.unwrap();
    let sql = objects::script(&mut session, &indexes[0].object, Script::Create)
        .await
        .unwrap();
    assert!(sql.contains("CREATE SPATIAL INDEX `pear_location`"));
    run(&mut session, "DROP INDEX pear_location ON pears").await;
    run(&mut session, &sql).await;
    assert_eq!(
        objects::indexes(&mut session, &table).await.unwrap()[0].method,
        "spatial"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_fulltext_index_scripts_preserve_the_parser() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "CREATE TABLE pears (description text, FULLTEXT KEY pear_description (description) WITH PARSER ngram)",
    )
    .await;
    let table = object(&mut session, "pears").await;
    let indexes = objects::indexes(&mut session, &table).await.unwrap();
    let sql = objects::script(&mut session, &indexes[0].object, Script::Create)
        .await
        .unwrap();
    assert!(sql.contains("WITH PARSER `ngram`"), "{sql}");
    artifact(&source, "mysql-fulltext-parser.sql", &sql);
    run(&mut session, "DROP INDEX pear_description ON pears").await;
    run(&mut session, &sql).await;
    let definition = run(&mut session, "SHOW CREATE TABLE pears").await;
    let Value::Text(definition) = &definition[0][1] else {
        panic!("Expected SHOW CREATE TABLE text");
    };
    assert!(definition.contains("WITH PARSER `ngram`"), "{definition}");
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_procedure_scripts_preserve_the_creation_collation_after_database_alter() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "ALTER DATABASE CHARACTER SET utf8mb4 COLLATE utf8mb4_bin;
        CREATE PROCEDURE pear_name(IN fruit_name varchar(20)) SELECT fruit_name;
        ALTER DATABASE CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci",
    )
    .await;
    let procedure = object(&mut session, "pear_name").await;
    let sql = objects::script(&mut session, &procedure, Script::Create)
        .await
        .unwrap();
    assert!(
        sql.contains("varchar(20) CHARACTER SET `utf8mb4` COLLATE `utf8mb4_bin`"),
        "{sql}"
    );
    assert!(!sql.contains("-- Created in a database"));
    artifact(&source, "mysql-procedure-collation.sql", &sql);
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_functional_index_scripts_recreate_the_expression() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "CREATE TABLE pears (quantity int);
        CREATE INDEX pear_quantity ON pears ((quantity * 2))",
    )
    .await;
    let table = object(&mut session, "pears").await;
    let indexes = objects::indexes(&mut session, &table).await.unwrap();
    assert_eq!(indexes[0].columns, vec!["((`quantity` * 2))"]);
    let sql = objects::script(&mut session, &indexes[0].object, Script::Create)
        .await
        .unwrap();
    run(&mut session, "DROP INDEX pear_quantity ON pears").await;
    run(&mut session, &sql).await;
    assert_eq!(
        objects::indexes(&mut session, &table).await.unwrap()[0].columns,
        vec!["((`quantity` * 2))"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_create_refuses_ansi_identifier_quotes_without_changing_sql_mode() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "CREATE TABLE pears (id int); SET sql_mode = 'ANSI_QUOTES'",
    )
    .await;
    let table = object(&mut session, "pears").await;
    assert!(matches!(
        objects::script(&mut session, &table, Script::Create).await,
        Err(objects::Error::AnsiQuotes)
    ));
    assert_eq!(
        run(&mut session, "SELECT @@sql_mode").await,
        vec![vec![Value::Text("ANSI_QUOTES".into())]]
    );
}
