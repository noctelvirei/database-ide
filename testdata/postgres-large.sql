-- Independent of postgres.sql; load once into a fresh throwaway database.
BEGIN;
CREATE SCHEMA IF NOT EXISTS shop;
CREATE TABLE shop.big_rows (id bigint PRIMARY KEY, payload text NOT NULL);
INSERT INTO shop.big_rows
SELECT n, 'Invented payload ' || n FROM generate_series(1, 1000000) AS n;
COMMIT;
