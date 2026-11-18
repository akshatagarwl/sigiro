---
title: duckdb-acp Extension Analysis
created: 2026-04-23
updated: 2026-04-23
type: research
tags: ['duckdb', 'natural-language-sql', 'acp', 'claude-code', 'duckdb-extension', 'community-extension']
---

# duckdb-acp Extension Analysis

## What It Is

DuckDB community extension (`acp`) that translates natural language to SQL using Claude Code via the [Agent Client Protocol (ACP)](https://agentclientprotocol.com/).

**Repository:** github.com/sidequery/duckdb-acp
**Status:** ARCHIVED — owner archived the repo on Mar 30, 2026. Read-only.
**Stars:** 53
**This week downloads:** 568

## How It Works

```sql
-- Load the extension
LOAD 'acp.duckdb_extension';

-- Statement syntax
CLAUDE show me total revenue by product;

-- Table function syntax
SELECT * FROM claude('which product has the highest average sale amount?');
```

The extension:
1. Intercepts NL queries
2. Spins up an embedded MCP server with schema exploration tools
3. Claude Code (or another ACP agent) discovers tables, columns, data
4. Generates SQL, executes it against your DuckDB instance

## Configuration Settings

| Setting | Default | Description |
|---|---|---|
| `acp_agent` | `claude-code` | ACP agent binary/path |
| `acp_safe_mode` | `true` | Block INSERT/UPDATE/DELETE |
| `acp_debug` | `false` | Debug output to stderr |
| `acp_timeout` | `300` | Seconds before timeout |
| `acp_show_sql` | — | Print generated SQL before executing |

## Requirements

- DuckDB 1.1.0+
- `bun` or `Node.js` (for agent runtime)
- Claude Code with Anthropic API credentials OR Claude Pro/Max account

## Relevance to Sigiro

**High.** Sigiro uses DuckDB as its unified store for telemetry (metrics, spans, logs, profiles). Adding `acp` would let users query their infrastructure data with natural language:

```sql
LOAD 'acp.duckdb_extension';
CLAUDE which service had the highest p99 latency last week?;
CLAUDE show me all error spikes correlated with recent deployments;
```

This is a natural fit — Sigiro already stores data in DuckDB, users already know their schema.

## Relevance to Autoresearch

**Medium.** The sidequery team built agent tooling around ACP. The [Agent Client Protocol](https://agentclientprotocol.com/) is the same protocol Hermes Agent uses for subprocess communication. The archived status suggests sidequery stopped maintaining their agent tooling — potential signal for the autoresearch work.

## Caveats

1. **Archived repo** — no active maintenance. Sidequery abandoned it Mar 2026.
2. **Requires Claude Code** — external dependency on Anthropic's CLI
3. **Safe mode on by default** — blocks mutations, but the agent has full SQL execution access
4. **Submodule-heavy build** — uses `corrosion`, `duckdb` as submodules. Complex to build from source.
5. **Security** — the MCP server embedded in the extension gives the agent access to your full DuckDB schema. In production telemetry scenarios with sensitive data, this is a consideration.

## What We'd Need to Do

To integrate into Sigiro:

1. **Fork or vendor** — the repo is archived, so we'd need to maintain our own copy
2. **Build system** — CMake + corrosion + submodule setup is complex; check if pre-built binaries work
3. **Safe defaults** — telemetry data is sensitive; ensure `acp_safe_mode` stays on
4. **Schema documentation** — users need to know what tables/columns exist to ask meaningful NL questions

## Alternative Approaches

- **Direct NL2SQL with local model** — duckdb-sqlpf (Postgres fuzzy search), local Ollama
- **Structured output prompting** — already in Sigiro's investigation block design
- **pg-sqlparser + LLM** — lighter than running full Claude Code subprocess

## Source

- duckdb.org/community_extensions/extensions/acp
- github.com/sidequery/duckdb-acp (archived)
