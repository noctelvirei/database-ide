# Database IDE

A desktop SQL IDE a DBA uses to write and run queries, read execution plans and export results against SQL Server, PostgreSQL and MySQL servers.

## Connections

**Connection**:
A saved set of details for reaching one database server: engine, address, sign-in method and colour.
_Avoid_: Server, data source, profile

**Engine**:
The database product a connection talks to: SQL Server, PostgreSQL or MySQL.
_Avoid_: Provider, dialect, backend

**Connection colour**:
The colour a connection marks its query tabs with, so a production server is recognisable at a glance.
_Avoid_: Environment tag, theme

## Queries

**Query tab**:
One editor bound to one connection and one current database, holding the SQL being written and the results of its last run.
_Avoid_: Window, document, session

**Run**:
Sending the query tab's SQL (or the selected part of it) to the server and receiving its results.
_Avoid_: Execute, submit

**Results grid**:
The table of rows a run returns, shown under the editor, where rows can be read, copied and edited.
_Avoid_: Data view, output

**Run to file**:
A run whose rows stream straight into an export file without being shown in the results grid.
_Avoid_: Bulk export, dump

**Dangerous statement**:
A statement the IDE asks the user to confirm before running: an `UPDATE` or `DELETE` with no `WHERE`, a `DROP` or a `TRUNCATE`.
_Avoid_: Destructive query, unsafe SQL

## Plans

**Estimated plan**:
The plan the server says it would use for a query, without running it.
_Avoid_: Explain, showplan

**Actual plan**:
The plan the server used for a run, with the row counts and timings it measured.
_Avoid_: Runtime plan, live plan, analyze

**Plan file**:
A saved plan on disk, in SQL Server's `.sqlplan` format, that the IDE can open as well as save.
_Avoid_: Plan export

## Objects

**Object explorer**:
The tree of a connection's databases and their tables, views, procedures, functions and indexes.
_Avoid_: Navigator, schema browser

**Script as**:
Producing the `CREATE`, `ALTER` or `SELECT` statement for an object in the object explorer.
_Avoid_: Generate script, DDL export
