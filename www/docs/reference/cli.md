---
title: CLI
description: Every sigiro subcommand, its arguments, and the environment variables it reads. sigiro has seven subcommands.
sidebar:
  order: 2
---

`sigiro` has seven subcommands. Client commands print indented JSON on success;
server commands remain attached to their protocol or listener.

The global flag `--key <TOKEN>` sets the bearer token for a hosted endpoint. It
overrides `SIGIRO_API_KEY`. A self-hosted server ignores the token.

All client subcommands reach the server at `SIGIRO_ENDPOINT`, which defaults to
`http://localhost:9999`.

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

## diagnose

`sigiro diagnose <SERVICE>` returns one diagnosis block for one service.

| Flag                     | Type    | Default               |
| ------------------------ | ------- | --------------------- |
| `--from <EPOCH_SECONDS>` | integer | 15 minutes before now |
| `--to <EPOCH_SECONDS>`   | integer | now                   |

`--from` and `--to` are epoch **seconds**. The equivalent HTTP field names are
`from_ts` and `to_ts`, and those are epoch **microseconds**. Both HTTP fields
are optional. If you omit both, the server reads the last 15 minutes, which is
the same window the CLI uses.

The command prints JSON. To narrow the output, pipe it to `jq`. The CLI applies
no filter of its own, so the CLI and the API return the same document.

## anomalies

`sigiro anomalies` lists the detected regime shifts, biggest shift first, with a
cross-signal `incident_id` on each row.

| Flag                     | Type    | Default        |
| ------------------------ | ------- | -------------- |
| `--service <NAME>`       | string  | all services   |
| `--from <EPOCH_SECONDS>` | integer | no lower bound |
| `--to <EPOCH_SECONDS>`   | integer | no upper bound |

The rows come from the `sigiro_anomalies` table, which a scheduled pass writes.
Nothing is computed for the request.

## query

`sigiro query "<SQL>"` is a **federated coordinator**, not a local client. It
resolves one or more edges, pushes a partial query to each edge, and folds the
partials into one answer.

| Flag                      | Type                   | Default |
| ------------------------- | ---------------------- | ------- |
| `<SQL>`                   | string                 | none    |
| `--edge <NAME=HOST:PORT>` | repeatable             | none    |
| `--full-scan`             | flag                   | off     |

Edge resolution has this order. `SIGIRO_EDGES` (comma-separated
`name=host:port`) and `--edge` are used verbatim, and either one skips discovery.
With neither set, sigiro runs `tailscale status --json` and takes every peer
tagged `tag:sigiro-edge`. With zero edges resolved, the command fails rather
than runs against nothing.

Therefore `sigiro query` does **not** work against a plain local server. An edge
must serve its data with `SIGIRO_QUERY_SURFACE=quack`, and you must name it with
`--edge` or find it on a tailnet. To run SQL against one local server, post the
SQL to `/v1/query` instead.

`--full-scan` allows a query with no `WHERE` bound on `timestamp`. sigiro refuses
such a query by default.

## status

`sigiro status` reads the server `/health` endpoint and prints the health block.
`/health` needs no key.

## healthcheck

`sigiro healthcheck` probes the local `/health` endpoint and exits `0` only when
the server is ready. This is the container `HEALTHCHECK`, because the distroless
image has no shell and no `curl`.

## mcp

`sigiro mcp` starts a stdio Model Context Protocol server. It exposes
`list_services`, `list_anomalies`, `diagnose_service`, and `run_query` so an
analysis agent can inspect sigiro without constructing HTTP requests itself.
