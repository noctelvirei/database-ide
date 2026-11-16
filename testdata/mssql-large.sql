-- SQL Server 2025, compatibility level >= 160; fresh throwaway database only.
-- Independent of mssql.sql; use sqlcmd -b.
IF SCHEMA_ID('shop') IS NULL EXEC('CREATE SCHEMA shop');
CREATE TABLE shop.big_rows (id bigint PRIMARY KEY, payload nvarchar(100) NOT NULL);
INSERT INTO shop.big_rows
SELECT value, CONCAT(N'Invented payload ', value) FROM GENERATE_SERIES(1, 1000000);
GO
