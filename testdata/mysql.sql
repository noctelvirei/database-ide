-- MySQL 8.4: run with the mysql client, once, on a throwaway server.
-- Uses the fresh database selected by the test harness.
SET NAMES utf8mb4;
SET time_zone = '+00:00';
CREATE TABLE customers (
    id bigint PRIMARY KEY AUTO_INCREMENT,
    display_name text NOT NULL,
    email text,
    notes text
);
CREATE TABLE orders (
    id bigint PRIMARY KEY AUTO_INCREMENT,
    customer_id bigint NOT NULL,
    ordered_at timestamp(6) NOT NULL,
    total decimal(12, 2) NOT NULL,
    FOREIGN KEY (customer_id) REFERENCES customers(id)
);
CREATE TABLE order_lines (
    order_id bigint NOT NULL,
    line_number integer NOT NULL,
    item_name text NOT NULL,
    quantity integer NOT NULL,
    unit_price decimal(12, 2) NOT NULL,
    PRIMARY KEY (order_id, line_number),
    FOREIGN KEY (order_id) REFERENCES orders(id)
);
-- Deliberately has no primary or unique key.
CREATE TABLE audit_log (occurred_at timestamp(6), message text);
-- MySQL has no zoned timestamp or native UUID type; UUIDs use binary(16).
-- Its floating point types do not support NaN or infinity.
CREATE TABLE common_types (
    id integer PRIMARY KEY,
    bool_value boolean,
    tiny_value tinyint,
    small_value smallint,
    medium_value mediumint,
    int_value integer,
    big_value bigint,
    unsigned_value bigint unsigned,
    decimal_value decimal(38, 10),
    real_value float,
    double_value double,
    fixed_value char(4),
    varying_value varchar(40),
    text_value longtext,
    fixed_binary_value binary(4),
    varying_binary_value varbinary(40),
    binary_value longblob,
    date_value date,
    time_value time(6),
    timestamp_value datetime(6),
    utc_timestamp_value timestamp(6) NULL,
    year_value year,
    uuid_value binary(16),
    json_value json,
    bits_value bit(8),
    enum_value enum('nebula', 'comet'),
    set_value set('blue', 'green')
);
INSERT INTO customers (display_name, email, notes)
WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM numbers WHERE n < 40)
SELECT concat('Zorb-', n, ' 雪é'),
       CASE WHEN n % 3 = 0 THEN NULL ELSE concat('zorb-', n, '@example.invalid') END,
       CASE WHEN n % 2 = 0 THEN NULL ELSE 'Invented café nebula' END
FROM numbers ORDER BY n;
INSERT INTO orders (customer_id, ordered_at, total)
WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM numbers WHERE n < 80)
SELECT (n - 1) % 40 + 1, timestampadd(MINUTE, n, '2024-02-29 12:00:00'), 7.50
FROM numbers ORDER BY n;
INSERT INTO order_lines
SELECT id, 1, 'Widget-1', 1, 2.50 FROM orders
UNION ALL SELECT id, 2, 'Widget-2', 2, 2.50 FROM orders;
INSERT INTO audit_log
WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM numbers WHERE n < 20)
SELECT '2024-02-29 12:00:00', concat('Invented event ', n) FROM numbers;
INSERT INTO common_types (id) VALUES (1);
INSERT INTO common_types VALUES
(2, false, -128, -32768, -8388608, -2147483648, -9223372036854775808, 0,
 -9999999999999999999999999999.9999999999, -3.4028234e38, -1.7976931348623157e308,
 '', '', '', X'00000000', X'', X'', '1000-01-01', '-838:59:59.000000',
 '1000-01-01 00:00:00', '1970-01-01 00:00:01', 1901,
 uuid_to_bin('00000000-0000-0000-0000-000000000000'), 'null', b'00000000', 'nebula', ''),
(3, true, 127, 32767, 8388607, 2147483647, 9223372036854775807, 18446744073709551615,
 9999999999999999999999999999.9999999999, 3.4028234e38, 1.7976931348623157e308,
 '雪é', 'Nebula 雪é', repeat('雪é', 5000), X'00017fff', X'80ff', X'00017fff80ff',
 '9999-12-31', '838:59:59.000000', '9999-12-31 23:59:59.999999',
 '2038-01-19 03:14:07', 2155, uuid_to_bin('ffffffff-ffff-ffff-ffff-ffffffffffff'),
 '{"invented":[1,null,"雪"]}', b'11111111', 'comet', 'blue,green');
CREATE VIEW order_summary AS
SELECT o.id, c.display_name, o.ordered_at, o.total
FROM orders AS o JOIN customers AS c ON c.id = o.customer_id;
DELIMITER //
CREATE PROCEDURE customer_orders(IN customer bigint)
READS SQL DATA
BEGIN
    SELECT * FROM orders WHERE customer_id = customer ORDER BY id;
END//
CREATE FUNCTION line_total(quantity integer, unit_price decimal(12, 2))
RETURNS decimal(12, 2) DETERMINISTIC NO SQL
RETURN quantity * unit_price//
DELIMITER ;
