//! The danger check's public contract for literal SQL batches.

use dbide_engine::danger::{DangerCheck, DangerKind, DangerousStatement, Engine, check};

macro_rules! flagged {
    ($name:ident, $engine:ident, $sql:literal, $kind:ident, $reason:literal, $parsed:literal) => {
        #[test]
        fn $name() {
            assert_eq!(
                check($sql, Engine::$engine),
                DangerCheck {
                    statements: vec![DangerousStatement {
                        kind: DangerKind::$kind,
                        reason: $reason,
                        line: 1
                    }],
                    fully_checked: $parsed,
                }
            );
        }
    };
}

macro_rules! clear {
    ($name:ident, $engine:ident, $sql:literal, $parsed:literal) => {
        #[test]
        fn $name() {
            assert_eq!(
                check($sql, Engine::$engine),
                DangerCheck {
                    statements: vec![],
                    fully_checked: $parsed,
                }
            );
        }
    };
}

flagged!(
    delete_without_where_is_flagged,
    SqlServer,
    "DELETE FROM t",
    Delete,
    "DELETE with no WHERE clause",
    true
);
flagged!(
    update_without_where_is_flagged,
    SqlServer,
    "UPDATE t SET x = 2",
    Update,
    "UPDATE with no WHERE clause",
    true
);
flagged!(
    delete_with_equal_numbers_is_flagged,
    PostgreSql,
    "DELETE FROM t WHERE 1=1",
    Delete,
    "DELETE with a trivially true WHERE clause",
    true
);
flagged!(
    update_with_spaced_equal_numbers_is_flagged,
    SqlServer,
    "UPDATE t SET x=2 WHERE 1 = 1",
    Update,
    "UPDATE with a trivially true WHERE clause",
    true
);
flagged!(
    delete_with_true_is_flagged,
    PostgreSql,
    "DELETE FROM t WHERE TRUE",
    Delete,
    "DELETE with a trivially true WHERE clause",
    true
);
flagged!(
    update_with_equal_strings_is_flagged,
    MySql,
    "UPDATE t SET x=2 WHERE 'a' = 'a'",
    Update,
    "UPDATE with a trivially true WHERE clause",
    true
);
flagged!(
    parenthesized_true_is_flagged,
    PostgreSql,
    "DELETE FROM t WHERE ((TRUE))",
    Delete,
    "DELETE with a trivially true WHERE clause",
    true
);
flagged!(
    equal_booleans_are_flagged,
    PostgreSql,
    "DELETE FROM t WHERE FALSE = FALSE",
    Delete,
    "DELETE with a trivially true WHERE clause",
    true
);
clear!(
    delete_with_where_is_not_flagged,
    SqlServer,
    "DELETE FROM t WHERE id=1",
    true
);
clear!(
    update_with_where_is_not_flagged,
    PostgreSql,
    "UPDATE t SET x=2 WHERE id=1",
    true
);
clear!(
    false_where_is_not_flagged,
    PostgreSql,
    "DELETE FROM t WHERE FALSE",
    true
);
clear!(
    unequal_literals_are_not_flagged,
    PostgreSql,
    "DELETE FROM t WHERE 1=2",
    true
);
clear!(
    null_equality_is_not_flagged,
    PostgreSql,
    "DELETE FROM t WHERE NULL=NULL",
    true
);
clear!(
    parameter_equality_is_not_flagged,
    PostgreSql,
    "DELETE FROM t WHERE $1=$1",
    true
);
clear!(
    column_equality_is_not_flagged,
    SqlServer,
    "DELETE FROM t WHERE id=id",
    true
);
clear!(
    true_prefix_with_restriction_is_not_flagged,
    SqlServer,
    "DELETE FROM t WHERE 1=1 AND id=1",
    true
);
clear!(
    drop_inside_a_comment_is_not_flagged,
    SqlServer,
    "-- DROP TABLE t\nSELECT 1; /* DROP TABLE t */",
    true
);
clear!(
    drop_inside_a_string_is_not_flagged,
    SqlServer,
    "SELECT 'DROP TABLE t; DELETE FROM t'",
    true
);
flagged!(
    drop_table_is_flagged,
    SqlServer,
    "DROP TABLE t",
    Drop,
    "DROP removes an object",
    true
);
flagged!(
    drop_view_is_flagged,
    PostgreSql,
    "DROP VIEW v",
    Drop,
    "DROP removes an object",
    true
);
flagged!(
    drop_function_is_flagged,
    PostgreSql,
    "DROP FUNCTION f()",
    Drop,
    "DROP removes an object",
    true
);
flagged!(
    truncate_is_flagged,
    MySql,
    "TRUNCATE TABLE t",
    Truncate,
    "TRUNCATE removes all rows",
    true
);
flagged!(
    drop_column_is_flagged,
    PostgreSql,
    "ALTER TABLE t DROP COLUMN x",
    DropColumn,
    "ALTER TABLE drops a column",
    true
);
clear!(
    drop_constraint_is_not_flagged,
    PostgreSql,
    "ALTER TABLE t DROP CONSTRAINT c",
    true
);
flagged!(
    merge_delete_by_source_is_flagged,
    SqlServer,
    "MERGE INTO t USING s ON t.id=s.id WHEN NOT MATCHED BY SOURCE THEN DELETE;",
    MergeDeleteBySource,
    "MERGE deletes rows not matched by source",
    true
);
flagged!(
    conditional_merge_delete_by_source_is_flagged,
    SqlServer,
    "MERGE INTO t USING s ON t.id=s.id WHEN NOT MATCHED BY SOURCE AND t.id=1 THEN DELETE;",
    MergeDeleteBySource,
    "MERGE deletes rows not matched by source",
    true
);
clear!(
    merge_matched_delete_is_not_flagged,
    SqlServer,
    "MERGE INTO t USING s ON t.id=s.id WHEN MATCHED THEN DELETE;",
    true
);
flagged!(
    restore_database_is_flagged,
    SqlServer,
    "RESTORE DATABASE sample FROM DISK = 'backup.bak'",
    RestoreDatabase,
    "RESTORE DATABASE overwrites a database",
    false
);
flagged!(
    offline_is_flagged,
    SqlServer,
    "ALTER DATABASE sample SET OFFLINE",
    AlterDatabase,
    "ALTER DATABASE sets OFFLINE",
    false
);
flagged!(
    single_user_is_flagged,
    SqlServer,
    "ALTER DATABASE sample SET SINGLE_USER WITH ROLLBACK IMMEDIATE",
    AlterDatabase,
    "ALTER DATABASE sets SINGLE_USER",
    false
);
flagged!(
    emergency_is_flagged,
    SqlServer,
    "ALTER DATABASE sample SET EMERGENCY",
    AlterDatabase,
    "ALTER DATABASE sets EMERGENCY",
    false
);
flagged!(
    repair_allow_data_loss_is_flagged,
    SqlServer,
    "DBCC CHECKDB (sample, REPAIR_ALLOW_DATA_LOSS)",
    RepairAllowDataLoss,
    "DBCC repair allows data loss",
    false
);
flagged!(
    shutdown_is_flagged,
    SqlServer,
    "SHUTDOWN WITH NOWAIT",
    Shutdown,
    "SHUTDOWN stops the server",
    false
);
flagged!(
    fallback_delete_without_where_is_flagged,
    SqlServer,
    "DELETE FROM t OPTION (RECOMPILE); UNRECOGNIZED",
    Delete,
    "DELETE with no WHERE clause",
    false
);

