# Sample plans (prototype only)

| File | What it is | Source | Licence |
|---|---|---|---|
| `mysql-estimated.json`, `mysql-actual.json` | MySQL 8.4.13 iterator JSON version 2, estimated and actual plans of the two-table join in `plans_mysql.rs` | Captured from a throwaway test database containing two trees and three leaves | Same as this repository |
| `row-count-spool-slow.sqlplan` | SQL Server actual plan: parallelism, two hash matches (one spilled to tempdb, with a warning), nested loops anti semi joins, a row count spool | [erikdarlingdata/PerformanceStudio](https://github.com/erikdarlingdata/PerformanceStudio) `tests/PlanViewer.Core.Tests/Plans/row-count-spool-slow.sqlplan` at commit `5b504ea2d9d6ec5fe0327acb5983b0f825788e28`, copied byte for byte (UTF-16) | MIT, see `LICENSE-PerformanceStudio.txt` |
| `serially-parallel.sqlplan` | SQL Server actual plan: parallelism, nested loops, a key lookup, row estimates off by 150× to 1,000× | same repository and commit, `tests/PlanViewer.Core.Tests/Plans/serially-parallel.sqlplan` | MIT, see `LICENSE-PerformanceStudio.txt` |
| `postgres-shop.json` (with `postgres-shop-query.sql`, whose statement the editor shows, and `postgres-shop-schema.sql`, to recreate the capture) | PostgreSQL 18 `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)` output | Captured for this prototype from a throwaway cluster loaded with `postgres-shop-schema.sql` and running `postgres-shop-query.sql`; the cluster was deleted afterwards | Same as this repository |

Neither SQL Server sample has every feature asked for: the first has no key lookup and the second has no hash match, so the prototype carries both.

The PostgreSQL schema makes `city` and `country` fully correlated. The planner multiplies their selectivities, so it expects 128 customers and finds 2,500, and every join above the scan inherits the 20× error.
