use super::*;

#[test]
fn self_join_is_not_a_single_table_query() {
    assert!(!single_table_query(
        "SELECT a.id, b.name FROM fruit a JOIN fruit b ON b.id = a.id + 1",
        Engine::PostgreSql
    ));
}

#[test]
fn single_table_alias_is_a_single_table_query() {
    assert!(single_table_query(
        "SELECT f.id, f.name FROM fruit AS f",
        Engine::PostgreSql
    ));
}

#[test]
fn comma_join_is_not_a_single_table_query() {
    assert!(!single_table_query(
        "SELECT a.id FROM fruit a, fruit b",
        Engine::PostgreSql
    ));
}

#[test]
fn derived_table_is_not_a_single_table_query() {
    assert!(!single_table_query(
        "SELECT id FROM (SELECT id FROM fruit) f",
        Engine::PostgreSql
    ));
}

#[test]
fn cte_is_not_a_single_table_query() {
    assert!(!single_table_query(
        "WITH f AS (SELECT id FROM fruit) SELECT id FROM f",
        Engine::PostgreSql
    ));
}

#[test]
fn unparseable_statement_is_not_a_single_table_query() {
    assert!(!single_table_query(
        "SELECT id FROM fruit WHERE",
        Engine::PostgreSql
    ));
}

#[test]
fn multiple_statements_are_not_a_single_table_query() {
    assert!(!single_table_query(
        "SELECT id FROM fruit; SELECT id FROM fruit",
        Engine::PostgreSql
    ));
}

#[test]
fn union_is_not_a_single_table_query() {
    assert!(!single_table_query(
        "SELECT id FROM fruit UNION ALL SELECT id FROM fruit",
        Engine::PostgreSql
    ));
}

#[test]
fn table_function_is_not_a_single_table_query() {
    assert!(!single_table_query(
        "SELECT id FROM fruit()",
        Engine::PostgreSql
    ));
}

#[test]
fn sql_server_single_table_uses_its_dialect() {
    assert!(single_table_query(
        "SELECT TOP 200 [id] FROM [fruit] AS f",
        Engine::SqlServer
    ));
}

#[test]
fn mysql_single_table_uses_its_dialect() {
    assert!(single_table_query(
        "SELECT `id` FROM `fruit` AS f LIMIT 200",
        Engine::MySql
    ));
}

#[test]
fn sql_server_float_originals_remain_numeric() {
    let script = result(Engine::SqlServer)
        .script(&[
            Change::Update {
                original: vec![Value::Float(0.1), Value::Float(0.2), Value::Null],
                cells: vec![Cell {
                    column: 1,
                    value: Value::Float(0.3),
                }],
            },
            Change::Delete {
                original: vec![Value::Float(0.1), Value::Float(0.2), Value::Null],
            },
        ])
        .unwrap();
    assert_eq!(
        script.sql(),
        "UPDATE [odd.schema].[t]]`\"] SET [v]]`\"] = 3e-1 WHERE [id] = 1e-1 AND [v]]`\"] = 2e-1;\nDELETE FROM [odd.schema].[t]]`\"] WHERE [id] = 1e-1 AND [v]]`\"] = 2e-1;\n"
    );
}

#[test]
fn mysql_float_originals_remain_numeric() {
    let script = result(Engine::MySql)
        .script(&[
            Change::Update {
                original: vec![Value::Float(0.1), Value::Float(0.2), Value::Null],
                cells: vec![Cell {
                    column: 1,
                    value: Value::Float(0.3),
                }],
            },
            Change::Delete {
                original: vec![Value::Float(0.1), Value::Float(0.2), Value::Null],
            },
        ])
        .unwrap();
    assert_eq!(
        script.sql(),
        "UPDATE `odd.schema`.`t]``\"` SET `v]``\"` = 3e-1 WHERE `id` = 1e-1 AND `v]``\"` = 2e-1;\nDELETE FROM `odd.schema`.`t]``\"` WHERE `id` = 1e-1 AND `v]``\"` = 2e-1;\n"
    );
}

#[test]
fn postgresql_update_compares_float_keys_in_the_column_type() {
    let script = result(Engine::PostgreSql)
        .script(&[Change::Update {
            original: vec![Value::Float(0.1), Value::Null, Value::Null],
            cells: vec![Cell {
                column: 1,
                value: Value::Text("pear".into()),
            }],
        }])
        .unwrap();
    assert_eq!(
        script.sql(),
        "UPDATE \"odd.schema\".\"t]`\"\"\" SET \"v]`\"\"\" = E'pear' WHERE \"id\" = E'0.1' AND \"v]`\"\"\" IS NULL;\n"
    );
}

