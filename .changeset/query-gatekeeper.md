---
"sigiro": minor
---

`/v1/query` now accepts any read-only statement DuckDB's gatekeeper extension allows over the `sigiro_*` tables: `WITH` and `WITH RECURSIVE`, `DESCRIBE`, `SUMMARIZE`, `PIVOT`, and every function on gatekeeper v0.4.5's reviewed default list (`string_agg`, `list`, `list_distinct`, `json_valid`, `left`, `md5`, `arg_max` and more). The hand-kept allowlist is gone from this route. `information_schema`, `pragma_database_size()`, `nvl` and `list_aggregate` are no longer available; use `DESCRIBE <table>` to list columns and `coalesce` for `nvl`. A refused query returns 400 with the rule it broke.
