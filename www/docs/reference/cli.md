---
title: CLI
description: Every sigiro subcommand, its arguments, and the environment variables it reads, including hosted signup and login.
sidebar:
  order: 2
---

`sigiro` has eight subcommands. Client commands print indented JSON on success;
server commands remain attached to their protocol or listener.

For a new hosted account, run `sigiro signup --name "Your Name" --email
you@example.com`. It emails a six-digit OTP, prompts for it securely in the
terminal, creates and activates exactly one personal workspace, completes OAuth
device authorization internally, and stores refresh/access credentials in the OS
credential store. For an existing account, use browser-based `sigiro auth login`;
`--no-browser` prints its verification URL instead. `sigiro auth status`,
`sigiro auth token`, and `sigiro auth logout` manage the stored session.

The global flag `--token <TOKEN>` is an explicit override for automation. It
overrides `SIGIRO_ACCESS_TOKEN`. Open-mode self-hosted servers do not require it;
self-hosted servers configured with static bearer authentication do.

All client subcommands (`status`, `mcp`) pick their server in this order:

1. `SIGIRO_ENDPOINT`, when it is set;
2. `https://sigiro.com`, when `sigiro auth login` or `sigiro signup` stored a
   session;
3. `http://localhost:9999` otherwise.

So after you sign in to hosted sigiro, these commands talk to `sigiro.com` even
when a local `sigiro serve` is running. Set `SIGIRO_ENDPOINT=http://localhost:9999`
to reach the local server instead, or run `sigiro auth logout`.

## Hosted agent authentication

Sigiro uses the official Agent Auth implementation internally and does not expose
a second onboarding CLI. For delegated host registration, one-time approval,
least-privilege capability grants, token use, key rotation, and revocation, see
[Onboard an agent with Agent Auth](/docs/how-to/agent-auth).

## ingest-keys

Fixed keys for long-running services on hosted sigiro, in place of the
15-minute `sigiro auth token`. A key belongs to the workspace your login is bound
to, is sent as `Authorization: Bearer <key>`, and can only send telemetry.

| Command                                  | Does                                                    |
| ---------------------------------------- | ------------------------------------------------------- |
| `sigiro ingest-keys create --name <NAME>` | Creates a key and prints it once; only its hash is kept |
| `sigiro ingest-keys list`                | Lists ids, names, last use and revocation, no secrets   |
| `sigiro ingest-keys revoke <ID>`         | Revokes a key; its senders get `401` from then on        |

Only your own login can manage keys; an Agent Auth token or an ingest key
cannot.

## serve

Starts the OTLP gRPC server, the OTLP HTTP server, and the query API.

| Flag                      | Overrides               | Default                 |
| ------------------------- | ----------------------- | ----------------------- |
| `--data-dir <PATH>`       | `SIGIRO_DATA_DIR`       | `~/.local/share/sigiro` |
| `--http-port <PORT>`      | `SIGIRO_HTTP_PORT`      | `9999`                  |
| `--otlp-http-port <PORT>` | `SIGIRO_OTLP_HTTP_PORT` | `4318`                  |
| `--otlp-grpc-port <PORT>` | `SIGIRO_GRPC_PORT`      | `4317`                  |

Everything the server needs is built into the binary: it downloads nothing at
start and runs offline.

A server without auth (the default for a self-hosted server) also serves the
web dashboard at `http://localhost:9999/dashboard/`. The dashboard is built into
the binary. Set `SIGIRO_DASHBOARD_DIR` to the directory that holds a different
static build (`dashboard/dist`) to serve that one instead.

### Storage and ingest

A `200` (OTLP/HTTP) or `OK` (OTLP/gRPC) means the request's rows are committed
to the project's DuckLake catalog and survive a restart. A request that cannot
be committed gets `503` with `Retry-After: 1` (gRPC `UNAVAILABLE`). Nothing from
it was stored, so retrying it cannot store it twice. Only the disk cap answers
`507`.

Small commits are stored in the catalog file instead of as one Parquet file
each. Each project's writer runs DuckLake's `CHECKPOINT` every hour while it is
busy and once when it goes idle. The checkpoint writes the stored rows to
Parquet and merges small files. A failed checkpoint is retried after 30
seconds, then less often, and never stops ingest. `GET /health` lists, under
`storage`, each project's last checkpoint, its failures since the last success,
its data files and the rows still waiting for a checkpoint.

These settings are environment variables only, read once at start:

| Variable                            | Default                    | Meaning                                                                     |
| ----------------------------------- | -------------------------- | --------------------------------------------------------------------------- |
| `SIGIRO_RETENTION_DAYS`             | `7`                        | Days of telemetry kept. `0` keeps everything                                |
| `SIGIRO_MAX_DISK_BYTES`             | `0` (off)                  | Bytes under the data directory above which ingest gets `507`                |
| `SIGIRO_CHECKPOINT_INTERVAL_SECS`   | `3600`                     | Seconds between checkpoints of a busy project                               |
| `SIGIRO_INGEST_QUEUE_BYTES`         | `134217728` (128 MiB)      | Decoded bytes waiting for a commit, all projects. Past it, requests get 503 |
| `SIGIRO_INGEST_PROJECT_QUEUE_BYTES` | `33554432` (32 MiB)        | The share of that one project may hold                                      |
| `SIGIRO_DUCKDB_MEMORY_LIMIT`        | `400MB`                    | DuckDB memory per open project                                              |
| `SIGIRO_DUCKDB_TOTAL_MEMORY_LIMIT`  | half the machine's memory  | DuckDB memory for all open projects; sets how many stay open at once        |
| `SIGIRO_DUCKDB_THREADS`             | number of CPUs             | DuckDB threads per project. A checkpoint uses at most 2                     |
| `SIGIRO_MAX_INGEST_BODY_BYTES`      | `8388608` (8 MiB)          | Largest request body, after decompression                                   |
| `SIGIRO_SHUTDOWN_TIMEOUT_SECS`      | `5`                        | One deadline for all writers to stop at shutdown                            |

The data directory records a storage format version. Before 1.0, sigiro does
not migrate stored telemetry: a release with a new format refuses to start on
an older directory and says which one. To back up, copy the whole data
directory while sigiro is stopped. To reset, stop sigiro, move the directory
aside, and start sigiro again.

## query

`sigiro query "<SQL>"` is a **federated coordinator**, not a local client. It
resolves one or more edges, pushes a partial query to each edge, and folds the
partials into one answer.

| Flag                      | Type       | Default |
| ------------------------- | ---------- | ------- |
| `<SQL>`                   | string     | none    |
| `--edge <NAME=HOST:PORT>` | repeatable | none    |
| `--full-scan`             | flag       | off     |

Edge resolution has this order. `SIGIRO_EDGES` (comma-separated
`name=host:port`) and `--edge` are used verbatim, and either one skips discovery.
With neither set, sigiro runs `tailscale status --json` and takes every peer
tagged `tag:sigiro-edge`. With zero edges resolved, the command fails rather
than runs against nothing.

Therefore `sigiro query` does **not** work against hosted `sigiro.com` or
against a plain local server. An edge
must serve its data with `SIGIRO_QUERY_SURFACE=quack`, and you must name it with
`--edge` or find it on a tailnet. To run SQL against hosted sigiro or one local server,
use the `run_query` tool of [`sigiro mcp`](#mcp), or post the SQL to `/v1/query`:

```bash
curl -s https://sigiro.com/v1/query \
  -H "Authorization: Bearer $(sigiro auth token)" \
  --data "SELECT service_name, count(*) AS spans FROM sigiro_spans
          WHERE timestamp > now() - INTERVAL '1 hour' GROUP BY 1"
```

`--full-scan` allows a query with no `WHERE` bound on `timestamp`. sigiro refuses
such a query by default.

## status

`sigiro status` reads the `/health` endpoint of the server chosen as above and
prints the health block, plus a `recent_ingest` block with span and log counts
for the last five minutes. The `endpoint` field names the server it probed.
`/health` needs no key.

## healthcheck

`sigiro healthcheck` probes the local `/health` endpoint and exits `0` only when
the server is ready. This is the container `HEALTHCHECK`, because the distroless
image has no shell and no `curl`.

## mcp

`sigiro mcp` starts a stdio Model Context Protocol server. It exposes
`list_services` and `run_query`, plus the documentation tools (`search_docs`,
`get_page`, `list_pages`, `get_navigation`), so an analysis agent can inspect
sigiro without constructing HTTP requests itself. It uses the server chosen as
above, so after `sigiro auth login` it reads your hosted workspace with the
stored session, and refreshes that session itself.

Register it with your agent as a stdio server:

```bash
claude mcp add sigiro -- sigiro mcp     # Claude Code
codex mcp add sigiro -- sigiro mcp      # Codex
```

Use an absolute path to `sigiro` if the agent starts with a narrower `PATH`
than your shell. In `run_query`, bound `timestamp` with timestamp values such as
`timestamp > now() - INTERVAL '1 hour'`; result rows render timestamps and
`duration` as microseconds since the Unix epoch, but comparing `timestamp` with
an integer fails.

The documentation tools come from the hosted docs MCP at
`https://sigiro.com/mcp/docs`, or from `SIGIRO_DOCS_MCP_URL` if you set it.
When that endpoint cannot be reached, because the machine is offline, the
endpoint does not answer within a few seconds, or it returns an error, the tools
answer from a copy of these docs that is built into the binary. The tool names,
arguments and answers stay the same, but the built-in copy is the docs as of the
release you run. Set `SIGIRO_DOCS_MCP_URL` to an empty value to skip the network
and always use the built-in copy.

Hosted sigiro also serves the same tools over streamable HTTP at
`https://sigiro.com/mcp`, for agents that sign in with OAuth in a browser instead
of using the CLI's session. The documentation tools alone, without sign-in, are
at `https://sigiro.com/mcp/docs`.