#[test]
fn postgresql_delete_compares_float_originals_in_the_column_type() {
    let script = result(Engine::PostgreSql)
        .script(&[Change::Delete {
            original: vec![Value::Integer(7), Value::Float(0.1), Value::Null],
        }])
        .unwrap();
    assert_eq!(
        script.sql(),
        "DELETE FROM \"odd.schema\".\"t]`\"\"\" WHERE \"id\" = E'7' AND \"v]`\"\"\" = E'0.1';\n"
    );
}

fn result(engine: Engine) -> EditableResult {
    EditableResult {
        engine,
        table: vec!["odd.schema".into(), "t]`\"".into()],
        columns: vec![
            EditColumn {
                name: Some("id".into()),
                read_only: false,
                comparable: true,
            },
            EditColumn {
                name: Some("v]`\"".into()),
                read_only: false,
                comparable: true,
            },
            EditColumn {
                name: Some("document".into()),
                read_only: false,
                comparable: false,
            },
        ],
        key: vec![0],
    }
}

#[test]
fn update_checks_null_originals_and_quotes_postgresql_identifiers_and_literals() {
    let script = result(Engine::PostgreSql)
        .script(&[Change::Update {
            original: vec![Value::Integer(7), Value::Null, Value::Json("{}".into())],
            cells: vec![Cell {
                column: 1,
                value: Value::Text("O'Brien\\tree".into()),
            }],
        }])
        .unwrap();
    assert_eq!(
        script.sql(),
        "UPDATE \"odd.schema\".\"t]`\"\"\" SET \"v]`\"\"\" = E'O''Brien\\\\tree' WHERE \"id\" = E'7' AND \"v]`\"\"\" IS NULL;\n"
    );
}

#[test]
fn update_checks_null_originals_and_quotes_sql_server_identifiers_and_literals() {
    let script = result(Engine::SqlServer)
        .script(&[Change::Update {
            original: vec![Value::Integer(7), Value::Null, Value::Other("xml".into())],
            cells: vec![Cell {
                column: 1,
                value: Value::Text("O'Brien\\tree".into()),
            }],
        }])
        .unwrap();
    assert_eq!(
        script.sql(),
        "UPDATE [odd.schema].[t]]`\"] SET [v]]`\"] = N'O''Brien\\tree' WHERE [id] = 7 AND [v]]`\"] IS NULL;\n"
    );
}

#[test]
fn update_checks_null_originals_and_quotes_identifiers_and_literals_for_mysql() {
    let script = result(Engine::MySql)
        .script(&[Change::Update {
            original: vec![Value::Integer(7), Value::Null, Value::Json("{}".into())],
            cells: vec![Cell {
                column: 1,
                value: Value::Text("O'Brien\\tree".into()),
            }],
        }])
        .unwrap();
    assert_eq!(
        script.sql(),
        "UPDATE `odd.schema`.`t]``\"` SET `v]``\"` = CONVERT(X'4f27427269656e5c74726565' USING utf8mb4) WHERE `id` = 7 AND `v]``\"` IS NULL;\n"
    );
}

#[test]
fn delete_omits_noncomparable_postgresql_columns_but_checks_comparable_originals() {
    let script = result(Engine::PostgreSql)
        .script(&[Change::Delete {
            original: vec![
                Value::Integer(7),
                Value::Text("old".into()),
                Value::Json("{}".into()),
            ],
        }])
        .unwrap();
    assert_eq!(
        script.sql(),
        "DELETE FROM \"odd.schema\".\"t]`\"\"\" WHERE \"id\" = E'7' AND \"v]`\"\"\" = E'old';\n"
    );
}

#[test]
fn delete_omits_noncomparable_sql_server_columns_but_checks_null_originals() {
    let script = result(Engine::SqlServer)
        .script(&[Change::Delete {
            original: vec![Value::Integer(7), Value::Null, Value::Other("xml".into())],
        }])
        .unwrap();
    assert_eq!(
        script.sql(),
        "DELETE FROM [odd.schema].[t]]`\"] WHERE [id] = 7 AND [v]]`\"] IS NULL;\n"
    );
}

#[test]
fn delete_checks_comparable_columns_for_mysql() {
    let script = result(Engine::MySql)
        .script(&[Change::Delete {
            original: vec![
                Value::Integer(7),
                Value::Null,
                Value::Other("unsupported".into()),
            ],
        }])
        .unwrap();
    assert_eq!(
        script.sql(),
        "DELETE FROM `odd.schema`.`t]``\"` WHERE `id` = 7 AND `v]``\"` IS NULL;\n"
    );
}

#[test]
fn insert_includes_only_filled_postgresql_cells_and_preserves_explicit_null() {
    let script = result(Engine::PostgreSql)
        .script(&[Change::Insert {
            cells: vec![Cell {
                column: 1,
                value: Value::Null,
            }],
        }])
        .unwrap();
    assert_eq!(
        script.sql(),
        "INSERT INTO \"odd.schema\".\"t]`\"\"\" (\"v]`\"\"\") VALUES (NULL);\n"
    );
    assert_eq!(script.statements()[0].kind(), Kind::Insert);
}

