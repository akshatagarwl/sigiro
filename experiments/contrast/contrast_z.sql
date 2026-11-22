-- Prevalence-free contrast: for each attribute value, the strongest
-- association between "trace has the value" and "trace is slow" over a grid
-- of slow thresholds.
--
-- For one threshold the 2x2 table (has value / not) x (slow / not) gives
--   phi = (P(value, slow) - P(value) P(slow)) / sqrt(P(value)(1-P(value)) P(slow)(1-P(slow)))
-- and z = phi * sqrt(n) is its significance. contrast.sql's separation equals
-- phi * sqrt(P(value)(1-P(value)) / (P(slow)(1-P(slow)))), so with a fixed
-- threshold a rare value scores low however slow it makes requests. Taking the
-- maximum over thresholds lets the threshold match the fault's prevalence.
--
-- Placeholders: {scope} (WHERE condition bounding `timestamp` with literals),
-- {quantiles} (a list literal such as [0.5, 0.9, 0.99]), {limit}.
SELECT
  c.dim,
  c.val,
  round(max(c.z), 3) AS z,
  max(c.n_val) AS traces,
  max(c.n) AS total
FROM (
  SELECT
    f.dim,
    f.val,
    l.thr,
    t.n,
    t.n_slow,
    count(DISTINCT f.trace_id) AS n_val,
    count(DISTINCT CASE WHEN l.slow THEN f.trace_id END) AS slow_val,
    (count(DISTINCT CASE WHEN l.slow THEN f.trace_id END) / t.n
      - count(DISTINCT f.trace_id) / t.n * t.n_slow / t.n)
      / sqrt(greatest(
          count(DISTINCT f.trace_id) / t.n * (1 - count(DISTINCT f.trace_id) / t.n)
          * t.n_slow / t.n * (1 - t.n_slow / t.n), 1e-12))
      * sqrt(t.n) AS z
  FROM (
    SELECT trace_id, 'service' AS dim, service_name AS val
    FROM sigiro_spans WHERE {scope}
    UNION
    SELECT trace_id, 'span' AS dim, span_name AS val
    FROM sigiro_spans WHERE {scope}
    UNION
    SELECT trace_id, concat('resource.', k) AS dim,
      json_extract_string(resource_attributes, concat('$."', k, '"')) AS val
    FROM (
      SELECT trace_id, resource_attributes, unnest(json_keys(resource_attributes)) AS k
      FROM sigiro_spans WHERE {scope}
    ) r
    UNION
    SELECT trace_id, concat('span.', k) AS dim,
      json_extract_string(span_attributes, concat('$."', k, '"')) AS val
    FROM (
      SELECT trace_id, span_attributes, unnest(json_keys(span_attributes)) AS k
      FROM sigiro_spans WHERE {scope}
    ) a
  ) f
  JOIN (
    SELECT r.trace_id, q.thr, r.duration > q.thr AS slow
    FROM (
      SELECT trace_id, duration FROM sigiro_spans
      WHERE {scope} AND coalesce(parent_span_id, '') = ''
    ) r
    CROSS JOIN (
      SELECT unnest(quantile_cont(duration, {quantiles})) AS thr FROM sigiro_spans
      WHERE {scope} AND coalesce(parent_span_id, '') = ''
    ) q
  ) l ON l.trace_id = f.trace_id
  JOIN (
    SELECT q.thr, count(*) AS n, sum(CASE WHEN r.duration > q.thr THEN 1 ELSE 0 END) AS n_slow
    FROM (
      SELECT duration FROM sigiro_spans
      WHERE {scope} AND coalesce(parent_span_id, '') = ''
    ) r
    CROSS JOIN (
      SELECT unnest(quantile_cont(duration, {quantiles})) AS thr FROM sigiro_spans
      WHERE {scope} AND coalesce(parent_span_id, '') = ''
    ) q
    GROUP BY q.thr
  ) t ON t.thr = l.thr
  WHERE t.n_slow > 0 AND t.n_slow < t.n
  GROUP BY f.dim, f.val, l.thr, t.n, t.n_slow
) c
GROUP BY c.dim, c.val
ORDER BY z DESC, c.dim, c.val
LIMIT {limit}
