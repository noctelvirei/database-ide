# Engine SQL text and scripting rules

This record holds the non-obvious rules behind the engine modules that read, format and write SQL text: formatting, completion, the danger check, Script as, grid edit scripts, plans, exports and saved connections: choices, invariants and server behaviour that the code alone doesn't show. Until 2026-11-20 each sat as a comment beside its code; CODE_STANDARDS.md now keeps such rules here, one place per rule.

A change to the code a rule names either keeps the rule or edits it here in the same commit. Rules are grouped by the file that holds the code.

## `crates/engine/src/completion.rs`

- Completion lexing in crates/engine/src/completion.rs detects string literals from the raw spelling (a quote or $ prefix) rather than by Token variant, because sqlparser has many dialect-specific string-literal variants.
- sqlparser can emit a COPY payload token before the semicolon that precedes it in the text, so completion lexing in crates/engine/src/completion.rs sorts tokens by start offset.
- When completion in crates/engine/src/completion.rs reads a table alias, clause words (sqlparser RESERVED_FOR_TABLE_ALIAS plus AS, VALUES, INTO) are never taken as the alias, even on an engine where they are not reserved, because they still start the next clause.

## `crates/engine/src/connections/credentials.rs`

- crates/engine/src/connections/credentials.rs keeps only the formatted text of a keyring::Error, never the error value, because keyring's BadEncoding carries the password bytes, including in its source chain.

## `crates/engine/src/danger.rs`

- The danger check in crates/engine/src/danger.rs takes each statement's line from a parallel Parser pass over the tokens, not from the AST span, because several sqlparser statement spans omit their leading keyword.
- Findings::keyword_line in crates/engine/src/danger.rs locates DROP-family statements by their first object name, because sqlparser leaves those statements' own spans empty while the names keep source locations.

## `crates/engine/src/danger/estimates.rs`

- rows in crates/engine/src/danger/estimates.rs reads the estimate from the Outer child of a PostgreSQL ModifyTable node, because ModifyTable counts output rows (zero without RETURNING), not the rows it changes.
- For MySQL JSON plans, rows in crates/engine/src/danger/estimates.rs descends through DML wrapper nodes that lack row counters only while each has exactly one child; with several inputs there is no unambiguous count, so no estimate is given.
- In crates/engine/src/danger/estimates.rs every finding from a re-checked statement slice becomes a Candidate, carrying SQL only when the slice is exactly that one statement; the SQL-less placeholders keep matching aligned so a nested finding cannot take the plan of a later statement on the same line.

## `crates/engine/src/danger/fallback.rs`

- The danger fallback in crates/engine/src/danger/fallback.rs judges a WHERE clause only when the whole predicate parses; a prefix that parses as always-true can be restricted by the rest, so a partial parse gives no verdict.

## `crates/engine/src/export.rs`

- Export finishing in crates/engine/src/export.rs publishes the file with a hard link, not a rename, so a destination file created by someone else while the export was writing is never replaced; the link fails instead.

## `crates/engine/src/export/xlsx.rs`

- supported_number in crates/engine/src/export/xlsx.rs writes a value as an Excel number only when it is zero or between f64::MIN_POSITIVE and 9.99999999999999e307 in magnitude, because Excel has no subnormal numbers and rejects larger entered values.

## `crates/engine/src/format.rs`

