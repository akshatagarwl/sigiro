---
title: Self-Host Edge Federation
created: 2026-07-11
updated: 2026-07-11
type: guide
tags: ['ai-agent', 'architecture', 'deployment', 'duckdb', 'cli']
---

# Self-Host Edge Federation

Operator guide for running more than one self-hosted Sigiro node and querying
across all of them from one place. Supersedes the old central/worker-registry
design (deleted — see [[architecture/distributed-query.md]] for the history).

---

## Concept

Each self-hosted Sigiro node is an **edge**. An edge ingests OTLP into its own
local [[architecture/ducklake-integration.md|DuckLake]] and never ships raw
data anywhere else. Recent data is cheap to keep close to where it lands —
shipping every row to a central store just to answer occasional cross-host
questions is the cost you're trying to avoid.

When you need an answer that spans multiple edges (say, p95 latency for the
`checkout` service across all three regions), you don't query a central
server — there isn't one. You run **`sigiro query "<SQL>"`** from your own machine.
That command is a coordinator, not a server: it

1. discovers the edges,
2. rewrites your SQL into a per-edge partial (pruned at the edge, using its
   local time-partitioned Parquet — see [Tiering](#hotcold-tiering-per-edge)
   below),
3. pushes the partial to every edge over the `quack` wire protocol,
4. and folds the partial results into one correct answer — never a
   naive average-of-averages (see [Querying](#querying) for why that
   matters).

Data stays on each edge. Only the (small) query results move. This is a
single-operator model: one tenant is your own fleet of edges, queried by you.
There's no multi-tenant control plane, no worker registry, no central node to
run.

---

## Perimeter (mandatory)

**quack has no working TLS.** The token (if you set one) and every row of
data it serves cross the wire in plaintext. This is not a "harden it later"
gap — quack's client-server protocol simply doesn't do TLS today.

The only correct way to run this is a private network: **Tailscale or
WireGuard** between every edge and the machine running `sigiro query`. Rules:

- **Bind quack to the private/tailnet address, never `0.0.0.0`.**
  `SIGIRO_QUACK_BIND` defaults to `127.0.0.1` specifically so a wide-open
  bind is never the accident — you opt into it by setting the address
  yourself. Set it to the edge's Tailscale IP (`tailscale ip -4`), not a
  public interface.
- **Access control is the Tailscale ACL**, not quack's own token. Tag every
  edge `tag:sigiro-edge` and scope your ACL so only the operator's own
  device (or a tag it owns) can reach that tag on the quack port:

  ```json
  // tailscale ACL (admin console → Access Controls)
  {
    "tagOwners": {
      "tag:sigiro-edge": ["autogroup:admin"]
    },
    "acls": [
      {
        "action": "accept",
        "src":    ["autogroup:admin"],
        "dst":    ["tag:sigiro-edge:7777"]
      }
    ]
  }
  ```

  Advertise the tag when you bring an edge up:
  `tailscale up --advertise-tags=tag:sigiro-edge`.

- **Set `SIGIRO_QUACK_TOKEN` too**, as defense in depth. It's not the
  perimeter (the ACL is), but it costs nothing and means a misconfigured ACL
  isn't the only thing standing between an attacker and read access.

If you can't put every edge and every querying client on the same tailnet,
don't enable `SIGIRO_QUERY_SURFACE=quack` at all.

---

## Edge setup

Run a node as a queryable edge by setting, on that node:

| Env var | Default | Purpose |
|---|---|---|
| `SIGIRO_QUERY_SURFACE` | `none` | Set to `quack` to serve this edge's DuckLake read-only. `none` serves nothing (also inert today — this is the only implemented surface). |
| `SIGIRO_QUACK_BIND` | `127.0.0.1` | Bind address. Set to the node's Tailscale IP. **Never** `0.0.0.0` — the server refuses to start if you do. |
| `SIGIRO_QUACK_PORT` | `7777` | quack listen port. |
| `SIGIRO_QUACK_TOKEN` | *(unset)* | Optional bearer token quack clients must present. If set, must be at least 4 characters (quack's own minimum). |

```sh
export SIGIRO_QUERY_SURFACE=quack
export SIGIRO_QUACK_BIND="$(tailscale ip -4)"
export SIGIRO_QUACK_TOKEN="$(openssl rand -hex 16)"
sigiro serve
```

The served surface is **read-only by construction**: every query quack
receives is classified by DuckDB's own parser (not a keyword blocklist), and
anything that isn't a pure `SELECT` — `INSERT`, `UPDATE`, `DELETE`, `CREATE`,
`ATTACH`, `COPY`, `PRAGMA`, `SET`, everything — is rejected before it runs.
This gate is reinstalled from source on every server boot, so a stale or
tampered gate can never survive a restart.

One current limitation: `SIGIRO_QUERY_SURFACE=quack` is refused at startup
if the node is running in distributed/multi-tenant mode (`SIGIRO_CATALOG_URL`
set) — quack only serves a node's local, single-tenant "default" project
today. See [Tiering](#hotcold-tiering-per-edge) for what that means for
storage.

---

## Discovery

`sigiro query` needs to know which edges exist. Two ways, in order of
precedence:

1. **Tailnet discovery (default).** With no override, the coordinator runs
   `tailscale status --json` locally and collects every peer (and itself)
   tagged `tag:sigiro-edge`, using that peer's first Tailscale IP and
   `SIGIRO_QUACK_PORT` (the coordinator has no way to learn an individual
   edge's actual bound port, so every discovered edge is assumed to use the
   same port — keep it consistent across your fleet, or use the override
   below for the ones that differ).

2. **Explicit override.** Set `SIGIRO_EDGES` (comma-separated
   `name=host:port` entries) or pass one or more repeatable `--edge
   name=host:port` flags. Either one **skips tailnet discovery entirely**.

   ```sh
   export SIGIRO_EDGES="us-east=100.64.1.2:7777,eu-west=100.64.1.3:7777"
   # or, per-invocation:
   sigiro query "..." --edge us-east=100.64.1.2:7777 --edge eu-west=100.64.1.3:7777
   ```

The coordinator refuses to run against zero edges (no silent no-op), and
refuses a duplicate `host:port` entry listed under two names (it would
silently double-count every `count`/`sum`/`avg`).

---

## Querying

```sh
sigiro query "SELECT service_name, count(*) AS n FROM sigiro_spans \
  WHERE timestamp > now() - interval 1 hour GROUP BY service_name ORDER BY n DESC LIMIT 20"
```

`sigiro query` supports a deliberately narrow SQL grammar — narrow enough
that every aggregate it accepts can be folded *correctly* across edges. A
naive federation that just unions per-edge results and re-aggregates gets
`avg` wrong by double digits (measured ~13% off) because avg-of-averages
isn't the average of the whole, and `count(DISTINCT ...)` can't be
reconstructed from partial counts at all. So the grammar only accepts shapes
it can fold exactly:

**Supported:**
- A single `SELECT` over exactly one `sigiro_*` table (`sigiro_spans`,
  `sigiro_logs`, `sigiro_metrics_*`, ...)
- A **required** time bound on the `timestamp` column somewhere in `WHERE`
  (plus other column/literal comparisons combined with `AND`/`OR`)
- Optional plain `GROUP BY` on real columns
- Foldable aggregates: `count(...)`, `sum(...)`, `min(...)`, `max(...)`,
  `avg(...)` — each needs an explicit `AS alias`
- Passthrough top-N: `ORDER BY` a column or a `SELECT`-list alias, plus
  `LIMIT n` (applied once, after the cross-edge fold, so it's a true global
  top-N, not top-N-per-edge)
- `SELECT` aliases (`AS`) on plain columns

**Fails loud (not silently wrong) on:**
- `count(DISTINCT ...)` — not reconstructable from per-edge partials
- Any non-foldable aggregate (`approx_quantile`, percentiles, etc.) — only
  count/sum/min/max/avg are foldable
- `HAVING`, CTEs (`WITH ...`), `JOIN`, subqueries, set operations
  (`UNION`/`INTERSECT`/`EXCEPT`), `QUALIFY`, `SAMPLE`/`TABLESAMPLE`, DuckLake
  time-travel (`AT (...)`)
- Qualified or struct-style column references (`t.attributes.status`) unless
  the qualifier is literally the FROM table's own name/alias — the planner
  runs pre-bind and can't safely tell a table qualifier from a struct field
  access otherwise
- A query with **no** bound on `timestamp` in `WHERE` — the un-prunable
  cross-edge blowup case. Refused unless you pass `--full-scan`.
- `LIMIT` above `SIGIRO_QUERY_ROW_CAP` (default 1,000,000)
- A dead, unreachable, or auth-rejected edge — the whole query fails,
  naming every offending edge (no silent partial/degraded result)
- An edge whose partial blew past the per-edge row cap — narrow the `WHERE`
  or pass `--full-scan` if you mean it

**Example queries:**

```sh
# Slowest spans in the last 15 minutes, across every edge
sigiro query "SELECT trace_id, span_name, duration FROM sigiro_spans \
  WHERE timestamp > now() - interval 15 minute ORDER BY duration DESC LIMIT 10"

# Correctly-folded average duration for one service, last hour
sigiro query "SELECT avg(duration) AS avg_duration_us FROM sigiro_spans \
  WHERE service_name = 'checkout' AND timestamp > now() - interval 1 hour"

# This fails loud, not silently:
sigiro query "SELECT count(DISTINCT service_name) FROM sigiro_spans \
  WHERE timestamp > now() - interval 1 hour"
# -> "DISTINCT aggregates (e.g. count(DISTINCT ...)) are not federatable
#     across edges; run per-edge or narrow the query"
```

`--sql-file <path>` (or `--sql-file -` for stdin) takes a query from a file
instead of the argument, for long queries. Per-edge fetches are bounded by
`SIGIRO_QUERY_TIMEOUT_SECONDS` (default 30s).

---

## Hot/cold tiering per edge

Each edge's own [[architecture/ducklake-integration.md|DuckLake]] handles
tiering, not custom sigiro code:

- **Hot:** the most recent rows are inlined directly in the catalog, plus a
  small Parquet file DuckLake writes for the current period.
- **Warm:** as data ages, DuckLake compacts it into day-partitioned Parquet
  files on the edge's **local disk**. This local, Hive-style partitioning is
  what makes edge-side time-range pruning fast — the required `WHERE
  timestamp ...` bound on every federated query (see [Querying](#querying))
  is what lets the edge skip straight to the relevant files instead of
  scanning everything.
- **Cold:** sigiro can write DuckLake's Parquet to an object store
  (`SIGIRO_S3_BUCKET` + related vars) — but **only in distributed/hosted
  mode** (`SIGIRO_CATALOG_URL` set). **Object-store-backed edges are NOT YET
  SUPPORTED for `SIGIRO_QUERY_SURFACE=quack`** — the server refuses to start
  quack at all when `SIGIRO_CATALOG_URL` is set, because quack's read-only
  gate is SELECT-only and doesn't (yet) block DuckDB's own file/URL reader
  functions (`read_parquet('https://...')`, `httpfs`, `glob`) — serving
  quack over an S3-backed catalog would expose those as an unguarded
  exfiltration surface. Lifting this requires a reader/table allowlist on
  top of the existing gate.

So today: a self-host edge you intend to query via `sigiro query` keeps its
own data on **local disk only** — no cold object-store tier on that edge
until the allowlist above lands.
