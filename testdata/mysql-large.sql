-- MySQL 8.4; independent of mysql.sql, only on a throwaway server.
CREATE TABLE big_rows (id bigint PRIMARY KEY, payload text NOT NULL);
-- Six decimal digits generate a million rows without changing recursion limits.
INSERT INTO big_rows
WITH digits(n) AS (
    SELECT 0 UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4
    UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9
), numbers AS (
    SELECT 1 + a.n + 10*b.n + 100*c.n + 1000*d.n + 10000*e.n + 100000*f.n AS n
    FROM digits a CROSS JOIN digits b CROSS JOIN digits c
    CROSS JOIN digits d CROSS JOIN digits e CROSS JOIN digits f
)
SELECT n, concat('Invented payload ', n) FROM numbers;
