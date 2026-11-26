---
"sigiro": minor
---

`SIGIRO_DUCKDB_TOTAL_MEMORY_LIMIT` caps the DuckDB memory of all open projects together, defaulting to half of the machine's memory (or of the cgroup limit). The server keeps at most that budget divided by `SIGIRO_DUCKDB_MEMORY_LIMIT` projects open at once, closing idle ones first, instead of up to 100 regardless of memory.