flagged!(
    equal_negative_numbers_are_flagged,
    PostgreSql,
    "DELETE FROM t WHERE -1=-1",
    Delete,
    "DELETE with a trivially true WHERE clause",
    true
);
flagged!(
    equal_decimal_values_are_flagged,
    PostgreSql,
    "DELETE FROM t WHERE 1.0=01",
    Delete,
    "DELETE with a trivially true WHERE clause",
    true
);
flagged!(
    equal_exponent_values_are_flagged,
    PostgreSql,
    "DELETE FROM t WHERE 10e-1=1",
    Delete,
    "DELETE with a trivially true WHERE clause",
    true
);
clear!(
    distinct_large_numbers_are_not_rounded_equal,
    PostgreSql,
    "DELETE FROM t WHERE 9007199254740992=9007199254740993",
    true
);
flagged!(
    fallback_update_true_is_flagged,
    PostgreSql,
    "UNRECOGNIZED; UPDATE t SET x=2 WHERE TRUE",
    Update,
    "UPDATE with a trivially true WHERE clause",
    false
);
flagged!(
    fallback_truncate_is_flagged,
    MySql,
    "UNRECOGNIZED; TRUNCATE TABLE t",
    Truncate,
    "TRUNCATE removes all rows",
    false
);
flagged!(
    fallback_drop_column_is_flagged,
    SqlServer,
    "UNRECOGNIZED; ALTER TABLE t DROP COLUMN x",
    DropColumn,
    "ALTER TABLE drops a column",
    false
);
flagged!(
    fallback_merge_delete_by_source_is_flagged,
    SqlServer,
    "UNRECOGNIZED; MERGE INTO t USING s ON t.id=s.id WHEN NOT MATCHED BY SOURCE AND t.id=1 THEN DELETE",
    MergeDeleteBySource,
    "MERGE deletes rows not matched by source",
    false
);
clear!(
    fallback_merge_actions_do_not_cross_branches,
    SqlServer,
    "UNRECOGNIZED; MERGE INTO t USING s ON t.id=s.id WHEN NOT MATCHED BY SOURCE THEN UPDATE SET x=1 WHEN MATCHED THEN DELETE",
    false
);
clear!(
    fallback_does_not_match_keyword_fragments,
    SqlServer,
    "UNRECOGNIZED; SELECT shutdown_status, drop_count FROM t",
    false
);
flagged!(
    fallback_keywords_are_case_insensitive,
    SqlServer,
    "dbcc checkdb (sample, repair_allow_data_loss)",
    RepairAllowDataLoss,
    "DBCC repair allows data loss",
    false
);
flagged!(
    delete_in_a_cte_is_flagged,
    PostgreSql,
    "WITH removed AS (DELETE FROM t RETURNING *) SELECT * FROM removed",
    Delete,
    "DELETE with no WHERE clause",
    true
);
clear!(
    fallback_skips_nested_comments,
    SqlServer,
    "UNRECOGNIZED; /* outer /* DROP TABLE t */ SHUTDOWN */ SELECT 1",
    false
);
clear!(
    fallback_skips_mysql_hash_comments,
    MySql,
    "UNRECOGNIZED; # DROP TABLE t\nSELECT 1",
    false
);
clear!(
    fallback_skips_postgres_escaped_strings,
    PostgreSql,
    "UNRECOGNIZED E'text \\' DROP TABLE t; SHUTDOWN'",
    false
);
clear!(
    repair_keyword_inside_a_string_is_not_flagged,
    SqlServer,
    "DBCC CHECKDB ('REPAIR_ALLOW_DATA_LOSS')",
    false
);
clear!(
    online_is_not_flagged,
    SqlServer,
    "ALTER DATABASE sample SET ONLINE",
    false
);
clear!(
    restore_verifyonly_is_not_flagged,
    SqlServer,
    "RESTORE VERIFYONLY FROM DISK='backup.bak'",
    false
);
clear!(
    unterminated_comment_contents_are_not_keywords,
    SqlServer,
    "SELECT 1; /* DROP TABLE t",
    false
);

