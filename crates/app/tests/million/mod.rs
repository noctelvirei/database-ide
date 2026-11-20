//! The million-row run the grid and run-to-file tests time and measure, in each engine's SQL.

use dbide_engine::danger::Engine;

/// A batch returning a million rows `n, label, odd` (n from 1 in order, label "row n", odd
/// NULL for even n) and then one row `name, n` of `'second', 2`.
pub fn query(engine: Engine) -> &'static str {
    match engine {
        Engine::SqlServer => {
            "SELECT value AS n, CONCAT('row ', value) AS label, \
             CASE WHEN value % 2 = 0 THEN NULL ELSE value END AS odd \
             FROM GENERATE_SERIES(1, 1000000) ORDER BY value; \
             SELECT 'second' AS name, 2 AS n"
        }
        Engine::PostgreSql => {
            "SELECT n, 'row ' || n AS label, CASE WHEN n % 2 = 0 THEN NULL ELSE n END AS odd \
             FROM generate_series(1, 1000000) AS n; \
             SELECT 'second' AS name, 2 AS n"
        }
        Engine::MySql => {
            "WITH d (v) AS (SELECT 0 UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 \
             UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 \
             UNION ALL SELECT 8 UNION ALL SELECT 9), \
             s (n) AS (SELECT a.v + 10 * b.v + 100 * c.v + 1000 * e.v + 10000 * f.v \
             + 100000 * g.v + 1 FROM d a, d b, d c, d e, d f, d g) \
             SELECT n, CONCAT('row ', n) AS label, CASE WHEN n % 2 = 0 THEN NULL ELSE n END AS odd \
             FROM s ORDER BY n; \
             SELECT 'second' AS name, 2 AS n"
        }
    }
}
