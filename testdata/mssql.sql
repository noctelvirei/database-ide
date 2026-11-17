-- SQL Server 2025: run with sqlcmd -b -f 65001 in a fresh throwaway database.
-- GO separates object definitions into the batches SQL Server requires.
SET NOCOUNT ON;
SET XACT_ABORT ON;
GO
CREATE SCHEMA shop;
GO
CREATE TABLE shop.customers (
    id bigint IDENTITY PRIMARY KEY,
    display_name nvarchar(100) NOT NULL,
    email nvarchar(200) NULL,
    notes nvarchar(max) NULL
);
CREATE TABLE shop.orders (
    id bigint IDENTITY PRIMARY KEY,
    customer_id bigint NOT NULL REFERENCES shop.customers(id),
    ordered_at datetimeoffset(7) NOT NULL,
    total decimal(12, 2) NOT NULL
);
CREATE INDEX orders_customer_id ON shop.orders(customer_id);
CREATE TABLE shop.order_lines (
    order_id bigint NOT NULL REFERENCES shop.orders(id),
    line_number int NOT NULL,
    item_name nvarchar(100) NOT NULL,
    quantity int NOT NULL,
    unit_price decimal(12, 2) NOT NULL,
    PRIMARY KEY (order_id, line_number)
);
-- Deliberately has no primary or unique key.
CREATE TABLE shop.audit_log (occurred_at datetimeoffset(7), message nvarchar(max));
-- SQL Server floating point types do not support NaN or infinity.
-- JSON uses validated Unicode text so this also covers text-based JSON results.
CREATE TABLE shop.common_types (
    id int PRIMARY KEY,
    bool_value bit NULL,
    tiny_value tinyint NULL,
    small_value smallint NULL,
    int_value int NULL,
    big_value bigint NULL,
    decimal_value decimal(38, 10) NULL,
    money_value money NULL,
    smallmoney_value smallmoney NULL,
    real_value real NULL,
    double_value float(53) NULL,
    fixed_value char(4) NULL,
    varying_value varchar(40) NULL,
    unicode_fixed_value nchar(4) NULL,
    unicode_varying_value nvarchar(40) NULL,
    text_value nvarchar(max) NULL,
    fixed_binary_value binary(4) NULL,
    varying_binary_value varbinary(40) NULL,
    binary_value varbinary(max) NULL,
    date_value date NULL,
    time_value time(7) NULL,
    legacy_timestamp_value datetime NULL,
    small_timestamp_value smalldatetime NULL,
    timestamp_value datetime2(7) NULL,
    zoned_timestamp_value datetimeoffset(7) NULL,
    uuid_value uniqueidentifier NULL,
    json_value nvarchar(max) NULL CHECK (ISJSON(json_value) = 1),
    xml_value xml NULL,
    variant_value sql_variant NULL,
    row_version rowversion
);
INSERT INTO shop.customers (display_name, email, notes)
SELECT CONCAT(N'Zorb-', value, N' 雪é'),
       CASE WHEN value % 3 = 0 THEN NULL ELSE CONCAT(N'zorb-', value, N'@example.invalid') END,
       CASE WHEN value % 2 = 0 THEN NULL ELSE N'Invented café nebula' END
FROM GENERATE_SERIES(1, 40) ORDER BY value;
INSERT INTO shop.orders (customer_id, ordered_at, total)
SELECT (value - 1) % 40 + 1, DATEADD(minute, value, CAST('2024-02-29T12:00:00+00:00' AS datetimeoffset)), 7.50
FROM GENERATE_SERIES(1, 80) ORDER BY value;
INSERT INTO shop.order_lines
SELECT id, 1, N'Widget-1', 1, 2.50 FROM shop.orders
UNION ALL SELECT id, 2, N'Widget-2', 2, 2.50 FROM shop.orders;
INSERT INTO shop.audit_log
SELECT '2024-02-29T12:00:00+00:00', CONCAT(N'Invented event ', value)
FROM GENERATE_SERIES(1, 20);
-- rowversion is server-generated and cannot be NULL.
INSERT INTO shop.common_types (id) VALUES (1);
INSERT INTO shop.common_types (
    id, bool_value, tiny_value, small_value, int_value, big_value, decimal_value,
    money_value, smallmoney_value, real_value, double_value, fixed_value, varying_value,
    unicode_fixed_value, unicode_varying_value, text_value, fixed_binary_value,
    varying_binary_value, binary_value, date_value, time_value, legacy_timestamp_value,
    small_timestamp_value, timestamp_value, zoned_timestamp_value, uuid_value,
    json_value, xml_value, variant_value
) VALUES
(2, 0, 0, -32768, -2147483648, -9223372036854775808,
 -9999999999999999999999999999.9999999999, -922337203685477.5808, -214748.3648,
 -3.4028234e38, -1.7976931348623157e308, '', '', N'', N'', N'', 0x00000000, 0x, 0x,
 '0001-01-01', '00:00:00', '1753-01-01', '1900-01-01', '0001-01-01T00:00:00',
 '0001-01-01T00:00:00+00:00', '00000000-0000-0000-0000-000000000000',
 N'{}', N'<v/>', CAST(CAST(-1 AS int) AS sql_variant)),