#[test]
fn insert_includes_only_filled_sql_server_cells() {
    let script = result(Engine::SqlServer)
        .script(&[Change::Insert {
            cells: vec![Cell {
                column: 1,
                value: Value::Text("O'Brien".into()),
            }],
        }])
        .unwrap();
    assert_eq!(
        script.sql(),
        "INSERT INTO [odd.schema].[t]]`\"] ([v]]`\"]) VALUES (N'O''Brien');\n"
    );
}

#[test]
fn insert_includes_only_filled_cells_for_mysql() {
    let script = result(Engine::MySql)
        .script(&[Change::Insert {
            cells: vec![Cell {
                column: 1,
                value: Value::Text("O'Brien".into()),
            }],
        }])
        .unwrap();
    assert_eq!(
        script.sql(),
        "INSERT INTO `odd.schema`.`t]``\"` (`v]``\"`) VALUES (CONVERT(X'4f27427269656e' USING utf8mb4));\n"
    );
}

#[test]
fn blank_insert_uses_postgresql_defaults() {
    assert_eq!(
        result(Engine::PostgreSql)
            .script(&[Change::Insert { cells: vec![] }])
            .unwrap()
            .sql(),
        "INSERT INTO \"odd.schema\".\"t]`\"\"\" DEFAULT VALUES;\n"
    );
}

#[test]
fn blank_insert_uses_sql_server_defaults() {
    assert_eq!(
        result(Engine::SqlServer)
            .script(&[Change::Insert { cells: vec![] }])
            .unwrap()
            .sql(),
        "INSERT INTO [odd.schema].[t]]`\"] DEFAULT VALUES;\n"
    );
}

#[test]
fn blank_insert_uses_empty_values_syntax_for_mysql() {
    assert_eq!(
        result(Engine::MySql)
            .script(&[Change::Insert { cells: vec![] }])
            .unwrap()
            .sql(),
        "INSERT INTO `odd.schema`.`t]``\"` () VALUES ();\n"
    );
}

#[test]
fn key_edits_match_the_original_key_and_each_original_only_once() {
    let script = result(Engine::PostgreSql)
        .script(&[Change::Update {
            original: vec![Value::Integer(7), Value::Null, Value::Null],
            cells: vec![Cell {
                column: 0,
                value: Value::Integer(8),
            }],
        }])
        .unwrap();
    assert_eq!(
        script.sql(),
        "UPDATE \"odd.schema\".\"t]`\"\"\" SET \"id\" = 8 WHERE \"id\" = E'7';\n"
    );
}

#[test]
fn wrong_original_width_is_refused_before_indexing() {
    assert!(matches!(
        result(Engine::PostgreSql).script(&[Change::Delete { original: vec![] }]),
        Err(Error::InvalidChange(_))
    ));
}

#[test]
fn null_original_key_is_refused() {
    assert!(matches!(
        result(Engine::PostgreSql).script(&[Change::Delete {
            original: vec![Value::Null, Value::Null, Value::Null]
        }]),
        Err(Error::InvalidChange(_))
    ));
}

#[test]
fn duplicate_cells_are_refused() {
    assert!(matches!(
        result(Engine::PostgreSql).script(&[Change::Insert {
            cells: vec![
                Cell {
                    column: 0,
                    value: Value::Integer(1)
                },
                Cell {
                    column: 0,
                    value: Value::Integer(2)
                }
            ]
        }]),
        Err(Error::InvalidChange(_))
    ));
}

#[test]
fn read_only_columns_cannot_be_filled() {
    let mut result = result(Engine::PostgreSql);
    result.columns[0].read_only = true;
    assert!(matches!(
        result.script(&[Change::Insert {
            cells: vec![Cell {
                column: 0,
                value: Value::Integer(1)
            }]
        }]),
        Err(Error::InvalidChange(_))
    ));
}

#[test]
fn noncomparable_edits_are_refused_instead_of_overwriting_without_an_original_check() {
    assert!(matches!(
        result(Engine::PostgreSql).script(&[Change::Update {
            original: vec![Value::Integer(1), Value::Null, Value::Json("{}".into())],
            cells: vec![Cell {
                column: 2,
                value: Value::Json("[]".into())
            }]
        }]),
        Err(Error::InvalidChange(_))
    ));
}

#[test]
fn invalid_numeric_literals_cannot_inject_a_statement() {
    assert!(matches!(
        result(Engine::PostgreSql).script(&[Change::Insert {
            cells: vec![Cell {
                column: 0,
                value: Value::Decimal("1); DELETE FROM fruit; --".into())
            }]
        }]),
        Err(Error::Sql(_))
    ));
}
