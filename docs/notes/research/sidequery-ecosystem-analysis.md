---
title: Sidequery Ecosystem Analysis
created: 2026-04-23
updated: 2026-04-23
type: research
tags: ['sidequery', 'sidemantic', 'sqlnb', 'metrics-layer', 'acp', 'duckdb', 'mcp', 'notebook', 'natural-language-sql']
---

# Sidequery Ecosystem Analysis

## What Is Sidequery

**sidequery.dev** is a company/product suite focused on local-first data tools. They build:

| Product | Description | Status |
|---|---|---|
| **sidemantic** | Universal metrics layer (semantic model + query engine) | Active, 83 stars, 549 commits |
| **sqlnb** | SQL notebooks for macOS (DuckDB, Postgres, MySQL) | In development, waitlist open |
| **pynb** | Reactive Python notebooks via uv & Apache Arrow | Active, macOS/iOS |
| **mdnb** | Markdown notebooks with backlinks + graph view | Active, macOS/iOS |
| **duckdb-acp** | NL→SQL DuckDB extension via ACP/Claude Code | **Archived Mar 2026** |

The company pivoted from "AI Slack bot" (sidequery.ai) to local-first developer tools. The old sidequery.ai product still exists but is separate.

## Sidemantic (github.com/sidequery/sidemantic)

**The headline: active, 549 commits, last commit 17 hours ago.** This is their main bet.

### What It Does

A semantic metrics layer that:
1. Reads metric definitions from 15+ formats: Cube, dbt MetricFlow, LookML, Hex, Rill, Superset, Omni, Snowflake Cortex, Malloy, OSI, BSL, GoodData LDM, AtScale SML, ThoughtSpot TML, and Sidemantic's own YAML/SQL/Python DSL
2. Connects to: DuckDB, MotherDuck, PostgreSQL, BigQuery, Snowflake, ClickHouse, Databricks, Spark (via ADBC)
3. Serves results via: Semantic SQL, Python API, CLI, **MCP server**, **Postgres wire protocol**

### Key Technical Features

**Fan-out correctness** — symmetric aggregates prevent double-counting across one-to-many joins. This is non-trivial.

**Automatic join inference** — cross-model queries infer join paths from declared relationships. No JOIN keyword needed in semantic SQL.

**Three authoring formats** — YAML, SQL, or Python. All three produce identical query behavior.

```yaml
# YAML definition
models:
  - name: orders
    table: public.orders
    dimensions:
      - name: status
        type: categorical
      - name: order_date
        type: time
        sql: created_at
    metrics:
      - name: revenue
        agg: sum
        sql: amount
      - name: order_count
        agg: count
```

### MCP Server

Sidemantic has an MCP server (`uvx sidemantic mcp`). This is the most relevant part for Sigiro — they serve metric definitions through MCP so AI agents (Claude Code, etc.) can query them safely.

```bash
$ claude mcp add sidemantic -- uvx sidemantic mcp
```

### Relevance to Sigiro

**High.** Sigiro has telemetry data in DuckDB. Sidemantic's semantic layer could provide:
- Metric definitions that map to Sigiro's schema
- Consistent metric names across different query surfaces (Slack bot, web, Claude Code)
- MCP server for AI agents to query telemetry
- Fan-out correctness for multi-dimensional aggregations

**The comparison point:** Sigiro's `investigation_block` is a structured response format. Sidemantic defines metrics declaratively and generates SQL. Different approaches to the same problem.

### How to Run

```bash
uvx sidemantic  # Starts interactive demo with built-in data
# Or Docker:
docker run -p 5444:5444 sidequery/sidemantic
```

## sqlnb (sqlnb.com)

**SQL notebooks for macOS. Free desktop app.** Still in development but actively built.

### Key Properties

- Local-first — data stays on your machine
- Works with DuckDB (file, MotherDuck), Postgres, MySQL, BigQuery, Snowflake via DuckDB extensions
- **AI agents built-in via ACP** — Claude Code, Codex, Gemini CLI
- Federated joins across sources (CSV, Parquet, JSON + remote DBs)
- Notebook format degrades gracefully to plain SQL (compatible with any SQL editor)
- **Beautiful charts by default** — visualization is first-class

### ACP Integration Model

sqlnb connects to any ACP-compatible agent CLI. You bring your own API subscription. This is the same model Sigiro uses — ACP as the agent protocol, agent runs externally.

### Notebook Format

sqlnb uses a SQL-based notebook format that can run in any SQL environment. Notebooks specify dashboards and charts declaratively in SQL comments:

```sql
-- # title: Revenue Dashboard
-- # chart: bar
SELECT region, revenue FROM orders;
```

This is similar in spirit to Sigiro's `InvestigationBlock` — structured output embedded in a familiar query format.

### Relevance to Sigiro

**Medium.** The SQL-as-notebook-format idea is interesting. Sigiro could emit `.sqlnb`-compatible notebooks for investigation results. The ACP integration is also a reference for how Sigiro should expose its query interface to agents.

## duckdb-acp (Archived)

**Already covered in [[research/duckdb-acp-analysis.md|duckdb-acp-analysis.md]].** Archived Mar 2026.

Key note: the sidequery team built this and then abandoned it. Their focus shifted to Sidemantic and sqlnb. The ACP extension for DuckDB is dead — the protocol lives on in sqlnb and Sidemantic's MCP server.

## Sidequery's ACP Bet

Sidequery is the most concrete reference implementation for ACP (Agent Client Protocol) as a product layer:

- **sqlnb**: ACP to connect AI agents to SQL notebooks
- **Sidemantic MCP**: ACP to connect AI agents to metric definitions
- Both use Claude Code and Gemini CLI as the agent runtimes

This confirms the thesis: ACP is a viable product-level protocol for connecting AI agents to data tools. Sigiro's use of ACP for the autoresearch loop is aligned with where Sidequery is going.

## What Sidequery Got Wrong (Honest Assessment)

The archived duckdb-acp and pivoted sidequery.ai (from AI Slack bot to local tools) suggest some product missteps:

1. **duckdb-acp was the right idea, wrong execution** — natural language SQL in DuckDB is great, but building a DuckDB extension (complex C++ build) was too heavy. Sidemantic is the better abstraction.
2. **Sidequery.ai (Slack bot)** — pivoted away from this. The AI data analysis market is crowded (Hex, Count, etc.).
3. **Product sprawl** — pynb, mdnb, sqlnb, sidemantic, duckdb-acp, sidequery.ai. That's 6 products for a small team. Focus may have been an issue.

## Strategic Takeaways for Sigiro

1. **Sidemantic is the right model** for the metrics/query layer: declarative definitions, multiple authoring formats, MCP server, fan-out correctness
2. **ACP as a product protocol works** — Sidequery proves you can build desktop apps + AI agent integration on ACP
3. **Local-first is the differentiator** — Sidequery's products all emphasize data stays on your machine
4. **DuckDB as the runtime** — confirmed as the right choice for local analytics
5. **SQL as the notebook format** — graceful degradation makes it portable

## References

- sidequery.dev
- github.com/sidequery/sidemantic (83 stars, 549 commits, active)
- sqlnb.com (in dev, waitlist open)
- pynb.dev
- mdnb.app
- Previous: [[research/duckdb-acp-analysis.md|duckdb-acp-analysis.md]]
