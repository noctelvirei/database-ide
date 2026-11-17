//! SQL Server catalog projections; substitutions are validated keys or escaped Unicode literals.

/// Lazy nodes, with index identities scoped to their owning object.
pub(super) const CATALOG: &str = r#"
SELECT CONCAT('object:', o.object_id) AS [key], s.name AS [schema], o.name,
    CASE o.type WHEN 'U' THEN 'table' WHEN 'V' THEN 'view' WHEN 'P' THEN 'procedure'
        ELSE 'function' END AS kind, o.is_ms_shipped AS [system]
FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id
WHERE o.type IN ('U','V','P','FN','IF','TF')
UNION ALL
SELECT CONCAT('index:', i.object_id, ':', i.index_id), s.name, i.name, 'index', o.is_ms_shipped
FROM sys.indexes i JOIN sys.objects o ON o.object_id = i.object_id
JOIN sys.schemas s ON s.schema_id = o.schema_id
WHERE i.index_id > 0 AND i.is_hypothetical = 0 AND o.type IN ('U','V')
UNION ALL
SELECT CONCAT('schema:', schema_id), NULL, name, 'schema',
    CAST(CASE WHEN name IN ('sys','INFORMATION_SCHEMA') OR schema_id BETWEEN 16384 AND 16393
        THEN 1 ELSE 0 END AS bit) FROM sys.schemas
"#;

/// Declaration-order columns, including SQL Server details absent from the common column type.
pub(super) const COLUMNS: &str = r#"
SELECT (SELECT c.name, CASE WHEN t.is_user_defined = 1 THEN QUOTENAME(SCHEMA_NAME(t.schema_id))
    + N'.' + QUOTENAME(t.name) ELSE t.name + CASE
    WHEN t.name IN ('varchar','char','varbinary','binary','nvarchar','nchar') THEN
        N'(' + CASE WHEN c.max_length = -1 THEN N'max' ELSE CONVERT(nvarchar(10),
            CASE WHEN t.name IN ('nvarchar','nchar') THEN c.max_length / 2 ELSE c.max_length END) END + N')'
    WHEN t.name IN ('decimal','numeric') THEN N'(' + CONVERT(nvarchar(10),c.precision)
        + N',' + CONVERT(nvarchar(10),c.scale) + N')'
    WHEN t.name IN ('time','datetime2','datetimeoffset') THEN N'(' + CONVERT(nvarchar(10),c.scale) + N')'
    WHEN t.name = 'float' THEN N'(' + CONVERT(nvarchar(10),c.precision) + N')'
    ELSE N'' END END AS type_name,
    c.is_nullable AS nullable, d.definition AS [default],
    CASE WHEN c.is_identity = 1 THEN 'always' END AS [identity],
    JSON_QUERY(CASE WHEN cc.object_id IS NOT NULL THEN
        (SELECT cc.definition AS expression, cc.is_persisted AS stored FOR JSON PATH, WITHOUT_ARRAY_WRAPPER) END) AS generated,
    CONVERT(nvarchar(100), ic.seed_value) AS identity_seed,
    CONVERT(nvarchar(100), ic.increment_value) AS identity_increment,
    ic.is_not_for_replication AS identity_not_for_replication,
    d.name AS default_constraint, c.collation_name AS collation,
    c.is_sparse AS sparse, c.is_rowguidcol AS rowguidcol,
    CAST(CASE WHEN c.generated_always_type <> 0 OR c.is_filestream = 1 OR c.is_column_set = 1
        OR c.encryption_type IS NOT NULL OR c.xml_collection_id <> 0
        OR EXISTS (SELECT 1 FROM sys.masked_columns mc WHERE mc.object_id = c.object_id
            AND mc.column_id = c.column_id AND mc.is_masked = 1)
        THEN 1 ELSE 0 END AS bit) AS unsupported
    FOR JSON PATH, WITHOUT_ARRAY_WRAPPER)
FROM sys.columns c JOIN sys.types t ON t.user_type_id = c.user_type_id
LEFT JOIN sys.computed_columns cc ON cc.object_id = c.object_id AND cc.column_id = c.column_id
LEFT JOIN sys.identity_columns ic ON ic.object_id = c.object_id AND ic.column_id = c.column_id
LEFT JOIN sys.default_constraints d ON d.object_id = c.default_object_id
WHERE c.object_id = {key} AND c.is_hidden = 0 ORDER BY c.column_id
"#;

