---
"sigiro": minor
---

Every point in time is stored as a `TIMESTAMP` (#236). `sigiro_spans.end_timestamp`, `sigiro_logs.observed_timestamp` and the metric tables' `start_timestamp` were microsecond `BIGINT`s, so `end_timestamp - timestamp`, or comparing either to `timestamp`, failed without a cast. Lengths of time such as `duration` stay microsecond `BIGINT`s. Bound a query with `TIMESTAMP '…'` or `make_timestamp(<µs>)`. This is storage format 2: move an older data directory aside before starting this release.
