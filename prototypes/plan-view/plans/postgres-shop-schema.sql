-- PROTOTYPE: invented schema and data for capturing one PostgreSQL plan. Throwaway.
CREATE TABLE customers (
    customer_id int PRIMARY KEY,
    name text NOT NULL,
    city text NOT NULL,
    country text NOT NULL
);
CREATE TABLE products (
    product_id int PRIMARY KEY,
    name text NOT NULL,
    category text NOT NULL,
    price numeric(10,2) NOT NULL
);
CREATE TABLE orders (
    order_id int PRIMARY KEY,
    customer_id int NOT NULL REFERENCES customers,
    created_at timestamptz NOT NULL,
    status text NOT NULL
);
CREATE TABLE order_lines (
    order_id int NOT NULL REFERENCES orders,
    line_no int NOT NULL,
    product_id int NOT NULL REFERENCES products,
    quantity int NOT NULL,
    PRIMARY KEY (order_id, line_no)
);

-- City and country are fully correlated (each city sits in one country), so the planner,
-- which multiplies their selectivities, underestimates a filter on both.
INSERT INTO customers
SELECT g, 'Customer ' || g,
       'City ' || (g % 200),
       'Country ' || ((g % 200) % 20)
FROM generate_series(1, 50000) g;

INSERT INTO products
SELECT g, 'Product ' || g, 'Category ' || (g % 25), (10 + (g % 90))::numeric
FROM generate_series(1, 2000) g;

INSERT INTO orders
SELECT g, 1 + (g::bigint * 7919) % 50000,
       timestamptz '2026-01-01' + ((g / 3) % 300) * interval '1 day',
       (ARRAY['open','paid','shipped','returned'])[1 + (g / 7) % 4]
FROM generate_series(1, 300000) g;

INSERT INTO order_lines
SELECT o, l, 1 + (o * 31 + l * 17) % 2000, 1 + (o + l) % 5
FROM generate_series(1, 300000) o, generate_series(1, 3) l;

CREATE INDEX orders_customer_ix ON orders (customer_id);
CREATE INDEX orders_created_ix ON orders (created_at);
ANALYZE;