(3, 1, 255, 32767, 2147483647, 9223372036854775807,
 9999999999999999999999999999.9999999999, 922337203685477.5807, 214748.3647,
 3.4028234e38, 1.7976931348623157e308, 'star', 'Nebula', N'雪é', N'Nebula 雪é',
 REPLICATE(CAST(N'雪é' AS nvarchar(max)), 5000), 0x00017fff, 0x80ff, 0x00017fff80ff,
 '9999-12-31', '23:59:59.9999999', '9999-12-31T23:59:59.997', '2079-06-06T23:59:00',
 '9999-12-31T23:59:59.9999999', '9999-12-31T23:59:59.9999999+00:00',
 'ffffffff-ffff-ffff-ffff-ffffffffffff', N'{"invented":[1,null,"雪"]}', N'<v>雪</v>', CAST(CAST(N'雪' AS nvarchar(1)) AS sql_variant));
GO
CREATE VIEW shop.order_summary AS
SELECT o.id, c.display_name, o.ordered_at, o.total
FROM shop.orders AS o JOIN shop.customers AS c ON c.id = o.customer_id;
GO
CREATE PROCEDURE shop.customer_orders @customer bigint
AS
BEGIN
    SET NOCOUNT ON;
    SELECT * FROM shop.orders WHERE customer_id = @customer ORDER BY id;
END;
GO
SET ANSI_NULLS ON;
SET QUOTED_IDENTIFIER ON;
SET ANSI_PADDING ON;
SET ANSI_WARNINGS ON;
SET ARITHABORT ON;
SET CONCAT_NULL_YIELDS_NULL ON;
SET NUMERIC_ROUNDABORT OFF;
GO
CREATE SCHEMA explorer;
GO
CREATE TABLE explorer.parents (id int CONSTRAINT parents_pk PRIMARY KEY);
CREATE TABLE explorer.catalog_items (
    id bigint IDENTITY(17,3) NOT NULL,
    code nvarchar(40) NOT NULL CONSTRAINT catalog_items_code_default DEFAULT N'雪]''s',
    parent_id int NULL,
    quantity int NOT NULL CONSTRAINT catalog_items_quantity_default DEFAULT (2),
    doubled AS (quantity * 2) PERSISTED,
    CONSTRAINT catalog_items_pk PRIMARY KEY NONCLUSTERED (id DESC),
    CONSTRAINT catalog_items_code_key UNIQUE CLUSTERED (code),
    CONSTRAINT catalog_items_quantity_check CHECK (quantity > 0),
    CONSTRAINT catalog_items_parent_fk FOREIGN KEY (parent_id) REFERENCES explorer.parents(id)
        ON DELETE SET NULL ON UPDATE CASCADE
);
CREATE NONCLUSTERED INDEX catalog_items_quantity ON explorer.catalog_items(quantity DESC)
    INCLUDE (parent_id) WHERE quantity > 1;
CREATE TABLE explorer.[odd]]果実] ([key]]雪] int NOT NULL CONSTRAINT [odd]]pk] PRIMARY KEY,
    [value]]text] nvarchar(20) CONSTRAINT [odd]]default] DEFAULT N'雪''s]');
GO
CREATE TRIGGER explorer.catalog_items_guard ON explorer.catalog_items AFTER INSERT AS
BEGIN
    SET NOCOUNT ON;
    IF EXISTS (SELECT 1 FROM inserted WHERE quantity = 13)
        THROW 51001, 'Invented quantity is reserved', 1;
END;
GO
CREATE FUNCTION shop.line_total(@quantity int, @unit_price decimal(12, 2))
RETURNS decimal(12, 2)
AS
BEGIN
    RETURN @quantity * @unit_price;
END;
GO
