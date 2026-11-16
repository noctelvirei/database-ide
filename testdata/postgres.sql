-- Load once into a fresh throwaway database with psql -v ON_ERROR_STOP=1 -f.
BEGIN;
SET LOCAL TIME ZONE 'UTC';
CREATE SCHEMA shop;
CREATE TABLE shop.customers (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    display_name text NOT NULL,
    email text,
    notes text
);
CREATE TABLE shop.orders (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    customer_id bigint NOT NULL REFERENCES shop.customers(id),
    ordered_at timestamptz NOT NULL,
    total numeric(12, 2) NOT NULL
);
CREATE INDEX orders_customer_id ON shop.orders(customer_id);
CREATE TABLE shop.order_lines (
    order_id bigint NOT NULL REFERENCES shop.orders(id),
    line_number integer NOT NULL,
    item_name text NOT NULL,
    quantity integer NOT NULL,
    unit_price numeric(12, 2) NOT NULL,
    PRIMARY KEY (order_id, line_number)
);
-- Deliberately has no primary or unique key: results are not editable.
CREATE TABLE shop.audit_log (occurred_at timestamptz, message text);
CREATE TABLE shop.common_types (
    id integer PRIMARY KEY,
    bool_value boolean,
    small_value smallint,
    int_value integer,
    big_value bigint,
    decimal_value numeric(38, 10),
    real_value real,
    double_value double precision,
    fixed_value char(4),
    varying_value varchar(40),
    text_value text,
    binary_value bytea,
    date_value date,
    time_value time,
    zoned_time_value timetz,
    timestamp_value timestamp,
    zoned_timestamp_value timestamptz,
    interval_value interval,
    uuid_value uuid,
    json_value json,
    jsonb_value jsonb,
    xml_value xml,
    bits_value bit(8),
    array_value integer[]
);
INSERT INTO shop.customers (display_name, email, notes)
SELECT 'Zorb-' || n || ' 雪é',
       CASE WHEN n % 3 = 0 THEN NULL ELSE 'zorb-' || n || '@example.invalid' END,
       CASE WHEN n % 2 = 0 THEN NULL ELSE 'Invented café nebula' END
FROM generate_series(1, 40) AS n ORDER BY n;
INSERT INTO shop.orders (customer_id, ordered_at, total)
SELECT (n - 1) % 40 + 1, timestamptz '2024-02-29 12:00:00+00' + n * interval '1 minute', 7.50
FROM generate_series(1, 80) AS n ORDER BY n;
INSERT INTO shop.order_lines
SELECT n, line, 'Widget-' || line, line, 2.50
FROM generate_series(1, 80) AS n CROSS JOIN generate_series(1, 2) AS line;
INSERT INTO shop.audit_log
SELECT timestamptz '2024-02-29 12:00:00+00', 'Invented event ' || n
FROM generate_series(1, 20) AS n;
-- Row 1 has NULL in every nullable column; rows 2/3 cover signed bounds.
INSERT INTO shop.common_types (id) VALUES (1);
INSERT INTO shop.common_types VALUES
(2, false, -32768, -2147483648, -9223372036854775808,
 -9999999999999999999999999999.9999999999, -3.4028234e38, -1.7976931348623157e308,
 '', '', '', decode('', 'hex'), '0001-01-01', '00:00:00', '00:00:00+00',
 '0001-01-01 00:00:00', '0001-01-01 00:00:00+00', '-1 day',
 '00000000-0000-0000-0000-000000000000', 'null', 'null', '<v/>', B'00000000', '{}'),
(3, true, 32767, 2147483647, 9223372036854775807,
 9999999999999999999999999999.9999999999, 3.4028234e38, 1.7976931348623157e308,
 '雪é', 'Nebula 雪é', repeat('雪é', 5000), decode('00017fff80ff', 'hex'),
 '9999-12-31', '23:59:59.999999', '23:59:59.999999+05:30',
 '9999-12-31 23:59:59.999999', '9999-12-31 23:59:59.999999+00', '1 year 2 days',
 'ffffffff-ffff-ffff-ffff-ffffffffffff', '{"invented":[1,null,"雪"]}',
 '{"invented":[1,null,"雪"]}', '<v>雪</v>', B'11111111', '{1,NULL,-1}');
INSERT INTO shop.common_types (id, real_value, double_value, text_value)
VALUES (4, 'NaN', 'NaN', 'not a number'),
       (5, 'Infinity', 'Infinity', 'positive infinity'),
       (6, '-Infinity', '-Infinity', 'negative infinity');
CREATE VIEW shop.order_summary AS
SELECT o.id, c.display_name, o.ordered_at, o.total
FROM shop.orders AS o JOIN shop.customers AS c ON c.id = o.customer_id;
-- A set-returning function is the PostgreSQL equivalent of the read procedure.
CREATE FUNCTION shop.customer_orders(customer bigint)
RETURNS SETOF shop.orders LANGUAGE sql STABLE
AS $$ SELECT * FROM shop.orders WHERE customer_id = customer ORDER BY id $$;
CREATE FUNCTION shop.line_total(quantity integer, unit_price numeric)
RETURNS numeric LANGUAGE sql IMMUTABLE
AS $$ SELECT quantity * unit_price $$;
COMMIT;