/// Key constraints preserve sort direction and clustering; foreign keys preserve both actions.
pub(super) const CONSTRAINTS: &str = r#"
SELECT (SELECT name, kind, definition, disabled, untrusted FOR JSON PATH, WITHOUT_ARRAY_WRAPPER)
FROM (
    SELECT k.name, CASE k.type WHEN 'PK' THEN 'primary_key' ELSE 'unique' END AS kind,
        CAST(CASE k.type WHEN 'PK' THEN N'PRIMARY KEY ' ELSE N'UNIQUE ' END AS nvarchar(max))
        + i.type_desc + N' (' + (
            SELECT STRING_AGG(CAST(QUOTENAME(c.name) + CASE WHEN ic.is_descending_key = 1
                THEN N' DESC' ELSE N' ASC' END AS nvarchar(max)), N', ')
                WITHIN GROUP (ORDER BY ic.key_ordinal)
            FROM sys.index_columns ic JOIN sys.columns c
                ON c.object_id = ic.object_id AND c.column_id = ic.column_id
            WHERE ic.object_id = k.parent_object_id AND ic.index_id = k.unique_index_id
                AND ic.key_ordinal > 0) + N')' AS definition,
        CAST(0 AS bit) AS disabled, CAST(0 AS bit) AS untrusted
    FROM sys.key_constraints k JOIN sys.indexes i
        ON i.object_id = k.parent_object_id AND i.index_id = k.unique_index_id
    WHERE k.parent_object_id = {key}
    UNION ALL
    SELECT f.name, 'foreign_key', N'FOREIGN KEY (' + (
        SELECT STRING_AGG(CAST(QUOTENAME(COL_NAME(fc.parent_object_id, fc.parent_column_id)) AS nvarchar(max)), N', ')
            WITHIN GROUP (ORDER BY fc.constraint_column_id)
        FROM sys.foreign_key_columns fc WHERE fc.constraint_object_id = f.object_id)
        + N') REFERENCES ' + QUOTENAME(OBJECT_SCHEMA_NAME(f.referenced_object_id))
        + N'.' + QUOTENAME(OBJECT_NAME(f.referenced_object_id)) + N' (' + (
        SELECT STRING_AGG(CAST(QUOTENAME(COL_NAME(fc.referenced_object_id, fc.referenced_column_id)) AS nvarchar(max)), N', ')
            WITHIN GROUP (ORDER BY fc.constraint_column_id)
        FROM sys.foreign_key_columns fc WHERE fc.constraint_object_id = f.object_id)
        + N') ON DELETE ' + REPLACE(f.delete_referential_action_desc, N'_', N' ')
        + N' ON UPDATE ' + REPLACE(f.update_referential_action_desc, N'_', N' ')
        + CASE WHEN f.is_not_for_replication = 1 THEN N' NOT FOR REPLICATION' ELSE N'' END,
        f.is_disabled, f.is_not_trusted
    FROM sys.foreign_keys f WHERE f.parent_object_id = {key}
    UNION ALL
    SELECT name, 'check', N'CHECK ' + CASE WHEN is_not_for_replication = 1
        THEN N'NOT FOR REPLICATION ' ELSE N'' END + definition, is_disabled, is_not_trusted
    FROM sys.check_constraints WHERE parent_object_id = {key}
) rules ORDER BY name COLLATE Latin1_General_100_BIN2
"#;

/// All index metadata used by the explorer and rowstore CREATE INDEX scripts.
pub(super) const INDEXES: &str = r#"
SELECT (SELECT i.index_id, i.name, i.type_desc AS method, i.is_unique AS [unique],
    CAST(CASE WHEN i.is_primary_key = 1 OR i.is_unique_constraint = 1 THEN 1 ELSE 0 END AS bit) AS constraint_backed,
    i.filter_definition AS filter, i.is_disabled AS disabled,
    JSON_QUERY((SELECT c.name, ic.is_descending_key AS descending, ic.is_included_column AS included
        FROM sys.index_columns ic JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id
        WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id
        ORDER BY ic.is_included_column, ic.key_ordinal, ic.index_column_id FOR JSON PATH)) AS columns
    FOR JSON PATH, WITHOUT_ARRAY_WRAPPER)
FROM sys.indexes i WHERE i.object_id = {key} AND i.index_id > 0 AND i.is_hypothetical = 0
ORDER BY i.name COLLATE Latin1_General_100_BIN2
"#;

/// Source settings accompany definitions; NULL definitions are rejected by the caller.
pub(super) const MODULES: &str = r#"
SELECT (SELECT o.name, SCHEMA_NAME(o.schema_id) AS [schema], m.definition,
    m.uses_ansi_nulls AS ansi_nulls, m.uses_quoted_identifier AS quoted_identifier,
    CAST(COALESCE(t.is_disabled, 0) AS bit) AS disabled
    FOR JSON PATH, WITHOUT_ARRAY_WRAPPER)
FROM sys.objects o LEFT JOIN sys.sql_modules m ON m.object_id = o.object_id
LEFT JOIN sys.triggers t ON t.object_id = o.object_id
WHERE {filter} ORDER BY o.name COLLATE Latin1_General_100_BIN2
"#;