#[test]
fn line_numbers_follow_the_multi_statement_batch() {
    assert_eq!(
        check(
            "-- header\nSELECT 1;\n\nDELETE FROM t;\n /* note */ DROP TABLE t;",
            Engine::SqlServer
        ),
        DangerCheck {
            statements: vec![
                DangerousStatement {
                    kind: DangerKind::Delete,
                    reason: "DELETE with no WHERE clause",
                    line: 4
                },
                DangerousStatement {
                    kind: DangerKind::Drop,
                    reason: "DROP removes an object",
                    line: 5
                },
            ],
            fully_checked: true,
        }
    );
}

#[test]
fn unparseable_batch_falls_back_and_says_so() {
    assert_eq!(
        check(
            "SHUTDOWN;\nDELETE FROM t WHERE 'a'='a';\nDROP TABLE t",
            Engine::SqlServer
        ),
        DangerCheck {
            statements: vec![
                DangerousStatement {
                    kind: DangerKind::Shutdown,
                    reason: "SHUTDOWN stops the server",
                    line: 1
                },
                DangerousStatement {
                    kind: DangerKind::Delete,
                    reason: "DELETE with a trivially true WHERE clause",
                    line: 2
                },
                DangerousStatement {
                    kind: DangerKind::Drop,
                    reason: "DROP removes an object",
                    line: 3
                },
            ],
            fully_checked: false,
        }
    );
}

