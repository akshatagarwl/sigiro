-- Contrast ranking: which attribute values separate slow traces from the rest?
--
-- A trace is "bad" when its root span is slower than the {quantile} quantile
-- of root durations in scope. Every trace is described by the set of
-- (dimension, value) pairs that appear on any of its spans: service, span
-- name, and every resource and span attribute. For each pair the query counts
-- bad and good traces that contain it and ranks pairs by
--
--   separation = share of bad traces with the pair - share of good traces with it
--
-- A pair that only shows up in slow traces scores near 1. A pair that is
-- spread evenly scores near 0.
--
-- Placeholders, filled by faults.py: {scope} is a WHERE condition on
-- sigiro_spans (it must bound `timestamp` with literals), {quantile} is a
-- number in (0, 1), {limit} is the number of rows to return.
--
-- /v1/query accepts no WITH clauses, so the labelled-trace subquery appears
-- twice.
SELECT
  s.dim,
  s.val,
  s.bad,
  s.good,
  n.total_bad,
  n.total_good,
  round(s.bad / n.total_bad, 4) AS bad_share,
  round(s.good / greatest(n.total_good, 1), 4) AS good_share,
  round(s.bad / n.total_bad - s.good / greatest(n.total_good, 1), 4) AS separation
FROM (
  SELECT
    f.dim,
    f.val,
    count(DISTINCT CASE WHEN t.is_bad THEN f.trace_id END) AS bad,
    count(DISTINCT CASE WHEN NOT t.is_bad THEN f.trace_id END) AS good
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
    SELECT r.trace_id, r.duration > q.threshold AS is_bad
    FROM (
      SELECT trace_id, duration FROM sigiro_spans
      WHERE {scope} AND coalesce(parent_span_id, '') = ''
    ) r
    CROSS JOIN (
      SELECT quantile_cont(duration, {quantile}) AS threshold FROM sigiro_spans
      WHERE {scope} AND coalesce(parent_span_id, '') = ''
    ) q
  ) t ON t.trace_id = f.trace_id
  GROUP BY f.dim, f.val
) s
CROSS JOIN (
  SELECT
    sum(CASE WHEN r.duration > q.threshold THEN 1 ELSE 0 END) AS total_bad,
    sum(CASE WHEN r.duration > q.threshold THEN 0 ELSE 1 END) AS total_good
  FROM (
    SELECT duration FROM sigiro_spans
    WHERE {scope} AND coalesce(parent_span_id, '') = ''
  ) r
  CROSS JOIN (
    SELECT quantile_cont(duration, {quantile}) AS threshold FROM sigiro_spans
    WHERE {scope} AND coalesce(parent_span_id, '') = ''
  ) q
) n
ORDER BY separation DESC, s.dim, s.val
LIMIT {limit}
