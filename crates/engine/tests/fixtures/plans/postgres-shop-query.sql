SET max_parallel_workers_per_gather = 0;
EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
SELECT p.category, c.city, count(*) AS lines, sum(ol.quantity * p.price) AS revenue
FROM customers c
JOIN orders o ON o.customer_id = c.customer_id
JOIN order_lines ol ON ol.order_id = o.order_id
JOIN products p ON p.product_id = ol.product_id
WHERE c.city IN ('City 2', 'City 22', 'City 42', 'City 62', 'City 82', 'City 102', 'City 122', 'City 142', 'City 162', 'City 182') AND c.country = 'Country 2'
  AND o.created_at >= '2026-03-01' AND o.status <> 'returned'
GROUP BY p.category, c.city
ORDER BY revenue DESC;