flagged!(
    sql_server_dialect_supports_brackets,
    SqlServer,
    "UPDATE [order] SET [value]=2",
    Update,
    "UPDATE with no WHERE clause",
    true
);
flagged!(
    postgres_dialect_supports_returning,
    PostgreSql,
    "DELETE FROM \"order\" RETURNING *",
    Delete,
    "DELETE with no WHERE clause",
    true
);
flagged!(
    mysql_dialect_supports_backticks_and_limit,
    MySql,
    "DELETE FROM `order` LIMIT 2",
    Delete,
    "DELETE with no WHERE clause",
    true
);
clear!(empty_batch_is_fully_checked, SqlServer, "", true);
clear!(
    comments_only_are_fully_checked,
    PostgreSql,
    "/* comment */ -- comment",
    true
);
clear!(
    unknown_statement_is_not_fully_checked,
    SqlServer,
    "UNRECOGNIZED nonsense",
    false
);
clear!(
    fallback_skips_comments_and_strings,
    SqlServer,
    "UNRECOGNIZED 'DROP TABLE t; SHUTDOWN'; /* DELETE FROM t */ -- UPDATE t\n",
    false
);
clear!(
    fallback_skips_bracketed_identifiers,
    SqlServer,
    "UNRECOGNIZED [DROP] [SHUTDOWN]",
    false
);
clear!(
    fallback_skips_double_quoted_identifiers,
    PostgreSql,
    "UNRECOGNIZED \"DROP\" \"SHUTDOWN\"",
    false
);
clear!(
    fallback_skips_backtick_identifiers,
    MySql,
    "UNRECOGNIZED `DROP` `SHUTDOWN`",
    false
);
clear!(
    fallback_skips_dollar_quoted_strings,
    PostgreSql,
    "UNRECOGNIZED $tag$ DROP TABLE t; SHUTDOWN $tag$",
    false
);
clear!(
    fallback_skips_escaped_quotes,
    SqlServer,
    "UNRECOGNIZED 'text '' DROP TABLE t; SHUTDOWN'",
    false
);
clear!(
    fallback_keeps_real_where,
    SqlServer,
    "UNRECOGNIZED; DELETE FROM t WHERE id=1",
    false
);
clear!(
    fallback_does_not_use_a_true_prefix,
    SqlServer,
    "UNRECOGNIZED; DELETE FROM t WHERE TRUE AND id=1",
    false
);
clear!(
    fallback_ignores_subquery_where,
    SqlServer,
    "UNRECOGNIZED; DELETE FROM t WHERE id IN (SELECT id FROM s WHERE TRUE)",
    false
);
flagged!(
    fallback_does_not_borrow_subquery_where,
    SqlServer,
    "UPDATE t SET x=(SELECT x FROM s WHERE id=1) OPTION (RECOMPILE)",
    Update,
    "UPDATE with no WHERE clause",
    false
);
flagged!(
    unterminated_string_keeps_earlier_findings,
    SqlServer,
    "DROP TABLE t; SELECT 'unfinished",
    Drop,
    "DROP removes an object",
    false
);
clear!(
    unterminated_string_contents_are_not_keywords,
    SqlServer,
    "SELECT 'unfinished DROP TABLE t",
    false
);

flagged!(
    fallback_finds_delete_in_a_cte,
    PostgreSql,
    "UNRECOGNIZED; WITH removed AS (DELETE FROM t RETURNING *) SELECT * FROM removed",
    Delete,
    "DELETE with no WHERE clause",
    false
);
clear!(
    fallback_preserves_table_hint_where,
    SqlServer,
    "UNRECOGNIZED; DELETE FROM t WITH (TABLOCK) WHERE id=1",
    false
);
flagged!(
    fallback_finds_drop_after_alter_without_a_semicolon,
    SqlServer,
    "ALTER DATABASE sample SET ONLINE DROP TABLE t",
    Drop,
    "DROP removes an object",
    false
);
flagged!(
    fallback_omitted_column_keyword_is_flagged,
    SqlServer,
    "UNRECOGNIZED; ALTER TABLE t DROP x",
    DropColumn,
    "ALTER TABLE drops a column",
    false
);

#[test]
fn nested_drop_uses_its_own_line() {
    assert_eq!(
        check(
            "CREATE PROCEDURE p AS BEGIN\nDROP TABLE t;\nEND",
            Engine::SqlServer
        ),
        DangerCheck {
            statements: vec![DangerousStatement {
                kind: DangerKind::Drop,
                reason: "DROP removes an object",
                line: 2
            }],
            fully_checked: true,
        }
    );
}

#[test]
fn nested_drop_function_uses_its_own_line() {
    assert_eq!(
        check(
            "CREATE PROCEDURE p AS BEGIN\nDROP FUNCTION f;\nEND",
            Engine::SqlServer
        ),
        DangerCheck {
            statements: vec![DangerousStatement {
                kind: DangerKind::Drop,
                reason: "DROP removes an object",
                line: 2
            }],
            fully_checked: true,
        }
    );
}
