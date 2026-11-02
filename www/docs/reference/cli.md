---
title: CLI
description: Every sigiro subcommand, its arguments, and the environment variables it reads, including hosted signup and login.
sidebar:
  order: 2
---

`sigiro` has seven subcommands. Client commands print indented JSON on success;
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

## serve

Starts the OTLP gRPC server, the OTLP HTTP server, and the query API.

| Flag                      | Overrides               | Default                 |
| ------------------------- | ----------------------- | ----------------------- |
| `--data-dir <PATH>`       | `SIGIRO_DATA_DIR`       | `~/.local/share/sigiro` |
| `--http-port <PORT>`      | `SIGIRO_HTTP_PORT`      | `9999`                  |
| `--otlp-http-port <PORT>` | `SIGIRO_OTLP_HTTP_PORT` | `4318`                  |
| `--otlp-grpc-port <PORT>` | `SIGIRO_GRPC_PORT`      | `4317`                  |

The first start downloads the query extensions into the data directory. That
download takes 30–90 seconds and blocks the start.

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

Hosted sigiro also serves the same tools over streamable HTTP at
`https://sigiro.com/mcp`, for agents that sign in with OAuth in a browser instead
of using the CLI's session. The documentation tools alone, without sign-in, are
at `https://sigiro.com/mcp/docs`.
