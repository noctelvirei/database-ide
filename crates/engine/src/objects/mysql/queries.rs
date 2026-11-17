//! MySQL 8.4 catalog SQL; substitutions are UTF8 hex literals, never SQL fragments from callers.
//! Name predicates convert utf8mb3 catalog text before comparing it with UTF8 literals.

/// Visible user databases; enumeration opens no additional connections.
pub(super) const DATABASES: &str = r#"
SELECT CAST(JSON_OBJECT('id', JSON_OBJECT('database', SCHEMA_NAME,
    'key', CONCAT('database:', HEX(SCHEMA_NAME))), 'schema', NULL, 'name', SCHEMA_NAME,
    'kind', 'database', 'system', CAST('false' AS JSON), 'signature', NULL) AS CHAR CHARACTER SET utf8mb4)
FROM information_schema.SCHEMATA
WHERE SCHEMA_NAME NOT IN ('mysql', 'sys', 'information_schema', 'performance_schema')
ORDER BY BINARY SCHEMA_NAME
"#;

/// Names identify objects; table name scopes index names, which are not database-unique.
pub(super) const CATALOG: &str = r#"
SELECT CONCAT('table:', HEX(TABLE_NAME)) AS object_key, TABLE_SCHEMA AS schema_name,
    TABLE_NAME AS name, IF(TABLE_TYPE = 'VIEW', 'view', 'table') AS kind
FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE()
UNION ALL
SELECT CONCAT(LOWER(ROUTINE_TYPE), ':', HEX(ROUTINE_NAME)), ROUTINE_SCHEMA,
    ROUTINE_NAME, LOWER(ROUTINE_TYPE)
FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA = DATABASE()
UNION ALL
SELECT DISTINCT CONCAT('index:', HEX(TABLE_NAME), ':', HEX(INDEX_NAME)), TABLE_SCHEMA,
    INDEX_NAME, 'index' FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = DATABASE()
UNION ALL
SELECT CONCAT('schema:', HEX(SCHEMA_NAME)), NULL, SCHEMA_NAME, 'schema'
FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = DATABASE()
"#;

/// Common object shape, including the synthetic database-as-schema node.
pub(super) const OBJECT_JSON: &str = r#"JSON_OBJECT(
    'id', JSON_OBJECT('database', DATABASE(), 'key', object_key),
    'schema', schema_name, 'name', name, 'kind', kind,
    'system', CAST('false' AS JSON), 'signature', NULL)"#;

/// Column expressions use server text; ordinary defaults become mode-independent SQL literals.
pub(super) const COLUMNS: &str = r#"
SELECT CAST(JSON_OBJECT('object_key', CONCAT('table:', HEX(TABLE_NAME)), 'name', COLUMN_NAME, 'type_name', COLUMN_TYPE,
    'nullable', CAST(IF(IS_NULLABLE = 'YES', 'true', 'false') AS JSON),
    'default', CASE WHEN EXTRA LIKE '%auto_increment%' OR GENERATION_EXPRESSION <> '' THEN NULL
        WHEN EXTRA LIKE '%DEFAULT_GENERATED%' THEN COLUMN_DEFAULT
        WHEN COLUMN_DEFAULT IS NULL THEN NULL
        WHEN DATA_TYPE IN ('tinyint','smallint','mediumint','int','bigint','decimal','float','double','bit')
            THEN COLUMN_DEFAULT
        ELSE CONCAT('_utf8mb4 X', CHAR(39), HEX(COLUMN_DEFAULT), CHAR(39)) END,
    'identity', IF(EXTRA LIKE '%auto_increment%', 'by_default', NULL),
    'generated', IF(COALESCE(GENERATION_EXPRESSION, '') = '', NULL,
        JSON_OBJECT('expression', GENERATION_EXPRESSION,
            'stored', CAST(IF(EXTRA LIKE '%STORED GENERATED%', 'true', 'false') AS JSON))))
    AS CHAR CHARACTER SET utf8mb4)
FROM information_schema.COLUMNS
"#;

/// One row per index key part, retaining order without GROUP_CONCAT's length limit.
pub(super) const INDEXES: &str = r#"
SELECT CAST(JSON_OBJECT('name', INDEX_NAME, 'column', COLUMN_NAME, 'expression', EXPRESSION,
    'prefix', SUB_PART, 'descending', IF(COLLATION = 'D', 1, 0), 'non_unique', NON_UNIQUE,
    'method', INDEX_TYPE, 'visible', IF(IS_VISIBLE = 'YES', 1, 0))
    AS CHAR CHARACTER SET utf8mb4)
FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = DATABASE()
    AND CONVERT(TABLE_NAME USING utf8mb4) COLLATE utf8mb4_bin = {table}
ORDER BY BINARY INDEX_NAME, SEQ_IN_INDEX
"#;

/// Ordered foreign key parts with their target identity and referential actions.
pub(super) const FOREIGN_KEYS: &str = r#"
SELECT CAST(JSON_OBJECT('name', k.CONSTRAINT_NAME, 'column', k.COLUMN_NAME,
    'database', k.REFERENCED_TABLE_SCHEMA, 'table', k.REFERENCED_TABLE_NAME,
    'target', k.REFERENCED_COLUMN_NAME, 'update', r.UPDATE_RULE, 'delete', r.DELETE_RULE)
    AS CHAR CHARACTER SET utf8mb4)
FROM information_schema.KEY_COLUMN_USAGE k
JOIN information_schema.REFERENTIAL_CONSTRAINTS r ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA
    AND r.TABLE_NAME = k.TABLE_NAME AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME
WHERE k.TABLE_SCHEMA = DATABASE()
    AND CONVERT(k.TABLE_NAME USING utf8mb4) COLLATE utf8mb4_bin = {table}
ORDER BY BINARY k.CONSTRAINT_NAME, k.ORDINAL_POSITION
"#;

/// Check expressions include enforcement, which MySQL permits disabling per constraint.
pub(super) const CHECKS: &str = r#"
SELECT CAST(JSON_OBJECT('name', t.CONSTRAINT_NAME, 'kind', 'check',
    'definition', CONCAT('CHECK (', c.CHECK_CLAUSE, ')', IF(t.ENFORCED = 'NO', ' NOT ENFORCED', '')))
    AS CHAR CHARACTER SET utf8mb4)
FROM information_schema.TABLE_CONSTRAINTS t JOIN information_schema.CHECK_CONSTRAINTS c
    ON c.CONSTRAINT_SCHEMA = t.CONSTRAINT_SCHEMA AND c.CONSTRAINT_NAME = t.CONSTRAINT_NAME
WHERE t.TABLE_SCHEMA = DATABASE()
    AND CONVERT(t.TABLE_NAME USING utf8mb4) COLLATE utf8mb4_bin = {table}
    AND t.CONSTRAINT_TYPE = 'CHECK'
ORDER BY BINARY t.CONSTRAINT_NAME
"#;
