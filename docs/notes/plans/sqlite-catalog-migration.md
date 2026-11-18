---
title: "Superseded: SQLite Catalog Migration"
created: 2026-04-10
updated: 2026-09-14
type: summary
tags: ['ai-agent', 'architecture', 'auth', 'competitor', 'data-model', 'duckdb']
---

# Superseded: SQLite Catalog Migration

**Date:** 2026-04-09
**[[plans/STATUS.md|Status]]:** Superseded — do not implement.

Sigiro retained its DuckDB-native local DuckLake catalog and its
PostgreSQL-backed distributed mode. The proposed SQLite catalog was never the
supported storage contract.

See [[architecture/ducklake-integration.md|DuckLake Integration]] for the
authoritative storage modes and current local data layout.