- The SQL Server path in crates/engine/src/format.rs finds each batch's offset by pointer arithmetic, which relies on crate::mssql::batches::split returning slices borrowed from its input; if split ever returns owned strings this must change.
- lex in crates/engine/src/format.rs leaves the whole input unformatted when it contains MySQL versioned comments (/*!) or COPY data, because sqlparser gives those constructs synthetic or reordered spans.
- The formatter in crates/engine/src/format.rs never treats a word after a dot as a keyword, because MySQL accepts reserved words unquoted as qualified identifiers (t.select).
- joined in crates/engine/src/format.rs keeps `$-2.5` together: the sign after a money sigil belongs to the money literal, not a binary operator.
- The formatter in crates/engine/src/format.rs keeps the original presence or absence of space around a dot, because in a fragment the spacing decides whether the dot is part of a number or a separate token.
- The formatter in crates/engine/src/format.rs never inserts a space inside `1--2`: it is arithmetic, and `--` followed by a space starts a MySQL comment.

## `crates/engine/src/grid_edit.rs`

- Grid editability in crates/engine/src/grid_edit.rs requires the statement to be a single-table query, because result-column provenance names base tables only and cannot tell two instances of one table in a self-join apart.
- Optimistic predicates in crates/engine/src/grid_edit.rs add a plain equality on text key columns for SQL Server and MySQL so the server can seek the key index; the binary comparison that follows still enforces the exact original text.
- For MySQL FLOAT columns crates/engine/src/grid_edit.rs compares CAST(CAST(column AS CHAR) AS DOUBLE) with the original, because CHAR uses the same FLOAT formatter as the wire protocol and the DOUBLE then equals the value the engine decoded.
- SQL Server text originals in crates/engine/src/grid_edit.rs are compared as varbinary(max), because ordinary equality treats collation-equivalent strings and trailing-space differences as equal and would miss a concurrent change.

## `crates/engine/src/grid_edit/mssql.rs`

- SQL Server grid editing in crates/engine/src/grid_edit/mssql.rs takes row keys from sys.indexes, not from browse-mode key flags, because those flags can include hidden columns and do not identify a complete unique index.
- comparison_type in crates/engine/src/grid_edit/mssql.rs admits only built-in SQL type names, because the type text is spliced into a generated CONVERT expression.

## `crates/engine/src/grid_edit/mysql.rs`

- MySQL grid editing in crates/engine/src/grid_edit/mysql.rs checks that the relation named in the query is itself a base table, because a view can report its underlying table's names as column provenance.
- For MySQL text columns crates/engine/src/grid_edit/mysql.rs converts the original into the column's stored character set before the binary comparison, so the compared bytes match what the server stores.
- MySQL FLOAT columns are never row keys in crates/engine/src/grid_edit/mysql.rs, because their decoded originals are rounded and cannot identify a unique stored value.

## `crates/engine/src/grid_edit/postgres.rs`

- PostgreSQL grid editing (crates/engine/src/grid_edit/postgres.rs, inspect) resolves the already-quoted provenance name with to_regclass and reads schema and table names back separately, so names containing dots are never split.

## `crates/engine/src/objects/mssql/scripting.rs`

- SQL Server table scripts in crates/engine/src/objects/mssql/scripting.rs emit foreign keys after the table's standalone unique indexes, because a self-referencing foreign key can target one of those indexes.
- For triggers, module_header in crates/engine/src/objects/mssql/scripting.rs replaces only the ON target identifier with the current table, because sp_rename leaves the old parent name in sys.sql_modules.

## `crates/engine/src/objects/mysql.rs`

- The MySQL catalog query in crates/engine/src/objects/mysql.rs converts information_schema text (utf8mb3) to utf8mb4 once, so filters can compare it with utf8mb4 literals without a collation clash.

## `crates/engine/src/objects/mysql/metadata.rs`

- Index scripts in crates/engine/src/objects/mysql/metadata.rs omit the prefix length on SPATIAL key parts: information_schema reports SUB_PART 32 for them, but MySQL rejects a prefix on a spatial key.
- The FULLTEXT parser lookup in crates/engine/src/objects/mysql/metadata.rs scans ordinary tokens because MySqlDialect expands versioned comments (such as /*!50100 WITH PARSER ... */) into their executable tokens.

## `crates/engine/src/objects/mysql/scripts.rs`

- The commented ALTER TABLE template in crates/engine/src/objects/mysql/scripts.rs re-comments every line break in the qualified name, so an identifier containing a newline cannot become executable SQL.
- MySQL table and view scripts in crates/engine/src/objects/mysql/scripts.rs drop NO_BACKSLASH_ESCAPES from the recorded sql_mode, because SHOW CREATE TABLE/VIEW text uses backslash escapes even when SHOW ran with them disabled; routines keep their stored mode.
- routine_charsets in crates/engine/src/objects/mysql/scripts.rs pushes every DECLARE as a candidate type position without filtering CURSOR, CONDITION and HANDLER declarations, relying on inherited_string_type finding no string type there.
- view_literals in crates/engine/src/objects/mysql/scripts.rs converts every introduced literal with the view's creation client charset, because SHOW CREATE VIEW tags the whole definition with that charset, even literals with other introducers.
- index_script in crates/engine/src/objects/mysql/scripts.rs finds the parent table through the catalog by key, never by decoding caller-supplied names into SQL.

## `crates/engine/src/objects/postgres.rs`

- PostgreSQL scripting in crates/engine/src/objects/postgres.rs re-resolves the object from the catalog by key and checks its name and schema before scripting; caller-supplied names are never trusted as SQL.
- PostgreSQL scripts from crates/engine/src/objects/postgres.rs record search_path with $user already resolved to schema names, because a destination login can have a different schema with the same name.
- query in crates/engine/src/objects/postgres.rs drains the whole run even after an error, so the caller's session is left idle and usable.

## `crates/engine/src/plan/mysql.rs`

- single_table in crates/engine/src/plan/mysql.rs accepts filtered as a decimal string (MySQL EXPLAIN JSON version 1) or a number (version 2).

## `crates/engine/src/plan/showplan.rs`

- Showplan parsing in crates/engine/src/plan/showplan.rs gives a Compute Scalar without runtime counters its input's actual rows and elapsed time, because SQL Server defers its evaluation to the input and omits the counters.

## `crates/engine/src/sql.rs`

- qualified_identifier in crates/engine/src/sql.rs decodes server-reported names with the generic and SQL Server dialects in turn, because their quoting follows the source server, not the dialect chosen for an INSERT export.
- MySQL string literals from crates/engine/src/sql.rs are hex wrapped in CONVERT(... USING utf8mb4), so they do not depend on NO_BACKSLASH_ESCAPES or the connection character set.
- original_literal in crates/engine/src/sql.rs sends PostgreSQL originals as untyped string literals so the server coerces them to the column type; a numeric literal would promote a real column to double and break equality.
