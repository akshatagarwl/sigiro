---
"sigiro": patch
---

Upgrade DuckDB to 2.0.1 and cap DuckLake Parquet row groups at 16MB, so a `CHECKPOINT` that merges large values no longer runs out of memory.
