---
"sigiro": minor
---

`/v1/query` turns every DuckDB time type into JSON the same way (#8). Timestamps of any unit (`TIMESTAMP_S`, `_MS`, `_NS`, `TIMESTAMPTZ`) are microseconds since the Unix epoch; an `INTERVAL`, such as `end_timestamp - timestamp`, is microseconds instead of a Rust debug string; a `DATE` is `YYYY-MM-DD` and a `TIME` is `HH:MM:SS.ffffff` instead of raw ticks. `STRUCT` values are JSON objects and `MAP` values lists of `{key, value}`. Hosted `/v1/query` returns timestamps as microseconds too, instead of ISO text.
